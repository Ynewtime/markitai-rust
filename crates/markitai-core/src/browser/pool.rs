//! An exclusive page lease keeps one protocol connection on one calling thread.
use super::{cdp::Browser, options::Options};
use crate::{Error, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::io::Write;
use std::path::Path;
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

const MAX_WAITERS: usize = 128;
const MAX_IDENTITY_BYTES: usize = 1024 * 1024;
const ISOLATED_IDLE: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, PartialEq, Eq)]
struct Key([u8; 32]);
struct Entry {
    id: u64,
    key: Key,
    persistent: bool,
    browser: Option<Browser>,
    last_used: Instant,
    ttl: Duration,
    #[cfg(test)]
    process: Option<(u32, std::path::PathBuf)>,
}
struct State {
    entries: Vec<Entry>,
    next_id: u64,
    waiting: usize,
    retiring: usize,
    closed: bool,
}

pub(crate) struct Pool {
    capacity: usize,
    salt: [u8; 32],
    state: Mutex<State>,
    changed: Condvar,
}

fn failed(message: &str) -> Error {
    Error::Fetch(message.into())
}

impl Pool {
    pub(crate) fn new(capacity: usize) -> Result<Self> {
        if !(1..=8).contains(&capacity) {
            return Err(Error::InvalidInput(
                "Browser runtime capacity must be between 1 and 8".into(),
            ));
        }
        // RandomState is independently seeded by the standard library. Keys are
        // private digests, not credentials, stable identifiers or authentication.
        let random = RandomState::new();
        let mut salt = [0; 32];
        for (index, chunk) in salt.as_chunks_mut::<8>().0.iter_mut().enumerate() {
            *chunk = random.hash_one(index).to_le_bytes();
        }
        Ok(Self {
            capacity,
            salt,
            state: Mutex::new(State {
                entries: Vec::new(),
                next_id: 0,
                waiting: 0,
                retiring: 0,
                closed: false,
            }),
            changed: Condvar::new(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn key(
        &self,
        executable: &Path,
        url: &url::Url,
        cfg: &Value,
        options: &Options,
    ) -> Result<Key> {
        let canonical = executable
            .canonicalize()
            .map_err(|_| failed("Browser executable is unavailable"))?;
        let metadata = canonical
            .metadata()
            .map_err(|_| failed("Browser executable is unavailable"))?;
        let modified = metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|value| (value.as_secs(), value.subsec_nanos()));
        let mut writer = Identity {
            hash: Sha256::new(),
            bytes: 0,
        };
        writer.hash.update(self.salt);
        // JSON tuples frame fields unambiguously. No raw identity is retained.
        serde_json::to_writer(
            &mut writer,
            &(
                "browser-runtime-v1",
                canonical.as_os_str().as_encoded_bytes(),
                metadata.len(),
                modified,
                &options.proxy,
                &options.bypass,
                options.persistent,
            ),
        )
        .map_err(|_| Error::Config("Browser session identity exceeds its resource limit".into()))?;
        if options.persistent {
            // Use configured cookies before implicit request-URL defaults so
            // visits to two paths of one origin retain the same session identity.
            // Actual injection still uses the validated, normalized CookieParams.
            serde_json::to_writer(
                &mut writer,
                &(
                    url.origin().ascii_serialization(),
                    options
                        .credentials
                        .as_ref()
                        .map(|credentials| credentials.identity()),
                    cfg.pointer("/fetch/playwright/cookies"),
                    &options.headers,
                    &options.user_agent,
                    options.session_ttl.as_secs(),
                ),
            )
            .map_err(|_| {
                Error::Config("Browser session identity exceeds its resource limit".into())
            })?;
        }
        Ok(Key(writer.hash.finalize().into()))
    }

    pub(super) fn acquire<'a>(
        &'a self,
        executable: &Path,
        url: &url::Url,
        cfg: &Value,
        options: &Options,
    ) -> Result<Lease<'a>> {
        let key = self.key(executable, url, cfg, options)?;
        let deadline = Instant::now() + Duration::from_millis(options.timeout);
        let ttl = if options.persistent {
            options.session_ttl
        } else {
            ISOLATED_IDLE
        };
        loop {
            let mut state = self.lock();
            if state.closed {
                return Err(failed("Browser runtime is closed"));
            }
            if Instant::now() >= deadline {
                return Err(failed("Browser runtime capacity wait timed out"));
            }
            if let Some(index) = state
                .entries
                .iter()
                .position(|entry| entry.browser.is_some() && entry.last_used.elapsed() >= entry.ttl)
            {
                let old = state.entries.swap_remove(index);
                state.retiring += 1;
                drop(state);
                drop(old); // Process shutdown must never hold the pool lock.
                self.lock().retiring -= 1;
                self.changed.notify_all();
                continue;
            }
            if let Some(index) = state
                .entries
                .iter()
                .position(|entry| entry.key == key && entry.browser.is_some())
            {
                let entry = &mut state.entries[index];
                let lease = Lease {
                    pool: self,
                    id: entry.id,
                    persistent: options.persistent,
                    browser: entry.browser.take(),
                    reusable: false,
                };
                drop(state);
                return Ok(lease);
            }
            // Persistent identity has exactly one context. Waiting cannot create
            // a second cookie jar just because the first is currently borrowed.
            let identity_busy = options.persistent
                && state
                    .entries
                    .iter()
                    .any(|entry| entry.persistent && entry.key == key);
            if !identity_busy
                && state.entries.len() + state.retiring >= self.capacity
                && let Some(index) = state
                    .entries
                    .iter()
                    .enumerate()
                    .filter(|(_, entry)| entry.browser.is_some())
                    .min_by_key(|(_, entry)| entry.last_used)
                    .map(|(index, _)| index)
            {
                let old = state.entries.swap_remove(index);
                state.retiring += 1;
                drop(state);
                drop(old);
                self.lock().retiring -= 1;
                self.changed.notify_all();
                continue;
            }
            if !identity_busy && state.entries.len() + state.retiring < self.capacity {
                let id = state.next_id;
                state.next_id = state
                    .next_id
                    .checked_add(1)
                    .ok_or_else(|| failed("Browser runtime identity capacity exhausted"))?;
                state.entries.push(Entry {
                    id,
                    key,
                    persistent: options.persistent,
                    browser: None,
                    last_used: Instant::now(),
                    ttl,
                    #[cfg(test)]
                    process: None,
                });
                drop(state);
                let mut lease = Lease {
                    pool: self,
                    id,
                    persistent: options.persistent,
                    browser: None,
                    reusable: false,
                };
                // Reserving before launch bounds simultaneous cold starts. A
                // launch error or unwind releases the reservation via Drop.
                lease.browser = Some(Browser::connect(executable, options)?);
                #[cfg(test)]
                if let Some(entry) = self.lock().entries.iter_mut().find(|entry| entry.id == id) {
                    entry.process = lease.browser.as_ref().and_then(Browser::test_process);
                }
                let closed = self.lock().closed;
                if closed {
                    return Err(failed("Browser runtime is closed"));
                }
                return Ok(lease);
            }
            if state.waiting >= MAX_WAITERS {
                return Err(failed("Browser runtime waiting capacity exceeded"));
            }
            state.waiting += 1;
            let remaining = deadline.saturating_duration_since(Instant::now());
            let (mut state, _) = self
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(|error| error.into_inner());
            state.waiting -= 1;
            drop(state);
        }
    }

    pub(crate) fn close(&self) {
        let idle = {
            let mut state = self.lock();
            state.closed = true;
            let mut idle = Vec::new();
            state.entries.retain_mut(|entry| {
                if let Some(browser) = entry.browser.take() {
                    idle.push(browser);
                    false
                } else {
                    true
                }
            });
            self.changed.notify_all();
            idle
        };
        drop(idle);
    }

    #[cfg(test)]
    pub(super) fn processes(&self) -> Vec<(u32, std::path::PathBuf)> {
        self.lock()
            .entries
            .iter()
            .filter_map(|entry| entry.process.clone())
            .collect()
    }
    #[cfg(test)]
    pub(super) fn waiting(&self) -> usize {
        self.lock().waiting
    }
    #[cfg(test)]
    pub(super) fn expire_idle(&self) {
        let mut state = self.lock();
        for entry in &mut state.entries {
            if entry.browser.is_some() {
                entry.last_used = Instant::now()
                    .checked_sub(entry.ttl + Duration::from_secs(1))
                    .unwrap();
            }
        }
    }
}
impl Drop for Pool {
    fn drop(&mut self) {
        self.close();
    }
}

struct Identity {
    hash: Sha256,
    bytes: usize,
}
impl Write for Identity {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .filter(|count| *count <= MAX_IDENTITY_BYTES)
            .ok_or_else(|| std::io::Error::other("Browser identity limit"))?;
        self.hash.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(super) struct Lease<'a> {
    pool: &'a Pool,
    id: u64,
    persistent: bool,
    browser: Option<Browser>,
    reusable: bool,
}
impl Lease<'_> {
    pub(super) fn browser(&mut self) -> &mut Browser {
        self.browser.as_mut().expect("browser lease is initialized")
    }
    pub(super) fn finish(&mut self) -> Result<()> {
        let persistent = self.persistent;
        self.browser().finish_page(persistent)?;
        self.reusable = true;
        Ok(())
    }
}
impl Drop for Lease<'_> {
    fn drop(&mut self) {
        let mut state = self.pool.lock();
        if let Some(index) = state.entries.iter().position(|entry| entry.id == self.id) {
            if self.reusable && !state.closed && !std::thread::panicking() {
                state.entries[index].browser = self.browser.take();
                state.entries[index].last_used = Instant::now();
            } else {
                state.entries.swap_remove(index);
            }
        }
        let retiring = self.browser.is_some();
        if retiring {
            state.retiring += 1;
        }
        drop(state);
        drop(self.browser.take());
        if retiring {
            self.pool.lock().retiring -= 1;
        }
        self.pool.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn capacity_closed_and_oversized_identity_fail_without_launch() {
        assert!(Pool::new(0).is_err());
        assert!(Pool::new(9).is_err());
        let pool = Pool::new(1).unwrap();
        let executable = std::env::current_exe().unwrap();
        let url = url::Url::parse("http://example.test/a").unwrap();
        let mut options = Options::diagnostic();
        options.persistent = true;
        options.headers.insert(
            "x-private".into(),
            json!("secret".repeat(MAX_IDENTITY_BYTES)),
        );
        let error = pool
            .key(&executable, &url, &json!({}), &options)
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("resource limit") && !error.contains("secret"));
        options.headers.clear();
        pool.close();
        pool.close();
        let error = pool
            .acquire(&executable, &url, &json!({}), &options)
            .err()
            .unwrap()
            .to_string();
        assert_eq!(error, "Browser runtime is closed");
        assert!(pool.lock().entries.is_empty());
    }

    #[test]
    fn persistent_identity_separates_origins_credentials_proxy_and_initial_options() {
        let pool = Pool::new(2).unwrap();
        let executable = std::env::current_exe().unwrap();
        let first = url::Url::parse("https://example.test/a").unwrap();
        let options = || {
            let mut value = Options::diagnostic();
            value.persistent = true;
            value
        };
        let baseline = pool
            .key(&executable, &first, &json!({}), &options())
            .unwrap();
        assert!(
            baseline
                == pool
                    .key(
                        &executable,
                        &url::Url::parse("https://example.test/b").unwrap(),
                        &json!({}),
                        &options()
                    )
                    .unwrap()
        );
        for origin in [
            "http://example.test/a",
            "https://example.test:8443/a",
            "https://other.test/a",
        ] {
            assert!(
                baseline
                    != pool
                        .key(
                            &executable,
                            &url::Url::parse(origin).unwrap(),
                            &json!({}),
                            &options()
                        )
                        .unwrap()
            );
        }
        for field in 0..7 {
            let mut changed = options();
            let mut cfg = json!({});
            match field {
                0 => {
                    changed.credentials = super::super::auth::parse(
                        Some(&json!({"username":"u","password":"private"})),
                        &first,
                    )
                    .unwrap()
                }
                1 => changed.proxy = Some("http://127.0.0.1:9".into()),
                2 => changed.bypass = "different.test".into(),
                3 => changed.user_agent = Some("different user agent".into()),
                4 => {
                    changed
                        .headers
                        .insert("authorization".into(), json!("private"));
                }
                5 => {
                    cfg = json!({"fetch":{"playwright":{"cookies":[{"name":"session","value":"private"}]}}})
                }
                _ => changed.session_ttl = Duration::from_secs(60),
            }
            assert!(baseline != pool.key(&executable, &first, &cfg, &changed).unwrap());
        }
        assert!(
            baseline
                != Pool::new(2)
                    .unwrap()
                    .key(&executable, &first, &json!({}), &options())
                    .unwrap()
        );
    }

    #[test]
    fn waiting_is_bounded_and_close_wakes_a_queued_lease() {
        let pool = Pool::new(1).unwrap();
        let executable = std::env::current_exe().unwrap();
        let url = url::Url::parse("http://example.test/a").unwrap();
        let options = Options::diagnostic();
        let key = pool.key(&executable, &url, &json!({}), &options).unwrap();
        pool.lock().entries.push(Entry {
            id: 0,
            key,
            persistent: false,
            browser: None,
            last_used: Instant::now(),
            ttl: ISOLATED_IDLE,
            process: None,
        });
        std::thread::scope(|scope| {
            let waiter = scope.spawn(|| {
                pool.acquire(&executable, &url, &json!({}), &options)
                    .err()
                    .unwrap()
                    .to_string()
            });
            let deadline = Instant::now() + Duration::from_secs(3);
            while pool.waiting() == 0 {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(2));
            }
            pool.close();
            assert_eq!(waiter.join().unwrap(), "Browser runtime is closed");
        });
        assert_eq!(pool.waiting(), 0);
        let pool = Pool::new(1).unwrap();
        pool.lock().entries.push(Entry {
            id: 0,
            key,
            persistent: false,
            browser: None,
            last_used: Instant::now(),
            ttl: ISOLATED_IDLE,
            process: None,
        });
        pool.lock().waiting = MAX_WAITERS;
        assert_eq!(
            pool.acquire(&executable, &url, &json!({}), &options)
                .err()
                .unwrap()
                .to_string(),
            "Browser runtime waiting capacity exceeded"
        );
    }
}
