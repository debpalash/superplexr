//! Local prefix routing precedes remote terminal key encoding.
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers as Mods};
use ultraplexr_terminal::{KeyAction, KeyInput, KeyModifiers};

#[derive(Debug, PartialEq)]
pub enum Action {
    Detach,
    Claim,
    Release,
    Pause,
    History,
    Search,
    Live,
    Copy,
    Help,
    Navigate,
    Attention,
    SplitVertical,
    SplitHorizontal,
    NextPane,
    ClosePane,
    Key(KeyInput),
    None,
}

#[derive(Default)]
pub struct Router {
    prefix: bool,
}

impl Router {
    pub fn waiting(&self) -> bool {
        self.prefix
    }
    pub fn route(&mut self, mut event: KeyEvent) -> Action {
        if event.kind == KeyEventKind::Release {
            return Action::None;
        }
        // Legacy byte 0x1d is ambiguous (Ctrl-] / Ctrl-5); Crossterm calls it
        // Ctrl-5. Normalize both spellings before routing or literal forwarding.
        let prefix =
            matches!(event.code, KeyCode::Char(']' | '5')) && event.modifiers == Mods::CONTROL;
        if prefix {
            event.code = KeyCode::Char(']');
        }
        if self.prefix {
            self.prefix = false;
            if prefix {
                return encode(event).map(Action::Key).unwrap_or(Action::None);
            }
            return match event.code {
                KeyCode::Char('d' | 'q') => Action::Detach,
                KeyCode::Char('c') => Action::Claim,
                KeyCode::Char('r') => Action::Release,
                KeyCode::Char('p') => Action::Pause,
                KeyCode::Char('h') | KeyCode::PageUp => Action::History,
                KeyCode::Char('/') => Action::Search,
                KeyCode::Char('l') | KeyCode::End => Action::Live,
                KeyCode::Char('y') => Action::Copy,
                KeyCode::Char('n') => Action::Navigate,
                KeyCode::Char('a') => Action::Attention,
                KeyCode::Char('v') => Action::SplitVertical,
                KeyCode::Char('s') => Action::SplitHorizontal,
                KeyCode::Char('o') | KeyCode::Tab => Action::NextPane,
                KeyCode::Char('x') => Action::ClosePane,
                _ => Action::Help,
            };
        }
        if prefix {
            self.prefix = true;
            return Action::Help;
        }
        encode(event).map(Action::Key).unwrap_or(Action::None)
    }
}

pub fn encode(event: KeyEvent) -> Option<KeyInput> {
    let (physical, text) = match event.code {
        KeyCode::Char(' ') => ("space".into(), Some(" ".into())),
        KeyCode::Char(c) => (
            c.to_ascii_lowercase().to_string(),
            (!c.is_control()).then(|| c.to_string()),
        ),
        KeyCode::Enter => ("enter".into(), None),
        KeyCode::Tab | KeyCode::BackTab => ("tab".into(), None),
        KeyCode::Backspace => ("backspace".into(), None),
        KeyCode::Esc => ("escape".into(), None),
        KeyCode::Left => ("left".into(), None),
        KeyCode::Right => ("right".into(), None),
        KeyCode::Up => ("up".into(), None),
        KeyCode::Down => ("down".into(), None),
        KeyCode::Home => ("home".into(), None),
        KeyCode::End => ("end".into(), None),
        KeyCode::PageUp => ("pageup".into(), None),
        KeyCode::PageDown => ("pagedown".into(), None),
        KeyCode::Delete => ("delete".into(), None),
        KeyCode::Insert => ("insert".into(), None),
        KeyCode::F(n) if n <= 25 => (format!("f{n}"), None),
        _ => return None,
    };
    Some(KeyInput {
        logical_key: text.clone().unwrap_or_else(|| physical.clone()),
        unshifted_codepoint: match event.code {
            KeyCode::Char(c) => Some(c.to_ascii_lowercase()),
            _ => None,
        },
        physical_key: physical,
        text,
        modifiers: KeyModifiers {
            shift: event.modifiers.contains(Mods::SHIFT) || event.code == KeyCode::BackTab,
            alt: event.modifiers.contains(Mods::ALT),
            control: event.modifiers.contains(Mods::CONTROL),
            super_key: event.modifiers.contains(Mods::SUPER),
            ..Default::default()
        },
        consumed_modifiers: KeyModifiers::default(),
        action: if event.kind == KeyEventKind::Repeat {
            KeyAction::Repeat
        } else {
            KeyAction::Press
        },
        composing: false,
    })
}

pub fn risky_paste(text: &str) -> bool {
    text.chars().any(char::is_control)
}

#[cfg(test)]
#[path = "input_tests.rs"]
mod tests;
