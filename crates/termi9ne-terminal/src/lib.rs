//! Backend-neutral terminal state for termi9ne.
//!
//! This module is the only product seam that understands `libghostty-vt`. It
//! translates Ghostty-owned state into frames that can cross a process or GUI
//! interface without leaking FFI types or lifetimes.

use std::{cell::RefCell, collections::HashMap, mem, rc::Rc, sync::Arc};

use libghostty_vt::{
    RenderState, Terminal,
    fmt::Format,
    focus,
    key::{self, Key},
    mouse, paste,
    render::{CellIterator, CursorVisualStyle, Dirty, RowIterator},
    screen::CellWide,
    selection::{FormatOptions, Selection},
    style::{RgbColor as GhosttyRgb, Underline as GhosttyUnderline},
    terminal::{Mode, Point, PointCoordinate, ScrollViewport},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Highest supported logical terminal width.
pub const MAX_COLUMNS: u16 = 1_000;
/// Highest supported logical terminal height.
pub const MAX_ROWS: u16 = 500;
/// Largest paste accepted by the terminal boundary before any encoding.
pub const MAX_PASTE_BYTES: usize = 8 * 1024 * 1024;
const MAX_TERMINAL_METADATA_BYTES: usize = 4 * 1024;
/// Default retained logical history per terminal Session.
pub const DEFAULT_SCROLLBACK_LINES: usize = 100_000;
/// Largest OSC 8 URI copied into a transport frame.
pub const MAX_HYPERLINK_BYTES: usize = 8 * 1024;
const MAX_CACHED_STYLES: usize = 4_096;

/// Errors returned by the terminal module interface.
#[derive(Debug, Error)]
pub enum TerminalError {
    /// The requested grid is outside the product contract.
    #[error(
        "terminal grid {columns}x{rows} is outside 2..={MAX_COLUMNS} columns and 1..={MAX_ROWS} rows"
    )]
    InvalidGrid { columns: u16, rows: u16 },
    /// Clipboard input exceeded the bounded paste contract.
    #[error("paste is {actual} bytes; the maximum is {MAX_PASTE_BYTES} bytes")]
    PasteTooLarge { actual: usize },
    /// The pinned Ghostty adapter rejected an operation or state query.
    #[error("libghostty-vt failed: {0}")]
    Ghostty(#[from] libghostty_vt::Error),
}

/// A validated terminal grid measured in whole cells.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GridSize {
    pub columns: u16,
    pub rows: u16,
}

impl GridSize {
    /// Validate and create a logical terminal size.
    pub fn new(columns: u16, rows: u16) -> Result<Self, TerminalError> {
        if !(2..=MAX_COLUMNS).contains(&columns) || !(1..=MAX_ROWS).contains(&rows) {
            return Err(TerminalError::InvalidGrid { columns, rows });
        }
        Ok(Self { columns, rows })
    }
}

/// An sRGB terminal color.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct Rgb {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl From<GhosttyRgb> for Rgb {
    fn from(value: GhosttyRgb) -> Self {
        Self {
            red: value.r,
            green: value.g,
            blue: value.b,
        }
    }
}

/// Backend-neutral underline semantics.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnderlineStyle {
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
    Unknown,
}

/// Interned visual attributes for one or more cells.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct CellStyle {
    pub foreground: Rgb,
    pub background: Rgb,
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub blink: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub strikethrough: bool,
    pub overline: bool,
    pub underline: UnderlineStyle,
}

/// One logical terminal cell.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Cell {
    pub grapheme: String,
    /// Display width: zero for a continuation/spacer, one for narrow, two for wide.
    pub width: u8,
    pub style_index: u32,
    #[serde(default)]
    pub hyperlink: Option<String>,
}

/// One complete visible terminal row.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Row {
    pub wrapped: bool,
    pub cells: Vec<Cell>,
}

impl Row {
    /// Return the row's visible graphemes without trailing empty cells.
    #[must_use]
    pub fn text(&self) -> String {
        let mut value = self
            .cells
            .iter()
            .map(|cell| cell.grapheme.as_str())
            .collect::<String>();
        while value.ends_with(' ') {
            value.pop();
        }
        value
    }
}

/// Backend-neutral cursor shape.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorShape {
    Bar,
    Block,
    Underline,
    HollowBlock,
    Unknown,
}

/// Visible cursor state relative to the frame viewport.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Cursor {
    pub column: u16,
    pub row: u16,
    pub shape: CursorShape,
    pub blinking: bool,
}

/// A self-contained backend-neutral terminal frame.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FullFrame {
    pub sequence: u64,
    pub grid: GridSize,
    pub rows: Vec<Arc<Row>>,
    pub styles: Vec<CellStyle>,
    pub cursor: Option<Cursor>,
    pub default_foreground: Rgb,
    pub default_background: Rgb,
    #[serde(default)]
    pub mouse_tracking: bool,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub current_directory: Option<String>,
}

/// A mutation accepted by the canonical terminal state.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct KeyModifiers {
    pub shift: bool,
    pub alt: bool,
    pub control: bool,
    pub super_key: bool,
    pub caps_lock: bool,
    pub num_lock: bool,
}

/// Lifecycle phase of a physical keyboard event.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyAction {
    Press,
    Repeat,
    Release,
}

/// Product-owned keyboard event, independent of GPUI and Ghostty types.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct KeyInput {
    /// Stable platform/GPUI key token, such as `a`, `enter`, or `up`.
    pub physical_key: String,
    /// Layout-resolved key identity retained for routing and diagnostics.
    pub logical_key: String,
    /// UTF-8 text produced by the current keyboard layout, if any.
    pub text: Option<String>,
    pub modifiers: KeyModifiers,
    pub consumed_modifiers: KeyModifiers,
    pub action: KeyAction,
    /// Whether this key event belongs to an active IME composition.
    pub composing: bool,
    pub unshifted_codepoint: Option<char>,
}

/// Why paste input must stop at the user-confirmation boundary.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PasteRisk {
    MultilineOrEscape,
    NonUtf8,
}

/// Data needed by the UI to render a confirmation without touching the PTY.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PasteConfirmation {
    pub bytes: Vec<u8>,
    pub risk: PasteRisk,
}

/// One cell in the currently visible viewport, used for native selection.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SelectionPoint {
    pub column: u16,
    pub row: u16,
}

/// One bounded plain-text match in terminal screen history.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SearchMatch {
    pub line: usize,
    pub column: usize,
    pub preview: String,
}

/// Product-owned viewport movement independent from Ghostty's FFI types.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewportScroll {
    Top,
    Bottom,
    Delta(i32),
}

/// Stable addressing for a read-only viewport in retained terminal history.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "anchor", content = "row", rename_all = "snake_case")]
pub enum HistoryViewport {
    /// Zero is the active bottom viewport; larger values move into older output.
    RowsBeforeBottom(u32),
    /// Absolute logical row from the beginning of retained history.
    RowFromTop(u32),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseAction {
    Press,
    Release,
    Motion,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    ScrollUp,
    ScrollDown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MouseInput {
    pub action: MouseAction,
    pub button: Option<MouseButton>,
    pub modifiers: KeyModifiers,
    pub x_px: u32,
    pub y_px: u32,
    pub screen_width_px: u32,
    pub screen_height_px: u32,
    pub cell_width_px: u32,
    pub cell_height_px: u32,
    pub any_button_pressed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalAction<'a> {
    Output(&'a [u8]),
    Resize {
        grid: GridSize,
        cell_width_px: u32,
        cell_height_px: u32,
    },
    /// Encode one rich key event using modes from canonical terminal state.
    EncodeKey(&'a KeyInput),
    /// Encode clipboard bytes, stopping unsafe content unless confirmed.
    Paste {
        bytes: &'a [u8],
        confirmed: bool,
    },
    /// Report focus when DEC mode 1004 is enabled by the running program.
    Focus {
        focused: bool,
    },
    Scroll(ViewportScroll),
    Select {
        anchor: SelectionPoint,
        head: SelectionPoint,
        rectangle: bool,
    },
    ClearSelection,
    Mouse(&'a MouseInput),
}

/// Observable effects produced while applying one terminal action.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TerminalEffects {
    pub pty_writes: Vec<Vec<u8>>,
    pub bells: u64,
    pub paste_confirmations: Vec<PasteConfirmation>,
}

/// Compile-time facts about the pinned Ghostty build.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GhosttyBuild {
    pub version: String,
    pub build: String,
    pub simd: bool,
    pub kitty_graphics: bool,
}

impl GhosttyBuild {
    /// Read facts from the statically linked library.
    pub fn current() -> Result<Self, TerminalError> {
        Ok(Self {
            version: libghostty_vt::build_info::version_string()?.to_owned(),
            build: libghostty_vt::build_info::build_version()?.to_owned(),
            simd: libghostty_vt::build_info::supports_simd()?,
            kitty_graphics: libghostty_vt::build_info::supports_kitty_graphics()?,
        })
    }
}

#[derive(Debug, Default)]
struct EffectBuffer {
    pty_writes: Vec<Vec<u8>>,
    bells: u64,
    paste_confirmations: Vec<PasteConfirmation>,
}

/// Canonical terminal state hidden behind the product-owned interface.
///
/// This type is intentionally neither `Send` nor `Sync`, matching the public
/// guarantees of the pinned Ghostty wrapper. A runtime Session must construct
/// and use it on one owning thread or local executor.
#[derive(Debug)]
pub struct TerminalModel {
    terminal: Terminal<'static, 'static>,
    render_state: RenderState<'static>,
    row_iterator: RowIterator<'static>,
    cell_iterator: CellIterator<'static>,
    key_encoder: key::Encoder<'static>,
    mouse_encoder: mouse::Encoder<'static>,
    effects: Rc<RefCell<EffectBuffer>>,
    grid: GridSize,
    sequence: u64,
    cached_rows: Vec<Arc<Row>>,
    cached_styles: Vec<CellStyle>,
    cached_style_indices: HashMap<CellStyle, u32>,
}

impl TerminalModel {
    /// Create canonical terminal state for a validated grid.
    pub fn new(grid: GridSize) -> Result<Self, TerminalError> {
        let effects = Rc::new(RefCell::new(EffectBuffer::default()));
        let mut terminal = Terminal::new(grid.columns, grid.rows)?;
        terminal.set_scrollback_max_lines(Some(DEFAULT_SCROLLBACK_LINES))?;

        terminal
            .on_pty_write({
                let effects = Rc::clone(&effects);
                move |_terminal, bytes| effects.borrow_mut().pty_writes.push(bytes.to_vec())
            })?
            .on_bell({
                let effects = Rc::clone(&effects);
                move |_terminal| effects.borrow_mut().bells += 1
            })?;

        Ok(Self {
            terminal,
            render_state: RenderState::new()?,
            row_iterator: RowIterator::new()?,
            cell_iterator: CellIterator::new()?,
            key_encoder: key::Encoder::new()?,
            mouse_encoder: mouse::Encoder::new()?,
            effects,
            grid,
            sequence: 0,
            cached_rows: Vec::new(),
            cached_styles: Vec::new(),
            cached_style_indices: HashMap::new(),
        })
    }

    /// Apply one ordered mutation and drain effects produced by that mutation.
    pub fn advance(
        &mut self,
        action: TerminalAction<'_>,
    ) -> Result<TerminalEffects, TerminalError> {
        match action {
            TerminalAction::Output(bytes) => {
                if !bytes.is_empty() {
                    self.terminal.vt_write(bytes);
                    self.sequence = self.sequence.saturating_add(1);
                }
            }
            TerminalAction::Resize {
                grid,
                cell_width_px,
                cell_height_px,
            } => {
                if grid != self.grid {
                    self.terminal
                        .resize(grid.columns, grid.rows, cell_width_px, cell_height_px)?;
                    self.grid = grid;
                    self.sequence = self.sequence.saturating_add(1);
                }
            }
            TerminalAction::EncodeKey(input) => self.encode_key(input)?,
            TerminalAction::Paste { bytes, confirmed } => {
                self.encode_paste(bytes, confirmed)?;
            }
            TerminalAction::Focus { focused } => self.encode_focus(focused)?,
            TerminalAction::Scroll(scroll) => {
                self.terminal.scroll_viewport(match scroll {
                    ViewportScroll::Top => ScrollViewport::Top,
                    ViewportScroll::Bottom => ScrollViewport::Bottom,
                    ViewportScroll::Delta(delta) => ScrollViewport::Delta(
                        isize::try_from(delta).expect("i32 always fits supported native targets"),
                    ),
                });
                self.sequence = self.sequence.saturating_add(1);
            }
            TerminalAction::Select {
                anchor,
                head,
                rectangle,
            } => {
                let anchor = self.terminal.grid_ref(viewport_point(anchor))?;
                let head = self.terminal.grid_ref(viewport_point(head))?;
                let selection = Selection::new(anchor, head, rectangle);
                self.terminal.set_selection(Some(&selection))?;
                self.sequence = self.sequence.saturating_add(1);
            }
            TerminalAction::ClearSelection => {
                self.terminal.set_selection(None)?;
                self.sequence = self.sequence.saturating_add(1);
            }
            TerminalAction::Mouse(input) => self.encode_mouse(input)?,
        }

        let pending = mem::take(&mut *self.effects.borrow_mut());
        Ok(TerminalEffects {
            pty_writes: pending.pty_writes,
            bells: pending.bells,
            paste_confirmations: pending.paste_confirmations,
        })
    }

    fn encode_key(&mut self, input: &KeyInput) -> Result<(), TerminalError> {
        let mut event = key::Event::new()?;
        event
            .set_action(ghostty_key_action(input.action))
            .set_key(ghostty_key(&input.physical_key))
            .set_mods(ghostty_modifiers(input.modifiers))
            .set_consumed_mods(ghostty_modifiers(input.consumed_modifiers))
            .set_composing(input.composing)
            .set_utf8(input.text.clone());
        if let Some(codepoint) = input.unshifted_codepoint {
            event.set_unshifted_codepoint(codepoint);
        }

        self.key_encoder.set_options_from_terminal(&self.terminal);
        let mut encoded = Vec::with_capacity(16);
        self.key_encoder.encode_to_vec(&event, &mut encoded)?;
        if !encoded.is_empty() {
            self.effects.borrow_mut().pty_writes.push(encoded);
        }
        Ok(())
    }

    fn encode_paste(&mut self, bytes: &[u8], confirmed: bool) -> Result<(), TerminalError> {
        if bytes.len() > MAX_PASTE_BYTES {
            return Err(TerminalError::PasteTooLarge {
                actual: bytes.len(),
            });
        }

        let risk = match std::str::from_utf8(bytes) {
            Ok(text) if paste::is_safe(text) => None,
            Ok(_) => Some(PasteRisk::MultilineOrEscape),
            Err(_) => Some(PasteRisk::NonUtf8),
        };
        if let Some(risk) = risk.filter(|_| !confirmed) {
            self.effects
                .borrow_mut()
                .paste_confirmations
                .push(PasteConfirmation {
                    bytes: bytes.to_vec(),
                    risk,
                });
            return Ok(());
        }

        let bracketed = self.terminal.mode(Mode::BRACKETED_PASTE)?;
        let mut mutable_bytes = bytes.to_vec();
        let mut encoded = vec![0_u8; mutable_bytes.len().saturating_add(12)];
        let written = paste::encode(&mut mutable_bytes, bracketed, &mut encoded)?;
        encoded.truncate(written);
        if !encoded.is_empty() {
            self.effects.borrow_mut().pty_writes.push(encoded);
        }
        Ok(())
    }

    fn encode_focus(&mut self, focused: bool) -> Result<(), TerminalError> {
        if !self.terminal.mode(Mode::FOCUS_EVENT)? {
            return Ok(());
        }
        let event = if focused {
            focus::Event::Gained
        } else {
            focus::Event::Lost
        };
        let mut bytes = [0_u8; 3];
        let written = event.encode(&mut bytes)?;
        self.effects
            .borrow_mut()
            .pty_writes
            .push(bytes[..written].to_vec());
        Ok(())
    }

    fn encode_mouse(&mut self, input: &MouseInput) -> Result<(), TerminalError> {
        if !self.terminal.is_mouse_tracking()? {
            return Ok(());
        }
        let mut event = mouse::Event::new()?;
        event
            .set_action(match input.action {
                MouseAction::Press => mouse::Action::Press,
                MouseAction::Release => mouse::Action::Release,
                MouseAction::Motion => mouse::Action::Motion,
            })
            .set_button(input.button.map(|button| match button {
                MouseButton::Left => mouse::Button::Left,
                MouseButton::Middle => mouse::Button::Middle,
                MouseButton::Right => mouse::Button::Right,
                MouseButton::ScrollUp => mouse::Button::Four,
                MouseButton::ScrollDown => mouse::Button::Five,
            }))
            .set_mods(ghostty_modifiers(input.modifiers))
            .set_position(mouse::Position {
                x: input.x_px as f32,
                y: input.y_px as f32,
            });
        self.mouse_encoder
            .set_options_from_terminal(&self.terminal)
            .set_size(mouse::EncoderSize {
                screen_width: input.screen_width_px.max(1),
                screen_height: input.screen_height_px.max(1),
                cell_width: input.cell_width_px.max(1),
                cell_height: input.cell_height_px.max(1),
                padding_top: 0,
                padding_bottom: 0,
                padding_right: 0,
                padding_left: 0,
            })
            .set_any_button_pressed(input.any_button_pressed);
        let mut encoded = Vec::with_capacity(32);
        self.mouse_encoder.encode_to_vec(&event, &mut encoded)?;
        if !encoded.is_empty() {
            self.effects.borrow_mut().pty_writes.push(encoded);
        }
        Ok(())
    }

    /// Capture a complete frame containing no Ghostty-owned references.
    pub fn frame(&mut self) -> Result<FullFrame, TerminalError> {
        let snapshot = self.render_state.update(&self.terminal)?;
        let dirty = snapshot.dirty()?;
        let colors = snapshot.colors()?;
        let cursor = if snapshot.cursor_visible()? {
            let shape = cursor_shape(snapshot.cursor_visual_style()?);
            let blinking = snapshot.cursor_blinking()?;
            snapshot.cursor_viewport()?.map(|position| Cursor {
                column: position.x,
                row: position.y,
                shape,
                blinking,
            })
        } else {
            None
        };

        let rebuild_all = dirty == Dirty::Full
            || self.cached_rows.len() != usize::from(self.grid.rows)
            || self.cached_styles.len() >= MAX_CACHED_STYLES;
        if rebuild_all {
            self.cached_styles.clear();
            self.cached_style_indices.clear();
        }
        let mut rows = Vec::with_capacity(usize::from(self.grid.rows));
        let mut row_iteration = self.row_iterator.update(&snapshot)?;

        let mut row_index = 0_u32;
        while let Some(row) = row_iteration.next() {
            let cached_index =
                usize::try_from(row_index).expect("terminal row limits fit every supported target");
            if !rebuild_all
                && !row.dirty()?
                && let Some(cached) = self.cached_rows.get(cached_index)
            {
                rows.push(Arc::clone(cached));
                row_index = row_index.saturating_add(1);
                continue;
            }
            let wrapped = row.raw_row()?.is_wrapped()?;
            let mut cells = Vec::with_capacity(usize::from(self.grid.columns));
            let mut cell_iteration = self.cell_iterator.update(row)?;

            let mut column_index = 0_u16;
            while let Some(cell) = cell_iteration.next() {
                let ghostty_style = cell.style()?;
                let style = CellStyle {
                    foreground: cell.fg_color()?.unwrap_or(colors.foreground).into(),
                    background: cell.bg_color()?.unwrap_or(colors.background).into(),
                    bold: ghostty_style.bold,
                    italic: ghostty_style.italic,
                    faint: ghostty_style.faint,
                    blink: ghostty_style.blink,
                    inverse: ghostty_style.inverse ^ cell.is_selected()?,
                    invisible: ghostty_style.invisible,
                    strikethrough: ghostty_style.strikethrough,
                    overline: ghostty_style.overline,
                    underline: underline_style(ghostty_style.underline),
                };
                let style_index = match self.cached_style_indices.get(&style) {
                    Some(index) => *index,
                    None => {
                        let index = u32::try_from(self.cached_styles.len())
                            .map_err(|_| libghostty_vt::Error::LimitExceeded)?;
                        self.cached_styles.push(style);
                        self.cached_style_indices.insert(style, index);
                        index
                    }
                };

                let raw_cell = cell.raw_cell()?;
                let grapheme = cell.graphemes()?.into_iter().collect();
                let hyperlink = if raw_cell.has_hyperlink()? {
                    let grid_ref = self.terminal.grid_ref(Point::Viewport(PointCoordinate {
                        x: column_index,
                        y: row_index,
                    }))?;
                    hyperlink_uri(&grid_ref)?
                } else {
                    None
                };
                cells.push(Cell {
                    grapheme,
                    width: cell_width(raw_cell.wide()?),
                    style_index,
                    hyperlink,
                });
                column_index = column_index.saturating_add(1);
            }

            let rendered = Arc::new(Row { wrapped, cells });
            if let Some(cached) = self.cached_rows.get_mut(cached_index) {
                *cached = Arc::clone(&rendered);
            } else {
                self.cached_rows.push(Arc::clone(&rendered));
            }
            rows.push(rendered);
            row.set_dirty(false)?;
            row_index = row_index.saturating_add(1);
        }
        self.cached_rows.truncate(rows.len());

        snapshot.set_dirty(Dirty::Clean)?;
        Ok(FullFrame {
            sequence: self.sequence,
            grid: self.grid,
            rows,
            styles: self.cached_styles.clone(),
            cursor,
            default_foreground: colors.foreground.into(),
            default_background: colors.background.into(),
            mouse_tracking: self.terminal.is_mouse_tracking()?,
            title: bounded_terminal_metadata(self.terminal.title()?),
            current_directory: bounded_terminal_metadata(self.terminal.pwd()?),
        })
    }

    /// Capture a read-only historical viewport without exposing Ghostty types.
    ///
    pub fn frame_at_history_viewport(
        &mut self,
        viewport: HistoryViewport,
    ) -> Result<FullFrame, TerminalError> {
        match viewport {
            HistoryViewport::RowsBeforeBottom(rows_before_bottom) => {
                self.terminal.scroll_viewport(ScrollViewport::Bottom);
                if rows_before_bottom != 0 {
                    let delta = isize::try_from(rows_before_bottom)
                        .expect("u32 history offsets fit every supported native target");
                    self.terminal.scroll_viewport(ScrollViewport::Delta(-delta));
                }
            }
            HistoryViewport::RowFromTop(row) => self.terminal.scroll_viewport(ScrollViewport::Row(
                usize::try_from(row).expect("u32 history rows fit every supported native target"),
            )),
        }
        self.sequence = self.sequence.saturating_add(1);
        self.frame()
    }

    /// Format Ghostty's active native selection for the system clipboard.
    pub fn selected_text(&self) -> Result<Option<String>, TerminalError> {
        let bytes = self.terminal.format_selection_alloc(
            None,
            FormatOptions::new()
                .with_emit_format(Format::Plain)
                .with_unwrap(true)
                .with_trim(true),
        )?;
        Ok(bytes.map(|bytes| String::from_utf8_lossy(&bytes).into_owned()))
    }

    /// Search Ghostty-owned screen history without mutating the active selection.
    pub fn search(
        &self,
        query: &str,
        case_sensitive: bool,
        limit: usize,
    ) -> Result<Vec<SearchMatch>, TerminalError> {
        if query.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let Some(selection) = self.terminal.select_all()? else {
            return Ok(Vec::new());
        };
        let Some(bytes) = self.terminal.format_selection_alloc(
            None,
            FormatOptions::new()
                .with_emit_format(Format::Plain)
                .with_unwrap(false)
                .with_trim(true)
                .with_selection(&selection),
        )?
        else {
            return Ok(Vec::new());
        };
        let text = String::from_utf8_lossy(&bytes);
        let folded_query = (!case_sensitive).then(|| query.to_lowercase());
        let mut matches = Vec::new();
        for (line_number, line) in text.lines().enumerate() {
            let folded_line = (!case_sensitive).then(|| line.to_lowercase());
            let haystack = folded_line.as_deref().unwrap_or(line);
            let needle = folded_query.as_deref().unwrap_or(query);
            for (byte_index, _) in haystack.match_indices(needle) {
                matches.push(SearchMatch {
                    line: line_number,
                    column: haystack[..byte_index].chars().count(),
                    preview: line.to_owned(),
                });
                if matches.len() == limit {
                    return Ok(matches);
                }
            }
        }
        Ok(matches)
    }
}

fn bounded_terminal_metadata(value: &str) -> Option<String> {
    let mut result = String::with_capacity(value.len().min(MAX_TERMINAL_METADATA_BYTES));
    for character in value.chars() {
        if character.is_control() {
            continue;
        }
        if result.len() + character.len_utf8() > MAX_TERMINAL_METADATA_BYTES {
            break;
        }
        result.push(character);
    }
    (!result.is_empty()).then_some(result)
}

fn hyperlink_uri(
    grid_ref: &libghostty_vt::screen::GridRef<'_>,
) -> Result<Option<String>, TerminalError> {
    let mut bytes = Vec::new();
    let length = match grid_ref.hyperlink_uri(&mut bytes) {
        Ok(length) => length,
        Err(libghostty_vt::Error::OutOfSpace { required }) if required <= MAX_HYPERLINK_BYTES => {
            bytes.resize(required, 0);
            grid_ref.hyperlink_uri(&mut bytes)?
        }
        Err(libghostty_vt::Error::OutOfSpace { .. }) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if length == 0 {
        return Ok(None);
    }
    bytes.truncate(length);
    Ok(String::from_utf8(bytes).ok())
}

fn viewport_point(point: SelectionPoint) -> Point {
    Point::Viewport(PointCoordinate {
        x: point.column,
        y: u32::from(point.row),
    })
}

fn ghostty_key_action(action: KeyAction) -> key::Action {
    match action {
        KeyAction::Press => key::Action::Press,
        KeyAction::Repeat => key::Action::Repeat,
        KeyAction::Release => key::Action::Release,
    }
}

fn ghostty_modifiers(modifiers: KeyModifiers) -> key::Mods {
    let mut result = key::Mods::empty();
    result.set(key::Mods::SHIFT, modifiers.shift);
    result.set(key::Mods::ALT, modifiers.alt);
    result.set(key::Mods::CTRL, modifiers.control);
    result.set(key::Mods::SUPER, modifiers.super_key);
    result.set(key::Mods::CAPS_LOCK, modifiers.caps_lock);
    result.set(key::Mods::NUM_LOCK, modifiers.num_lock);
    result
}

fn ghostty_key(name: &str) -> Key {
    match name.to_ascii_lowercase().as_str() {
        "a" => Key::A,
        "b" => Key::B,
        "c" => Key::C,
        "d" => Key::D,
        "e" => Key::E,
        "f" => Key::F,
        "g" => Key::G,
        "h" => Key::H,
        "i" => Key::I,
        "j" => Key::J,
        "k" => Key::K,
        "l" => Key::L,
        "m" => Key::M,
        "n" => Key::N,
        "o" => Key::O,
        "p" => Key::P,
        "q" => Key::Q,
        "r" => Key::R,
        "s" => Key::S,
        "t" => Key::T,
        "u" => Key::U,
        "v" => Key::V,
        "w" => Key::W,
        "x" => Key::X,
        "y" => Key::Y,
        "z" => Key::Z,
        "0" => Key::Digit0,
        "1" => Key::Digit1,
        "2" => Key::Digit2,
        "3" => Key::Digit3,
        "4" => Key::Digit4,
        "5" => Key::Digit5,
        "6" => Key::Digit6,
        "7" => Key::Digit7,
        "8" => Key::Digit8,
        "9" => Key::Digit9,
        "`" => Key::Backquote,
        "\\" => Key::Backslash,
        "[" => Key::BracketLeft,
        "]" => Key::BracketRight,
        "," => Key::Comma,
        "=" => Key::Equal,
        "-" => Key::Minus,
        "." => Key::Period,
        "'" => Key::Quote,
        ";" => Key::Semicolon,
        "/" => Key::Slash,
        "backspace" => Key::Backspace,
        "delete" => Key::Delete,
        "enter" => Key::Enter,
        "space" => Key::Space,
        "tab" => Key::Tab,
        "escape" => Key::Escape,
        "left" => Key::ArrowLeft,
        "right" => Key::ArrowRight,
        "up" => Key::ArrowUp,
        "down" => Key::ArrowDown,
        "home" => Key::Home,
        "end" => Key::End,
        "insert" => Key::Insert,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "f1" => Key::F1,
        "f2" => Key::F2,
        "f3" => Key::F3,
        "f4" => Key::F4,
        "f5" => Key::F5,
        "f6" => Key::F6,
        "f7" => Key::F7,
        "f8" => Key::F8,
        "f9" => Key::F9,
        "f10" => Key::F10,
        "f11" => Key::F11,
        "f12" => Key::F12,
        "f13" => Key::F13,
        "f14" => Key::F14,
        "f15" => Key::F15,
        "f16" => Key::F16,
        "f17" => Key::F17,
        "f18" => Key::F18,
        "f19" => Key::F19,
        "f20" => Key::F20,
        "f21" => Key::F21,
        "f22" => Key::F22,
        "f23" => Key::F23,
        "f24" => Key::F24,
        "f25" => Key::F25,
        _ => Key::Unidentified,
    }
}

fn cell_width(wide: CellWide) -> u8 {
    match wide {
        CellWide::Narrow => 1,
        CellWide::Wide => 2,
        CellWide::SpacerTail | CellWide::SpacerHead => 0,
    }
}

fn underline_style(style: GhosttyUnderline) -> UnderlineStyle {
    match style {
        GhosttyUnderline::None => UnderlineStyle::None,
        GhosttyUnderline::Single => UnderlineStyle::Single,
        GhosttyUnderline::Double => UnderlineStyle::Double,
        GhosttyUnderline::Curly => UnderlineStyle::Curly,
        GhosttyUnderline::Dotted => UnderlineStyle::Dotted,
        GhosttyUnderline::Dashed => UnderlineStyle::Dashed,
        _ => UnderlineStyle::Unknown,
    }
}

fn cursor_shape(style: CursorVisualStyle) -> CursorShape {
    match style {
        CursorVisualStyle::Bar => CursorShape::Bar,
        CursorVisualStyle::Block => CursorShape::Block,
        CursorVisualStyle::Underline => CursorShape::Underline,
        CursorVisualStyle::BlockHollow => CursorShape::HollowBlock,
        _ => CursorShape::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &[u8] = b"\x1b[2J\x1b[Htermi9ne\r\n\x1b[1;32mready\x1b[0m \xE7\x95\x8C\x07";

    #[test]
    fn translates_a_deterministic_backend_neutral_frame() {
        let grid = GridSize::new(12, 4).expect("fixture grid should be valid");
        let first = fixture_frame(grid);
        let second = fixture_frame(grid);

        assert_eq!(first, second);
        assert_eq!(first.sequence, 1);
        assert_eq!(first.rows.len(), 4);
        assert_eq!(first.rows[0].text(), "termi9ne");
        assert_eq!(first.rows[1].text(), "ready 界");
        assert!(first.styles.iter().any(|style| style.bold));
        assert!(first.rows[1].cells.iter().any(|cell| cell.width == 2));
        assert!(first.rows[1].cells.iter().any(|cell| cell.width == 0));
    }

    #[test]
    fn incremental_frames_retain_clean_rows_by_identity() {
        let mut model = TerminalModel::new(GridSize::new(20, 4).expect("valid grid"))
            .expect("terminal should initialize");
        let initial = model.frame().expect("initial frame should render");
        model
            .advance(TerminalAction::Output(b"\x1b[Hchanged"))
            .expect("output should parse");
        let changed = model.frame().expect("incremental frame should render");

        assert!(!Arc::ptr_eq(&initial.rows[0], &changed.rows[0]));
        for row in 1..initial.rows.len() {
            assert!(Arc::ptr_eq(&initial.rows[row], &changed.rows[row]));
        }
    }

    #[test]
    fn preserves_bounded_osc8_hyperlinks_in_transport_frames() {
        let mut model = TerminalModel::new(GridSize::new(20, 2).expect("valid grid"))
            .expect("terminal should initialize");
        model
            .advance(TerminalAction::Output(
                b"\x1b]8;;https://example.com/docs\x1b\\docs\x1b]8;;\x1b\\",
            ))
            .expect("OSC 8 link should parse");
        let frame = model.frame().expect("linked frame should render");
        assert_eq!(
            frame.rows[0].cells[0].hyperlink.as_deref(),
            Some("https://example.com/docs")
        );
        assert!(frame.rows[0].cells[4].hyperlink.is_none());
    }

    #[test]
    fn carries_bounded_title_and_working_directory_metadata() {
        let mut model = fixture_model();
        model
            .advance(TerminalAction::Output(
                b"\x1b]2;agent build\x07\x1b]7;file://localhost/tmp/project\x07",
            ))
            .expect("OSC metadata should parse");
        let frame = model.frame().expect("metadata frame should translate");
        assert_eq!(frame.title.as_deref(), Some("agent build"));
        assert_eq!(
            frame.current_directory.as_deref(),
            Some("file://localhost/tmp/project")
        );

        let noisy = format!("line\n{}", "界".repeat(MAX_TERMINAL_METADATA_BYTES));
        let bounded = bounded_terminal_metadata(&noisy).expect("visible metadata should remain");
        assert!(!bounded.contains('\n'));
        assert!(bounded.len() <= MAX_TERMINAL_METADATA_BYTES);
        assert!(bounded.is_char_boundary(bounded.len()));
    }

    #[test]
    fn encodes_application_mouse_only_when_the_terminal_requests_tracking() {
        let mut model = TerminalModel::new(GridSize::new(20, 2).expect("valid grid"))
            .expect("terminal should initialize");
        let input = MouseInput {
            action: MouseAction::Press,
            button: Some(MouseButton::Left),
            modifiers: KeyModifiers::default(),
            x_px: 4,
            y_px: 4,
            screen_width_px: 160,
            screen_height_px: 32,
            cell_width_px: 8,
            cell_height_px: 16,
            any_button_pressed: true,
        };
        assert!(
            model
                .advance(TerminalAction::Mouse(&input))
                .expect("disabled mouse should be ignored")
                .pty_writes
                .is_empty()
        );
        model
            .advance(TerminalAction::Output(b"\x1b[?1000h\x1b[?1006h"))
            .expect("mouse modes should parse");
        assert!(model.frame().expect("frame should render").mouse_tracking);
        let effects = model
            .advance(TerminalAction::Mouse(&input))
            .expect("tracked mouse should encode");
        assert_eq!(effects.pty_writes, vec![b"\x1b[<0;1;1M".to_vec()]);
    }

    #[test]
    fn retains_and_navigates_a_large_scrollback_viewport() {
        let mut model = TerminalModel::new(GridSize::new(20, 3).expect("valid grid"))
            .expect("terminal should initialize");
        model
            .advance(TerminalAction::Output(
                b"line-0\r\nline-1\r\nline-2\r\nline-3\r\nline-4\r\nline-5\r\n",
            ))
            .expect("history should parse");
        let bottom = model.frame().expect("bottom frame should render");
        assert!(bottom.rows.iter().any(|row| row.text().contains("line-5")));

        model
            .advance(TerminalAction::Scroll(ViewportScroll::Top))
            .expect("viewport should scroll");
        let top = model.frame().expect("top frame should render");

        assert!(top.rows.iter().any(|row| row.text().contains("line-0")));
        assert_eq!(
            model
                .terminal
                .scrollback_max_lines()
                .expect("scrollback setting should remain readable"),
            Some(DEFAULT_SCROLLBACK_LINES)
        );
    }

    #[test]
    fn historical_frames_are_addressed_from_the_bottom_without_mutating_output() {
        let mut model = TerminalModel::new(GridSize::new(20, 3).expect("valid grid"))
            .expect("terminal should initialize");
        model
            .advance(TerminalAction::Output(
                b"line-0\r\nline-1\r\nline-2\r\nline-3\r\nline-4\r\nline-5\r\n",
            ))
            .expect("history should parse");

        let bottom = model
            .frame_at_history_viewport(HistoryViewport::RowsBeforeBottom(0))
            .expect("bottom history frame should render");
        let older = model
            .frame_at_history_viewport(HistoryViewport::RowsBeforeBottom(4))
            .expect("older history frame should render");
        let bottom_again = model
            .frame_at_history_viewport(HistoryViewport::RowsBeforeBottom(0))
            .expect("bottom address should be repeatable");
        let absolute = model
            .frame_at_history_viewport(HistoryViewport::RowFromTop(0))
            .expect("absolute history frame should render");

        assert!(bottom.rows.iter().any(|row| row.text().contains("line-5")));
        assert!(older.rows.iter().any(|row| row.text().contains("line-0")));
        assert!(
            bottom_again
                .rows
                .iter()
                .any(|row| row.text().contains("line-5"))
        );
        assert!(
            absolute
                .rows
                .iter()
                .any(|row| row.text().contains("line-0"))
        );
    }

    #[test]
    fn native_selection_formats_clipboard_text_and_marks_rendered_cells() {
        let mut model = TerminalModel::new(GridSize::new(20, 3).expect("valid grid"))
            .expect("terminal should initialize");
        model
            .advance(TerminalAction::Output(b"alpha beta"))
            .expect("text should parse");
        model
            .advance(TerminalAction::Select {
                anchor: SelectionPoint { column: 0, row: 0 },
                head: SelectionPoint { column: 4, row: 0 },
                rectangle: false,
            })
            .expect("selection should install");

        assert_eq!(
            model.selected_text().expect("selection should format"),
            Some("alpha".to_owned())
        );
        let frame = model.frame().expect("selection should render");
        let first = &frame.rows[0].cells[0];
        assert!(frame.styles[first.style_index as usize].inverse);
    }

    #[test]
    fn searches_scrollback_without_disturbing_the_active_selection() {
        let mut model = TerminalModel::new(GridSize::new(24, 3).expect("valid grid"))
            .expect("terminal should initialize");
        model
            .advance(TerminalAction::Output(
                b"Alpha one\r\nbeta\r\nalpha two\r\ngamma\r\n",
            ))
            .expect("history should parse");

        let matches = model
            .search("alpha", false, 10)
            .expect("history should search");

        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].column, 0);
        assert_eq!(matches[0].preview, "Alpha one");
        assert_eq!(matches[1].preview, "alpha two");
        assert_eq!(
            model
                .selected_text()
                .expect("selection should remain absent"),
            None
        );
    }

    #[test]
    fn captures_terminal_device_replies_and_bells_as_effects() {
        let mut model =
            TerminalModel::new(GridSize::new(80, 24).expect("fixture grid should be valid"))
                .expect("terminal should initialize");

        let effects = model
            .advance(TerminalAction::Output(b"\x1b[6n\x07"))
            .expect("fixture should parse");

        assert_eq!(effects.bells, 1);
        assert_eq!(effects.pty_writes, vec![b"\x1b[1;1R".to_vec()]);
    }

    #[test]
    fn rejects_grids_outside_the_product_contract() {
        assert!(matches!(
            GridSize::new(1, 24),
            Err(TerminalError::InvalidGrid { .. })
        ));
        assert!(matches!(
            GridSize::new(80, 0),
            Err(TerminalError::InvalidGrid { .. })
        ));
    }

    #[test]
    fn encodes_text_control_and_navigation_keys_from_owned_input() {
        let mut model = fixture_model();

        let text = model
            .advance(TerminalAction::EncodeKey(&key_input("a", Some("a"))))
            .expect("text key should encode");
        assert_eq!(text.pty_writes, vec![b"a".to_vec()]);

        let mut ctrl_c = key_input("c", Some("c"));
        ctrl_c.modifiers.control = true;
        let control = model
            .advance(TerminalAction::EncodeKey(&ctrl_c))
            .expect("control key should encode");
        assert_eq!(control.pty_writes, vec![vec![0x03]]);

        let up = model
            .advance(TerminalAction::EncodeKey(&key_input("up", None)))
            .expect("navigation key should encode");
        assert_eq!(up.pty_writes, vec![b"\x1b[A".to_vec()]);
    }

    #[test]
    fn encodes_ime_commit_without_a_physical_key_identity() {
        let mut model = fixture_model();
        let committed = KeyInput {
            physical_key: "unidentified".to_owned(),
            logical_key: "日本語".to_owned(),
            text: Some("日本語".to_owned()),
            modifiers: KeyModifiers::default(),
            consumed_modifiers: KeyModifiers::default(),
            action: KeyAction::Press,
            composing: false,
            unshifted_codepoint: Some('日'),
        };

        let effects = model
            .advance(TerminalAction::EncodeKey(&committed))
            .expect("IME commit should encode");
        assert_eq!(effects.pty_writes, vec!["日本語".as_bytes().to_vec()]);
    }

    #[test]
    fn preserves_repeat_and_release_for_kitty_keyboard_mode() {
        let mut model = fixture_model();
        model
            .advance(TerminalAction::Output(b"\x1b[>3u"))
            .expect("kitty keyboard mode should parse");

        let mut repeat = key_input("a", Some("a"));
        repeat.action = KeyAction::Repeat;
        let repeated = model
            .advance(TerminalAction::EncodeKey(&repeat))
            .expect("repeat should encode");

        let mut release = key_input("a", None);
        release.action = KeyAction::Release;
        let released = model
            .advance(TerminalAction::EncodeKey(&release))
            .expect("release should encode");

        assert!(!repeated.pty_writes.is_empty());
        assert!(!released.pty_writes.is_empty());
        assert_ne!(repeated.pty_writes, released.pty_writes);
    }

    /// The prompt-time reset the shell integration emits must actually take
    /// the terminal out of Kitty keyboard mode, or a crashed TUI leaves the
    /// shell typing key events as text.
    #[test]
    fn setting_kitty_flags_to_zero_restores_plain_key_encoding() {
        let mut model = fixture_model();
        model
            .advance(TerminalAction::Output(b"\x1b[>3u"))
            .expect("kitty keyboard mode should parse");
        let mut release = key_input("a", None);
        release.action = KeyAction::Release;
        let in_kitty_mode = model
            .advance(TerminalAction::EncodeKey(&release))
            .expect("release should encode");
        assert!(
            !in_kitty_mode.pty_writes.is_empty(),
            "kitty mode reports releases"
        );

        model
            .advance(TerminalAction::Output(b"\x1b[=0;1u"))
            .expect("flag reset should parse");
        let after_reset = model
            .advance(TerminalAction::EncodeKey(&release))
            .expect("release should encode");
        assert!(
            after_reset.pty_writes.is_empty(),
            "plain mode has no release events, so the shell sees nothing"
        );
    }

    #[test]
    fn reports_focus_only_when_the_terminal_requests_it() {
        let mut model = fixture_model();
        let disabled = model
            .advance(TerminalAction::Focus { focused: true })
            .expect("disabled focus reporting should be a no-op");
        assert!(disabled.pty_writes.is_empty());

        model
            .advance(TerminalAction::Output(b"\x1b[?1004h"))
            .expect("focus mode should parse");
        let gained = model
            .advance(TerminalAction::Focus { focused: true })
            .expect("focus gained should encode");
        let lost = model
            .advance(TerminalAction::Focus { focused: false })
            .expect("focus lost should encode");

        assert_eq!(gained.pty_writes, vec![b"\x1b[I".to_vec()]);
        assert_eq!(lost.pty_writes, vec![b"\x1b[O".to_vec()]);
    }

    #[test]
    fn gates_unsafe_paste_and_uses_terminal_bracketed_paste_mode() {
        let mut model = fixture_model();
        let unsafe_bytes = b"echo first\necho second";

        let gated = model
            .advance(TerminalAction::Paste {
                bytes: unsafe_bytes,
                confirmed: false,
            })
            .expect("unsafe paste should become a confirmation effect");
        assert!(gated.pty_writes.is_empty());
        assert_eq!(
            gated.paste_confirmations,
            vec![PasteConfirmation {
                bytes: unsafe_bytes.to_vec(),
                risk: PasteRisk::MultilineOrEscape,
            }]
        );

        model
            .advance(TerminalAction::Output(b"\x1b[?2004h"))
            .expect("bracketed paste mode should parse");
        let confirmed = model
            .advance(TerminalAction::Paste {
                bytes: unsafe_bytes,
                confirmed: true,
            })
            .expect("confirmed paste should encode");
        assert_eq!(
            confirmed.pty_writes,
            vec![b"\x1b[200~echo first\necho second\x1b[201~".to_vec()]
        );
    }

    #[test]
    fn rejects_paste_above_the_eight_megabyte_contract() {
        let mut model = fixture_model();
        let bytes = vec![b'x'; MAX_PASTE_BYTES + 1];
        assert!(matches!(
            model.advance(TerminalAction::Paste {
                bytes: &bytes,
                confirmed: true,
            }),
            Err(TerminalError::PasteTooLarge { .. })
        ));
    }

    fn fixture_frame(grid: GridSize) -> FullFrame {
        let mut model = TerminalModel::new(grid).expect("terminal should initialize");
        let effects = model
            .advance(TerminalAction::Output(FIXTURE))
            .expect("fixture should parse");
        assert_eq!(effects.bells, 1);
        model.frame().expect("frame should translate")
    }

    fn fixture_model() -> TerminalModel {
        TerminalModel::new(GridSize::new(80, 24).expect("fixture grid should be valid"))
            .expect("terminal should initialize")
    }

    fn key_input(physical_key: &str, text: Option<&str>) -> KeyInput {
        KeyInput {
            physical_key: physical_key.to_owned(),
            logical_key: text.unwrap_or(physical_key).to_owned(),
            text: text.map(str::to_owned),
            modifiers: KeyModifiers::default(),
            consumed_modifiers: KeyModifiers::default(),
            action: KeyAction::Press,
            composing: false,
            unshifted_codepoint: physical_key.chars().next(),
        }
    }
}
