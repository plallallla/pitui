mod input;
mod render;

pub use input::map_key;
pub use render::render;

use std::{
    error::Error,
    io::{self, Stdout, Write},
    time::{Duration, Instant},
};

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::{app::Action, app::App};

const TICK_RATE: Duration = Duration::from_millis(200);
const EVENT_POLL_RATE: Duration = Duration::from_millis(50);

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalSession {
    fn enter() -> Result<Self, Box<dyn Error>> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, EnableMouseCapture) {
            let _ = disable_raw_mode();
            return Err(error.into());
        }

        match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(mut terminal) => {
                // Entering the alternate screen already gives us a clean
                // surface. `Terminal::clear` queries the cursor position on
                // some backends, which breaks otherwise valid minimal PTYs.
                terminal.hide_cursor()?;
                Ok(Self { terminal })
            }
            Err(error) => {
                let _ = disable_raw_mode();
                let mut stdout = io::stdout();
                let _ = execute!(stdout, LeaveAlternateScreen, DisableMouseCapture);
                Err(error.into())
            }
        }
    }

    fn copy_to_clipboard(&mut self, value: &str) -> io::Result<()> {
        // OSC 52 keeps clipboard integration terminal-native and avoids
        // platform-specific helpers such as pbcopy/xclip. Modern terminals
        // either apply it directly or safely ignore it when disabled.
        let encoded = base64_encode(value.as_bytes());
        write!(self.terminal.backend_mut(), "\x1b]52;c;{encoded}\x1b\\")?;
        self.terminal.backend_mut().flush()
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

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        );
        let _ = self.terminal.show_cursor();
    }
}

pub fn run(mut app: App) -> Result<(), Box<dyn Error>> {
    let mut terminal = TerminalSession::enter()?;
    let mut last_tick = Instant::now();

    while !app.should_quit() {
        app.poll_git();
        if last_tick.elapsed() >= TICK_RATE {
            app.dispatch(Action::Tick);
            last_tick = Instant::now();
        }

        if let Some(value) = app.take_clipboard_request() {
            terminal.copy_to_clipboard(&value)?;
        }

        terminal
            .terminal
            .draw(|frame| render::render(frame, &app.state))?;

        if event::poll(EVENT_POLL_RATE)? {
            match event::read()? {
                Event::Key(key) => {
                    if let Some(action) = input::map_key(&app.state, key) {
                        app.dispatch(action);
                    }
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::base64_encode;

    #[test]
    fn encodes_osc52_payloads_without_an_external_dependency() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode("提交".as_bytes()), "5o+Q5Lqk");
    }
}
