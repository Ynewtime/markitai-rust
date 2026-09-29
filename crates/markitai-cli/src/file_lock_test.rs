use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::time::{Duration, Instant};

pub(super) struct InheritedChild {
    pid: libc::pid_t,
    ready: File,
    release: File,
}
impl InheritedChild {
    pub(super) fn start() -> Self {
        let (mut ready, mut release) = ([-1; 2], [-1; 2]);
        assert_eq!(unsafe { libc::pipe(ready.as_mut_ptr()) }, 0);
        assert_eq!(unsafe { libc::pipe(release.as_mut_ptr()) }, 0);
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0);
        if pid == 0 {
            // No Rust allocation, lock, or destructor runs in the fork child.
            // Its inherited checkpoint descriptor stays open until _exit.
            unsafe {
                libc::close(ready[0]);
                libc::close(release[1]);
                let mut byte = 1_u8;
                loop {
                    let count = libc::write(ready[1], (&byte as *const u8).cast(), 1);
                    if count == 1 {
                        break;
                    }
                    if count < 0 && *libc::__errno_location() == libc::EINTR {
                        continue;
                    }
                    libc::_exit(91);
                }
                loop {
                    let count = libc::read(release[0], (&mut byte as *mut u8).cast(), 1);
                    if count == 1 {
                        libc::_exit(0);
                    }
                    if count < 0 && *libc::__errno_location() == libc::EINTR {
                        continue;
                    }
                    libc::_exit(92);
                }
            }
        }
        unsafe {
            libc::close(ready[1]);
            libc::close(release[0]);
        }
        let mut child = Self {
            pid,
            ready: unsafe { File::from_raw_fd(ready[0]) },
            release: unsafe { File::from_raw_fd(release[1]) },
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "fork child did not become ready");
            let mut descriptor = libc::pollfd {
                fd: child.ready.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let result =
                unsafe { libc::poll(&mut descriptor, 1, remaining.as_millis().max(1) as i32) };
            if result < 0
                && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
            {
                continue;
            }
            assert!(result > 0, "fork child readiness failed");
            let mut byte = [0];
            child.ready.read_exact(&mut byte).unwrap();
            assert_eq!(byte, [1]);
            return child;
        }
    }
    pub(super) fn finish(&mut self) {
        self.release.write_all(&[1]).unwrap();
        let mut status = 0;
        loop {
            let result = unsafe { libc::waitpid(self.pid, &mut status, 0) };
            if result < 0
                && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
            {
                continue;
            }
            assert_eq!(result, self.pid);
            self.pid = -1;
            assert_eq!(status, 0);
            return;
        }
    }
}
impl Drop for InheritedChild {
    fn drop(&mut self) {
        if self.pid > 0 {
            unsafe {
                libc::kill(self.pid, libc::SIGKILL);
            }
            loop {
                let result = unsafe { libc::waitpid(self.pid, std::ptr::null_mut(), 0) };
                if result < 0
                    && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
                {
                    continue;
                }
                break;
            }
        }
    }
}
