use super::{State, tools::Options};
use futures_util::{StreamExt, stream::FuturesUnordered};
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, atomic::Ordering},
};

struct Job {
    directory: PathBuf,
    status: &'static str,
    slots: Vec<Option<Value>>,
    done: usize,
    failed: usize,
}

#[derive(Default)]
pub(super) struct Table {
    jobs: IndexMap<String, Job>,
    forgotten: VecDeque<String>,
}

impl Table {
    pub fn snapshot(&self, id: &str) -> Result<Value, String> {
        let Some(job) = self.jobs.get(id) else {
            return Err(if self.forgotten.iter().any(|old| old == id) {
                format!(
                    "Job {id:?} finished and has been forgotten — this server keeps 100 recent finished jobs. Its output files remain on disk."
                )
            } else {
                format!(
                    "Unknown job id {id:?}. Jobs live only in server memory; check the id or read the files the job wrote."
                )
            });
        };
        Ok(
            json!({"job_id":id,"status":job.status,"total":job.slots.len(),"done":job.done,
            "failed":job.failed,"output_dir":job.directory,"results":job.slots.iter().flatten().collect::<Vec<_>>()}),
        )
    }

    fn finish(&mut self, id: &str, cancelled: bool) {
        if let Some(job) = self.jobs.get_mut(id) {
            job.status = if cancelled && job.done < job.slots.len() {
                "cancelled"
            } else {
                "completed"
            };
        }
        let finished: Vec<_> = self
            .jobs
            .iter()
            .filter(|(_, job)| job.status != "running")
            .map(|(id, _)| id.clone())
            .collect();
        for id in finished.iter().take(finished.len().saturating_sub(100)) {
            self.jobs.shift_remove(id);
            self.forgotten.push_back(id.clone());
            if self.forgotten.len() > 500 {
                self.forgotten.pop_front();
            }
        }
    }
}

pub(super) fn start(
    state: &Arc<State>,
    sources: Vec<String>,
    directory: PathBuf,
    options: Options,
    concurrency: usize,
) -> Result<Value, String> {
    let cfg = state.config()?;
    let runtime = Arc::new(
        markitai_core::LlmRuntime::new(cfg["llm"]["concurrency"].as_u64().unwrap_or(10) as usize)
            .map_err(|error| error.to_string())?,
    );
    let mut background = state.background.lock().unwrap();
    if state.closing.load(Ordering::SeqCst) {
        return Err("MCP server is shutting down".into());
    }
    background.retain(|task| !task.is_finished());
    let mut table = state.jobs.lock().unwrap();
    if table
        .jobs
        .values()
        .filter(|job| job.status == "running")
        .count()
        >= 100
    {
        return Err("MCP server already has 100 running jobs".into());
    }
    let id = loop {
        let candidate = uuid::Uuid::new_v4().simple().to_string()[..8].to_owned();
        if !table.jobs.contains_key(&candidate)
            && !table.forgotten.contains(&candidate)
            && !directory.join(format!("batch-{candidate}")).exists()
        {
            break candidate;
        }
    };
    let total = sources.len();
    table.jobs.insert(
        id.clone(),
        Job {
            directory: directory.clone(),
            status: "running",
            slots: vec![None; total],
            done: 0,
            failed: 0,
        },
    );
    drop(table);
    let ack = json!({"job_id":id,"status":"running","total":total,"output_dir":directory});
    let state = state.clone();
    background.push(tokio::spawn(async move {
        let mut next = sources.into_iter().enumerate();
        let mut pending = FuturesUnordered::new();
        let mut exhausted = false;
        loop {
            while pending.len() < concurrency && !exhausted && !state.closing.load(Ordering::SeqCst) {
                let Some((index, source)) = next.next() else { exhausted = true; break; };
                let output = directory.join(format!("batch-{id}")).join(format!("{:04}", index + 1));
                let state = state.clone();
                let options = options.clone();
                let runtime = runtime.clone();
                pending.push(async move {
                    let result = state.convert(source.clone(), output, options, Some(runtime)).await;
                    (index, source, result)
                });
            }
            let Some((index, source, result)) = pending.next().await else { break; };
            let (value, failed) = match result {
                Ok(result) => (json!({"source":source,"status":"ok","markdown_file":result["markdown_file"],"cost_usd":result["cost_usd"],"warnings":result["warnings"]}), false),
                Err(error) => (json!({"source":source,"status":"error","error":error}), true),
            };
            if let Some(job) = state.jobs.lock().unwrap().jobs.get_mut(&id) {
                job.slots[index] = Some(value);
                job.done += 1;
                job.failed += usize::from(failed);
            }
        }
        state.jobs.lock().unwrap().finish(&id, state.closing.load(Ordering::SeqCst));
    }));
    Ok(ack)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retention_preserves_running_jobs_and_distinguishes_expired_ids() {
        let mut table = Table::default();
        table.jobs.insert(
            "running".into(),
            Job {
                directory: PathBuf::from("/isolated"),
                status: "running",
                slots: vec![None],
                done: 0,
                failed: 0,
            },
        );
        for index in 0..605 {
            let id = format!("job-{index}");
            table.jobs.insert(
                id.clone(),
                Job {
                    directory: PathBuf::from("/isolated"),
                    status: "running",
                    slots: vec![Some(
                        json!({"source":"x","status":"error","error":"missing"}),
                    )],
                    done: 1,
                    failed: 1,
                },
            );
            table.finish(&id, false);
        }
        assert_eq!(table.jobs.len(), 101);
        assert_eq!(table.forgotten.len(), 500);
        assert_eq!(table.snapshot("running").unwrap()["status"], "running");
        assert!(table.snapshot("job-0").unwrap_err().contains("Unknown"));
        assert!(table.snapshot("job-5").unwrap_err().contains("forgotten"));
        let status = table.snapshot("job-604").unwrap();
        assert_eq!(status["done"], 1);
        assert_eq!(status["failed"], 1);
        assert_eq!(status["status"], "completed");
    }
}
