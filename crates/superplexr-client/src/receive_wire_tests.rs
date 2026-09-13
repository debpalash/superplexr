use super::*;
use std::io::Read;

fn pair() -> (Arc<MultiplexedWire>, SyncWire<UnixStream>) {
    let (socket, peer) = UnixStream::pair().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    peer.set_write_timeout(Some(Duration::from_secs(2)))
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
    (
        MultiplexedWire::start(connection).unwrap(),
        SyncWire::new(peer),
    )
}

fn accept(wire: &Arc<MultiplexedWire>, peer: &mut SyncWire<UnixStream>) {
    let client = wire.clone();
    let caller = thread::spawn(move || {
        client.exchange(&ClientRequest::for_client(
            Uuid::new_v4(),
            Request::SubscribeMissions,
        ))
    });
    let request: ClientRequest = peer.receive_json(FrameKind::Request, 0).unwrap();
    peer.send_json(
        FrameKind::Response,
        0,
        &ServerResponse::success(
            request.request_id,
            ResponseBody::MissionSubscriptionAccepted { stream_id: 7 },
        ),
    )
    .unwrap();
    caller.join().unwrap().unwrap();
}

fn ping(wire: &Arc<MultiplexedWire>, peer: &mut SyncWire<UnixStream>) {
    let client = wire.clone();
    let caller = thread::spawn(move || {
        client.exchange(&ClientRequest::for_client(Uuid::new_v4(), Request::Ping))
    });
    let request: ClientRequest = peer.receive_json(FrameKind::Request, 0).unwrap();
    peer.send_json(
        FrameKind::Response,
        0,
        &ServerResponse::success(request.request_id, ResponseBody::Pong),
    )
    .unwrap();
    caller.join().unwrap().unwrap();
}

#[test]
fn final_owner_drop_interrupts_the_idle_socket_reader() {
    let (wire, peer) = pair();
    let weak = Arc::downgrade(&wire);
    drop(wire);
    assert!(weak.upgrade().is_none());
    assert_eq!(peer.get_ref().read(&mut [0]).unwrap(), 0);
}

#[test]
fn oversized_record_is_permanent_before_receiver_attach_and_stops_metadata_reconnect() {
    let (wire, mut peer) = pair();
    accept(&wire, &mut peer);
    peer.send_bytes(FrameKind::EventBatch, 7, vec![b' '; 9 * 1024 * 1024])
        .unwrap();
    assert_eq!(peer.get_ref().read(&mut [0]).unwrap(), 0);
    let error = match wire.subscribe(
        7,
        ClientRequest::for_client(Uuid::new_v4(), Request::Unsubscribe { stream_id: 7 }),
    ) {
        Ok(_) => panic!("oversized record cannot attach"),
        Err(error) => error,
    };
    assert!(
        matches!(error, ClientError::ReceiveLimit { bytes, limit } if bytes == 9 * 1024 * 1024 && limit == 8 * 1024 * 1024)
    );
    assert!(matches!(
        wire.exchange(&ClientRequest::for_client(
            Uuid::new_v4(),
            Request::SubscribeMissions
        )),
        Err(ClientError::ReceiveLimit { .. })
    ));

    let (wire, mut peer) = pair();
    accept(&wire, &mut peer);
    let subscription = wire
        .subscribe(
            7,
            ClientRequest::for_client(Uuid::new_v4(), Request::Unsubscribe { stream_id: 7 }),
        )
        .unwrap();
    let receiver = MetadataReceiver::new(
        subscription,
        || panic!("permanent limit must not reconnect"),
        |bytes| serde_json::from_slice::<serde_json::Value>(bytes),
    );
    peer.send_bytes(FrameKind::EventBatch, 7, vec![b' '; 9 * 1024 * 1024])
        .unwrap();
    for _ in 0..3 {
        assert!(matches!(
            receiver.recv_delivery(),
            Err(ClientError::ReceiveLimit { .. })
        ));
    }
}

#[test]
fn oversized_record_preserves_failure_for_rpc_waiting_on_writer() {
    let (wire, mut peer) = pair();
    accept(&wire, &mut peer);
    let occupied = wire.writer.lock().unwrap();
    let client = wire.clone();
    let caller = thread::spawn(move || {
        client.exchange(&ClientRequest::for_client(
            Uuid::new_v4(),
            Request::SubscribeMissions,
        ))
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while wire.pending.lock().unwrap().is_empty() {
        assert!(std::time::Instant::now() < deadline);
        thread::yield_now();
    }
    peer.send_bytes(FrameKind::EventBatch, 7, vec![b' '; 9 * 1024 * 1024])
        .unwrap();
    assert_eq!(peer.get_ref().read(&mut [0]).unwrap(), 0);
    drop(occupied);
    assert!(matches!(
        caller.join().unwrap(),
        Err(ClientError::ReceiveLimit { .. })
    ));
}

#[test]
fn ack_dispatch_registers_early_frames_in_the_same_fifo_as_attached_frames() {
    let (wire, mut peer) = pair();
    accept(&wire, &mut peer);
    peer.send_json(FrameKind::EventBatch, 7, &1).unwrap();
    // An acknowledged Ping is a deterministic dispatcher barrier: frame 1
    // has been dispatched, but the subscription caller has not attached yet.
    ping(&wire, &mut peer);
    let receive = wire.streams.attach(7).unwrap();
    peer.send_json(FrameKind::EventBatch, 7, &2).unwrap();
    ping(&wire, &mut peer);
    assert_eq!(receive.recv().unwrap().payload, b"1");
    assert_eq!(receive.recv().unwrap().payload, b"2");
}

#[test]
fn overflow_interrupts_peer_and_pending_rpc_even_with_writer_mutex_occupied() {
    let (wire, mut peer) = pair();
    accept(&wire, &mut peer);
    let receive = wire.streams.attach(7).unwrap();
    let client = wire.clone();
    let (send, result) = std::sync::mpsc::sync_channel(1);
    let caller = thread::spawn(move || {
        send.send(client.exchange(&ClientRequest::for_client(Uuid::new_v4(), Request::Ping)))
            .unwrap()
    });
    let _: ClientRequest = peer.receive_json(FrameKind::Request, 0).unwrap();
    let occupied = wire.writer.lock().unwrap();
    for n in 0..129 {
        peer.send_json(FrameKind::EventBatch, 7, &n).unwrap();
    }
    let error = result
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("receive queue limit"), "{error}");
    assert!(wire.closed.load(Ordering::Acquire));
    assert!(
        receive.recv().is_err(),
        "queued data must not survive retirement"
    );
    assert_eq!(peer.get_ref().read(&mut [0]).unwrap(), 0);
    drop(occupied);
    caller.join().unwrap();
}

#[test]
fn unannounced_events_and_connection_close_retire_the_wire() {
    for (kind, id) in [(FrameKind::EventBatch, 99), (FrameKind::Close, 0)] {
        let (wire, mut peer) = pair();
        peer.send_json(kind, id, &0).unwrap();
        assert_eq!(peer.get_ref().read(&mut [0]).unwrap(), 0);
        assert!(wire.closed.load(Ordering::Acquire));
    }
}

#[test]
fn overflow_interrupts_an_actual_backpressured_socket_write() {
    let (wire, mut peer) = pair();
    accept(&wire, &mut peer);
    let receive = wire.streams.attach(7).unwrap();
    let client = wire.clone();
    let (send, result) = std::sync::mpsc::sync_channel(1);
    let caller = thread::spawn(move || {
        let request = ClientRequest::for_client(
            Uuid::new_v4(),
            Request::TerminalPaste {
                session_id: SessionId::new(),
                bytes: vec![b'x'; 1024 * 1024],
                confirmed: true,
            },
        );
        send.send(client.exchange(&request)).unwrap();
    });
    // Read just one header byte. The remaining multi-megabyte request cannot
    // fit in the socket buffer, so the sender is blocked inside write_all.
    assert_eq!(peer.get_ref().read(&mut [0]).unwrap(), 1);
    assert!(matches!(
        result.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    assert!(wire.writer.try_lock().is_err());
    for n in 0..129 {
        peer.send_json(FrameKind::EventBatch, 7, &n).unwrap();
    }
    assert!(
        result
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .is_err()
    );
    caller.join().unwrap();
    assert!(wire.closed.load(Ordering::Acquire));
    assert!(receive.recv().is_err());
    // Draining the already admitted fragment reaches EOF, not another frame.
    let mut fragment = Vec::new();
    peer.get_ref().read_to_end(&mut fragment).unwrap();
    assert!(fragment.len() < 1024 * 1024);
}

#[test]
fn unsubscribe_ack_retires_mailbox_without_retaining_late_events() {
    let (wire, mut peer) = pair();
    accept(&wire, &mut peer);
    let receive = wire.streams.attach(7).unwrap();
    let request = ClientRequest::for_client(Uuid::new_v4(), Request::Unsubscribe { stream_id: 7 });
    wire.unsubscribe(7, &request);
    let _: ClientRequest = peer.receive_json(FrameKind::Request, 0).unwrap();
    peer.send_json(FrameKind::EventBatch, 7, &1).unwrap();
    peer.send_json(
        FrameKind::Response,
        0,
        &ServerResponse::success(
            request.request_id,
            ResponseBody::SubscriptionEnded { stream_id: 7 },
        ),
    )
    .unwrap();
    ping(&wire, &mut peer);
    assert!(receive.recv().is_err());
    // With the ACK's boundary passed, post-retirement data is a protocol fault.
    peer.send_json(FrameKind::EventBatch, 7, &2).unwrap();
    assert_eq!(peer.get_ref().read(&mut [0]).unwrap(), 0);
    assert!(wire.closed.load(Ordering::Acquire));
}
