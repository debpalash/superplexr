//! Bounded native client search receiver on the shared wire. No relay thread,
//! implicit replay, or capability migration to a reconnected transport.
use super::*;
use superplexr_protocol::search_stream::{
    FEATURE, MAX_CONNECTION_SEARCHES, MAX_PAGE_MATCHES, SearchPage,
};

#[derive(Clone)]
pub struct SearchCancel {
    wire: Arc<MultiplexedWire>,
    request: ClientRequest,
    cancelled: Arc<AtomicBool>,
    search_id: Uuid,
}
impl SearchCancel {
    /// Observe loss of this exact transport without reconnecting or doing I/O.
    /// Useful when a frontend is backpressured and cannot ask for another page.
    pub fn connection_closed(&self) -> bool {
        self.wire.closed.load(Ordering::Acquire)
    }

    /// Retire this exact connection's search and wake a pending receiver.
    /// As with other blocking client writes, call from a frontend worker.
    pub fn cancel(&self) {
        if self.cancelled.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Ok(mut searches) = self.wire.searches.lock() {
            searches.remove(&self.search_id);
        }
        if !self.wire.closed.load(Ordering::Acquire)
            && let Ok(mut writer) = self.wire.writer.lock()
        {
            let _ = writer.send_json(FrameKind::Request, 0, &self.request);
        }
    }
}

/// Pages are acknowledged only when the caller asks for the next page. At most
/// one data page and one terminal error can be queued. Dropping cancels; no
/// reconnect/retry can combine results from different live-history reads.
pub struct SearchStream {
    cancel: SearchCancel,
    receive: Receiver<ReceivedFrame>,
    ack: ClientRequest,
    session_id: SessionId,
    stream_id: u32,
    next_sequence: u64,
    acknowledge: u64,
    complete: bool,
}
impl SearchStream {
    pub fn cancellation(&self) -> SearchCancel {
        self.cancel.clone()
    }

    pub fn next_page(&mut self) -> Result<Option<SearchPage>, ClientError> {
        if self.complete {
            return Ok(None);
        }
        let result = self.receive_page();
        if result.is_err() {
            self.cancel.cancel();
        }
        result
    }
    fn receive_page(&mut self) -> Result<Option<SearchPage>, ClientError> {
        if self.cancel.cancelled.load(Ordering::Acquire) || self.cancel.connection_closed() {
            return Err(disconnected_error("search cancelled"));
        }
        if self.acknowledge != 0 {
            self.ack.request_id = Uuid::new_v4();
            self.ack.action = Request::TerminalSearchAck {
                search_id: self.cancel.search_id,
                sequence: self.acknowledge,
            };
            self.cancel
                .wire
                .writer
                .lock()
                .map_err(|_| ClientError::ControlPoisoned)?
                .send_json(FrameKind::Request, 0, &self.ack)
                .map_err(ProtocolError::from)?;
            self.acknowledge = 0;
        }
        let frame = self
            .receive
            .recv_timeout(Duration::from_secs(20))
            .map_err(|_| disconnected_error("search ended without a complete page"))?;
        if self.cancel.cancelled.load(Ordering::Acquire) || self.cancel.connection_closed() {
            return Err(disconnected_error("search cancelled"));
        }
        let page: SearchPage =
            serde_json::from_slice(&frame.payload).map_err(ProtocolError::from)?;
        if frame.header.stream_id != self.stream_id
            || page.session_id != self.session_id
            || page.search_id != self.cancel.search_id
            || page.sequence != self.next_sequence
            || page.matches.len() > MAX_PAGE_MATCHES
        {
            return Err(disconnected_error(
                "invalid search page identity, order or bound",
            ));
        }
        if let Some(message) = page.error {
            return Err(ClientError::Remote {
                code: "search_failed".into(),
                message,
            });
        }
        self.next_sequence += 1;
        self.complete = page.complete;
        if page.complete
            && let Ok(mut searches) = self.cancel.wire.searches.lock()
        {
            searches.remove(&self.cancel.search_id);
        }
        if !page.complete {
            self.acknowledge = page.sequence;
        }
        Ok(Some(page))
    }
}
impl Drop for SearchStream {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl DaemonSession {
    /// Start a negotiated, acknowledged page stream on the current connection.
    /// Older runtimes return an explicit unsupported-feature error locally.
    pub fn search_pages(
        &self,
        query: impl Into<String>,
        case_sensitive: bool,
        limit: usize,
    ) -> Result<SearchStream, ClientError> {
        let wire = self.control.subscription_wire()?;
        if !wire.features.iter().any(|feature| feature == FEATURE) {
            return Err(ClientError::Remote {
                code: "unsupported_search_stream".into(),
                message: "runtime does not advertise acknowledged search pages".into(),
            });
        }
        let search_id = Uuid::new_v4();
        let request = self.control.client_request(Request::TerminalSearchStart {
            search_id,
            session_id: self.session_id,
            query: query.into(),
            case_sensitive,
            limit,
        });
        let cancel = SearchCancel {
            wire: wire.clone(),
            request: self
                .control
                .client_request(Request::TerminalSearchCancel { search_id }),
            cancelled: Arc::new(AtomicBool::new(false)),
            search_id,
        };
        let (send, receive) = std::sync::mpsc::sync_channel(2);
        {
            let mut searches = wire
                .searches
                .lock()
                .map_err(|_| ClientError::ControlPoisoned)?;
            if searches.len() >= MAX_CONNECTION_SEARCHES {
                return Err(disconnected_error(
                    "four searches already active on this connection",
                ));
            }
            searches.insert(search_id, send);
        }
        let mut search = SearchStream {
            cancel,
            receive,
            ack: self.control.client_request(Request::TerminalSearchAck {
                search_id,
                sequence: 0,
            }),
            session_id: self.session_id,
            stream_id: 0,
            next_sequence: 1,
            acknowledge: 0,
            complete: false,
        };
        match decode_response(&request, wire.exchange(&request)?)? {
            ResponseBody::TerminalSearchStarted {
                search_id: actual,
                stream_id,
            } if actual == search_id && stream_id != 0 => {
                search.stream_id = stream_id;
                Ok(search)
            }
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }
}
