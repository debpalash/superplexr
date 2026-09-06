use std::sync::Arc;

use gpui::{
    App, BorderStyle, Bounds, Element, ElementId, ElementInputHandler, Entity, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, PaintQuad, Pixels, ShapedLine, SharedString,
    StrikethroughStyle, Style, TextAlign, TextRun, UnderlineStyle as GpuiUnderlineStyle, Window,
    fill, outline, point, px, relative, size,
};
use ultraplexr_terminal::{
    CursorShape, FullFrame, GridSize, MAX_COLUMNS, MAX_ROWS, Rgb, Row,
    UnderlineStyle as TerminalUnderlineStyle,
};

use crate::terminal_surface::TerminalSurface;
use crate::theme::{RELAY, rgb as theme_rgb};

/// Imperatively paints one backend-neutral terminal frame.
///
/// GPUI owns layout and GPU presentation; this element only consumes ultraplexr
/// frame types. Ghostty handles never cross this boundary.
pub(crate) struct TerminalElement {
    id: SharedString,
    frame: Arc<FullFrame>,
    surface: Entity<TerminalSurface>,
    composition: String,
}

pub(crate) struct PrepaintState {
    backgrounds: Vec<PaintQuad>,
    glyphs: Vec<(ShapedLine, gpui::Point<Pixels>)>,
    cursor: Option<PaintQuad>,
}

struct CachedBackgroundRun {
    start: usize,
    end: usize,
    color: Rgb,
}

struct CachedGlyphRun {
    column: usize,
    line: ShapedLine,
}

struct CachedRowPaint {
    source: Arc<Row>,
    backgrounds: Vec<CachedBackgroundRun>,
    glyphs: Vec<CachedGlyphRun>,
}

struct TerminalPaintCache {
    text_style: gpui::TextStyle,
    cell_width: Pixels,
    line_height: Pixels,
    default_background: Rgb,
    rows: Vec<CachedRowPaint>,
}

impl TerminalElement {
    pub(crate) fn new(
        id: impl Into<SharedString>,
        frame: Arc<FullFrame>,
        surface: Entity<TerminalSurface>,
        composition: String,
    ) -> Self {
        Self {
            id: id.into(),
            frame,
            surface,
            composition,
        }
    }
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone().into())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = relative(1.0).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let text_style = window.text_style();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();
        let cached_cell_width = self.surface.read(cx).cached_cell_width(line_height);
        let cell_width = cached_cell_width.unwrap_or_else(|| {
            let probe_run = TextRun {
                len: 1,
                font: text_style.font(),
                color: text_style.color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let probe = window.text_system().shape_line(
                SharedString::from("M"),
                font_size,
                &[probe_run],
                None,
            );
            if probe.width() > px(0.0) {
                probe.width()
            } else {
                px(8.0)
            }
        });
        let grid = grid_for_bounds(bounds, cell_width, line_height);
        let cell_width_px = f32::from(cell_width).round().max(1.0) as u32;
        let cell_height_px = f32::from(line_height).round().max(1.0) as u32;
        self.surface.update(cx, |surface, cx| {
            surface.set_geometry(bounds, cell_width, line_height);
            surface.resize_to(grid, cell_width_px, cell_height_px, cx);
        });

        let mut backgrounds = vec![fill(bounds, terminal_color(self.frame.default_background))];
        let mut glyphs = Vec::new();

        let global_id = _id.expect("terminal elements always expose a stable element id");
        let (cached_backgrounds, cached_glyphs) =
            window.with_element_state(global_id, |cache: Option<TerminalPaintCache>, window| {
                let mut cache = cache.unwrap_or_else(|| TerminalPaintCache {
                    text_style: text_style.clone(),
                    cell_width,
                    line_height,
                    default_background: self.frame.default_background,
                    rows: Vec::new(),
                });
                if cache.text_style != text_style
                    || cache.cell_width != cell_width
                    || cache.line_height != line_height
                    || cache.default_background != self.frame.default_background
                {
                    cache.text_style.clone_from(&text_style);
                    cache.cell_width = cell_width;
                    cache.line_height = line_height;
                    cache.default_background = self.frame.default_background;
                    cache.rows.clear();
                }

                for (row_index, row) in self.frame.rows.iter().enumerate() {
                    let reusable = cache
                        .rows
                        .get(row_index)
                        .is_some_and(|cached| Arc::ptr_eq(&cached.source, row));
                    if reusable {
                        continue;
                    }
                    let rendered = cache_terminal_row(
                        Arc::clone(row),
                        &self.frame,
                        &text_style,
                        font_size,
                        cell_width,
                        window,
                    );
                    if let Some(cached) = cache.rows.get_mut(row_index) {
                        *cached = rendered;
                    } else {
                        cache.rows.push(rendered);
                    }
                }
                cache.rows.truncate(self.frame.rows.len());

                let mut row_backgrounds = Vec::new();
                let mut row_glyphs = Vec::new();
                for (row_index, row) in cache.rows.iter().enumerate() {
                    for run in &row.backgrounds {
                        let origin = point(
                            bounds.left() + cell_width * run.start as f32,
                            bounds.top() + line_height * row_index as f32,
                        );
                        row_backgrounds.push(fill(
                            Bounds::new(
                                origin,
                                size(cell_width * (run.end - run.start) as f32, line_height),
                            ),
                            terminal_color(run.color),
                        ));
                    }
                    for run in &row.glyphs {
                        row_glyphs.push((
                            run.line.clone(),
                            point(
                                bounds.left() + cell_width * run.column as f32,
                                bounds.top() + line_height * row_index as f32,
                            ),
                        ));
                    }
                }
                ((row_backgrounds, row_glyphs), cache)
            });
        backgrounds.extend(cached_backgrounds);
        if let Some(selection) = self.surface.read(cx).selection() {
            let selection_color = theme_rgb(RELAY).alpha(0.42);
            for row in 0..self.frame.grid.rows {
                let Some(columns) = selection.column_range(row, self.frame.grid.columns) else {
                    continue;
                };
                backgrounds.push(fill(
                    Bounds::new(
                        point(
                            bounds.left() + cell_width * f32::from(columns.start),
                            bounds.top() + line_height * f32::from(row),
                        ),
                        size(
                            cell_width * f32::from(columns.end - columns.start),
                            line_height,
                        ),
                    ),
                    selection_color,
                ));
            }
        }
        glyphs.extend(cached_glyphs);

        let cursor = self.frame.cursor.map(|cursor| {
            let origin = point(
                bounds.left() + cell_width * f32::from(cursor.column),
                bounds.top() + line_height * f32::from(cursor.row),
            );
            let color = gpui::rgb(rgb_hex(self.frame.default_foreground)).alpha(0.65);
            match cursor.shape {
                CursorShape::Bar => fill(Bounds::new(origin, size(px(2.0), line_height)), color),
                CursorShape::Underline => fill(
                    Bounds::new(
                        point(origin.x, origin.y + line_height - px(2.0)),
                        size(cell_width, px(2.0)),
                    ),
                    color,
                ),
                CursorShape::HollowBlock => outline(
                    Bounds::new(origin, size(cell_width, line_height)),
                    color,
                    BorderStyle::Solid,
                ),
                CursorShape::Block | CursorShape::Unknown => {
                    fill(Bounds::new(origin, size(cell_width, line_height)), color)
                }
            }
        });

        if !self.composition.is_empty()
            && let Some(cursor) = self.frame.cursor
        {
            let origin = point(
                bounds.left() + cell_width * f32::from(cursor.column),
                bounds.top() + line_height * f32::from(cursor.row),
            );
            let color = terminal_color(self.frame.default_foreground);
            let run = TextRun {
                len: self.composition.len(),
                font: text_style.font(),
                color,
                background_color: None,
                underline: Some(GpuiUnderlineStyle {
                    thickness: px(1.0),
                    color: Some(color),
                    wavy: false,
                }),
                strikethrough: None,
            };
            let line = window.text_system().shape_line(
                SharedString::from(self.composition.clone()),
                font_size,
                &[run],
                None,
            );
            glyphs.push((line, origin));
        }

        PrepaintState {
            backgrounds,
            glyphs,
            cursor,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.surface.read(cx).focus_handle_owned();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.surface.clone()),
            cx,
        );
        for background in prepaint.backgrounds.drain(..) {
            window.paint_quad(background);
        }
        let line_height = window.line_height();
        for (line, origin) in prepaint.glyphs.drain(..) {
            line.paint(origin, line_height, TextAlign::Left, None, window, cx)
                .expect("a pre-shaped terminal glyph must remain paintable");
        }
        if let Some(cursor) = prepaint.cursor.take() {
            window.paint_quad(cursor);
        }
    }

    fn a11y_role(&self) -> Option<gpui::accesskit::Role> {
        Some(gpui::accesskit::Role::Terminal)
    }

    fn write_a11y_info(&self, node: &mut gpui::accesskit::Node) {
        let title = self.frame.title.as_deref().unwrap_or("Terminal output");
        node.set_label(format!(
            "{title}, {} columns by {} rows",
            self.frame.grid.columns, self.frame.grid.rows
        ));
        node.set_description("Visible terminal text; live output is not announced continuously");
        node.set_value(
            self.frame
                .rows
                .iter()
                .map(|row| row.text())
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
}

fn cache_terminal_row(
    row: Arc<Row>,
    frame: &FullFrame,
    text_style: &gpui::TextStyle,
    font_size: Pixels,
    cell_width: Pixels,
    window: &mut Window,
) -> CachedRowPaint {
    let mut backgrounds = Vec::new();
    let mut column = 0;
    while column < row.cells.len() {
        let background = row
            .cells
            .get(column)
            .and_then(|cell| usize::try_from(cell.style_index).ok())
            .and_then(|index| frame.styles.get(index))
            .map(|style| {
                if style.inverse {
                    style.foreground
                } else {
                    style.background
                }
            })
            .unwrap_or(frame.default_background);
        let start = column;
        column += 1;
        while column < row.cells.len() {
            let next = usize::try_from(row.cells[column].style_index)
                .ok()
                .and_then(|index| frame.styles.get(index))
                .map(|style| {
                    if style.inverse {
                        style.foreground
                    } else {
                        style.background
                    }
                })
                .unwrap_or(frame.default_background);
            if next != background {
                break;
            }
            column += 1;
        }
        if background != frame.default_background {
            backgrounds.push(CachedBackgroundRun {
                start,
                end: column,
                color: background,
            });
        }
    }

    let mut glyphs = Vec::new();
    let mut column = 0;
    while column < row.cells.len() {
        let cell = &row.cells[column];
        let Some(style) = usize::try_from(cell.style_index)
            .ok()
            .and_then(|index| frame.styles.get(index))
        else {
            column += 1;
            continue;
        };
        if cell.width == 0 || cell.grapheme.is_empty() || style.invisible {
            column += 1;
            continue;
        }

        let start = column;
        let style_index = cell.style_index;
        let batch_ascii = cell.width == 1 && cell.grapheme.is_ascii();
        let mut text = cell.grapheme.clone();
        column += 1;
        if batch_ascii {
            while column < row.cells.len() {
                let next = &row.cells[column];
                if next.style_index != style_index
                    || next.width != 1
                    || next.grapheme.is_empty()
                    || !next.grapheme.is_ascii()
                {
                    break;
                }
                text.push_str(&next.grapheme);
                column += 1;
            }
        }

        let foreground = if style.inverse {
            style.background
        } else {
            style.foreground
        };
        let mut font = text_style.font();
        if style.bold {
            font = font.bold();
        }
        if style.italic {
            font = font.italic();
        }
        let color = if style.faint {
            gpui::rgb(rgb_hex(foreground)).alpha(0.6).into()
        } else {
            terminal_color(foreground)
        };
        let run = TextRun {
            len: text.len(),
            font,
            color,
            background_color: None,
            underline: terminal_underline(style.underline, color),
            strikethrough: style.strikethrough.then_some(StrikethroughStyle {
                thickness: px(1.0),
                color: Some(color),
            }),
        };
        glyphs.push(CachedGlyphRun {
            column: start,
            line: window.text_system().shape_line(
                SharedString::from(text),
                font_size,
                &[run],
                Some(cell_width),
            ),
        });
    }

    CachedRowPaint {
        source: row,
        backgrounds,
        glyphs,
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn grid_for_bounds(bounds: Bounds<Pixels>, cell_width: Pixels, line_height: Pixels) -> GridSize {
    let columns = (f32::from(bounds.size.width) / f32::from(cell_width))
        .floor()
        .clamp(2.0, f32::from(MAX_COLUMNS)) as u16;
    let rows = (f32::from(bounds.size.height) / f32::from(line_height))
        .floor()
        .clamp(1.0, f32::from(MAX_ROWS)) as u16;
    GridSize { columns, rows }
}

pub(crate) fn terminal_color(color: Rgb) -> gpui::Hsla {
    gpui::rgb(rgb_hex(color)).into()
}

fn rgb_hex(color: Rgb) -> u32 {
    (u32::from(color.red) << 16) | (u32::from(color.green) << 8) | u32::from(color.blue)
}

fn terminal_underline(
    underline: TerminalUnderlineStyle,
    color: gpui::Hsla,
) -> Option<GpuiUnderlineStyle> {
    if underline == TerminalUnderlineStyle::None {
        return None;
    }
    Some(GpuiUnderlineStyle {
        thickness: if underline == TerminalUnderlineStyle::Double {
            px(2.0)
        } else {
            px(1.0)
        },
        color: Some(color),
        wavy: underline == TerminalUnderlineStyle::Curly,
    })
}
