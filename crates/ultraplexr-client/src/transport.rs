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

/// A paired device reaching a runtime over its TLS gateway. The endpoint is
/// settled in place: a pairing code is exchanged once, then cleared, and the
/// minted token is kept for reconnects and reported via [`Self::take_minted_token`].
pub struct GatewayConnector {
    endpoint: std::sync::Mutex<crate::gateway::GatewayEndpoint>,
    minted: std::sync::Mutex<Option<String>>,
}

impl GatewayConnector {
    #[must_use]
    pub fn new(endpoint: crate::gateway::GatewayEndpoint) -> Self {
        Self {
            endpoint: std::sync::Mutex::new(endpoint),
            minted: std::sync::Mutex::new(None),
        }
    }

    /// The token minted by the latest pairing, if any. Take it once: the
    /// runtime never sends it again.
    pub fn take_minted_token(&self) -> Option<String> {
        self.minted.lock().ok()?.take()
    }
}

impl Connector for GatewayConnector {
    fn connect(&self, client_id: Uuid) -> Result<Connection, ClientError> {
        use crate::gateway::GatewayEndpoint;
        use ultraplexr_protocol::wire_v3::{
            BlockingWireReader, BlockingWireWriter, Close, FrameKind, Hello, Welcome,
        };

        let endpoint: GatewayEndpoint = self
            .endpoint
            .lock()
            .map_err(|_| ClientError::ControlPoisoned)?
            .clone();
        let (read_half, write_half) =
            crate::gateway::connect(&endpoint).map_err(ClientError::Io)?;
        let shutdown_read = read_half.shutdown_handle();
        let shutdown_write = write_half.shutdown_handle();
        let mut reader =
            BlockingWireReader::new(Box::new(read_half) as Box<dyn std::io::Read + Send>);
        let mut writer =
            BlockingWireWriter::new(Box::new(write_half) as Box<dyn std::io::Write + Send>);
        let mut hello = Hello::new(client_id, "device", endpoint.device_id);
        hello.device_token = endpoint.device_token.clone();
        hello.pairing_code = endpoint.pairing_code.clone();
        writer
            .send_json(FrameKind::Hello, 0, &hello)
            .map_err(ultraplexr_protocol::ProtocolError::from)?;
        let frame = reader
            .receive()
            .map_err(ultraplexr_protocol::ProtocolError::from)?;
        let welcome: Welcome = match frame.header.kind {
            FrameKind::Welcome => serde_json::from_slice(&frame.payload)
                .map_err(ultraplexr_protocol::ProtocolError::from)?,
            FrameKind::Close => {
                let close: Close = serde_json::from_slice(&frame.payload)
                    .map_err(ultraplexr_protocol::ProtocolError::from)?;
                return Err(ClientError::GatewayRefused {
                    code: close.code,
                    message: close.message,
                });
            }
            other => {
                return Err(ClientError::Io(std::io::Error::other(format!(
                    "gateway answered the handshake with {other:?}"
                ))));
            }
        };
        if welcome.device_token.is_some() {
            if let Ok(mut settled) = self.endpoint.lock() {
                settled.pairing_code = None;
                settled.device_token.clone_from(&welcome.device_token);
            }
            if let Ok(mut minted) = self.minted.lock() {
                *minted = welcome.device_token.clone();
            }
        }
        let compression = welcome.zstd_enabled();
        reader.set_compression(compression);
        writer.set_compression(compression);
        let features = welcome.features.clone();
        Ok(Connection {
            reader,
            writer,
            features,
            interrupt: std::sync::Arc::new(move || {
                use std::net::Shutdown;
                if let Some(socket) = shutdown_read.as_ref() {
                    let _ = socket.shutdown(Shutdown::Both);
                }
                if let Some(socket) = shutdown_write.as_ref() {
                    let _ = socket.shutdown(Shutdown::Both);
                }
            }),
        })
    }
}
