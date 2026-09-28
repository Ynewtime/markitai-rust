//! Signal handlers only set lock-free atomics; the coordinator performs I/O.
use std::sync::atomic::{AtomicI32, Ordering};

static INTERRUPTED: AtomicI32 = AtomicI32::new(0);

pub(crate) fn interrupted() -> Option<i32> {
    match INTERRUPTED.load(Ordering::Relaxed) {
        0 => None,
        signal => Some(signal),
    }
}

#[cfg(unix)]
extern "C" fn interrupt(signal: libc::c_int) {
    if INTERRUPTED.swap(signal, Ordering::Relaxed) != 0 {
        // A second explicit interrupt forces termination without running cleanup.
        unsafe { libc::_exit(128 + signal) }
    }
}

pub(crate) struct Guard {
    #[cfg(unix)]
    previous: [(libc::c_int, libc::sigaction); 2],
}

impl Guard {
    pub(crate) fn install() -> std::io::Result<Self> {
        INTERRUPTED.store(0, Ordering::Relaxed);
        #[cfg(unix)]
        {
            // sigaction's all-zero representation is valid before sigemptyset;
            // only the C signal handler pointer is installed, with no Rust state.
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            action.sa_sigaction = interrupt as *const () as libc::sighandler_t;
            action.sa_flags = libc::SA_RESTART;
            unsafe {
                libc::sigemptyset(&mut action.sa_mask);
            }
            let mut previous = [
                (libc::SIGINT, unsafe { std::mem::zeroed() }),
                (libc::SIGTERM, unsafe { std::mem::zeroed() }),
            ];
            for index in 0..previous.len() {
                let (signal, old) = &mut previous[index];
                if unsafe { libc::sigaction(*signal, &action, old) } != 0 {
                    let error = std::io::Error::last_os_error();
                    for (signal, old) in &previous[..index] {
                        unsafe {
                            libc::sigaction(*signal, old, std::ptr::null_mut());
                        }
                    }
                    return Err(error);
                }
            }
            Ok(Self { previous })
        }
        #[cfg(not(unix))]
        {
            Err(std::io::Error::other(
                "Controlled recovery interruption is not supported on this platform yet",
            ))
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        #[cfg(unix)]
        for (signal, old) in &self.previous {
            unsafe {
                libc::sigaction(*signal, old, std::ptr::null_mut());
            }
        }
    }
}
