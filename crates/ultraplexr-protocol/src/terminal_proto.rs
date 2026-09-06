//! Checked-in Rust representation of `proto/ultraplexr/terminal/v1.proto`.
//!
//! Keeping these derives in source makes locked, offline release builds
//! independent of a host `protoc` installation. The `.proto` file remains the
//! normative cross-language schema and field numbers must stay identical.

#[derive(Clone, PartialEq, prost::Message)]
pub struct GridSize {
    #[prost(uint32, tag = "1")]
    pub columns: u32,
    #[prost(uint32, tag = "2")]
    pub rows: u32,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Viewport {
    #[prost(sint64, tag = "1")]
    pub absolute_top: i64,
    #[prost(uint32, tag = "2")]
    pub visible_rows: u32,
    #[prost(uint64, tag = "3")]
    pub history_epoch: u64,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Color {
    #[prost(oneof = "color::Value", tags = "1, 2, 3")]
    pub value: Option<color::Value>,
}

pub mod color {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Value {
        #[prost(bool, tag = "1")]
        DefaultColor(bool),
        #[prost(uint32, tag = "2")]
        PaletteIndex(u32),
        #[prost(uint32, tag = "3")]
        Srgb(u32),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, prost::Enumeration)]
#[repr(i32)]
pub enum UnderlineStyle {
    None = 0,
    Single = 1,
    Double = 2,
    Curly = 3,
    Dotted = 4,
    Dashed = 5,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct CellStyle {
    #[prost(message, optional, tag = "1")]
    pub foreground: Option<Color>,
    #[prost(message, optional, tag = "2")]
    pub background: Option<Color>,
    #[prost(message, optional, tag = "3")]
    pub underline_color: Option<Color>,
    #[prost(bool, tag = "4")]
    pub bold: bool,
    #[prost(bool, tag = "5")]
    pub faint: bool,
    #[prost(bool, tag = "6")]
    pub italic: bool,
    #[prost(enumeration = "UnderlineStyle", tag = "7")]
    pub underline: i32,
    #[prost(bool, tag = "8")]
    pub strikethrough: bool,
    #[prost(bool, tag = "9")]
    pub inverse: bool,
    #[prost(bool, tag = "10")]
    pub invisible: bool,
    #[prost(bool, tag = "11")]
    pub blink: bool,
    #[prost(bool, tag = "12")]
    pub overline: bool,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Hyperlink {
    #[prost(string, tag = "1")]
    pub uri: String,
    #[prost(string, tag = "2")]
    pub id: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Cell {
    #[prost(string, tag = "1")]
    pub grapheme: String,
    #[prost(uint32, tag = "2")]
    pub display_width: u32,
    #[prost(uint32, tag = "3")]
    pub style_index: u32,
    #[prost(uint32, tag = "4")]
    pub hyperlink_index: u32,
    #[prost(uint32, tag = "5")]
    pub flags: u32,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Row {
    #[prost(sint64, tag = "1")]
    pub absolute_line: i64,
    #[prost(bool, tag = "2")]
    pub wrapped: bool,
    #[prost(message, repeated, tag = "3")]
    pub cells: Vec<Cell>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, prost::Enumeration)]
#[repr(i32)]
pub enum CursorShape {
    Block = 0,
    Beam = 1,
    Underline = 2,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Cursor {
    #[prost(uint32, tag = "1")]
    pub row: u32,
    #[prost(uint32, tag = "2")]
    pub column: u32,
    #[prost(enumeration = "CursorShape", tag = "3")]
    pub shape: i32,
    #[prost(bool, tag = "4")]
    pub visible: bool,
    #[prost(bool, tag = "5")]
    pub blinking: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, prost::Enumeration)]
#[repr(i32)]
pub enum MouseMode {
    None = 0,
    X10 = 1,
    Normal = 2,
    Button = 3,
    Any = 4,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct TerminalModes {
    #[prost(bool, tag = "1")]
    pub alternate_screen: bool,
    #[prost(enumeration = "MouseMode", tag = "2")]
    pub mouse: i32,
    #[prost(bool, tag = "3")]
    pub focus_reporting: bool,
    #[prost(bool, tag = "4")]
    pub bracketed_paste: bool,
    #[prost(uint32, tag = "5")]
    pub kitty_keyboard_flags: u32,
    #[prost(bool, tag = "6")]
    pub application_cursor_keys: bool,
    #[prost(bool, tag = "7")]
    pub application_keypad: bool,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Palette {
    #[prost(message, repeated, tag = "1")]
    pub colors: Vec<Color>,
    #[prost(message, optional, tag = "2")]
    pub default_foreground: Option<Color>,
    #[prost(message, optional, tag = "3")]
    pub default_background: Option<Color>,
    #[prost(message, optional, tag = "4")]
    pub cursor: Option<Color>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct FullFrameV1 {
    #[prost(bytes = "vec", tag = "1")]
    pub session_id: Vec<u8>,
    #[prost(uint64, tag = "2")]
    pub frame_sequence: u64,
    #[prost(uint64, tag = "3")]
    pub output_offset: u64,
    #[prost(message, optional, tag = "4")]
    pub grid: Option<GridSize>,
    #[prost(message, optional, tag = "5")]
    pub viewport: Option<Viewport>,
    #[prost(message, repeated, tag = "6")]
    pub rows: Vec<Row>,
    #[prost(message, optional, tag = "7")]
    pub cursor: Option<Cursor>,
    #[prost(message, optional, tag = "8")]
    pub modes: Option<TerminalModes>,
    #[prost(string, tag = "9")]
    pub title: String,
    #[prost(string, tag = "10")]
    pub icon: String,
    #[prost(message, optional, tag = "11")]
    pub palette: Option<Palette>,
    #[prost(message, repeated, tag = "12")]
    pub styles: Vec<CellStyle>,
    #[prost(message, repeated, tag = "13")]
    pub hyperlinks: Vec<Hyperlink>,
    #[prost(uint64, tag = "14")]
    pub control_epoch: u64,
    #[prost(string, optional, tag = "15")]
    pub current_directory: Option<String>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct RowReplacement {
    #[prost(uint32, tag = "1")]
    pub visible_index: u32,
    /// The replaced cells. With `span` set these cover `start_column..`
    /// rather than the whole row.
    #[prost(message, optional, tag = "2")]
    pub row: Option<Row>,
    #[prost(uint32, tag = "3")]
    pub start_column: u32,
    #[prost(bool, tag = "4")]
    pub span: bool,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct FrameDeltaV1 {
    #[prost(bytes = "vec", tag = "1")]
    pub session_id: Vec<u8>,
    #[prost(uint64, tag = "2")]
    pub base_sequence: u64,
    #[prost(uint64, tag = "3")]
    pub frame_sequence: u64,
    #[prost(uint64, tag = "4")]
    pub output_offset: u64,
    #[prost(message, repeated, tag = "5")]
    pub changed_rows: Vec<RowReplacement>,
    #[prost(message, optional, tag = "6")]
    pub grid: Option<GridSize>,
    #[prost(message, optional, tag = "7")]
    pub viewport: Option<Viewport>,
    #[prost(message, optional, tag = "8")]
    pub cursor: Option<Cursor>,
    #[prost(message, optional, tag = "9")]
    pub modes: Option<TerminalModes>,
    #[prost(string, optional, tag = "10")]
    pub title: Option<String>,
    #[prost(string, optional, tag = "11")]
    pub icon: Option<String>,
    #[prost(message, optional, tag = "12")]
    pub palette: Option<Palette>,
    #[prost(message, repeated, tag = "13")]
    pub styles: Vec<CellStyle>,
    #[prost(message, repeated, tag = "14")]
    pub hyperlinks: Vec<Hyperlink>,
    #[prost(uint64, optional, tag = "15")]
    pub control_epoch: Option<u64>,
    #[prost(string, optional, tag = "16")]
    pub current_directory: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, prost::Enumeration)]
#[repr(i32)]
pub enum LifecycleState {
    Unspecified = 0,
    Creating = 1,
    Running = 2,
    Exited = 3,
    Lost = 4,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ExitStatus {
    #[prost(oneof = "exit_status::Value", tags = "1, 2, 3")]
    pub value: Option<exit_status::Value>,
}

pub mod exit_status {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Value {
        #[prost(int32, tag = "1")]
        Code(i32),
        #[prost(int32, tag = "2")]
        Signal(i32),
        #[prost(bool, tag = "3")]
        Unknown(bool),
    }
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct TerminalLifecycleV1 {
    #[prost(bytes = "vec", tag = "1")]
    pub session_id: Vec<u8>,
    #[prost(enumeration = "LifecycleState", tag = "2")]
    pub state: i32,
    #[prost(message, optional, tag = "3")]
    pub exit_status: Option<ExitStatus>,
    #[prost(string, optional, tag = "4")]
    pub reason: Option<String>,
    #[prost(uint64, tag = "5")]
    pub final_frame_sequence: u64,
    #[prost(uint64, tag = "6")]
    pub final_output_offset: u64,
}
