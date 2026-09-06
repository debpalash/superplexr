//! Local protocol v3 framing and negotiation.
//!
//! This module is the transport seam. It owns limits, network byte order,
//! stream sequencing, compression, and handshake ordering so callers cannot
//! accidentally implement subtly different wire profiles.

use std::{
    collections::HashMap,
    io::{Read, Write},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use uuid::Uuid;

pub const MAGIC: [u8; 4] = *b"T9NE";
pub const WIRE_MAJOR: u16 = 3;
pub const WIRE_MINOR: u16 = 1;
pub const MULTIPLEXED_STREAMS_FEATURE: &str = "multiplexed_streams_v1";
pub const HEADER_BYTES: usize = 32;
pub const MAX_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_UNCOMPRESSED_BYTES: usize = 64 * 1024 * 1024;
const COMPRESSED: u16 = 1;
const COMPRESSION_THRESHOLD: usize = 1024;
const ZSTD_LEVEL: i32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum FrameKind {
    Hello = 1,
    Welcome = 2,
    Close = 3,
    Request = 10,
    Response = 11,
    EventBatch = 12,
    FullFrame = 20,
    FrameDelta = 21,
    ResyncRequired = 22,
    HistoryPage = 23,
    SearchPage = 24,
    TerminalLifecycle = 25,
    MetadataSnapshot = 26,
    Ping = 30,
    Pong = 31,
}

impl TryFrom<u16> for FrameKind {
    type Error = WireError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Hello),
            2 => Ok(Self::Welcome),
            3 => Ok(Self::Close),
            10 => Ok(Self::Request),
            11 => Ok(Self::Response),
            12 => Ok(Self::EventBatch),
            20 => Ok(Self::FullFrame),
            21 => Ok(Self::FrameDelta),
            22 => Ok(Self::ResyncRequired),
            23 => Ok(Self::HistoryPage),
            24 => Ok(Self::SearchPage),
            25 => Ok(Self::TerminalLifecycle),
            26 => Ok(Self::MetadataSnapshot),
            30 => Ok(Self::Ping),
            31 => Ok(Self::Pong),
            _ => Err(WireError::UnsupportedKind(value)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameHeader {
    pub kind: FrameKind,
    pub flags: u16,
    pub stream_id: u32,
    pub stream_sequence: u64,
    pub payload_length: u32,
    pub uncompressed_length: u32,
}

impl FrameHeader {
    fn encode(self) -> [u8; HEADER_BYTES] {
        let mut bytes = [0; HEADER_BYTES];
        bytes[0..4].copy_from_slice(&MAGIC);
        bytes[4..6].copy_from_slice(&WIRE_MAJOR.to_be_bytes());
        bytes[6..8].copy_from_slice(&WIRE_MINOR.to_be_bytes());
        bytes[8..10].copy_from_slice(&(self.kind as u16).to_be_bytes());
        bytes[10..12].copy_from_slice(&self.flags.to_be_bytes());
        bytes[12..16].copy_from_slice(&self.stream_id.to_be_bytes());
        bytes[16..24].copy_from_slice(&self.stream_sequence.to_be_bytes());
        bytes[24..28].copy_from_slice(&self.payload_length.to_be_bytes());
        bytes[28..32].copy_from_slice(&self.uncompressed_length.to_be_bytes());
        bytes
    }

    fn decode(bytes: [u8; HEADER_BYTES]) -> Result<Self, WireError> {
        if bytes[0..4] != MAGIC {
            return Err(WireError::InvalidMagic);
        }
        let major = u16::from_be_bytes([bytes[4], bytes[5]]);
        let minor = u16::from_be_bytes([bytes[6], bytes[7]]);
        if major != WIRE_MAJOR || minor > WIRE_MINOR {
            return Err(WireError::UnsupportedVersion { major, minor });
        }
        let flags = u16::from_be_bytes([bytes[10], bytes[11]]);
        if flags & !COMPRESSED != 0 {
            return Err(WireError::UnsupportedFlags(flags));
        }
        let payload_length = u32::from_be_bytes(bytes[24..28].try_into().expect("fixed range"));
        let uncompressed_length =
            u32::from_be_bytes(bytes[28..32].try_into().expect("fixed range"));
        if payload_length == 0 {
            return Err(WireError::EmptyPayload);
        }
        if payload_length as usize > MAX_PAYLOAD_BYTES {
            return Err(WireError::PayloadTooLarge(payload_length));
        }
        if flags & COMPRESSED == 0 && uncompressed_length != 0 {
            return Err(WireError::UnexpectedUncompressedLength);
        }
        if flags & COMPRESSED != 0
            && (uncompressed_length == 0 || uncompressed_length as usize > MAX_UNCOMPRESSED_BYTES)
        {
            return Err(WireError::UncompressedPayloadTooLarge(uncompressed_length));
        }
        Ok(Self {
            kind: FrameKind::try_from(u16::from_be_bytes([bytes[8], bytes[9]]))?,
            flags,
            stream_id: u32::from_be_bytes(bytes[12..16].try_into().expect("fixed range")),
            stream_sequence: u64::from_be_bytes(bytes[16..24].try_into().expect("fixed range")),
            payload_length,
            uncompressed_length,
        })
    }
}

#[derive(Clone, Debug, serde::Deserialize, Eq, PartialEq, serde::Serialize)]
pub struct ProtocolRange {
    pub major: u16,
    pub min_minor: u16,
    pub max_minor: u16,
}

#[derive(Clone, Debug, serde::Deserialize, Eq, PartialEq, serde::Serialize)]
pub struct Hello {
    pub client_id: Uuid,
    pub client_kind: String,
    pub client_version: String,
    pub protocol: ProtocolRange,
    pub features: Vec<String>,
    pub device_id: Uuid,
}

impl Hello {
    #[must_use]
    pub fn new(client_id: Uuid, client_kind: impl Into<String>, device_id: Uuid) -> Self {
        Self {
            client_id,
            client_kind: client_kind.into(),
            client_version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol: ProtocolRange {
                major: WIRE_MAJOR,
                min_minor: WIRE_MINOR,
                max_minor: WIRE_MINOR,
            },
            features: supported_features(),
            device_id,
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize, Eq, PartialEq, serde::Serialize)]
pub struct WireLimits {
    pub max_payload_bytes: u32,
    pub max_uncompressed_bytes: u32,
}

#[derive(Clone, Debug, serde::Deserialize, Eq, PartialEq, serde::Serialize)]
pub struct Welcome {
    pub runtime_id: Uuid,
    pub runtime_version: String,
    pub selected_minor: u16,
    pub features: Vec<String>,
    pub connection_id: Uuid,
    pub limits: WireLimits,
    pub server_time_unix_micros: u64,
}

impl Welcome {
    fn negotiate(hello: &Hello, runtime_id: Uuid) -> Result<Self, WireError> {
        if hello.protocol.major != WIRE_MAJOR || hello.protocol.min_minor > WIRE_MINOR {
            return Err(WireError::NoCommonVersion);
        }
        if !hello
            .features
            .iter()
            .any(|feature| feature == MULTIPLEXED_STREAMS_FEATURE)
        {
            return Err(WireError::RequiredFeatureMissing(
                MULTIPLEXED_STREAMS_FEATURE.to_owned(),
            ));
        }
        let supported = supported_features();
        let features = hello
            .features
            .iter()
            .filter(|feature| supported.contains(feature))
            .cloned()
            .collect();
        Ok(Self {
            runtime_id,
            runtime_version: env!("CARGO_PKG_VERSION").to_owned(),
            selected_minor: WIRE_MINOR,
            features,
            connection_id: Uuid::new_v4(),
            limits: WireLimits {
                max_payload_bytes: MAX_PAYLOAD_BYTES as u32,
                max_uncompressed_bytes: MAX_UNCOMPRESSED_BYTES as u32,
            },
            server_time_unix_micros: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_micros()
                .try_into()
                .unwrap_or(u64::MAX),
        })
    }

    #[must_use]
    pub fn zstd_enabled(&self) -> bool {
        self.features.iter().any(|feature| feature == "zstd")
    }

    fn validate_required_features(&self) -> Result<(), WireError> {
        if self
            .features
            .iter()
            .any(|feature| feature == MULTIPLEXED_STREAMS_FEATURE)
        {
            Ok(())
        } else {
            Err(WireError::RequiredFeatureMissing(
                MULTIPLEXED_STREAMS_FEATURE.to_owned(),
            ))
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize, Eq, PartialEq, serde::Serialize)]
pub struct Close {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Error)]
pub enum WireError {
    #[error("wire I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("wire JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid T9NE frame magic")]
    InvalidMagic,
    #[error("unsupported wire version {major}.{minor}")]
    UnsupportedVersion { major: u16, minor: u16 },
    #[error("unsupported wire frame kind {0}")]
    UnsupportedKind(u16),
    #[error("unsupported wire flags 0x{0:04x}")]
    UnsupportedFlags(u16),
    #[error("wire frame payload is empty")]
    EmptyPayload,
    #[error("wire payload is {0} bytes; maximum is {MAX_PAYLOAD_BYTES}")]
    PayloadTooLarge(u32),
    #[error("wire uncompressed payload is {0} bytes; maximum is {MAX_UNCOMPRESSED_BYTES}")]
    UncompressedPayloadTooLarge(u32),
    #[error("uncompressed length is set on an uncompressed frame")]
    UnexpectedUncompressedLength,
    #[error("compressed frame arrived before zstd was negotiated")]
    CompressionNotNegotiated,
    #[error("zstd decode produced a length other than the authenticated header length")]
    DecompressedLengthMismatch,
    #[error(
        "expected {expected:?} on stream {expected_stream}, received {actual:?} on stream {actual_stream}"
    )]
    UnexpectedFrame {
        expected: FrameKind,
        expected_stream: u32,
        actual: FrameKind,
        actual_stream: u32,
    },
    #[error("stream {stream_id} sequence gap: expected {expected}, received {actual}")]
    SequenceGap {
        stream_id: u32,
        expected: u64,
        actual: u64,
    },
    #[error("client and runtime have no common local protocol version")]
    NoCommonVersion,
    #[error("peer does not support required wire feature {0}")]
    RequiredFeatureMissing(String),
}

#[derive(Debug)]
pub struct ReceivedFrame {
    pub header: FrameHeader,
    pub payload: Vec<u8>,
}

#[derive(Debug, Default)]
struct SequenceState {
    next: HashMap<u32, u64>,
}

impl SequenceState {
    fn issue(&mut self, stream_id: u32) -> u64 {
        let next = self.next.entry(stream_id).or_insert(1);
        let value = *next;
        *next = next.saturating_add(1);
        value
    }

    fn accept(&mut self, stream_id: u32, actual: u64) -> Result<(), WireError> {
        let expected = self.next.entry(stream_id).or_insert(1);
        if actual != *expected {
            return Err(WireError::SequenceGap {
                stream_id,
                expected: *expected,
                actual,
            });
        }
        *expected = expected.saturating_add(1);
        Ok(())
    }
}

fn supported_features() -> Vec<String> {
    [
        "zstd",
        "terminal_v1",
        "mission_events_v1",
        "session_groups_v1",
        crate::search_stream::FEATURE,
        crate::collection_stream::FEATURE,
        crate::VERIFICATION_STATUS_FEATURE,
        crate::VERIFICATION_CATALOG_FEATURE,
        MULTIPLEXED_STREAMS_FEATURE,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn encode_payload(bytes: Vec<u8>, compression: bool) -> Result<(u16, u32, Vec<u8>), WireError> {
    if bytes.is_empty() {
        return Err(WireError::EmptyPayload);
    }
    if bytes.len() > MAX_UNCOMPRESSED_BYTES {
        return Err(WireError::UncompressedPayloadTooLarge(
            u32::try_from(bytes.len()).unwrap_or(u32::MAX),
        ));
    }
    if compression && bytes.len() >= COMPRESSION_THRESHOLD {
        let compressed = zstd::bulk::compress(&bytes, ZSTD_LEVEL)?;
        if compressed.len() < bytes.len() && compressed.len() <= MAX_PAYLOAD_BYTES {
            return Ok((
                COMPRESSED,
                u32::try_from(bytes.len()).expect("bounded uncompressed payload fits u32"),
                compressed,
            ));
        }
    }
    if bytes.len() > MAX_PAYLOAD_BYTES {
        return Err(WireError::PayloadTooLarge(
            u32::try_from(bytes.len()).unwrap_or(u32::MAX),
        ));
    }
    Ok((0, 0, bytes))
}

fn decode_payload(
    header: FrameHeader,
    bytes: Vec<u8>,
    compression: bool,
) -> Result<Vec<u8>, WireError> {
    if header.flags & COMPRESSED == 0 {
        return Ok(bytes);
    }
    if !compression {
        return Err(WireError::CompressionNotNegotiated);
    }
    let expected = header.uncompressed_length as usize;
    let decoded = zstd::bulk::decompress(&bytes, expected)?;
    if decoded.len() != expected {
        return Err(WireError::DecompressedLengthMismatch);
    }
    Ok(decoded)
}

fn expect_frame(
    frame: ReceivedFrame,
    kind: FrameKind,
    stream_id: u32,
) -> Result<Vec<u8>, WireError> {
    if frame.header.kind != kind || frame.header.stream_id != stream_id {
        return Err(WireError::UnexpectedFrame {
            expected: kind,
            expected_stream: stream_id,
            actual: frame.header.kind,
            actual_stream: frame.header.stream_id,
        });
    }
    Ok(frame.payload)
}

/// Blocking bidirectional wire used by the desktop-facing client module.
pub struct SyncWire<S> {
    io: S,
    received: SequenceState,
    sent: SequenceState,
    compression: bool,
    features: Vec<String>,
}

impl<S> SyncWire<S> {
    #[must_use]
    pub fn new(io: S) -> Self {
        Self {
            io,
            received: SequenceState::default(),
            sent: SequenceState::default(),
            compression: false,
            features: Vec::new(),
        }
    }

    #[must_use]
    pub const fn get_ref(&self) -> &S {
        &self.io
    }

    pub fn negotiated_features(&self) -> &[String] {
        &self.features
    }

    #[must_use]
    pub fn into_inner(self) -> S {
        self.io
    }

    pub fn set_compression(&mut self, enabled: bool) {
        self.compression = enabled;
    }

    /// Split an already negotiated connection without resetting sequencing or
    /// compression. The adapter owns how its transport is split.
    pub fn split_with<R, W>(
        self,
        split: impl FnOnce(S) -> std::io::Result<(R, W)>,
    ) -> std::io::Result<(BlockingWireReader<R>, BlockingWireWriter<W>)> {
        let (reader, writer) = split(self.io)?;
        Ok((
            BlockingWireReader {
                io: reader,
                received: self.received,
                compression: self.compression,
            },
            BlockingWireWriter {
                io: writer,
                sent: self.sent,
                compression: self.compression,
            },
        ))
    }
}

impl<S: Read + Write> SyncWire<S> {
    pub fn send_bytes(
        &mut self,
        kind: FrameKind,
        stream_id: u32,
        bytes: Vec<u8>,
    ) -> Result<(), WireError> {
        let (flags, uncompressed_length, payload) = encode_payload(bytes, self.compression)?;
        let header = FrameHeader {
            kind,
            flags,
            stream_id,
            stream_sequence: self.sent.issue(stream_id),
            payload_length: u32::try_from(payload.len()).expect("bounded payload fits u32"),
            uncompressed_length,
        };
        self.io.write_all(&header.encode())?;
        self.io.write_all(&payload)?;
        self.io.flush()?;
        Ok(())
    }

    pub fn receive(&mut self) -> Result<ReceivedFrame, WireError> {
        let mut header_bytes = [0; HEADER_BYTES];
        self.io.read_exact(&mut header_bytes)?;
        let header = FrameHeader::decode(header_bytes)?;
        self.received
            .accept(header.stream_id, header.stream_sequence)?;
        let mut payload = vec![0; header.payload_length as usize];
        self.io.read_exact(&mut payload)?;
        Ok(ReceivedFrame {
            header,
            payload: decode_payload(header, payload, self.compression)?,
        })
    }

    pub fn send_json<T: Serialize>(
        &mut self,
        kind: FrameKind,
        stream_id: u32,
        value: &T,
    ) -> Result<(), WireError> {
        self.send_bytes(kind, stream_id, serde_json::to_vec(value)?)
    }

    pub fn receive_json<T: DeserializeOwned>(
        &mut self,
        kind: FrameKind,
        stream_id: u32,
    ) -> Result<T, WireError> {
        let frame = self.receive()?;
        Ok(serde_json::from_slice(&expect_frame(
            frame, kind, stream_id,
        )?)?)
    }

    pub fn client_handshake(&mut self, hello: &Hello) -> Result<Welcome, WireError> {
        self.send_json(FrameKind::Hello, 0, hello)?;
        let welcome: Welcome = self.receive_json(FrameKind::Welcome, 0)?;
        welcome.validate_required_features()?;
        self.set_compression(welcome.zstd_enabled());
        self.features = welcome.features.clone();
        Ok(welcome)
    }

    pub fn server_handshake(&mut self, runtime_id: Uuid) -> Result<(Hello, Welcome), WireError> {
        let hello: Hello = self.receive_json(FrameKind::Hello, 0)?;
        let welcome = match Welcome::negotiate(&hello, runtime_id) {
            Ok(welcome) => welcome,
            Err(error) => {
                self.send_json(
                    FrameKind::Close,
                    0,
                    &Close {
                        code: "unsupported_protocol".to_owned(),
                        message: "upgrade ultraplexr so the client and runtime share protocol v3"
                            .to_owned(),
                    },
                )?;
                return Err(error);
            }
        };
        self.send_json(FrameKind::Welcome, 0, &welcome)?;
        self.set_compression(welcome.zstd_enabled());
        self.features = welcome.features.clone();
        Ok((hello, welcome))
    }
}

/// Read half of a negotiated blocking connection. It keeps receive sequence
/// state independent from the writer so one dispatcher can block on incoming
/// frames while other threads enqueue control requests.
pub struct BlockingWireReader<R> {
    io: R,
    received: SequenceState,
    compression: bool,
}

impl<R> BlockingWireReader<R> {
    #[must_use]
    pub const fn get_ref(&self) -> &R {
        &self.io
    }
}

impl<R: Read> BlockingWireReader<R> {
    pub fn receive(&mut self) -> Result<ReceivedFrame, WireError> {
        let mut header_bytes = [0; HEADER_BYTES];
        self.io.read_exact(&mut header_bytes)?;
        let header = FrameHeader::decode(header_bytes)?;
        self.received
            .accept(header.stream_id, header.stream_sequence)?;
        let mut payload = vec![0; header.payload_length as usize];
        self.io.read_exact(&mut payload)?;
        Ok(ReceivedFrame {
            header,
            payload: decode_payload(header, payload, self.compression)?,
        })
    }
}

/// Write half of a negotiated blocking connection.
pub struct BlockingWireWriter<W> {
    io: W,
    sent: SequenceState,
    compression: bool,
}

impl<W> BlockingWireWriter<W> {
    #[must_use]
    pub const fn get_ref(&self) -> &W {
        &self.io
    }
}

impl<W: Write> BlockingWireWriter<W> {
    pub fn send_json<T: Serialize>(
        &mut self,
        kind: FrameKind,
        stream_id: u32,
        value: &T,
    ) -> Result<(), WireError> {
        let bytes = serde_json::to_vec(value)?;
        let (flags, uncompressed_length, payload) = encode_payload(bytes, self.compression)?;
        let header = FrameHeader {
            kind,
            flags,
            stream_id,
            stream_sequence: self.sent.issue(stream_id),
            payload_length: u32::try_from(payload.len()).expect("bounded payload fits u32"),
            uncompressed_length,
        };
        self.io.write_all(&header.encode())?;
        self.io.write_all(&payload)?;
        self.io.flush()?;
        Ok(())
    }
}

#[cfg(unix)]
impl SyncWire<std::os::unix::net::UnixStream> {
    pub fn into_blocking_split(
        self,
    ) -> std::io::Result<(
        BlockingWireReader<std::os::unix::net::UnixStream>,
        BlockingWireWriter<std::os::unix::net::UnixStream>,
    )> {
        let reader = self.io.try_clone()?;
        Ok((
            BlockingWireReader {
                io: reader,
                received: self.received,
                compression: self.compression,
            },
            BlockingWireWriter {
                io: self.io,
                sent: self.sent,
                compression: self.compression,
            },
        ))
    }
}

pub struct AsyncWireReader<R> {
    io: R,
    received: SequenceState,
    compression: bool,
}

impl<R> AsyncWireReader<R> {
    #[must_use]
    pub fn new(io: R) -> Self {
        Self {
            io,
            received: SequenceState::default(),
            compression: false,
        }
    }

    pub fn set_compression(&mut self, enabled: bool) {
        self.compression = enabled;
    }

    #[must_use]
    pub fn get_mut(&mut self) -> &mut R {
        &mut self.io
    }
}

impl<R: AsyncRead + Unpin> AsyncWireReader<R> {
    pub async fn receive(&mut self) -> Result<ReceivedFrame, WireError> {
        let mut header_bytes = [0; HEADER_BYTES];
        self.io.read_exact(&mut header_bytes).await?;
        let header = FrameHeader::decode(header_bytes)?;
        self.received
            .accept(header.stream_id, header.stream_sequence)?;
        let mut payload = vec![0; header.payload_length as usize];
        self.io.read_exact(&mut payload).await?;
        Ok(ReceivedFrame {
            header,
            payload: decode_payload(header, payload, self.compression)?,
        })
    }

    pub async fn receive_json<T: DeserializeOwned>(
        &mut self,
        kind: FrameKind,
        stream_id: u32,
    ) -> Result<T, WireError> {
        let frame = self.receive().await?;
        Ok(serde_json::from_slice(&expect_frame(
            frame, kind, stream_id,
        )?)?)
    }
}

pub struct AsyncWireWriter<W> {
    io: W,
    sent: SequenceState,
    compression: bool,
}

impl<W> AsyncWireWriter<W> {
    #[must_use]
    pub fn new(io: W) -> Self {
        Self {
            io,
            sent: SequenceState::default(),
            compression: false,
        }
    }

    pub fn set_compression(&mut self, enabled: bool) {
        self.compression = enabled;
    }
}

impl<W: AsyncWrite + Unpin> AsyncWireWriter<W> {
    pub async fn send_bytes(
        &mut self,
        kind: FrameKind,
        stream_id: u32,
        bytes: Vec<u8>,
    ) -> Result<(), WireError> {
        let (flags, uncompressed_length, payload) = encode_payload(bytes, self.compression)?;
        let header = FrameHeader {
            kind,
            flags,
            stream_id,
            stream_sequence: self.sent.issue(stream_id),
            payload_length: u32::try_from(payload.len()).expect("bounded payload fits u32"),
            uncompressed_length,
        };
        self.io.write_all(&header.encode()).await?;
        self.io.write_all(&payload).await?;
        self.io.flush().await?;
        Ok(())
    }

    pub async fn send_json<T: Serialize>(
        &mut self,
        kind: FrameKind,
        stream_id: u32,
        value: &T,
    ) -> Result<(), WireError> {
        self.send_bytes(kind, stream_id, serde_json::to_vec(value)?)
            .await
    }
}

pub async fn server_handshake<R, W>(
    reader: &mut AsyncWireReader<R>,
    writer: &mut AsyncWireWriter<W>,
    runtime_id: Uuid,
) -> Result<(Hello, Welcome), WireError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let hello: Hello = reader.receive_json(FrameKind::Hello, 0).await?;
    let welcome = match Welcome::negotiate(&hello, runtime_id) {
        Ok(welcome) => welcome,
        Err(error) => {
            writer
                .send_json(
                    FrameKind::Close,
                    0,
                    &Close {
                        code: "unsupported_protocol".to_owned(),
                        message: "upgrade ultraplexr so the client and runtime share protocol v3"
                            .to_owned(),
                    },
                )
                .await?;
            return Err(error);
        }
    };
    writer.send_json(FrameKind::Welcome, 0, &welcome).await?;
    let compression = welcome.zstd_enabled();
    reader.set_compression(compression);
    writer.set_compression(compression);
    Ok((hello, welcome))
}

pub async fn client_handshake<R, W>(
    reader: &mut AsyncWireReader<R>,
    writer: &mut AsyncWireWriter<W>,
    hello: &Hello,
) -> Result<Welcome, WireError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    writer.send_json(FrameKind::Hello, 0, hello).await?;
    let welcome: Welcome = reader.receive_json(FrameKind::Welcome, 0).await?;
    welcome.validate_required_features()?;
    let compression = welcome.zstd_enabled();
    reader.set_compression(compression);
    writer.set_compression(compression);
    Ok(welcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_has_the_normative_layout() {
        let header = FrameHeader {
            kind: FrameKind::FrameDelta,
            flags: COMPRESSED,
            stream_id: 0x0102_0304,
            stream_sequence: 0x0102_0304_0506_0708,
            payload_length: 123,
            uncompressed_length: 456,
        };
        let bytes = header.encode();
        assert_eq!(&bytes[..4], b"T9NE");
        assert_eq!(&bytes[4..6], &3_u16.to_be_bytes());
        assert_eq!(&bytes[8..10], &21_u16.to_be_bytes());
        assert_eq!(&bytes[12..16], &0x0102_0304_u32.to_be_bytes());
        assert_eq!(FrameHeader::decode(bytes).expect("valid header"), header);
    }

    #[test]
    fn stream_sequences_are_independent_and_gapless() {
        let mut sequences = SequenceState::default();
        assert_eq!(sequences.issue(0), 1);
        assert_eq!(sequences.issue(4), 1);
        assert_eq!(sequences.issue(0), 2);
        sequences.accept(9, 1).expect("first frame");
        assert!(matches!(
            sequences.accept(9, 3),
            Err(WireError::SequenceGap {
                expected: 2,
                actual: 3,
                ..
            })
        ));
    }

    #[test]
    fn negotiated_sync_connection_compresses_large_payloads() {
        let mut wire = SyncWire::new(std::io::Cursor::new(Vec::<u8>::new()));
        wire.set_compression(true);
        wire.send_json(FrameKind::EventBatch, 7, &"x".repeat(16_384))
            .expect("frame should encode");
        let bytes = wire.into_inner().into_inner();
        let header = FrameHeader::decode(bytes[..HEADER_BYTES].try_into().expect("header"))
            .expect("header should decode");
        assert_eq!(header.flags, COMPRESSED);
        assert_eq!(header.stream_id, 7);
        assert!(header.payload_length < header.uncompressed_length);
    }

    #[test]
    fn malformed_lengths_fail_before_payload_allocation() {
        let mut bytes = FrameHeader {
            kind: FrameKind::Request,
            flags: 0,
            stream_id: 0,
            stream_sequence: 1,
            payload_length: 1,
            uncompressed_length: 0,
        }
        .encode();
        bytes[24..28].copy_from_slice(&((MAX_PAYLOAD_BYTES + 1) as u32).to_be_bytes());
        assert!(matches!(
            FrameHeader::decode(bytes),
            Err(WireError::PayloadTooLarge(_))
        ));
    }

    #[test]
    fn arbitrary_headers_are_rejected_without_panicking() {
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        for _ in 0..10_000 {
            let mut bytes = [0_u8; HEADER_BYTES];
            for chunk in bytes.chunks_mut(8) {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                chunk.copy_from_slice(&state.to_be_bytes()[..chunk.len()]);
            }
            let _ = FrameHeader::decode(bytes);
        }
    }

    #[test]
    fn negotiation_selects_only_supported_features() {
        let mut hello = Hello::new(Uuid::new_v4(), "test", Uuid::new_v4());
        hello.features.push("future_unknown_feature".to_owned());
        let welcome = Welcome::negotiate(&hello, Uuid::new_v4()).expect("v3 must negotiate");
        assert!(welcome.zstd_enabled());
        assert!(
            !welcome
                .features
                .iter()
                .any(|item| item == "future_unknown_feature")
        );

        hello.protocol.major = WIRE_MAJOR + 1;
        assert!(matches!(
            Welcome::negotiate(&hello, Uuid::new_v4()),
            Err(WireError::NoCommonVersion)
        ));
    }

    #[test]
    fn negotiation_rejects_the_pre_multiplexed_wire_profile() {
        let mut hello = Hello::new(Uuid::new_v4(), "test", Uuid::new_v4());
        hello
            .features
            .retain(|feature| feature != MULTIPLEXED_STREAMS_FEATURE);

        assert!(matches!(
            Welcome::negotiate(&hello, Uuid::new_v4()),
            Err(WireError::RequiredFeatureMissing(feature))
                if feature == MULTIPLEXED_STREAMS_FEATURE
        ));
    }

    #[test]
    fn client_rejects_a_welcome_without_multiplexed_streams() {
        let hello = Hello::new(Uuid::new_v4(), "test", Uuid::new_v4());
        let mut welcome =
            Welcome::negotiate(&hello, Uuid::new_v4()).expect("current peers must negotiate");
        welcome
            .features
            .retain(|feature| feature != MULTIPLEXED_STREAMS_FEATURE);

        assert!(matches!(
            welcome.validate_required_features(),
            Err(WireError::RequiredFeatureMissing(feature))
                if feature == MULTIPLEXED_STREAMS_FEATURE
        ));
    }
}
