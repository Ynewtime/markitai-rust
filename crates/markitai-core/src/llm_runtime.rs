//! Shared, blocking request capacity for one caller-controlled conversion run.

use crate::{Error, Result, llm::flight};
use std::sync::{Arc, Condvar, Mutex};

/// Share this runtime between conversions that should obey one LLM request cap.
///
/// Clones share capacity. Independently constructed runtimes do not constrain
/// one another. Typed requests may share validated in-flight answers; private
/// request fingerprints are never exposed through Debug or persisted.
#[derive(Clone, Debug)]
pub struct LlmRuntime {
    inner: Arc<Inner>,
}

struct Inner {
    limit: usize,
    flights: Arc<flight::Table>,
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Debug, Default)]
struct State {
    active: usize,
    next_ticket: u64,
    serving: u64,
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmRuntime")
            .field("concurrency", &self.limit)
            .finish_non_exhaustive()
    }
}

impl LlmRuntime {
    pub fn new(concurrency: usize) -> Result<Self> {
        if concurrency == 0 {
            return Err(Error::InvalidInput(
                "LLM concurrency must be greater than zero".into(),
            ));
        }
        Ok(Self {
            inner: Arc::new(Inner {
                limit: concurrency,
                flights: Arc::new(flight::Table::new()),
                state: Mutex::new(State::default()),
                changed: Condvar::new(),
            }),
        })
    }

    pub(crate) fn flights(&self) -> &Arc<flight::Table> {
        &self.inner.flights
    }

    pub fn concurrency(&self) -> usize {
        self.inner.limit
    }

    pub(crate) fn acquire(&self) -> Permit<'_> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let ticket = state.next_ticket;
        state.next_ticket = state.next_ticket.wrapping_add(1);
        while ticket != state.serving || state.active == self.inner.limit {
            state = self
                .inner
                .changed
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }
        state.active += 1;
        state.serving = state.serving.wrapping_add(1);
        // Another queued caller may use the remaining capacity immediately.
        self.inner.changed.notify_all();
        Permit { inner: &self.inner }
    }
}

pub(crate) struct Permit<'a> {
    inner: &'a Inner,
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.active -= 1;
        self.inner.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Barrier, mpsc};
    use std::time::Duration;

    #[test]
    fn zero_is_rejected_and_independent_runs_do_not_share_capacity() {
        assert!(LlmRuntime::new(0).is_err());
        let first = LlmRuntime::new(1).unwrap();
        let second = LlmRuntime::new(2).unwrap();
        let _first = first.acquire();
        let _second = second.acquire();
        let _third = second.acquire();
        assert_eq!(second.concurrency(), 2);
    }

    #[test]
    fn clones_share_capacity_and_waiters_resume_after_a_release() {
        let runtime = LlmRuntime::new(2).unwrap();
        let first = runtime.acquire();
        let second = runtime.acquire();
        let clone = runtime.clone();
        let ready = Arc::new(Barrier::new(2));
        let worker_ready = ready.clone();
        let (entered, received) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            worker_ready.wait();
            let _permit = clone.acquire();
            entered.send(()).unwrap();
        });
        ready.wait();
        assert!(received.recv_timeout(Duration::from_millis(50)).is_err());
        drop(first);
        received.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
        drop(second);
        let _first = runtime.acquire();
        let _second = runtime.acquire();
    }

    #[test]
    fn error_and_unwind_release_their_permits() {
        fn failed(runtime: &LlmRuntime) -> std::result::Result<(), ()> {
            let _permit = runtime.acquire();
            Err(())
        }
        let runtime = LlmRuntime::new(1).unwrap();
        assert!(failed(&runtime).is_err());
        let panic = std::panic::catch_unwind(|| {
            let _permit = runtime.acquire();
            panic!("authored request failure");
        });
        assert!(panic.is_err());
        let (entered, received) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _permit = runtime.acquire();
            entered.send(()).unwrap();
        });
        received.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
    }
}
