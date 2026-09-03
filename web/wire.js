// wire_v3 in the browser: the same 32-byte header and JSON control plane the
// native shells use, carried over a WebSocket. The wire is self-delimiting,
// so bytes are accumulated and frames are read out of the stream regardless
// of how the socket chunked them.

export const MAGIC = new Uint8Array([0x54, 0x39, 0x4e, 0x45]); // "T9NE"
export const WIRE_MAJOR = 3;
export const WIRE_MINOR = 1;
export const HEADER_BYTES = 32;
export const PROTOCOL_VERSION = 26;

export const Kind = Object.freeze({
  Hello: 1,
  Welcome: 2,
  Close: 3,
  Request: 10,
  Response: 11,
  EventBatch: 12,
  FullFrame: 20,
  FrameDelta: 21,
  ResyncRequired: 22,
  HistoryPage: 23,
  SearchPage: 24,
  TerminalLifecycle: 25,
  Ping: 30,
  Pong: 31,
});

const FEATURES = ["terminal_v1", "mission_events_v1", "session_groups_v1", "multiplexed_streams_v1"];

const encoder = new TextEncoder();
const decoder = new TextDecoder();

export function encodeFrame(kind, streamId, payload, sequence = 0n) {
  const bytes = new Uint8Array(HEADER_BYTES + payload.length);
  const view = new DataView(bytes.buffer);
  bytes.set(MAGIC, 0);
  view.setUint16(4, WIRE_MAJOR);
  view.setUint16(6, WIRE_MINOR);
  view.setUint16(8, kind);
  view.setUint16(10, 0);
  view.setUint32(12, streamId);
  view.setBigUint64(16, sequence);
  view.setUint32(24, payload.length);
  view.setUint32(28, 0);
  bytes.set(payload, HEADER_BYTES);
  return bytes;
}

/** Per-stream frame numbering: each stream counts from 1 on every connection. */
export class Sequencer {
  constructor() {
    this.next = new Map();
  }

  take(streamId) {
    const value = this.next.get(streamId) ?? 1n;
    this.next.set(streamId, value + 1n);
    return value;
  }
}

export function encodeJson(kind, streamId, value, sequencer) {
  return encodeFrame(kind, streamId, encoder.encode(JSON.stringify(value)), sequencer.take(streamId));
}

export function hello(clientId, deviceId, { deviceToken = null, pairingCode = null } = {}) {
  return {
    client_id: clientId,
    client_kind: "web",
    client_version: "0.1.0",
    protocol: { major: WIRE_MAJOR, min_minor: WIRE_MINOR, max_minor: WIRE_MINOR },
    features: FEATURES,
    device_id: deviceId,
    device_token: deviceToken,
    pairing_code: pairingCode,
  };
}

/** Accumulates bytes; `take()` yields complete frames in order. */
export class FrameReader {
  constructor() {
    this.chunks = [];
    this.length = 0;
  }

  push(bytes) {
    this.chunks.push(bytes);
    this.length += bytes.length;
  }

  compact() {
    if (this.chunks.length <= 1) return;
    const joined = new Uint8Array(this.length);
    let offset = 0;
    for (const chunk of this.chunks) {
      joined.set(chunk, offset);
      offset += chunk.length;
    }
    this.chunks = [joined];
  }

  *take() {
    for (;;) {
      if (this.length < HEADER_BYTES) return;
      this.compact();
      const buffer = this.chunks[0];
      const view = new DataView(buffer.buffer, buffer.byteOffset, buffer.byteLength);
      for (let i = 0; i < 4; i++) {
        if (buffer[i] !== MAGIC[i]) throw new Error("wire: bad magic");
      }
      const major = view.getUint16(4);
      if (major !== WIRE_MAJOR) throw new Error(`wire: unsupported major ${major}`);
      const flags = view.getUint16(10);
      if (flags & 1) throw new Error("wire: compressed frame though zstd was not negotiated");
      const payloadLength = view.getUint32(24);
      if (this.length < HEADER_BYTES + payloadLength) return;
      const frame = {
        kind: view.getUint16(8),
        streamId: view.getUint32(12),
        sequence: view.getBigUint64(16),
        payload: buffer.subarray(HEADER_BYTES, HEADER_BYTES + payloadLength),
      };
      const rest = buffer.subarray(HEADER_BYTES + payloadLength);
      this.chunks = rest.length ? [rest] : [];
      this.length = rest.length;
      yield frame;
    }
  }
}

export function json(frame) {
  return JSON.parse(decoder.decode(frame.payload));
}

export function uuid() {
  if (crypto.randomUUID) return crypto.randomUUID();
  const b = crypto.getRandomValues(new Uint8Array(16));
  b[6] = (b[6] & 0x0f) | 0x40;
  b[8] = (b[8] & 0x3f) | 0x80;
  const h = [...b].map((x) => x.toString(16).padStart(2, "0")).join("");
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}
