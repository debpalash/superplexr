//! Protobuf terminal data-plane adapter.

use std::{collections::HashMap, sync::Arc};

use prost::Message;
use superplexr_core::SessionId;
use superplexr_terminal::{
    Cell, CellStyle, Cursor, CursorShape, FullFrame, GridSize, Rgb, Row, UnderlineStyle,
};
use uuid::Uuid;

use crate::{
    ChangedRow, FrameDelta, ProtocolError, ServerEvent, terminal_proto as proto, wire_v3::FrameKind,
};

pub fn encode(event: &ServerEvent) -> Result<(FrameKind, Vec<u8>), ProtocolError> {
    match event {
        ServerEvent::TerminalFrame { session_id, frame } => Ok((
            FrameKind::FullFrame,
            encode_full_frame(*session_id, frame).encode_to_vec(),
        )),
        ServerEvent::TerminalDelta { session_id, delta } => Ok((
            FrameKind::FrameDelta,
            encode_delta(*session_id, delta).encode_to_vec(),
        )),
        ServerEvent::TerminalExited {
            session_id,
            code,
            signal,
            ..
        } => {
            let code = i32::try_from(*code).map_err(|_| {
                ProtocolError::InvalidTerminalProtobuf("exit code does not fit int32".to_owned())
            })?;
            Ok((
                FrameKind::TerminalLifecycle,
                proto::TerminalLifecycleV1 {
                    session_id: session_id.as_uuid().as_bytes().to_vec(),
                    state: proto::LifecycleState::Exited as i32,
                    exit_status: Some(proto::ExitStatus {
                        value: Some(proto::exit_status::Value::Code(code)),
                    }),
                    reason: signal.clone(),
                    final_frame_sequence: 0,
                    final_output_offset: 0,
                }
                .encode_to_vec(),
            ))
        }
        ServerEvent::TerminalFailed {
            session_id,
            message,
        } => Ok((
            FrameKind::TerminalLifecycle,
            proto::TerminalLifecycleV1 {
                session_id: session_id.as_uuid().as_bytes().to_vec(),
                state: proto::LifecycleState::Lost as i32,
                exit_status: Some(proto::ExitStatus {
                    value: Some(proto::exit_status::Value::Unknown(true)),
                }),
                reason: Some(message.clone()),
                final_frame_sequence: 0,
                final_output_offset: 0,
            }
            .encode_to_vec(),
        )),
        event => Ok((FrameKind::EventBatch, serde_json::to_vec(event)?)),
    }
}

pub fn decode(kind: FrameKind, bytes: &[u8]) -> Result<ServerEvent, ProtocolError> {
    match kind {
        FrameKind::FullFrame => decode_full_frame(proto::FullFrameV1::decode(bytes)?),
        FrameKind::FrameDelta => decode_delta(proto::FrameDeltaV1::decode(bytes)?),
        FrameKind::TerminalLifecycle => {
            decode_lifecycle(proto::TerminalLifecycleV1::decode(bytes)?)
        }
        FrameKind::EventBatch => Ok(serde_json::from_slice(bytes)?),
        _ => Err(ProtocolError::InvalidTerminalProtobuf(format!(
            "frame kind {kind:?} is not a terminal event"
        ))),
    }
}

fn encode_full_frame(session_id: SessionId, frame: &FullFrame) -> proto::FullFrameV1 {
    let (rows, hyperlinks) = encode_rows(frame.rows.iter().map(AsRef::as_ref));
    proto::FullFrameV1 {
        session_id: session_id.as_uuid().as_bytes().to_vec(),
        frame_sequence: frame.sequence,
        output_offset: 0,
        grid: Some(encode_grid(frame.grid)),
        viewport: Some(proto::Viewport {
            absolute_top: 0,
            visible_rows: u32::from(frame.grid.rows),
            history_epoch: 0,
        }),
        rows,
        cursor: frame.cursor.map(encode_cursor),
        modes: Some(encode_modes(frame.mouse_tracking)),
        title: frame.title.clone().unwrap_or_default(),
        icon: String::new(),
        palette: Some(encode_palette(
            frame.default_foreground,
            frame.default_background,
        )),
        styles: frame.styles.iter().copied().map(encode_style).collect(),
        hyperlinks,
        control_epoch: 0,
        current_directory: frame.current_directory.clone(),
    }
}

fn encode_delta(session_id: SessionId, delta: &FrameDelta) -> proto::FrameDeltaV1 {
    let (rows, hyperlinks) = encode_rows(delta.changed_rows.iter().map(|changed| &changed.row));
    let changed_rows = delta
        .changed_rows
        .iter()
        .zip(rows)
        .map(|(changed, row)| proto::RowReplacement {
            visible_index: u32::from(changed.index),
            row: Some(row),
        })
        .collect();
    proto::FrameDeltaV1 {
        session_id: session_id.as_uuid().as_bytes().to_vec(),
        base_sequence: delta.base_sequence,
        frame_sequence: delta.sequence,
        output_offset: 0,
        changed_rows,
        grid: Some(encode_grid(delta.grid)),
        viewport: None,
        cursor: delta.cursor.map(encode_cursor),
        modes: Some(encode_modes(delta.mouse_tracking)),
        title: delta.title.clone(),
        icon: None,
        palette: Some(encode_palette(
            delta.default_foreground,
            delta.default_background,
        )),
        styles: delta.styles.iter().copied().map(encode_style).collect(),
        hyperlinks,
        control_epoch: None,
        current_directory: delta.current_directory.clone(),
    }
}

fn encode_rows<'a>(
    rows: impl Iterator<Item = &'a Row>,
) -> (Vec<proto::Row>, Vec<proto::Hyperlink>) {
    let mut indexes = HashMap::<String, u32>::new();
    let mut hyperlinks = Vec::new();
    let encoded = rows
        .enumerate()
        .map(|(index, row)| proto::Row {
            absolute_line: i64::try_from(index).expect("terminal row bounds fit i64"),
            wrapped: row.wrapped,
            cells: row
                .cells
                .iter()
                .map(|cell| {
                    let hyperlink_index = cell.hyperlink.as_ref().map_or(0, |uri| {
                        if let Some(index) = indexes.get(uri) {
                            *index
                        } else {
                            let index = u32::try_from(hyperlinks.len() + 1)
                                .expect("terminal row bounds fit u32");
                            indexes.insert(uri.clone(), index);
                            hyperlinks.push(proto::Hyperlink {
                                uri: uri.clone(),
                                id: String::new(),
                            });
                            index
                        }
                    });
                    proto::Cell {
                        grapheme: cell.grapheme.clone(),
                        display_width: u32::from(cell.width),
                        style_index: cell.style_index,
                        hyperlink_index,
                        flags: 0,
                    }
                })
                .collect(),
        })
        .collect();
    (encoded, hyperlinks)
}

fn encode_grid(grid: GridSize) -> proto::GridSize {
    proto::GridSize {
        columns: u32::from(grid.columns),
        rows: u32::from(grid.rows),
    }
}

fn encode_cursor(cursor: Cursor) -> proto::Cursor {
    proto::Cursor {
        row: u32::from(cursor.row),
        column: u32::from(cursor.column),
        shape: match cursor.shape {
            CursorShape::Bar => proto::CursorShape::Beam,
            CursorShape::Underline => proto::CursorShape::Underline,
            CursorShape::Block | CursorShape::HollowBlock | CursorShape::Unknown => {
                proto::CursorShape::Block
            }
        } as i32,
        visible: true,
        blinking: cursor.blinking,
    }
}

fn encode_modes(mouse_tracking: bool) -> proto::TerminalModes {
    proto::TerminalModes {
        alternate_screen: false,
        mouse: if mouse_tracking {
            proto::MouseMode::Normal
        } else {
            proto::MouseMode::None
        } as i32,
        focus_reporting: false,
        bracketed_paste: false,
        kitty_keyboard_flags: 0,
        application_cursor_keys: false,
        application_keypad: false,
    }
}

fn encode_style(style: CellStyle) -> proto::CellStyle {
    proto::CellStyle {
        foreground: Some(encode_color(style.foreground)),
        background: Some(encode_color(style.background)),
        underline_color: None,
        bold: style.bold,
        faint: style.faint,
        italic: style.italic,
        underline: match style.underline {
            UnderlineStyle::None | UnderlineStyle::Unknown => proto::UnderlineStyle::None,
            UnderlineStyle::Single => proto::UnderlineStyle::Single,
            UnderlineStyle::Double => proto::UnderlineStyle::Double,
            UnderlineStyle::Curly => proto::UnderlineStyle::Curly,
            UnderlineStyle::Dotted => proto::UnderlineStyle::Dotted,
            UnderlineStyle::Dashed => proto::UnderlineStyle::Dashed,
        } as i32,
        strikethrough: style.strikethrough,
        inverse: style.inverse,
        invisible: style.invisible,
        blink: style.blink,
        overline: style.overline,
    }
}

fn encode_palette(foreground: Rgb, background: Rgb) -> proto::Palette {
    proto::Palette {
        colors: Vec::new(),
        default_foreground: Some(encode_color(foreground)),
        default_background: Some(encode_color(background)),
        cursor: None,
    }
}

fn encode_color(color: Rgb) -> proto::Color {
    proto::Color {
        value: Some(proto::color::Value::Srgb(
            u32::from(color.red) << 16 | u32::from(color.green) << 8 | u32::from(color.blue),
        )),
    }
}

fn decode_full_frame(message: proto::FullFrameV1) -> Result<ServerEvent, ProtocolError> {
    let session_id = decode_session_id(&message.session_id)?;
    let grid = decode_grid(message.grid)?;
    if message.rows.len() != usize::from(grid.rows) {
        return Err(ProtocolError::InvalidTerminalProtobuf(format!(
            "frame has {} rows for a {}-row grid",
            message.rows.len(),
            grid.rows
        )));
    }
    let styles = decode_styles(message.styles)?;
    let hyperlinks = message
        .hyperlinks
        .into_iter()
        .map(|link| link.uri)
        .collect::<Vec<_>>();
    let rows = message
        .rows
        .into_iter()
        .map(|row| decode_row(row, &hyperlinks, styles.len()).map(Arc::new))
        .collect::<Result<Vec<_>, _>>()?;
    let (default_foreground, default_background) = decode_palette(message.palette)?;
    Ok(ServerEvent::TerminalFrame {
        session_id,
        frame: Box::new(FullFrame {
            sequence: message.frame_sequence,
            grid,
            rows,
            styles,
            cursor: decode_cursor(message.cursor)?,
            default_foreground,
            default_background,
            mouse_tracking: decode_mouse_tracking(message.modes),
            title: (!message.title.is_empty()).then_some(message.title),
            current_directory: message.current_directory,
        }),
    })
}

fn decode_delta(message: proto::FrameDeltaV1) -> Result<ServerEvent, ProtocolError> {
    let session_id = decode_session_id(&message.session_id)?;
    let grid = decode_grid(message.grid)?;
    let styles = decode_styles(message.styles)?;
    let hyperlinks = message
        .hyperlinks
        .into_iter()
        .map(|link| link.uri)
        .collect::<Vec<_>>();
    let changed_rows = message
        .changed_rows
        .into_iter()
        .map(|replacement| {
            let index = u16::try_from(replacement.visible_index).map_err(|_| {
                ProtocolError::InvalidTerminalProtobuf(
                    "changed row index does not fit uint16".to_owned(),
                )
            })?;
            if index >= grid.rows {
                return Err(ProtocolError::InvalidDeltaRow(index));
            }
            let row = replacement.row.ok_or_else(|| {
                ProtocolError::InvalidTerminalProtobuf("changed row is missing".to_owned())
            })?;
            Ok(ChangedRow {
                index,
                row: decode_row(row, &hyperlinks, styles.len())?,
            })
        })
        .collect::<Result<Vec<_>, ProtocolError>>()?;
    let (default_foreground, default_background) = decode_palette(message.palette)?;
    Ok(ServerEvent::TerminalDelta {
        session_id,
        delta: Box::new(FrameDelta {
            base_sequence: message.base_sequence,
            sequence: message.frame_sequence,
            grid,
            changed_rows,
            styles,
            cursor: decode_cursor(message.cursor)?,
            default_foreground,
            default_background,
            mouse_tracking: decode_mouse_tracking(message.modes),
            title: message.title,
            current_directory: message.current_directory,
        }),
    })
}

fn decode_lifecycle(message: proto::TerminalLifecycleV1) -> Result<ServerEvent, ProtocolError> {
    let session_id = decode_session_id(&message.session_id)?;
    match proto::LifecycleState::try_from(message.state).ok() {
        Some(proto::LifecycleState::Exited) => {
            let code = match message.exit_status.and_then(|status| status.value) {
                Some(proto::exit_status::Value::Code(code)) => {
                    u32::try_from(code).map_err(|_| {
                        ProtocolError::InvalidTerminalProtobuf(
                            "negative process exit code".to_owned(),
                        )
                    })?
                }
                Some(proto::exit_status::Value::Signal(signal)) => {
                    return Ok(ServerEvent::TerminalExited {
                        session_id,
                        code: 0,
                        signal: Some(signal.to_string()),
                        success: false,
                    });
                }
                _ => 1,
            };
            Ok(ServerEvent::TerminalExited {
                session_id,
                code,
                signal: message.reason,
                success: code == 0,
            })
        }
        Some(proto::LifecycleState::Lost) => Ok(ServerEvent::TerminalFailed {
            session_id,
            message: message
                .reason
                .unwrap_or_else(|| "terminal connection was lost".to_owned()),
        }),
        _ => Err(ProtocolError::InvalidTerminalProtobuf(
            "unsupported terminal lifecycle transition".to_owned(),
        )),
    }
}

fn decode_session_id(bytes: &[u8]) -> Result<SessionId, ProtocolError> {
    let uuid = Uuid::from_slice(bytes).map_err(|_| {
        ProtocolError::InvalidTerminalProtobuf("session UUID must contain 16 bytes".to_owned())
    })?;
    Ok(SessionId::from_uuid(uuid))
}

fn decode_grid(grid: Option<proto::GridSize>) -> Result<GridSize, ProtocolError> {
    let grid = grid.ok_or_else(|| {
        ProtocolError::InvalidTerminalProtobuf("terminal grid is missing".to_owned())
    })?;
    let columns = u16::try_from(grid.columns).map_err(|_| {
        ProtocolError::InvalidTerminalProtobuf("terminal columns do not fit uint16".to_owned())
    })?;
    let rows = u16::try_from(grid.rows).map_err(|_| {
        ProtocolError::InvalidTerminalProtobuf("terminal rows do not fit uint16".to_owned())
    })?;
    GridSize::new(columns, rows).map_err(|error| {
        ProtocolError::InvalidTerminalProtobuf(format!("invalid terminal grid: {error}"))
    })
}

fn decode_row(
    row: proto::Row,
    hyperlinks: &[String],
    style_count: usize,
) -> Result<Row, ProtocolError> {
    let cells = row
        .cells
        .into_iter()
        .map(|cell| {
            let width = u8::try_from(cell.display_width).map_err(|_| {
                ProtocolError::InvalidTerminalProtobuf("cell width does not fit uint8".to_owned())
            })?;
            if width > 2 {
                return Err(ProtocolError::InvalidTerminalProtobuf(format!(
                    "cell width {width} is outside 0..=2"
                )));
            }
            if usize::try_from(cell.style_index).unwrap_or(usize::MAX) >= style_count {
                return Err(ProtocolError::InvalidTerminalProtobuf(format!(
                    "cell style index {} is outside the style table",
                    cell.style_index
                )));
            }
            let hyperlink = if cell.hyperlink_index == 0 {
                None
            } else {
                hyperlinks
                    .get(cell.hyperlink_index as usize - 1)
                    .cloned()
                    .ok_or_else(|| {
                        ProtocolError::InvalidTerminalProtobuf(
                            "cell hyperlink index is outside the hyperlink table".to_owned(),
                        )
                    })?
                    .into()
            };
            Ok(Cell {
                grapheme: cell.grapheme,
                width,
                style_index: cell.style_index,
                hyperlink,
            })
        })
        .collect::<Result<Vec<_>, ProtocolError>>()?;
    Ok(Row {
        wrapped: row.wrapped,
        cells,
    })
}

fn decode_styles(styles: Vec<proto::CellStyle>) -> Result<Vec<CellStyle>, ProtocolError> {
    styles
        .into_iter()
        .map(|style| {
            Ok(CellStyle {
                foreground: decode_color(style.foreground)?,
                background: decode_color(style.background)?,
                bold: style.bold,
                italic: style.italic,
                faint: style.faint,
                blink: style.blink,
                inverse: style.inverse,
                invisible: style.invisible,
                strikethrough: style.strikethrough,
                overline: style.overline,
                underline: match proto::UnderlineStyle::try_from(style.underline).ok() {
                    Some(proto::UnderlineStyle::None) => UnderlineStyle::None,
                    Some(proto::UnderlineStyle::Single) => UnderlineStyle::Single,
                    Some(proto::UnderlineStyle::Double) => UnderlineStyle::Double,
                    Some(proto::UnderlineStyle::Curly) => UnderlineStyle::Curly,
                    Some(proto::UnderlineStyle::Dotted) => UnderlineStyle::Dotted,
                    Some(proto::UnderlineStyle::Dashed) => UnderlineStyle::Dashed,
                    None => UnderlineStyle::Unknown,
                },
            })
        })
        .collect()
}

fn decode_cursor(cursor: Option<proto::Cursor>) -> Result<Option<Cursor>, ProtocolError> {
    cursor
        .filter(|cursor| cursor.visible)
        .map(|cursor| {
            Ok(Cursor {
                column: u16::try_from(cursor.column).map_err(|_| {
                    ProtocolError::InvalidTerminalProtobuf(
                        "cursor column does not fit uint16".to_owned(),
                    )
                })?,
                row: u16::try_from(cursor.row).map_err(|_| {
                    ProtocolError::InvalidTerminalProtobuf(
                        "cursor row does not fit uint16".to_owned(),
                    )
                })?,
                shape: match proto::CursorShape::try_from(cursor.shape).ok() {
                    Some(proto::CursorShape::Beam) => CursorShape::Bar,
                    Some(proto::CursorShape::Underline) => CursorShape::Underline,
                    _ => CursorShape::Block,
                },
                blinking: cursor.blinking,
            })
        })
        .transpose()
}

fn decode_palette(palette: Option<proto::Palette>) -> Result<(Rgb, Rgb), ProtocolError> {
    let palette = palette.ok_or_else(|| {
        ProtocolError::InvalidTerminalProtobuf("terminal palette is missing".to_owned())
    })?;
    Ok((
        decode_color(palette.default_foreground)?,
        decode_color(palette.default_background)?,
    ))
}

fn decode_color(color: Option<proto::Color>) -> Result<Rgb, ProtocolError> {
    let Some(proto::color::Value::Srgb(value)) = color.and_then(|color| color.value) else {
        return Err(ProtocolError::InvalidTerminalProtobuf(
            "current terminal model requires an sRGB color".to_owned(),
        ));
    };
    if value > 0x00ff_ffff {
        return Err(ProtocolError::InvalidTerminalProtobuf(
            "sRGB color uses reserved high bits".to_owned(),
        ));
    }
    Ok(Rgb {
        red: ((value >> 16) & 0xff) as u8,
        green: ((value >> 8) & 0xff) as u8,
        blue: (value & 0xff) as u8,
    })
}

fn decode_mouse_tracking(modes: Option<proto::TerminalModes>) -> bool {
    modes.is_some_and(|modes| modes.mouse != proto::MouseMode::None as i32)
}
