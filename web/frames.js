// The terminal data plane, decoded by hand: the protobuf messages in
// proto/ultraplexr/terminal/v1.proto are few and flat, so a small reader is
// less to ship than a protobuf runtime. Frames come out in the same shape as
// the JSON `FullFrame` a snapshot returns, so the renderer sees one model.

const decoder = new TextDecoder();

class Reader {
  constructor(bytes) {
    this.bytes = bytes;
    this.pos = 0;
  }

  get done() {
    return this.pos >= this.bytes.length;
  }

  varint() {
    let result = 0n;
    let shift = 0n;
    for (;;) {
      if (this.pos >= this.bytes.length) throw new Error("protobuf: truncated varint");
      const byte = this.bytes[this.pos++];
      result |= BigInt(byte & 0x7f) << shift;
      if ((byte & 0x80) === 0) return result;
      shift += 7n;
      if (shift > 63n) throw new Error("protobuf: varint too long");
    }
  }

  uint() {
    return Number(this.varint());
  }

  sint() {
    const raw = this.varint();
    return Number((raw >> 1n) ^ -(raw & 1n));
  }

  tag() {
    const key = this.uint();
    return { field: key >>> 3, wire: key & 7 };
  }

  bytes_() {
    const length = this.uint();
    if (this.pos + length > this.bytes.length) throw new Error("protobuf: truncated bytes");
    const slice = this.bytes.subarray(this.pos, this.pos + length);
    this.pos += length;
    return slice;
  }

  string() {
    return decoder.decode(this.bytes_());
  }

  skip(wire) {
    switch (wire) {
      case 0: this.varint(); break;
      case 1: this.pos += 8; break;
      case 2: this.bytes_(); break;
      case 5: this.pos += 4; break;
      default: throw new Error(`protobuf: unsupported wire type ${wire}`);
    }
  }
}

function message(bytes, fields) {
  const reader = new Reader(bytes);
  const out = {};
  while (!reader.done) {
    const { field, wire } = reader.tag();
    const handler = fields[field];
    if (handler) handler(reader, out);
    else reader.skip(wire);
  }
  return out;
}

const bool = (key) => (r, o) => { o[key] = r.uint() !== 0; };
const uint = (key) => (r, o) => { o[key] = r.uint(); };
const big = (key) => (r, o) => { o[key] = r.varint(); };
const str = (key) => (r, o) => { o[key] = r.string(); };
const sub = (key, parse) => (r, o) => { o[key] = parse(r.bytes_()); };
const many = (key, parse) => (r, o) => { (o[key] ??= []).push(parse(r.bytes_())); };

function color(bytes) {
  const c = message(bytes, { 1: uint("default_color"), 2: uint("palette_index"), 3: uint("srgb") });
  const srgb = c.srgb ?? 0;
  return { red: (srgb >>> 16) & 0xff, green: (srgb >>> 8) & 0xff, blue: srgb & 0xff };
}

const UNDERLINE = ["none", "single", "double", "curly", "dotted", "dashed"];
const CURSOR = ["block", "bar", "underline"];

function style(bytes) {
  const s = message(bytes, {
    1: sub("foreground", color),
    2: sub("background", color),
    4: bool("bold"),
    5: bool("faint"),
    6: bool("italic"),
    7: uint("underline"),
    8: bool("strikethrough"),
    9: bool("inverse"),
    10: bool("invisible"),
    11: bool("blink"),
    12: bool("overline"),
  });
  return {
    foreground: s.foreground ?? { red: 0, green: 0, blue: 0 },
    background: s.background ?? { red: 0, green: 0, blue: 0 },
    bold: !!s.bold,
    faint: !!s.faint,
    italic: !!s.italic,
    underline: UNDERLINE[s.underline ?? 0] ?? "unknown",
    strikethrough: !!s.strikethrough,
    inverse: !!s.inverse,
    invisible: !!s.invisible,
    blink: !!s.blink,
    overline: !!s.overline,
  };
}

function cell(bytes) {
  const c = message(bytes, {
    1: str("grapheme"),
    2: uint("display_width"),
    3: uint("style_index"),
    4: uint("hyperlink_index"),
  });
  return {
    grapheme: c.grapheme ?? "",
    width: c.display_width ?? 1,
    style_index: c.style_index ?? 0,
    hyperlink_index: c.hyperlink_index ?? 0,
  };
}

function row(bytes) {
  const r = message(bytes, { 2: bool("wrapped"), 3: many("cells", cell) });
  return { wrapped: !!r.wrapped, cells: r.cells ?? [] };
}

function grid(bytes) {
  const g = message(bytes, { 1: uint("columns"), 2: uint("rows") });
  return { columns: g.columns ?? 0, rows: g.rows ?? 0 };
}

function cursor(bytes) {
  const c = message(bytes, { 1: uint("row"), 2: uint("column"), 3: uint("shape"), 4: bool("visible"), 5: bool("blinking") });
  if (!c.visible) return null;
  return { row: c.row ?? 0, column: c.column ?? 0, shape: CURSOR[c.shape ?? 0] ?? "unknown", blinking: !!c.blinking };
}

function modes(bytes) {
  const m = message(bytes, { 2: uint("mouse") });
  return { mouse_tracking: (m.mouse ?? 0) !== 0 };
}

function palette(bytes) {
  return message(bytes, { 2: sub("default_foreground", color), 3: sub("default_background", color) });
}

function hyperlink(bytes) {
  const h = message(bytes, { 1: str("uri"), 2: str("id") });
  return h.uri ?? "";
}

function attachHyperlinks(rows, hyperlinks) {
  for (const r of rows) {
    for (const c of r.cells) {
      c.hyperlink = c.hyperlink_index ? hyperlinks[c.hyperlink_index - 1] ?? null : null;
    }
  }
}

function sessionId(bytes) {
  const h = [...bytes].map((x) => x.toString(16).padStart(2, "0")).join("");
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}

export function decodeFullFrame(bytes) {
  const m = message(bytes, {
    1: (r, o) => { o.session_id = sessionId(r.bytes_()); },
    2: big("frame_sequence"),
    4: sub("grid", grid),
    6: many("rows", row),
    7: sub("cursor", cursor),
    8: sub("modes", modes),
    9: str("title"),
    11: sub("palette", palette),
    12: many("styles", style),
    13: many("hyperlinks", hyperlink),
    15: str("current_directory"),
  });
  const rows = m.rows ?? [];
  attachHyperlinks(rows, m.hyperlinks ?? []);
  return {
    session_id: m.session_id,
    frame: {
      sequence: m.frame_sequence ?? 0n,
      grid: m.grid ?? { columns: 0, rows: 0 },
      rows,
      styles: m.styles ?? [],
      cursor: m.cursor ?? null,
      default_foreground: m.palette?.default_foreground ?? { red: 230, green: 237, blue: 243 },
      default_background: m.palette?.default_background ?? { red: 13, green: 17, blue: 23 },
      mouse_tracking: m.modes?.mouse_tracking ?? false,
      title: m.title || null,
      current_directory: m.current_directory ?? null,
    },
  };
}

function replacement(bytes) {
  const r = message(bytes, { 1: uint("visible_index"), 2: sub("row", row), 3: uint("start_column"), 4: bool("span") });
  return { index: r.visible_index ?? 0, start: r.span ? r.start_column ?? 0 : 0, cells: r.row?.cells ?? [], wrapped: r.row?.wrapped ?? false };
}

export function decodeFrameDelta(bytes) {
  const m = message(bytes, {
    1: (r, o) => { o.session_id = sessionId(r.bytes_()); },
    2: big("base_sequence"),
    3: big("frame_sequence"),
    5: many("changed_rows", replacement),
    6: sub("grid", grid),
    8: sub("cursor", cursor),
    9: sub("modes", modes),
    10: str("title"),
    12: sub("palette", palette),
    13: many("styles", style),
    14: many("hyperlinks", hyperlink),
    16: str("current_directory"),
  });
  const changed = m.changed_rows ?? [];
  attachHyperlinks(changed, m.hyperlinks ?? []);
  return {
    session_id: m.session_id,
    delta: {
      base_sequence: m.base_sequence ?? 0n,
      sequence: m.frame_sequence ?? 0n,
      grid: m.grid ?? null,
      changed_rows: changed,
      styles: m.styles ?? [],
      cursor: m.cursor ?? null,
      default_foreground: m.palette?.default_foreground ?? null,
      default_background: m.palette?.default_background ?? null,
      mouse_tracking: m.modes?.mouse_tracking ?? false,
      title: m.title ?? null,
      current_directory: m.current_directory ?? null,
    },
  };
}

const LIFECYCLE = ["unspecified", "creating", "running", "exited", "lost"];

export function decodeLifecycle(bytes) {
  const m = message(bytes, {
    1: (r, o) => { o.session_id = sessionId(r.bytes_()); },
    2: uint("state"),
    3: sub("exit", (b) => message(b, { 1: uint("code"), 2: uint("signal"), 3: bool("unknown") })),
    4: str("reason"),
  });
  return { session_id: m.session_id, state: LIFECYCLE[m.state ?? 0] ?? "unspecified", exit: m.exit ?? null, reason: m.reason ?? null };
}

/** Mirror of `FrameDelta::apply_to`: same checks, same replacements. */
export function applyDelta(frame, delta) {
  if (BigInt(frame.sequence) !== BigInt(delta.base_sequence)) {
    throw new Error(`delta base ${delta.base_sequence} does not match frame ${frame.sequence}`);
  }
  if (delta.grid && (delta.grid.columns !== frame.grid.columns || delta.grid.rows !== frame.grid.rows)) {
    throw new Error("delta grid does not match frame");
  }
  for (const changed of delta.changed_rows) {
    const target = frame.rows[changed.index];
    if (!target) throw new Error(`delta row ${changed.index} is past the grid`);
    const end = changed.start + changed.cells.length;
    if (end > target.cells.length) throw new Error(`delta row ${changed.index} is wider than the grid`);
    target.cells.splice(changed.start, changed.cells.length, ...changed.cells);
    target.wrapped = changed.wrapped;
  }
  frame.sequence = delta.sequence;
  frame.styles = delta.styles;
  frame.cursor = delta.cursor;
  if (delta.default_foreground) frame.default_foreground = delta.default_foreground;
  if (delta.default_background) frame.default_background = delta.default_background;
  frame.mouse_tracking = delta.mouse_tracking;
  frame.title = delta.title;
  frame.current_directory = delta.current_directory;
}
