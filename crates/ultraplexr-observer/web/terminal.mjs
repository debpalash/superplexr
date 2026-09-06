// Render canonical styles; never interpret terminal bytes, HTML, links, or CSS
// supplied by a terminal process. Styles use numeric RGB and fixed properties.
const rgb = value => {
  if (!value || ![value.red, value.green, value.blue].every(v => Number.isInteger(v) && v >= 0 && v <= 255)) {
    throw new Error("Invalid terminal color");
  }
  return `rgb(${value.red}, ${value.green}, ${value.blue})`;
};
function styleFor(style) {
  if (!style || typeof style !== "object") throw new Error("Invalid terminal style");
  const foreground = rgb(style.inverse ? style.background : style.foreground);
  const background = rgb(style.inverse ? style.foreground : style.background);
  const underline = {none:"none", single:"solid", double:"double", curly:"wavy", dotted:"dotted", dashed:"dashed", unknown:"solid"}[style.underline];
  if (!underline) throw new Error("Invalid terminal underline");
  return {foreground, background, bold:!!style.bold, italic:!!style.italic,
    faint:!!style.faint, invisible:!!style.invisible, underline,
    decoration:[underline !== "none" && "underline", style.strikethrough && "line-through", style.overline && "overline"].filter(Boolean).join(" ") || "none"};
}
function applyStyle(element, style) {
  element.style.backgroundColor = style.background;
  element.style.color = style.invisible ? "transparent" : style.foreground;
  // Dim glyphs, not their background. An inner span also prevents invisible
  // glyph decorations from revealing terminal-concealed text.
  const glyph = document.createElement("span");
  glyph.className = "terminal-glyph";
  // Weight/style belong to the glyph, leaving the outer box's ch unit tied to
  // the grid's regular monospace face rather than a potentially wider bold face.
  glyph.style.fontWeight = style.bold ? "700" : "400";
  glyph.style.fontStyle = style.italic ? "italic" : "normal";
  glyph.style.opacity = style.faint ? "0.5" : "1";
  glyph.style.textDecorationLine = style.invisible ? "none" : style.decoration;
  if (style.underline !== "none") glyph.style.textDecorationStyle = style.underline;
  return glyph;
}

export class TerminalDisplay {
  constructor(output) {
    this.output = output; this.grid = null; this.signatures = []; this.styleKey = ""; this.text = "";
  }
  clear(message = "") {
    this.grid = null; this.signatures = []; this.styleKey = ""; this.text = "";
    this.output.style.removeProperty("color"); this.output.style.removeProperty("background-color");
    this.output.textContent = message;
  }
  render(frame, {cursor = true} = {}) {
    this.text = frame.text || "";
    try {
      if (!frame.display) throw new Error("Text-only frame");
      this.renderStyled(frame.display, cursor);
      return true;
    } catch {
      // A malformed/unsupported visual projection cannot erase readable output
      // or confer terminal input. Do not retain a partially constructed grid.
      const text = this.text;
      this.clear(); this.text = text; this.output.textContent = text;
      return false;
    }
  }
  renderStyled(display, showCursor) {
    if (!Number.isInteger(display.columns) || display.columns < 1 || display.columns > 400
        || !Array.isArray(display.rows) || display.rows.length > 200
        || !Array.isArray(display.styles) || display.styles.length > 4096) throw new Error("Unsupported grid");
    const styles = display.styles.map(styleFor);
    const foreground = rgb(display.foreground), background = rgb(display.background);
    const styleKey = JSON.stringify(display.styles);
    const changedStyles = styleKey !== this.styleKey;
    if (!this.grid || this.grid.parentNode !== this.output) {
      this.grid = document.createElement("span"); this.grid.className = "terminal-grid";
      this.output.replaceChildren(this.grid); this.signatures = [];
    }
    this.output.style.color = foreground; this.output.style.backgroundColor = background;
    this.grid.style.width = `${display.columns}ch`;
    let bytes = 0, count = 0;
    for (let index = 0; index < display.rows.length; index++) {
      const runs = display.rows[index];
      if (!Array.isArray(runs)) throw new Error("Invalid terminal row");
      let columns = 0;
      for (const run of runs) {
        if (!run || typeof run.text !== "string" || !Number.isInteger(run.columns) || run.columns < 1
            || !Number.isInteger(run.style) || !styles[run.style]) throw new Error("Invalid terminal run");
        columns += run.columns; bytes += run.text.length; count++;
        if (columns > display.columns || bytes > 2 * 1024 * 1024 || count > 80_000) throw new Error("Oversized terminal display");
      }
      const signature = JSON.stringify(runs);
      let row = this.grid.children[index];
      if (!row) { row = document.createElement("span"); row.className = "terminal-row"; this.grid.append(row); }
      if (changedStyles || this.signatures[index] !== signature) {
        const fragment = document.createDocumentFragment();
        for (const run of runs) {
          const span = document.createElement("span"); span.className = "terminal-run";
          span.style.width = `${run.columns}ch`;
          const glyph = applyStyle(span, styles[run.style]); glyph.textContent = run.text;
          span.append(glyph); fragment.append(span);
        }
        row.replaceChildren(fragment); this.signatures[index] = signature;
      }
    }
    while (this.grid.children.length > display.rows.length) this.grid.lastElementChild.remove();
    this.signatures.length = display.rows.length; this.styleKey = styleKey;
    // Cursor is CSS-only: no extra character in selection or copied output.
    for (const row of this.grid.children) { row.classList.remove("terminal-cursor-row"); row.style.removeProperty("--cursor-column"); }
    const position = display.cursor;
    if (showCursor && position && Number.isInteger(position.row) && Number.isInteger(position.column)
        && position.row >= 0 && position.row < display.rows.length && position.column >= 0 && position.column < display.columns) {
      const row = this.grid.children[position.row];
      row.classList.add("terminal-cursor-row"); row.style.setProperty("--cursor-column", `${position.column}ch`);
      row.dataset.cursor = ["bar", "underline", "hollow_block", "block"].includes(position.shape) ? position.shape : "block";
    }
  }
}
