use std::{
    cell::Cell,
    time::{Duration, Instant},
};

use libghostty_vt::{
    Terminal,
    terminal::{CompressionActivity, CompressionMode, CompressionResult},
};

use super::TerminalError;

const IDLE_DELAY: Duration = Duration::from_secs(1);
const STEP_INTERVAL: Duration = Duration::from_millis(8);

/// Scheduling and backend activity tracking stay inside the terminal module.
#[derive(Debug, Default)]
pub(super) struct HistoryMaintenance {
    activity: Option<CompressionActivity>,
    next_step: Option<Instant>,
    disabled: bool,
    history_read: Cell<bool>,
}

impl HistoryMaintenance {
    /// Formatting can restore native pages without advancing its activity
    /// token. Reads remain `&self` at the adapter boundary, so remember this
    /// actor-local scheduling fact without changing visible terminal state.
    pub(super) fn note_history_read(&self) {
        self.history_read.set(true);
    }

    pub(super) fn poll(
        &mut self,
        terminal: &mut Terminal<'_, '_>,
        now: Instant,
    ) -> Result<Option<Duration>, TerminalError> {
        if self.disabled {
            return Ok(None);
        }
        // Optimization failure must not turn into repeated retries or PTY loss.
        self.disabled = true;
        let activity = terminal.compression_activity()?;
        let history_read = self.history_read.replace(false);
        if self.activity != Some(activity) || history_read {
            self.activity = Some(activity);
            self.next_step = Some(now + IDLE_DELAY);
        }
        if self.next_step.is_some_and(|deadline| now >= deadline) {
            self.next_step = match terminal.compress(CompressionMode::Incremental)? {
                CompressionResult::Pending => Some(now + STEP_INTERVAL),
                CompressionResult::Complete => None,
                CompressionResult::Unsupported => return Ok(None),
            };
        }
        self.disabled = false;
        Ok(self
            .next_step
            .map(|deadline| deadline.saturating_duration_since(now)))
    }
}
