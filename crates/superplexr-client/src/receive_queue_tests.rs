use super::*;
use superplexr_protocol::wire_v3::FrameHeader;

fn frame(id: u32, sequence: u64, capacity: usize) -> ReceivedFrame {
    ReceivedFrame {
        header: FrameHeader {
            kind: FrameKind::EventBatch,
            flags: 0,
            stream_id: id,
            stream_sequence: sequence,
            payload_length: 0,
            uncompressed_length: 0,
        },
        payload: Vec::with_capacity(capacity),
    }
}

fn budget() -> Arc<Budget> {
    Arc::new(Budget::new(64 * 1024 * 1024, 8192, 1024))
}

#[test]
fn early_and_attached_frames_share_one_fifo_and_close_drains_in_order() {
    let budget = budget();
    let streams = Streams::new(budget.clone());
    streams.announce(7).unwrap();
    streams.push(frame(7, 1, 20)).unwrap();
    let receive = streams.attach(7).unwrap();
    assert!(streams.attach(7).is_err());
    streams.push(frame(7, 2, 30)).unwrap();
    let mut end = frame(7, 3, 0);
    end.header.kind = FrameKind::Close;
    streams.push(end).unwrap();
    assert!(streams.push(frame(7, 4, 1)).is_err());
    for sequence in 1..=3 {
        assert_eq!(receive.recv().unwrap().header.stream_sequence, sequence);
    }
    assert!(receive.recv().is_err());
    assert_eq!(budget.bytes.load(Ordering::Acquire), 0);
    assert_eq!(budget.frames.load(Ordering::Acquire), 0);
    streams.finish(7);
    assert_eq!(budget.streams.load(Ordering::Acquire), 0);
}

#[test]
fn overflow_then_retirement_discards_stale_frames_and_releases_all_charges() {
    let budget = budget();
    let streams = Streams::new(budget.clone());
    streams.announce(1).unwrap();
    let receive = streams.attach(1).unwrap();
    for sequence in 0..STREAM_FRAMES as u64 {
        streams.push(frame(1, sequence, 8)).unwrap();
    }
    assert!(streams.push(frame(1, 129, 0)).is_err());
    streams.close("overflow");
    assert_eq!(receive.recv().unwrap_err(), "overflow");
    assert_eq!(budget.bytes.load(Ordering::Acquire), 0);
    assert_eq!(budget.frames.load(Ordering::Acquire), 0);
    assert_eq!(budget.streams.load(Ordering::Acquire), 0);
    assert!(streams.announce(2).is_err());
}

#[test]
fn payload_capacity_not_length_is_charged_per_stream_and_per_wire() {
    let budget = budget();
    let streams = Streams::new(budget.clone());
    for id in 1..=5 {
        streams.announce(id).unwrap();
    }
    assert!(streams.push(frame(1, 1, STREAM_BYTES + 1)).is_err());
    for id in 1..=4 {
        streams.push(frame(id, 1, STREAM_BYTES)).unwrap();
    }
    assert_eq!(budget.bytes.load(Ordering::Acquire), WIRE_BYTES);
    assert!(streams.push(frame(1, 2, 1)).is_err());
    assert!(streams.push(frame(5, 1, 1)).is_err());
    let receive = streams.attach(1).unwrap();
    drop(receive.recv().unwrap());
    streams.push(frame(5, 1, STREAM_BYTES)).unwrap();
    streams.cancel(5);
    assert_eq!(budget.bytes.load(Ordering::Acquire), 3 * STREAM_BYTES);
    streams.close("done");
    assert_eq!(budget.bytes.load(Ordering::Acquire), 0);
}

#[test]
fn process_budget_is_shared_across_connections_and_failed_admission_rolls_back() {
    let budget = Arc::new(Budget::new(16, 2, 2));
    let a = Streams::new(budget.clone());
    let b = Streams::new(budget.clone());
    a.announce(1).unwrap();
    b.announce(1).unwrap();
    assert!(b.announce(2).is_err());
    a.push(frame(1, 1, 8)).unwrap();
    b.push(frame(1, 1, 8)).unwrap();
    assert!(b.push(frame(1, 2, 1)).is_err());
    assert!(b.push(frame(1, 2, 0)).is_err());
    a.finish(1);
    b.push(frame(1, 2, 4)).unwrap();
    assert!(b.push(frame(1, 3, 4)).is_err());
    assert_eq!(budget.bytes.load(Ordering::Acquire), 12);
    assert_eq!(budget.frames.load(Ordering::Acquire), 2);
    a.announce(2).unwrap();
    drop(a);
    drop(b);
    assert_eq!(budget.bytes.load(Ordering::Acquire), 0);
    assert_eq!(budget.frames.load(Ordering::Acquire), 0);
    assert_eq!(budget.streams.load(Ordering::Acquire), 0);
}

#[test]
fn cancellation_discards_late_frames_until_ack_without_an_orphan_queue() {
    let budget = budget();
    let streams = Streams::new(budget.clone());
    streams.announce(1).unwrap();
    let receive = streams.attach(1).unwrap();
    streams.push(frame(1, 1, 20)).unwrap();
    drop(receive);
    streams.push(frame(1, 2, STREAM_BYTES + 1)).unwrap();
    assert_eq!(budget.bytes.load(Ordering::Acquire), 0);
    assert_eq!(budget.streams.load(Ordering::Acquire), 1);
    assert!(streams.attach(1).is_err());
    streams.finish(1);
    assert!(streams.push(frame(1, 3, 8)).is_err());
    let mut end = frame(9, 1, 0);
    end.header.kind = FrameKind::Close;
    streams.push(end).unwrap();
    assert_eq!(budget.streams.load(Ordering::Acquire), 0);
}

#[test]
fn mailbox_count_is_bounded_even_when_streams_never_deliver_data() {
    let budget = budget();
    let streams = Streams::new(budget.clone());
    assert!(streams.announce(0).is_err());
    for id in 1..=WIRE_STREAMS as u32 {
        streams.announce(id).unwrap();
    }
    assert!(streams.announce(1).is_err());
    assert!(streams.announce(257).is_err());
    streams.cancel(1);
    assert!(streams.announce(257).is_err());
    streams.finish(1);
    streams.announce(257).unwrap();
    streams.close("done");
    assert_eq!(budget.streams.load(Ordering::Acquire), 0);
}

#[test]
fn close_wakes_a_blocked_receiver_without_waiting_for_any_producer() {
    let streams = Streams::new(budget());
    streams.announce(1).unwrap();
    let receive = streams.attach(1).unwrap();
    let (send, result) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || send.send(receive.recv().unwrap_err()).unwrap());
    streams.close("closed");
    assert_eq!(
        result
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap(),
        "closed"
    );
    worker.join().unwrap();
}
