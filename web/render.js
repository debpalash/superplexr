// Paint a frame on a canvas. Cells are drawn in runs of equal background,
// then text, then the cursor, at device-pixel resolution so a retina screen
// gets crisp glyphs. Nothing here knows about the wire.

const FONT_PX = 14;
const LINE = 1.28;

export class Renderer {
  constructor(canvas) {
    this.canvas = canvas;
    this.ctx = canvas.getContext("2d", { alpha: false });
    this.family = getComputedStyle(document.documentElement).getPropertyValue("--mono").trim() || "monospace";
    this.dpr = 1;
    this.cell = { width: 8, height: 17 };
    this.measure();
  }

  measure() {
    this.dpr = Math.max(1, window.devicePixelRatio || 1);
    this.ctx.font = this.font(false, false);
    const width = this.ctx.measureText("M").width;
    this.cell = {
      width: Math.max(1, Math.round(width * this.dpr) / this.dpr),
      height: Math.round(FONT_PX * LINE * this.dpr) / this.dpr,
    };
  }

  font(bold, italic) {
    return `${italic ? "italic " : ""}${bold ? "600 " : "400 "}${FONT_PX * this.dpr}px ${this.family}`;
  }

  /** Columns and rows that fit the canvas's CSS size. */
  fit() {
    const rect = this.canvas.getBoundingClientRect();
    const columns = Math.max(2, Math.floor(rect.width / this.cell.width));
    const rows = Math.max(1, Math.floor(rect.height / this.cell.height));
    return { columns, rows, cellWidthPx: Math.round(this.cell.width * this.dpr), cellHeightPx: Math.round(this.cell.height * this.dpr) };
  }

  resizeBacking() {
    const rect = this.canvas.getBoundingClientRect();
    const width = Math.round(rect.width * this.dpr);
    const height = Math.round(rect.height * this.dpr);
    if (this.canvas.width !== width || this.canvas.height !== height) {
      this.canvas.width = width;
      this.canvas.height = height;
    }
  }

  clear(background) {
    this.resizeBacking();
    this.ctx.fillStyle = background;
    this.ctx.fillRect(0, 0, this.canvas.width, this.canvas.height);
  }

  paint(frame, { focused = true, selection = null } = {}) {
    this.measure();
    const ctx = this.ctx;
    const dpr = this.dpr;
    const cw = this.cell.width * dpr;
    const ch = this.cell.height * dpr;
    const defaultFg = rgb(frame.default_foreground);
    const defaultBg = rgb(frame.default_background);
    this.clear(defaultBg);
    const resolved = frame.styles.map((s) => resolve(s, defaultFg, defaultBg));
    const fallback = { fg: defaultFg, bg: defaultBg, bold: false, italic: false, faint: false, underline: "none", strikethrough: false, invisible: false };
    ctx.textBaseline = "alphabetic";
    const ascent = Math.round(ch * 0.78);

    frame.rows.forEach((row, y) => {
      let x = 0;
      // Backgrounds first, merged into runs.
      let runStart = 0;
      let runBg = null;
      const flushRun = (endX) => {
        if (runBg !== null && runBg !== defaultBg) {
          ctx.fillStyle = runBg;
          ctx.fillRect(runStart * cw, y * ch, (endX - runStart) * cw, ch);
        }
      };
      row.cells.forEach((c) => {
        const st = resolved[c.style_index] ?? fallback;
        const bg = selectionCovers(selection, x, y) ? "#3b5bdb" : st.bg;
        if (bg !== runBg) {
          flushRun(x);
          runStart = x;
          runBg = bg;
        }
        x += Math.max(1, c.width);
      });
      flushRun(x);
      x = 0;
      row.cells.forEach((c) => {
        const st = resolved[c.style_index] ?? fallback;
        const width = Math.max(1, c.width);
        if (c.grapheme && c.grapheme !== " " && !st.invisible) {
          ctx.font = this.font(st.bold, st.italic);
          ctx.fillStyle = st.fg;
          ctx.globalAlpha = st.faint ? 0.6 : 1;
          ctx.fillText(c.grapheme, x * cw, y * ch + ascent);
          ctx.globalAlpha = 1;
        }
        if (st.underline !== "none") {
          ctx.fillStyle = st.fg;
          ctx.fillRect(x * cw, y * ch + ch - Math.max(1, dpr), width * cw, Math.max(1, dpr));
        }
        if (st.strikethrough) {
          ctx.fillStyle = st.fg;
          ctx.fillRect(x * cw, y * ch + ch * 0.5, width * cw, Math.max(1, dpr));
        }
        x += width;
      });
    });

    if (frame.cursor) {
      const { row, column, shape } = frame.cursor;
      ctx.fillStyle = defaultFg;
      if (!focused || shape === "hollow_block") {
        ctx.strokeStyle = defaultFg;
        ctx.lineWidth = dpr;
        ctx.strokeRect(column * cw + 0.5, row * ch + 0.5, cw - 1, ch - 1);
      } else if (shape === "bar") {
        ctx.fillRect(column * cw, row * ch, Math.max(1, dpr * 1.5), ch);
      } else if (shape === "underline") {
        ctx.fillRect(column * cw, row * ch + ch - dpr * 2, cw, dpr * 2);
      } else {
        ctx.fillRect(column * cw, row * ch, cw, ch);
        const under = frame.rows[row]?.cells[column];
        if (under && under.grapheme && under.grapheme !== " ") {
          const st = resolved[under.style_index] ?? fallback;
          ctx.font = this.font(st.bold, st.italic);
          ctx.fillStyle = defaultBg;
          ctx.fillText(under.grapheme, column * cw, row * ch + ascent);
        }
      }
    }
  }

  /** Cell under a pointer position in CSS pixels relative to the canvas. */
  cellAt(px, py) {
    return { column: Math.floor(px / this.cell.width), row: Math.floor(py / this.cell.height) };
  }
}

function rgb(c) {
  return `rgb(${c.red},${c.green},${c.blue})`;
}

function resolve(s, defaultFg, defaultBg) {
  let fg = rgb(s.foreground);
  let bg = rgb(s.background);
  if (s.inverse) [fg, bg] = [bg, fg];
  return { fg, bg, bold: s.bold, italic: s.italic, faint: s.faint, underline: s.underline, strikethrough: s.strikethrough, invisible: s.invisible };
}

function selectionCovers(selection, x, y) {
  if (!selection) return false;
  const [a, b] = order(selection.anchor, selection.head);
  if (y < a.row || y > b.row) return false;
  if (a.row === b.row) return x >= a.column && x <= b.column;
  if (y === a.row) return x >= a.column;
  if (y === b.row) return x <= b.column;
  return true;
}

export function order(p, q) {
  if (p.row < q.row || (p.row === q.row && p.column <= q.column)) return [p, q];
  return [q, p];
}

/** Text of a frame inside a selection, rows joined with newlines. */
export function selectedText(frame, selection) {
  if (!selection) return "";
  const [a, b] = order(selection.anchor, selection.head);
  const lines = [];
  for (let y = a.row; y <= b.row; y++) {
    const row = frame.rows[y];
    if (!row) continue;
    let x = 0;
    let text = "";
    for (const c of row.cells) {
      const width = Math.max(1, c.width);
      const from = y === a.row ? a.column : 0;
      const to = y === b.row ? b.column : Infinity;
      if (x >= from && x <= to) text += c.grapheme || " ";
      x += width;
    }
    lines.push(text.replace(/\s+$/, ""));
  }
  return lines.join("\n");
}
