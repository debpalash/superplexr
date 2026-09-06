//! Bounded canonical-history reads without moving the viewport or selection.
use libghostty_vt::{
    fmt::Format,
    screen::{Screen, TrackedGridRef},
    selection::{FormatOptions, Selection},
    terminal::{Point, PointCoordinate, PointSpace},
};
use std::collections::VecDeque;

use super::{SearchMatch, TerminalError, TerminalModel};

const ROWS_PER_STEP: usize = 32;
const MATCHES_PER_STEP: usize = 64;
const ROW_BYTES: usize = 64 * 1024;

/// An actor-local, opaque search continuation. Drop it to cancel. Appended rows
/// beyond the captured end are excluded. Edits to not-yet-read rows may be seen:
/// this is an incremental live read, not an immutable terminal snapshot.
/// Pruning the origin, resizing, or switching screens returns `SearchInvalidated`
/// rather than combining incompatible coordinates. Never move this across actors.
#[derive(Debug)]
pub struct HistorySearch {
    origin: TrackedGridRef,
    end: TrackedGridRef,
    screen: Screen,
    layout_epoch: u64,
    columns: u16,
    row: u32,
    query: String,
    case_sensitive: bool,
    remaining: usize,
    buffer: Vec<u8>,
    pending: Option<PendingRow>,
    rows: VecDeque<String>,
    rows_per_read: usize,
    complete: bool,
}

#[derive(Debug)]
struct PendingRow {
    text: String,
    searchable: String,
    next_byte: usize,
}

/// A bounded group of matches. An empty incomplete batch still advances work.
#[derive(Debug)]
pub struct SearchBatch {
    pub matches: Vec<SearchMatch>,
    pub complete: bool,
    pub rows_scanned: usize,
}

impl TerminalModel {
    /// Start a bounded search through the currently retained screen. Queries
    /// are literal Unicode text, at most 1024 UTF-8 bytes. Each step reads at
    /// most 32 rows into a reusable 64 KiB row buffer and returns <=64 matches.
    /// A single row exceeding that buffer fails explicitly, never truncates.
    pub fn begin_search(
        &self,
        query: &str,
        case_sensitive: bool,
        limit: usize,
    ) -> Result<HistorySearch, TerminalError> {
        if query.len() > 1024 {
            return Err(TerminalError::SearchQueryTooLong);
        }
        let rows = self.terminal.total_rows()?;
        let columns = self.terminal.cols()?;
        Ok(HistorySearch {
            origin: self.terminal.track_grid_ref(point(0, 0))?,
            end: self
                .terminal
                .track_grid_ref(point(columns - 1, (rows - 1) as u32))?,
            screen: self.terminal.active_screen()?,
            layout_epoch: self.search_layout_epoch,
            columns,
            row: 0,
            query: if case_sensitive {
                query.to_owned()
            } else {
                query.to_lowercase()
            },
            case_sensitive,
            remaining: limit,
            buffer: vec![0; ROW_BYTES],
            pending: None,
            rows: VecDeque::new(),
            rows_per_read: ROWS_PER_STEP,
            complete: query.is_empty() || limit == 0,
        })
    }

    /// Continue on the same model. Native references never escape this call:
    /// fresh snapshots are reconstructed only after validating the continuation.
    pub fn search_step(&self, search: &mut HistorySearch) -> Result<SearchBatch, TerminalError> {
        // Snapshot validates model ownership even for an already completed job.
        if search.origin.snapshot(&self.terminal)?.is_none()
            || search
                .origin
                .point(PointSpace::Screen)?
                .is_none_or(|p| p.y != 0)
            || search.layout_epoch != self.search_layout_epoch
            || search.screen != self.terminal.active_screen()?
            || search.columns != self.terminal.cols()?
        {
            return Err(TerminalError::SearchInvalidated);
        }
        let end = search
            .end
            .point(PointSpace::Screen)?
            .ok_or(TerminalError::SearchInvalidated)?
            .y;
        let mut batch = SearchBatch {
            matches: Vec::new(),
            complete: search.complete,
            rows_scanned: 0,
        };
        while !search.complete
            && batch.rows_scanned < ROWS_PER_STEP
            && batch.matches.len() < MATCHES_PER_STEP
        {
            if search.row > end {
                search.complete = true;
                break;
            }
            if search.pending.is_none() {
                if search.rows.is_empty() {
                    self.history_maintenance.note_history_read();
                    let count = (end - search.row + 1)
                        .min(search.rows_per_read.min(ROWS_PER_STEP - batch.rows_scanned) as u32);
                    let selection = Selection::new(
                        self.terminal.grid_ref(point(0, search.row))?,
                        self.terminal
                            .grid_ref(point(search.columns - 1, search.row + count - 1))?,
                        false,
                    );
                    let length = match self.terminal.format_selection_buf(
                        FormatOptions::new()
                            .with_emit_format(Format::Plain)
                            .with_unwrap(false)
                            .with_trim(true)
                            .with_selection(&selection),
                        &mut search.buffer,
                    ) {
                        Ok(Some(length)) => length,
                        Ok(None) => 0,
                        Err(libghostty_vt::Error::OutOfSpace { .. }) if count > 1 => {
                            // Yield before retrying a smaller range. Retain
                            // the learned size so wide rows do not repeatedly
                            // pay for the same oversized formatting attempts.
                            search.rows_per_read = count.div_ceil(2) as usize;
                            batch.rows_scanned += count as usize;
                            return Ok(batch);
                        }
                        Err(libghostty_vt::Error::OutOfSpace { .. }) => {
                            return Err(TerminalError::SearchRowTooLarge);
                        }
                        Err(error) => return Err(error.into()),
                    };
                    let text = String::from_utf8_lossy(&search.buffer[..length]);
                    search.rows.extend(
                        text.split('\n')
                            .take(count as usize)
                            .map(|row| row.trim_end_matches('\r').to_owned()),
                    );
                    search.rows.resize(count as usize, String::new());
                    batch.rows_scanned += count as usize;
                }
                let text = search.rows.pop_front().expect("formatted rows available");
                let searchable = if search.case_sensitive {
                    text.clone()
                } else {
                    text.to_lowercase()
                };
                search.pending = Some(PendingRow {
                    text,
                    searchable,
                    next_byte: 0,
                });
            }
            let row = search.pending.as_mut().expect("row prepared above");
            if let Some(offset) = row.searchable[row.next_byte..].find(&search.query) {
                let byte = row.next_byte + offset;
                // Lowercasing may expand characters (e.g. İ). Map folded byte
                // offsets back to the original scalar coordinate, preserving
                // the existing SearchMatch column contract.
                let column = if search.case_sensitive {
                    row.text[..byte].chars().count()
                } else {
                    let mut folded_bytes = 0;
                    row.text
                        .chars()
                        .take_while(|ch| {
                            if folded_bytes > byte {
                                return false;
                            }
                            folded_bytes += ch.to_lowercase().map(char::len_utf8).sum::<usize>();
                            folded_bytes <= byte
                        })
                        .count()
                };
                batch.matches.push(SearchMatch {
                    line: search.row as usize,
                    column,
                    preview: row.text.clone(),
                });
                row.next_byte = byte + search.query.len();
                search.remaining -= 1;
                if search.remaining == 0 {
                    search.complete = true;
                }
            } else {
                search.pending = None;
                search.row += 1;
            }
        }
        batch.complete = search.complete;
        Ok(batch)
    }
}

fn point(x: u16, y: u32) -> Point {
    Point::Screen(PointCoordinate { x, y })
}
