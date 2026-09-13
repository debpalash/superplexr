// One attachment-local canonical frame. Patches are admitted atomically against
// an exact string revision; a missing base causes read-only reconnection.
const revision = value => typeof value === "string" && /^(0|[1-9][0-9]{0,19})$/.test(value)
  && (value.length < 20 || value <= "18446744073709551615");
const newer = (next, previous) => next.length > previous.length || next.length === previous.length && next > previous;
export class FrameContinuityError extends Error {}
const invalid = () => { throw new FrameContinuityError("Frame continuity lost; requesting a full snapshot"); };
function boundedDisplay(display) {
  if (!display || !Number.isInteger(display.columns) || display.columns < 1 || display.columns > 400
      || !Array.isArray(display.rows) || display.rows.length > 200
      || !Array.isArray(display.styles) || display.styles.length > 4096) return invalid();
  let count = 0, text = 0;
  for (const row of display.rows) {
    if (!Array.isArray(row)) return invalid();
    let columns = 0;
    for (const run of row) {
      if (!run || typeof run.text !== "string" || !Number.isInteger(run.columns) || run.columns < 1
          || !Number.isInteger(run.style) || run.style < 0 || run.style >= display.styles.length) return invalid();
      columns += run.columns; text += run.text.length; count++;
      if (columns > display.columns || text > 2 * 1024 * 1024 || count > 80000) return invalid();
    }
  }
}
export class FrameDecoder {
  constructor() { this.reset(); }
  reset() { this.current = null; }
  accept(event, data) {
    let value;
    try { value = JSON.parse(data); } catch { return invalid(); }
    if (event === "frame") {
      if (!value || typeof value.text !== "string" || value.text.length > 8 * 1024 * 1024
          || !Number.isInteger(value.rows) || value.rows < 0 || value.rows > 65535) return invalid();
      // Legacy full-frame feeds can still render; only a valid revision and
      // bounded visual projection can become a delta base.
      this.current = value;
      return value;
    }
    const base = this.current;
    if (event !== "frame-delta" || !base || !value || !revision(base.revision)
        || value.base_revision !== base.revision || !revision(value.revision)
        || !newer(value.revision, base.revision) || value.rows !== base.rows
        || !Number.isFinite(value.sequence) || !Number.isInteger(value.sequence) || value.sequence < 0
        || typeof value.status !== "string" || value.status.length > 32
        || ![value.title, value.directory].every(v => v === null || typeof v === "string" && v.length <= 65536)
        || !Array.isArray(value.changed) || value.changed.length > 200) return invalid();
    boundedDisplay(base.display);
    const lines = base.text.split("\n"), rows = base.display.rows.slice(), seen = new Set();
    if (lines.length !== rows.length) return invalid();
    for (const change of value.changed) {
      if (!change || !Number.isInteger(change.index) || change.index < 0 || change.index >= rows.length
          || seen.has(change.index) || typeof change.text !== "string" || /[\r\n]/.test(change.text)
          || change.text.length > 2 * 1024 * 1024 || !Array.isArray(change.runs)) return invalid();
      seen.add(change.index); lines[change.index] = change.text; rows[change.index] = change.runs;
    }
    const display = {...base.display, rows, cursor:value.cursor};
    boundedDisplay(display);
    const text = lines.join("\n");
    if (text.length > 8 * 1024 * 1024) return invalid();
    // No mutation of the base or displayed history before complete admission.
    const next = {...base, revision:value.revision, sequence:value.sequence,
      title:value.title, directory:value.directory, status:value.status, text, display};
    this.current = next;
    return next;
  }
}
