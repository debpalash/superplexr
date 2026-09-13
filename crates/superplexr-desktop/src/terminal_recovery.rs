//! Bounded recovery policy for terminal shells.

use std::collections::HashSet;
#[cfg(not(test))]
use std::time::Duration;

use superplexr_protocol::SessionGroupId;

/// A replacement shell must remain alive this long before another automatic
/// recovery is allowed for the same session group.
#[cfg(not(test))]
pub(crate) const TERMINAL_STABILITY_WINDOW: Duration = Duration::from_secs(10);

/// Prevents a broken shell configuration from becoming an infinite restart
/// loop while still recovering a transient PTY or shell failure automatically.
#[derive(Default)]
pub(crate) struct TerminalRecovery {
    recovering_groups: HashSet<SessionGroupId>,
}

/// A bound Run has an authoritative terminal outcome and must not be replaced
/// by an unrelated shell. Only owner-controlled interactive shells self-heal.
pub(crate) const fn should_auto_heal(has_run_binding: bool) -> bool {
    !has_run_binding
}

impl TerminalRecovery {
    /// Returns `true` exactly once until the replacement proves stable.
    pub(crate) fn begin(&mut self, group_id: SessionGroupId) -> bool {
        self.recovering_groups.insert(group_id)
    }

    pub(crate) fn is_recovering(&self, group_id: SessionGroupId) -> bool {
        self.recovering_groups.contains(&group_id)
    }

    pub(crate) fn mark_stable(&mut self, group_id: SessionGroupId) {
        self.recovering_groups.remove(&group_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_is_single_shot_until_the_replacement_is_stable() {
        let group_id = SessionGroupId::new();
        let mut recovery = TerminalRecovery::default();

        assert!(recovery.begin(group_id));
        assert!(!recovery.begin(group_id));
        assert!(recovery.is_recovering(group_id));

        recovery.mark_stable(group_id);
        assert!(!recovery.is_recovering(group_id));
        assert!(recovery.begin(group_id));
    }

    #[test]
    fn only_interactive_shells_auto_heal() {
        assert!(should_auto_heal(false));
        assert!(!should_auto_heal(true));
    }
}
