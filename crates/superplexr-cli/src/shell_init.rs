//! Shell integration that makes failing commands visible to superplexr.
//!
//! The runtime detects individual command failures from OSC 133 marks, which
//! only appear if the shell emits them. This module produces that snippet and
//! can install it into a startup file.
//!
//! The snippet states the command inside the `C` mark rather than relying on
//! the echoed line: tab completion and line editing rewrite the echo, so the
//! echoed text is not reliably replayable.
//!
//! It is written to be inert outside superplexr (it checks `SUPERPLEXR_SESSION`),
//! to avoid clobbering existing integrations, and to never change a shell's
//! exit status.

use std::{fmt, str::FromStr};

/// Marker lines that make an installed block findable and replaceable.
pub(crate) const BEGIN_MARKER: &str = "# >>> superplexr shell integration >>>";
pub(crate) const END_MARKER: &str = "# <<< superplexr shell integration <<<";
/// Markers written before the rename. A startup file carrying one of these
/// blocks is upgraded in place rather than gaining a second block.
const LEGACY_BEGIN_MARKER: &str = "# >>> termi9ne shell integration >>>";
const LEGACY_END_MARKER: &str = "# <<< termi9ne shell integration <<<";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Shell {
    Zsh,
    Bash,
    Fish,
}

impl Shell {
    /// Guess the shell from `$SHELL`, defaulting to the most common one.
    pub(crate) fn detect(shell_path: Option<&str>) -> Self {
        let name = shell_path
            .and_then(|path| path.rsplit('/').next())
            .unwrap_or_default();
        match name {
            "bash" => Self::Bash,
            "fish" => Self::Fish,
            _ => Self::Zsh,
        }
    }

    /// Conventional startup file, relative to the home directory.
    pub(crate) fn startup_file(self) -> &'static str {
        match self {
            Self::Zsh => ".zshrc",
            Self::Bash => ".bashrc",
            Self::Fish => ".config/fish/config.fish",
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Zsh => "zsh",
            Self::Bash => "bash",
            Self::Fish => "fish",
        }
    }
}

impl fmt::Display for Shell {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl FromStr for Shell {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "zsh" => Ok(Self::Zsh),
            "bash" => Ok(Self::Bash),
            "fish" => Ok(Self::Fish),
            other => Err(format!("unsupported shell {other}")),
        }
    }
}

/// The integration snippet for one shell, wrapped in its markers.
pub(crate) fn snippet(shell: Shell) -> String {
    format!("{BEGIN_MARKER}\n{}{END_MARKER}\n", body(shell))
}

fn body(shell: Shell) -> String {
    match shell {
        Shell::Zsh => ZSH.to_owned(),
        Shell::Bash => BASH.to_owned(),
        Shell::Fish => FISH.to_owned(),
    }
}

/// zsh exposes exactly the hooks this needs: `preexec` knows the command
/// before it runs, and `precmd` knows its status afterwards.
const ZSH: &str = r#"# Reports each command's exit status to superplexr so failures become Faults.
# Inert outside a superplexr terminal, and never changes $?.
if [[ -n "$SUPERPLEXR_SESSION" ]] && [[ -o interactive ]]; then
  autoload -Uz add-zsh-hook

  __superplexr_preexec() {
    # The command is stated here because the echoed line is rewritten by
    # completion and line editing.
    printf '\e]133;C;%s\a' "$1"
  }

  __superplexr_precmd() {
    local __superplexr_status=$?
    printf '\e]133;D;%s\a' "$__superplexr_status"
    # Report the directory too, so the session can be named by where it runs.
    printf '\e]7;file://%s%s\a' "$HOST" "$PWD"
    # Leave the keyboard in plain mode at the prompt. A program that enabled
    # the Kitty keyboard protocol and crashed would otherwise leave the shell
    # typing out key events as text. Ignored by terminals without it.
    printf '\e[=0;1u'
    printf '\e]133;A\a'
    return $__superplexr_status
  }

  add-zsh-hook preexec __superplexr_preexec
  add-zsh-hook precmd __superplexr_precmd
fi
"#;

/// bash has no `preexec`, so the DEBUG trap stands in for it. `BASH_COMMAND`
/// is the command about to run; the guard keeps the trap from firing for each
/// command of a compound statement or during the prompt itself.
const BASH: &str = r#"# Reports each command's exit status to superplexr so failures become Faults.
# Inert outside a superplexr terminal, and never changes $?.
if [ -n "$SUPERPLEXR_SESSION" ] && [ -n "$PS1" ]; then
  __superplexr_preexec() {
    # Skip while the prompt command runs, and report only the first command
    # of a line.
    [ -n "$COMP_LINE" ] && return
    [ "$__superplexr_armed" != "1" ] && return
    __superplexr_armed=0
    printf '\e]133;C;%s\a' "$BASH_COMMAND"
  }

  __superplexr_precmd() {
    local __superplexr_status=$?
    printf '\e]133;D;%s\a' "$__superplexr_status"
    # Report the directory too, so the session can be named by where it runs.
    printf '\e]7;file://%s%s\a' "$HOSTNAME" "$PWD"
    # Leave the keyboard in plain mode at the prompt; see the zsh snippet.
    printf '\e[=0;1u'
    printf '\e]133;A\a'
    __superplexr_armed=1
    return $__superplexr_status
  }

  __superplexr_armed=1
  trap '__superplexr_preexec' DEBUG
  case "$PROMPT_COMMAND" in
    *__superplexr_precmd*) ;;
    "") PROMPT_COMMAND="__superplexr_precmd" ;;
    *) PROMPT_COMMAND="__superplexr_precmd;$PROMPT_COMMAND" ;;
  esac
fi
"#;

/// fish has first-class events, so no trap gymnastics are needed.
const FISH: &str = r#"# Reports each command's exit status to superplexr so failures become Faults.
# Inert outside a superplexr terminal.
if set -q SUPERPLEXR_SESSION; and status is-interactive
    function __superplexr_preexec --on-event fish_preexec
        printf '\e]133;C;%s\a' "$argv[1]"
    end

    function __superplexr_postexec --on-event fish_postexec
        set -l __superplexr_status $status
        printf '\e]133;D;%s\a' $__superplexr_status
        # Report the directory too, so the session can be named by where it runs.
        printf '\e]7;file://%s%s\a' (hostname) "$PWD"
        # Leave the keyboard in plain mode at the prompt; see the zsh snippet.
        printf '\e[=0;1u'
        printf '\e]133;A\a'
    end
end
"#;

/// What installing into a startup file would do.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InstallOutcome {
    /// The file had no superplexr block; one was appended.
    Added,
    /// An older block was replaced in place.
    Updated,
    /// The file already contained exactly this block.
    Unchanged,
}

/// Produce the new contents of a startup file with the snippet installed.
///
/// Installation is idempotent and replaces any previous block in place, so
/// running it repeatedly never stacks duplicates.
pub(crate) fn install_into(existing: &str, shell: Shell) -> (String, InstallOutcome) {
    let block = snippet(shell);
    let (begin_marker, end_marker) = if existing.contains(BEGIN_MARKER) {
        (BEGIN_MARKER, END_MARKER)
    } else {
        (LEGACY_BEGIN_MARKER, LEGACY_END_MARKER)
    };
    let Some(start) = existing.find(begin_marker) else {
        let mut updated = existing.to_owned();
        if !updated.is_empty() && !updated.ends_with('\n') {
            updated.push('\n');
        }
        if !updated.is_empty() {
            updated.push('\n');
        }
        updated.push_str(&block);
        return (updated, InstallOutcome::Added);
    };
    // Replace from the marker through the end marker's line, leaving anything
    // the person wrote around it untouched.
    let end = match existing[start..].find(end_marker) {
        Some(offset) => {
            let absolute = start + offset + end_marker.len();
            existing[absolute..]
                .find('\n')
                .map_or(existing.len(), |newline| absolute + newline + 1)
        }
        // A truncated block: replace everything from the marker onward rather
        // than leaving a half block behind.
        None => existing.len(),
    };
    if existing[start..end] == block {
        return (existing.to_owned(), InstallOutcome::Unchanged);
    }
    let mut updated = String::with_capacity(existing.len() + block.len());
    updated.push_str(&existing[..start]);
    updated.push_str(&block);
    updated.push_str(&existing[end..]);
    (updated, InstallOutcome::Updated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shells_are_detected_from_the_path_with_a_sensible_default() {
        assert_eq!(Shell::detect(Some("/bin/bash")), Shell::Bash);
        assert_eq!(Shell::detect(Some("/usr/local/bin/fish")), Shell::Fish);
        assert_eq!(Shell::detect(Some("/bin/zsh")), Shell::Zsh);
        assert_eq!(Shell::detect(None), Shell::Zsh);
        assert_eq!(Shell::detect(Some("/usr/bin/nu")), Shell::Zsh);
        assert_eq!("fish".parse::<Shell>(), Ok(Shell::Fish));
        assert!("elvish".parse::<Shell>().is_err());
    }

    #[test]
    fn every_snippet_emits_the_marks_the_parser_needs() {
        for shell in [Shell::Zsh, Shell::Bash, Shell::Fish] {
            let snippet = snippet(shell);
            assert!(snippet.starts_with(BEGIN_MARKER), "{shell}");
            assert!(snippet.trim_end().ends_with(END_MARKER), "{shell}");
            // The command is stated in the C mark, and the status in D.
            assert!(snippet.contains(r"\e]133;C;%s\a"), "{shell}");
            assert!(snippet.contains(r"\e]133;D;%s\a"), "{shell}");
            assert!(snippet.contains(r"\e]133;A\a"), "{shell}");
            // The directory is reported at every prompt, so a Session can be
            // named by where it runs even before any command finishes.
            assert!(snippet.contains(r"\e]7;file://%s%s\a"), "{shell}");
            // The keyboard is returned to plain mode at every prompt, so a
            // crashed TUI cannot leave the shell typing key events as text.
            assert!(snippet.contains(r"\e[=0;1u"), "{shell}");
            // Inert outside superplexr.
            assert!(snippet.contains("SUPERPLEXR_SESSION"), "{shell}");
        }
    }

    #[test]
    fn the_posix_snippets_preserve_the_exit_status() {
        // A prompt hook that swallows $? would break every `&&` a person
        // types after a failing command.
        for shell in [Shell::Zsh, Shell::Bash] {
            let snippet = snippet(shell);
            assert!(
                snippet.contains("return $__superplexr_status"),
                "{shell} must restore the status it observed"
            );
        }
    }

    #[test]
    fn installing_appends_once_and_then_replaces_in_place() {
        let (added, outcome) = install_into("export PATH=/usr/bin\n", Shell::Zsh);
        assert_eq!(outcome, InstallOutcome::Added);
        assert!(added.starts_with("export PATH=/usr/bin\n"));
        assert!(added.contains(BEGIN_MARKER));

        // Running it again changes nothing.
        let (again, outcome) = install_into(&added, Shell::Zsh);
        assert_eq!(outcome, InstallOutcome::Unchanged);
        assert_eq!(again, added);

        // A stale block is replaced, not duplicated.
        let stale = added.replace("133;A", "133;OLD");
        let (fixed, outcome) = install_into(&stale, Shell::Zsh);
        assert_eq!(outcome, InstallOutcome::Updated);
        assert_eq!(fixed.matches(BEGIN_MARKER).count(), 1);
        assert!(!fixed.contains("133;OLD"));
        assert!(fixed.contains("133;A"));
    }

    #[test]
    fn installing_keeps_whatever_surrounds_the_block() {
        let before = format!(
            "alias a=b\n\n{}\n# my own note\nalias c=d\n",
            snippet(Shell::Zsh).trim_end()
        );
        let (updated, outcome) = install_into(&before, Shell::Bash);
        assert_eq!(outcome, InstallOutcome::Updated);
        assert!(updated.starts_with("alias a=b\n"));
        assert!(updated.contains("# my own note"));
        assert!(updated.contains("alias c=d\n"));
        assert_eq!(updated.matches(BEGIN_MARKER).count(), 1);
        assert!(updated.contains("BASH_COMMAND"));
    }

    /// A startup file from before the rename is upgraded, not duplicated.
    #[test]
    fn a_block_from_before_the_rename_is_replaced_in_place() {
        let legacy = format!(
            "alias a=b\n{LEGACY_BEGIN_MARKER}\nif [[ -n \"$TERMI9NE_SESSION\" ]]; then :; fi\n{LEGACY_END_MARKER}\nalias c=d\n"
        );
        let (updated, outcome) = install_into(&legacy, Shell::Zsh);
        assert_eq!(outcome, InstallOutcome::Updated);
        assert!(!updated.contains(LEGACY_BEGIN_MARKER), "the old block must go");
        assert!(!updated.contains("TERMI9NE_SESSION"));
        assert_eq!(updated.matches(BEGIN_MARKER).count(), 1);
        assert!(updated.starts_with("alias a=b\n"));
        assert!(updated.ends_with("alias c=d\n"));
    }

    #[test]
    fn a_truncated_block_is_repaired_rather_than_left_behind() {
        let broken = format!("alias a=b\n{BEGIN_MARKER}\nhalf written\n");
        let (updated, outcome) = install_into(&broken, Shell::Zsh);
        assert_eq!(outcome, InstallOutcome::Updated);
        assert!(!updated.contains("half written"));
        assert_eq!(updated.matches(BEGIN_MARKER).count(), 1);
        assert_eq!(updated.matches(END_MARKER).count(), 1);
    }

    #[test]
    fn installing_into_an_empty_file_adds_no_leading_blank_line() {
        let (updated, outcome) = install_into("", Shell::Fish);
        assert_eq!(outcome, InstallOutcome::Added);
        assert!(updated.starts_with(BEGIN_MARKER));
        assert!(updated.contains("fish_preexec"));
    }

    #[test]
    fn startup_files_follow_each_shell_convention() {
        assert_eq!(Shell::Zsh.startup_file(), ".zshrc");
        assert_eq!(Shell::Bash.startup_file(), ".bashrc");
        assert_eq!(Shell::Fish.startup_file(), ".config/fish/config.fish");
    }
}
