use crossterm::event::{
    Event, KeyCode as CrosstermKeyCode, KeyEvent, KeyEventKind, KeyModifiers as CrosstermModifiers,
};
use pitui_data::{InputIntent, KeyCode, KeyModifiers, KeyStroke};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TerminalEvent {
    Input(InputIntent),
    Resize { columns: u16, rows: u16 },
}

pub fn event_to_intent(event: Event) -> Option<InputIntent> {
    match event_to_terminal_event(event) {
        Some(TerminalEvent::Input(intent)) => Some(intent),
        Some(TerminalEvent::Resize { .. }) | None => None,
    }
}

pub fn event_to_terminal_event(event: Event) -> Option<TerminalEvent> {
    match event {
        Event::Key(event) => key_event_to_stroke(event)
            .map(InputIntent::Key)
            .map(TerminalEvent::Input),
        Event::Paste(text) => Some(TerminalEvent::Input(InputIntent::Paste(text))),
        Event::Resize(columns, rows) => Some(TerminalEvent::Resize { columns, rows }),
        _ => None,
    }
}

pub fn key_event_to_stroke(event: KeyEvent) -> Option<KeyStroke> {
    if !matches!(event.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return None;
    }
    let code = match event.code {
        CrosstermKeyCode::Char(' ') => KeyCode::Space,
        CrosstermKeyCode::Char(character) => KeyCode::Character(character.to_ascii_lowercase()),
        CrosstermKeyCode::Up => KeyCode::Up,
        CrosstermKeyCode::Down => KeyCode::Down,
        CrosstermKeyCode::Left => KeyCode::Left,
        CrosstermKeyCode::Right => KeyCode::Right,
        CrosstermKeyCode::Home => KeyCode::Home,
        CrosstermKeyCode::End => KeyCode::End,
        CrosstermKeyCode::PageUp => KeyCode::PageUp,
        CrosstermKeyCode::PageDown => KeyCode::PageDown,
        CrosstermKeyCode::Enter => KeyCode::Enter,
        CrosstermKeyCode::Esc => KeyCode::Escape,
        CrosstermKeyCode::Backspace => KeyCode::Backspace,
        CrosstermKeyCode::Tab | CrosstermKeyCode::BackTab => KeyCode::Tab,
        _ => return None,
    };
    Some(KeyStroke {
        code,
        modifiers: KeyModifiers {
            control: event.modifiers.contains(CrosstermModifiers::CONTROL),
            alt: event.modifiers.contains(CrosstermModifiers::ALT),
            shift: event.modifiers.contains(CrosstermModifiers::SHIFT)
                || event.code == CrosstermKeyCode::BackTab,
            super_key: event.modifiers.contains(CrosstermModifiers::SUPER),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_navigation_chords_and_ignores_key_release() {
        assert_eq!(
            key_event_to_stroke(KeyEvent::new(
                CrosstermKeyCode::Char('w'),
                CrosstermModifiers::NONE,
            )),
            Some(KeyStroke::character('w'))
        );
        assert_eq!(
            key_event_to_stroke(KeyEvent::new(
                CrosstermKeyCode::Char('c'),
                CrosstermModifiers::CONTROL,
            )),
            Some(KeyStroke::control('c'))
        );
        assert_eq!(
            key_event_to_stroke(KeyEvent::new(
                CrosstermKeyCode::PageDown,
                CrosstermModifiers::NONE,
            )),
            Some(KeyStroke::plain(KeyCode::PageDown))
        );
        assert_eq!(
            key_event_to_stroke(KeyEvent::new_with_kind(
                CrosstermKeyCode::Char('w'),
                CrosstermModifiers::NONE,
                KeyEventKind::Release,
            )),
            None
        );
    }

    #[test]
    fn preserves_resize_as_terminal_adapter_data() {
        assert_eq!(
            event_to_terminal_event(Event::Resize(120, 40)),
            Some(TerminalEvent::Resize {
                columns: 120,
                rows: 40,
            })
        );
        assert_eq!(event_to_intent(Event::Resize(120, 40)), None);
        assert_eq!(
            event_to_intent(Event::Paste("quit".into())),
            Some(InputIntent::Paste("quit".into()))
        );
    }
}
