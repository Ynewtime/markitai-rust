include!("bounded_read.rs");

#[test]
fn interrupted_read_retries_without_changing_the_result_or_deadline() {
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut remaining_seen = Vec::new();
    let result = retry_until(deadline, |remaining| {
        remaining_seen.push(remaining);
        if remaining_seen.len() == 1 {
            Err(io::Error::from(io::ErrorKind::Interrupted))
        } else {
            Ok(3)
        }
    }).unwrap();
    assert_eq!(result, 3);
    assert_eq!(remaining_seen.len(), 2);
    assert!(remaining_seen[1] <= remaining_seen[0]);
}

#[test]
fn an_interrupt_cannot_restart_an_expired_request_deadline() {
    let start = Instant::now();
    let deadline = start + Duration::from_millis(10);
    let mut clock_calls = 0;
    let mut attempts = 0;
    let result: io::Result<usize> = retry_until_with(deadline, || {
        clock_calls += 1;
        if clock_calls == 1 { start } else { deadline }
    }, |_| {
        attempts += 1;
        Err(io::Error::from(io::ErrorKind::Interrupted))
    });
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
    assert_eq!(attempts, 1);
}

#[test]
fn eof_and_noninterrupted_errors_are_not_retried() {
    for expected in [io::ErrorKind::ConnectionReset, io::ErrorKind::WouldBlock, io::ErrorKind::TimedOut] {
        let mut attempts = 0;
        let result: io::Result<usize> = retry_until(Instant::now() + Duration::from_secs(1), |_| {
            attempts += 1;
            Err(io::Error::from(expected))
        });
        assert_eq!(result.unwrap_err().kind(), expected);
        assert_eq!(attempts, 1);
    }
    let mut attempts = 0;
    let result = retry_until(Instant::now() + Duration::from_secs(1), |_| {
        attempts += 1;
        Ok(0)
    }).unwrap();
    assert_eq!((result, attempts), (0, 1));
}

#[test]
fn actual_socket_bytes_and_eof_remain_distinct() {
    use std::io::Write;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut server, _) = listener.accept().unwrap();
    server.write_all(b"abc").unwrap();
    drop(server);
    let mut reader = Reader::new(&client, Instant::now() + Duration::from_secs(1));
    let mut bytes = [0; 3];
    reader.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"abc");
    assert_eq!(reader.read(&mut bytes).unwrap(), 0);
}
