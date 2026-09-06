use super::*;

#[test]
fn bounded_input_retired_behind_writer_lock_never_reaches_transport() {
    let (socket, peer) = UnixStream::pair().unwrap();
    peer.set_read_timeout(Some(Duration::from_millis(150)))
        .unwrap();
    let shutdown = socket.try_clone().unwrap();
    let connection = transport::Connection::from_negotiated(
        SyncWire::new(socket),
        |socket| Ok((socket.try_clone()?, socket)),
        move || {
            let _ = shutdown.shutdown(std::net::Shutdown::Both);
        },
    )
    .unwrap();
    let wire = MultiplexedWire::start(connection).unwrap();
    let writer = wire.writer.lock().unwrap();
    let active = Arc::new(AtomicBool::new(true));
    let sender = wire.clone();
    let valid = active.clone();
    let task = thread::spawn(move || {
        sender.exchange_checked(
            &ClientRequest::for_client(Uuid::new_v4(), Request::Ping),
            || valid.load(Ordering::Acquire),
        )
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    while wire.pending.lock().unwrap().is_empty() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    active.store(false, Ordering::Release);
    drop(writer);
    assert!(task.join().unwrap().is_err());
    assert!(wire.pending.lock().unwrap().is_empty());
    let mut server = SyncWire::new(peer);
    assert!(
        server
            .receive_json::<ClientRequest>(FrameKind::Request, 0)
            .is_err()
    );
    server.get_ref().shutdown(std::net::Shutdown::Both).unwrap();
}
