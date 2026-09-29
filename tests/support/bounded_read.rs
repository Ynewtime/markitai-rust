// Test-only socket reader. One deadline covers every read in a fixture request,
// including interrupted reads; EOF and all other errors retain their meaning.
use std::io::{self, Read};
use std::net::TcpStream;
use std::time::{Duration, Instant};

pub struct Reader<'a> {
    stream: &'a TcpStream,
    deadline: Instant,
}

impl<'a> Reader<'a> {
    pub fn new(stream: &'a TcpStream, deadline: Instant) -> Self {
        Self { stream, deadline }
    }
}

impl Read for Reader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        retry_until(self.deadline, |remaining| {
            self.stream.set_read_timeout(Some(remaining))?;
            let mut stream = self.stream;
            stream.read(buffer)
        })
    }
}

fn retry_until<T>(deadline: Instant, operation: impl FnMut(Duration) -> io::Result<T>) -> io::Result<T> {
    retry_until_with(deadline, Instant::now, operation)
}

fn retry_until_with<T>(deadline: Instant, mut now: impl FnMut() -> Instant, mut operation: impl FnMut(Duration) -> io::Result<T>) -> io::Result<T> {
    loop {
        let remaining = deadline.saturating_duration_since(now());
        if remaining.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "fixture request deadline exceeded"));
        }
        match operation(remaining) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => return result,
        }
    }
}
