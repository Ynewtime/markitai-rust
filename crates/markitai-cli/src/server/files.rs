use super::{
    State, http,
    jobs::{self, Job},
    store,
    types::{ApiError, ApiResult},
};
use axum::{
    Json,
    body::Body,
    extract::{Path, State as ExtractState},
    http::{HeaderValue, header},
    response::Response,
};
use futures_util::stream;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    sync::Arc,
};
use tokio::io::AsyncReadExt;

const MAX_RESULT: u64 = 64 * 1024 * 1024;
fn encoded(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}
fn body(
    file: File,
    temporary: Option<tempfile::NamedTempFile>,
    content_type: &str,
    filename: &str,
) -> ApiResult<Response> {
    let size = file.metadata().map_err(ApiError::internal)?.len();
    let stream = stream::try_unfold(
        (tokio::fs::File::from_std(file), temporary),
        |(mut file, temporary)| async move {
            let mut bytes = vec![0u8; 64 * 1024];
            let count = file.read(&mut bytes).await?;
            if count == 0 {
                Ok::<_, std::io::Error>(None)
            } else {
                bytes.truncate(count);
                Ok(Some((bytes, (file, temporary))))
            }
        },
    );
    let mut response = Response::new(Body::from_stream(stream));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(content_type).unwrap(),
    );
    response
        .headers_mut()
        .insert(header::CONTENT_LENGTH, HeaderValue::from(size));
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "attachment; filename*=UTF-8''{}",
            encoded(filename)
        ))
        .map_err(ApiError::internal)?,
    );
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}
fn content_type(path: &str) -> &'static str {
    match path
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "md" | "txt" => "text/plain; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

pub(super) async fn download(
    ExtractState(state): ExtractState<Arc<State>>,
    Path((id, relative)): Path<(String, String)>,
) -> ApiResult<Response> {
    let job = jobs::get(&state, &id)?;
    let filename = relative.clone();
    let file = tokio::task::spawn_blocking(move || {
        let _guard = job.access.lock().unwrap();
        let path = store::safe_file(&job.folder.join("out"), &relative)?;
        File::open(path).map_err(ApiError::internal)
    })
    .await
    .map_err(ApiError::internal)??;
    body(
        file,
        None,
        content_type(&filename),
        filename.rsplit('/').next().unwrap_or("download"),
    )
}

pub(super) async fn result(
    ExtractState(state): ExtractState<Arc<State>>,
    Path((id, item_id)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let job = jobs::get(&state, &id)?;
    tokio::task::spawn_blocking(move||{
        let _guard=job.access.lock().unwrap();let data=job.data.lock().unwrap();
        let item=data.items.iter().find(|item|item.item_id==item_id).ok_or_else(||ApiError::new(404,"item not found"))?;
        let selected=item.output.as_deref().filter(|_|item.status=="done").ok_or_else(||ApiError::new(404,"item result not available"))?;
        let out=job.folder.join("out");store::safe_file(&out,selected)?;
        let base=data.bases.get(&item_id).cloned().unwrap_or_else(||{
            let source=item.output_name.as_deref().unwrap_or(selected);
            let stem=source.strip_suffix(".md").unwrap_or(source);
            if item.llm_enhanced&&item.output_name.is_none(){stem.strip_suffix(".llm").unwrap_or(stem).to_owned()}else{stem.to_owned()}
        });
        let base_name=format!("{base}.md");let enhanced_name=format!("{base}.llm.md");
        let base_path=store::safe_file(&out,&base_name).ok();let enhanced_path=store::safe_file(&out,&enhanced_name).ok();
        let (variant,path)=if item.llm_enhanced&&enhanced_path.is_some(){("llm",enhanced_path.clone().unwrap())}else if let Some(path)=base_path.clone(){("base",path)}else if let Some(path)=enhanced_path.clone(){("llm",path)}else{return Err(ApiError::new(404,"item result not available"));};
        if fs::metadata(&path).map_err(ApiError::internal)?.len()>MAX_RESULT{return Err(ApiError::new(413,"result is too large for JSON; use the file download endpoint"));}
        let markdown=fs::read_to_string(&path).map_err(ApiError::internal)?;
        let mut artifacts=Vec::new();let mut seen=HashSet::new();
        let mut add=|relative:String|->ApiResult<()> {
            if seen.insert(relative.clone())&&let Ok(path)=store::safe_file(&out,&relative){artifacts.push(json!({"relpath":relative,"size":fs::metadata(path).map_err(ApiError::internal)?.len()}));}Ok(())
        };
        if base_path.is_some() {
            add(base_name)?;
        }
        if item.llm_enhanced && enhanced_path.is_some() {
            add(enhanced_name)?;
        }
        if let Some(assets)=data.assets.get(&item_id){for asset in assets{add(asset.clone())?;}}
        else {
            // Older CLI/server histories lack an item asset index. Recover ownership
            // through actual destinations rather than exposing unrelated job assets.
            let candidates=store::files(&out).map_err(ApiError::internal)?.into_iter().filter(|(name,_)|name.starts_with(".markitai/assets/")||name.starts_with(".markitai/screenshots/")||name.starts_with("assets/")).map(|(name,_)|name).collect::<Vec<_>>();
            let marker=format!("MARKITAI-ARTIFACT-{}-",uuid::Uuid::new_v4().simple());
            let replacements=candidates.iter().enumerate().map(|(i,name)|(name.clone(),format!("{marker}{i}-END"))).collect::<HashMap<_,_>>();
            let mut text=markdown.clone();
            if item.llm_enhanced&&let Some(base_path)=base_path&&base_path!=path&&fs::metadata(&base_path).map_err(ApiError::internal)?.len()<=MAX_RESULT{text.push_str(&fs::read_to_string(base_path).map_err(ApiError::internal)?);}
            let rewritten=markitai_core::output::rewrite_asset_references(&text,&replacements);
            for (index,name) in candidates.into_iter().enumerate(){if rewritten.contains(&format!("{marker}{index}-END")){add(name)?;}}
        }
        Ok(Json(json!({"name":item.name,"variant":variant,"markdown":markdown,"artifacts":artifacts})))
    }).await.map_err(ApiError::internal)?
}

fn zip_jobs(
    root: &std::path::Path,
    jobs: Vec<Arc<Job>>,
) -> ApiResult<(File, tempfile::NamedTempFile)> {
    let temporary = tempfile::Builder::new()
        .prefix(".archive-")
        .suffix(".tmp")
        .tempfile_in(root)
        .map_err(ApiError::internal)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(ApiError::internal)?;
    }
    let mut writer = zip::ZipWriter::new(temporary.reopen().map_err(ApiError::internal)?);
    let multiple = jobs.len() > 1;
    let mut directories = HashSet::new();
    let mut count = 0usize;
    for job in jobs {
        let _guard = job.access.lock().unwrap();
        let data = job.data.lock().unwrap();
        if data.status == "running" {
            return Err(ApiError::new(409, "job is still running"));
        }
        let prefix = if multiple {
            let raw = data
                .items
                .iter()
                .find_map(|item| item.output.as_deref())
                .map(|name| {
                    let filename = name.rsplit('/').next().unwrap_or(name);
                    filename
                        .strip_suffix(".llm.md")
                        .or_else(|| filename.strip_suffix(".md"))
                        .unwrap_or(filename)
                        .to_owned()
                })
                .unwrap_or_else(|| format!("job-{}", data.id));
            let base = jobs::sanitize_name(&raw);
            let mut name = base.clone();
            let mut count = 2;
            while !directories.insert(caseless::default_case_fold_str(&name)) {
                name = format!("{base} ({count})");
                count += 1;
            }
            format!("{name}/")
        } else {
            String::new()
        };
        drop(data);
        for (relative, path) in store::files(&job.folder.join("out")).map_err(ApiError::internal)? {
            count += 1;
            if count > 100_000 {
                return Err(ApiError::new(413, "archive has too many files"));
            }
            writer
                .start_file(
                    format!("{prefix}{relative}"),
                    zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated)
                        .unix_permissions(0o600),
                )
                .map_err(ApiError::internal)?;
            let mut input = File::open(path).map_err(ApiError::internal)?;
            std::io::copy(&mut input, &mut writer).map_err(ApiError::internal)?;
        }
    }
    writer
        .finish()
        .map_err(ApiError::internal)?
        .sync_all()
        .map_err(ApiError::internal)?;
    let file = temporary.reopen().map_err(ApiError::internal)?;
    Ok((file, temporary))
}
pub(super) async fn job_archive(
    ExtractState(state): ExtractState<Arc<State>>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let job = jobs::get(&state, &id)?;
    let root = state.root.clone();
    let (file, temp) = tokio::task::spawn_blocking(move || zip_jobs(&root, vec![job]))
        .await
        .map_err(ApiError::internal)??;
    body(
        file,
        Some(temp),
        "application/zip",
        &format!("markitai-{id}.zip"),
    )
}
pub(super) async fn history_archive(
    ExtractState(state): ExtractState<Arc<State>>,
) -> ApiResult<Response> {
    http::refresh(&state).await?;
    let mut jobs = state
        .jobs
        .lock()
        .unwrap()
        .values()
        .filter(|job| job.data.lock().unwrap().status == "done")
        .cloned()
        .collect::<Vec<_>>();
    jobs.sort_by_key(|job| job.data.lock().unwrap().created_at.clone());
    if jobs.is_empty() {
        return Err(ApiError::new(404, "history is empty"));
    }
    let root = state.root.clone();
    let (file, temp) = tokio::task::spawn_blocking(move || zip_jobs(&root, jobs))
        .await
        .map_err(ApiError::internal)??;
    body(file, Some(temp), "application/zip", "markitai-all.zip")
}
