use super::{Result, failure};
use std::{
    fs,
    path::Path,
    process::{Child, Command, Stdio},
    sync::{Condvar, Mutex},
    time::{Duration, Instant},
};

static ACTIVE: Mutex<usize> = Mutex::new(0);
static READY: Condvar = Condvar::new();

pub(super) struct Permit;
impl Drop for Permit {
    fn drop(&mut self) {
        let mut active = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
        *active -= 1;
        READY.notify_one();
    }
}

pub(super) fn acquire(deadline: Instant) -> Result<Permit> {
    let mut active = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
    while *active >= 2 {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| failure("timed out waiting for an Office export slot"))?;
        let (next, status) = READY
            .wait_timeout(active, remaining)
            .unwrap_or_else(|e| e.into_inner());
        active = next;
        if status.timed_out() {
            return Err(failure("timed out waiting for an Office export slot"));
        }
    }
    *active += 1;
    Ok(Permit)
}

struct Running {
    child: Child,
    reaped: bool,
}
impl Drop for Running {
    fn drop(&mut self) {
        if self.reaped {
            return;
        }
        // A dedicated process group also terminates the real soffice behind a wrapper.
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.child.id() as libc::pid_t), libc::SIGKILL);
        }
        #[cfg(windows)]
        {
            let _ = Command::new("taskkill")
                .args(["/F", "/T", "/PID", &self.child.id().to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn check_output(directory: &Path, limit: u64) -> Result<()> {
    let mut bytes = 0u64;
    let mut entries = 0usize;
    for entry in fs::read_dir(directory)? {
        entries += 1;
        let metadata = fs::symlink_metadata(entry?.path())?;
        if entries > 8 || !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(failure("export produced unexpected non-regular output"));
        }
        bytes = bytes
            .checked_add(metadata.len())
            .ok_or_else(|| failure("export size overflow"))?;
        if bytes > limit {
            return Err(failure("export exceeded its byte limit"));
        }
    }
    Ok(())
}

pub(super) fn convert(
    program: &Path,
    input: &Path,
    output: &Path,
    profile: &Path,
    filter: &str,
    deadline: Instant,
    limit: u64,
) -> Result<()> {
    if Instant::now() >= deadline {
        return Err(failure("Office export timed out"));
    }
    let profile_url = url::Url::from_directory_path(profile)
        .map_err(|_| failure("invalid private profile path"))?;
    let mut command = Command::new(program);
    command
        .args([
            "--headless",
            "--nologo",
            "--nodefault",
            "--nolockcheck",
            "--nofirststartwizard",
            "--norestore",
        ])
        .arg(format!("-env:UserInstallation={profile_url}"))
        .arg("--convert-to")
        .arg(filter)
        .arg("--outdir")
        .arg(output)
        .arg(input)
        .current_dir(profile)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Do not inherit credentials, model settings, Python hooks or the user's LO profile.
    // PATH is needed by platform launchers; HOME itself is never reassigned.
    let path = std::env::var_os("PATH");
    let system_root = std::env::var_os("SystemRoot");
    command.env_clear();
    if let Some(value) = path {
        command.env("PATH", value);
    }
    if let Some(value) = system_root {
        command.env("SystemRoot", value);
    }
    command
        .env("TMPDIR", profile)
        .env("TMP", profile)
        .env("TEMP", profile)
        .env("XDG_CACHE_HOME", profile.join("cache"));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut running = Running {
        child: command
            .spawn()
            .map_err(|_| failure("LibreOffice could not be started"))?,
        reaped: false,
    };
    loop {
        check_output(output, limit)?;
        if let Some(status) = running
            .child
            .try_wait()
            .map_err(|_| failure("LibreOffice process status is unavailable"))?
        {
            running.reaped = true;
            if !status.success() {
                return Err(failure("LibreOffice export failed"));
            }
            check_output(output, limit)?;
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(failure("Office export timed out"));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
