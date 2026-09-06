//! Transport adapters establish a trusted byte stream; domain requests and
//! subscription/reconnect behavior stay in ControlClient. This does not add
//! network authentication: use an authenticated tunnel for remote streams.
use crate::ClientError;
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use ultraplexr_protocol::{
    ProtocolError,
    wire_v3::{BlockingWireReader, BlockingWireWriter, Hello, SyncWire},
};
use uuid::Uuid;

pub type TransportReader = BlockingWireReader<Box<dyn Read + Send>>;
pub type TransportWriter = BlockingWireWriter<Box<dyn Write + Send>>;
pub(crate) type Interrupt = Arc<dyn Fn() + Send + Sync>;

/// A fresh, negotiated wire connection. Adapters must bound handshake I/O and
/// return independent read/write halves, allowing requests during subscriptions.
pub struct Connection {
    pub(crate) reader: TransportReader,
    pub(crate) writer: TransportWriter,
    pub(crate) features: Vec<String>,
    pub(crate) interrupt: Interrupt,
}

impl Connection {
    /// Preserve the handshake's sequence/compression state when splitting.
    /// `interrupt` MUST promptly shut down both directions, including blocked
    /// reads/writes, without taking either wire lock or waiting for a peer.
    /// It must be thread-safe, idempotent and must not panic.
    pub fn from_negotiated<S, R: Read + Send + 'static, W: Write + Send + 'static>(
        wire: SyncWire<S>,
        split: impl FnOnce(S) -> std::io::Result<(R, W)>,
        interrupt: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, ClientError> {
        let features = wire.negotiated_features().to_vec();
        let (reader, writer) = wire.split_with(|io| {
            let (reader, writer) = split(io)?;
            Ok((
                Box::new(reader) as Box<dyn Read + Send>,
                Box::new(writer) as Box<dyn Write + Send>,
            ))
        })?;
        Ok(Self {
            reader,
            writer,
            features,
            interrupt: Arc::new(interrupt),
        })
    }
}

/// Reconnectable trusted transport. Share authorization is still performed by
/// the daemon on each request; adapters must not interpret or broaden it.
pub trait Connector: Send + Sync {
    fn connect(&self, client_id: Uuid) -> Result<Connection, ClientError>;
}

pub struct UnixSocketConnector(pub PathBuf);

impl Connector for UnixSocketConnector {
    fn connect(&self, client_id: Uuid) -> Result<Connection, ClientError> {
        let stream = UnixStream::connect(&self.0)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut wire = SyncWire::new(stream);
        wire.client_handshake(&Hello::new(client_id, "native", client_id))
            .map_err(ProtocolError::from)?;
        wire.get_ref().set_read_timeout(None)?;
        wire.get_ref().set_write_timeout(None)?;
        let shutdown = wire.get_ref().try_clone()?;
        Connection::from_negotiated(
            wire,
            |stream| Ok((stream.try_clone()?, stream)),
            move || {
                let _ = shutdown.shutdown(std::net::Shutdown::Both);
            },
        )
    }
}
