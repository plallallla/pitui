use std::{
    io::{self, Stdout, Write},
    time::Duration,
};

use crossterm::{
    event, execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use pitui_data::{UiFrame, ViewportMeasurement};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::{TerminalEvent, event_to_terminal_event, render};

/// Owns only terminal lifecycle and presentation. It never receives an ECS
/// World; callers pass the immutable frame and feed returned measurements back
/// into their data runtime.
pub struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalSession {
    pub fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(error);
        }

        match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(mut terminal) => {
                if let Err(error) = terminal.hide_cursor() {
                    let _ = disable_raw_mode();
                    let mut stdout = io::stdout();
                    let _ = execute!(stdout, LeaveAlternateScreen);
                    return Err(error);
                }
                Ok(Self { terminal })
            }
            Err(error) => {
                let _ = disable_raw_mode();
                let mut stdout = io::stdout();
                let _ = execute!(stdout, LeaveAlternateScreen);
                Err(error)
            }
        }
    }

    pub fn draw(&mut self, frame: &UiFrame) -> io::Result<Vec<ViewportMeasurement>> {
        let mut measurements = Vec::new();
        self.terminal
            .draw(|terminal_frame| measurements = render(terminal_frame, frame))?;
        Ok(measurements)
    }

    pub fn poll_event(&mut self, timeout: Duration) -> io::Result<Option<TerminalEvent>> {
        if !event::poll(timeout)? {
            return Ok(None);
        }
        Ok(event_to_terminal_event(event::read()?))
    }

    pub fn copy_to_clipboard(&mut self, value: &str) -> io::Result<()> {
        // OSC 52 stays terminal-native and carries only base64-encoded data.
        let encoded = base64_encode(value.as_bytes());
        write!(self.terminal.backend_mut(), "\x1b]52;c;{encoded}\x1b\\")?;
        self.terminal.backend_mut().flush()
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = self.terminal.show_cursor();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        output.push(ALPHABET[(first >> 2) as usize] as char);
        output.push(ALPHABET[(((first & 0x03) << 4) | (second >> 4)) as usize] as char);
        output.push(if chunk.len() > 1 {
            ALPHABET[(((second & 0x0f) << 2) | (third >> 6)) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            ALPHABET[(third & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    output
}

#[cfg(test)]
mod tests {
    use super::base64_encode;

    #[test]
    fn encodes_osc52_payload_without_external_processes() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode("提交".as_bytes()), "5o+Q5Lqk");
    }
}
