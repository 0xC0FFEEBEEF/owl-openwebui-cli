use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::Widget;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use ratatui::widgets::WidgetRef;
use ratatui::widgets::Wrap;
use std::path::PathBuf;
use std::time::Instant;

use codex_core::DEFAULT_OPENWEBUI_PORT;
use codex_core::OPENWEBUI_PROVIDER_ID;
use codex_core::config::edit::ConfigEdit;
use codex_core::config::edit::ConfigEditsBuilder;
use toml_edit::value;

use crate::little_owl::LittleOwl;
use crate::onboarding::onboarding_screen::KeyboardHandler;
use crate::onboarding::onboarding_screen::StepState;
use crate::onboarding::onboarding_screen::StepStateProvider;
use crate::tui::FrameRequester;

const OPENWEBUI_DISPLAY_NAME: &str = "Open WebUI";

/// Values recorded by the connect step (the Open WebUI replacement for the
/// ChatGPT login screen): the server base URL and an optional API token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConnectSelection {
    pub base_url: String,
    pub token: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectField {
    Server,
    Token,
}

pub(crate) struct ConnectWidget {
    pub request_frame: FrameRequester,
    /// The idle owl shown above the fields while the user fills them in.
    pub owl: LittleOwl,
    pub codex_home: PathBuf,
    pub server_input: String,
    pub token_input: String,
    pub focused: ConnectField,
    pub error: Option<String>,
    pub saved: Option<ConnectSelection>,
}

pub(crate) fn default_base_url() -> String {
    format!("http://localhost:{DEFAULT_OPENWEBUI_PORT}/api/v1")
}

/// Normalize what the user typed into a usable Open WebUI base URL.
///
/// - Empty input keeps the built-in default.
/// - A full `http(s)://` URL is kept as typed, with `/api/v1` appended when no
///   path is given.
/// - A bare hostname or IP (optionally `host:port`) gets the default Open WebUI
///   port and path, e.g. `owui.lan` -> `http://owui.lan:8080/api/v1`.
pub(crate) fn normalize_base_url(input: &str) -> Result<String, String> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(default_base_url());
    }
    let had_scheme = input.contains("://");
    let (scheme, rest) = match input.split_once("://") {
        Some((scheme @ ("http" | "https"), rest)) => (scheme, rest),
        Some(_) => return Err("the URL must start with http:// or https://".to_string()),
        None => ("http", input),
    };
    let rest = rest.trim_end_matches('/');
    if rest.is_empty() {
        return Err("the server address is empty".to_string());
    }
    let (host, path) = match rest.split_once('/') {
        Some((host, path)) => (host, format!("/{path}")),
        None => (rest, "/api/v1".to_string()),
    };
    if host.starts_with('[') {
        // Bracketed IPv6 literal: trust the shape, validate the port if present.
        if let Some((_, port)) = host.split_once("]:")
            && (port.is_empty() || !port.chars().all(|c| c.is_ascii_digit()))
        {
            return Err(format!("\"{host}\" is not a valid host:port"));
        }
        return Ok(format!("{scheme}://{host}{path}"));
    }
    let (host_name, port) = match host.split_once(':') {
        Some((name, port)) => {
            if name.is_empty() || port.is_empty() || !port.chars().all(|c| c.is_ascii_digit()) {
                return Err(format!("\"{host}\" is not a valid host:port"));
            }
            (name, Some(port))
        }
        None => (host, None),
    };
    if !is_valid_host_name(host_name) {
        return Err(format!("\"{host}\" is not a valid hostname or IP"));
    }
    // Only a bare host gets the default port appended; anything with an
    // explicit scheme or port is used as typed.
    let authority = if port.is_some() || had_scheme {
        host.to_string()
    } else {
        format!("{host_name}:{DEFAULT_OPENWEBUI_PORT}")
    };
    Ok(format!("{scheme}://{authority}{path}"))
}

fn is_valid_host_name(host: &str) -> bool {
    !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// Show the connect step only on first run with the Open WebUI provider active:
/// once the server comes from `config.toml` or `OPENWEBUI_BASE_URL`, the values
/// are already configured and the prompt is skipped.
pub(crate) fn should_show_connect_screen(
    provider_id: &str,
    user_defined_openwebui: bool,
    env_base_url_set: bool,
) -> bool {
    provider_id == OPENWEBUI_PROVIDER_ID && !user_defined_openwebui && !env_base_url_set
}

impl ConnectWidget {
    fn set_path(key: &str, value_str: &str) -> ConfigEdit {
        ConfigEdit::SetPath {
            segments: vec![
                "model_providers".to_string(),
                OPENWEBUI_PROVIDER_ID.to_string(),
                key.to_string(),
            ],
            value: value(value_str.to_string()),
        }
    }

    fn save(&mut self) {
        let base_url = match normalize_base_url(&self.server_input) {
            Ok(url) => url,
            Err(err) => {
                self.error = Some(err);
                return;
            }
        };
        let token =
            (!self.token_input.trim().is_empty()).then(|| self.token_input.trim().to_string());
        let selection = ConnectSelection { base_url, token };
        // `name` is required on user-defined provider sections, so persist the
        // display name matching the built-in Open WebUI provider.
        let mut edits = vec![
            Self::set_path("name", OPENWEBUI_DISPLAY_NAME),
            Self::set_path("base_url", &selection.base_url),
        ];
        if let Some(token) = &selection.token {
            edits.push(Self::set_path("experimental_bearer_token", token));
        }
        match ConfigEditsBuilder::new(&self.codex_home)
            .with_edits(edits)
            .apply_blocking()
        {
            Ok(()) => {
                self.saved = Some(selection);
            }
            Err(err) => self.error = Some(format!("could not save config.toml: {err}")),
        }
    }
}

impl KeyboardHandler for ConnectWidget {
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        if key_event.kind != KeyEventKind::Press {
            return;
        }
        self.error = None;
        let changed = match key_event.code {
            KeyCode::Tab | KeyCode::Down | KeyCode::Up => {
                self.focused = match self.focused {
                    ConnectField::Server => ConnectField::Token,
                    ConnectField::Token => ConnectField::Server,
                };
                true
            }
            KeyCode::Enter => {
                match self.focused {
                    ConnectField::Server => match normalize_base_url(&self.server_input) {
                        Ok(url) => {
                            self.server_input = url;
                            self.focused = ConnectField::Token;
                        }
                        Err(err) => self.error = Some(err),
                    },
                    ConnectField::Token => self.save(),
                }
                true
            }
            KeyCode::Backspace => {
                match self.focused {
                    ConnectField::Server => self.server_input.pop(),
                    ConnectField::Token => self.token_input.pop(),
                };
                true
            }
            KeyCode::Esc => {
                if self.focused == ConnectField::Token {
                    self.focused = ConnectField::Server;
                    true
                } else {
                    false
                }
            }
            KeyCode::Char(c) if !key_event.modifiers.contains(KeyModifiers::CONTROL) => {
                match self.focused {
                    ConnectField::Server => self.server_input.push(c),
                    ConnectField::Token => self.token_input.push(c),
                }
                true
            }
            _ => false,
        };
        if changed {
            self.request_frame.schedule_frame();
        }
    }

    fn handle_paste(&mut self, pasted: String) {
        match self.focused {
            ConnectField::Server => self.server_input.push_str(pasted.trim()),
            ConnectField::Token => self.token_input.push_str(pasted.trim()),
        }
        self.error = None;
        self.request_frame.schedule_frame();
    }
}

impl StepStateProvider for &ConnectWidget {
    fn get_step_state(&self) -> StepState {
        if self.saved.is_some() {
            StepState::Complete
        } else {
            StepState::InProgress
        }
    }
}

fn caret(active: bool) -> Span<'static> {
    if active { "›".bold() } else { " ".into() }
}

impl WidgetRef for &ConnectWidget {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);

        // The owl keeps you company while the form is open, and steps aside once the
        // settings are saved so the rest of onboarding stays on screen.
        let mut header: Vec<Line> = Vec::new();
        if self.saved.is_none() {
            header.extend(self.owl.lines_at(Instant::now()));
            header.push("".into());
        }

        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(vec![
            "  ".into(),
            "Connect Owl to ".into(),
            "Open WebUI".bold(),
        ]));
        lines.push("".into());

        lines.push(Line::from(
            "  Server URL or hostname/IP (Enter accepts the default):".dim(),
        ));
        let server_value = if self.server_input.is_empty() {
            Span::from(default_base_url()).dim()
        } else {
            Span::from(self.server_input.clone())
        };
        lines.push(Line::from(vec![
            caret(self.focused == ConnectField::Server),
            server_value,
        ]));

        lines.push("".into());
        lines.push(Line::from("  API token (optional — Enter to skip):".dim()));
        let token_value = if self.token_input.is_empty() {
            Span::from("—").dim()
        } else {
            Span::from("*".repeat(self.token_input.len()))
        };
        lines.push(Line::from(vec![
            caret(self.focused == ConnectField::Token),
            token_value,
        ]));

        lines.push("".into());
        if let Some(error) = &self.error {
            lines.push(Line::from(vec!["  error: ".red(), error.clone().red()]));
        } else if self.saved.is_some() {
            lines.push(Line::from(vec![
                "  ".into(),
                "Saved to config.toml".green(),
                " · onboarding continues below".dim(),
            ]));
        } else {
            lines.push(Line::from(vec![
                "  Tab switches fields · Enter moves on; Enter on the token saves".dim(),
            ]));
        }

        let show_owl = (header.len() + lines.len()) as u16 <= area.height;
        if show_owl {
            self.owl.schedule_next_frame_at(Instant::now());
        }
        let lines: Vec<Line> = if show_owl {
            header.into_iter().chain(lines).collect()
        } else {
            lines
        };

        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;
    use pretty_assertions::assert_eq;
    use ratatui::Terminal;
    use tempfile::TempDir;

    use crate::little_owl::OwlState;
    use crate::test_backend::VT100Backend;

    fn test_owl() -> LittleOwl {
        LittleOwl::new(OwlState::Idle, FrameRequester::test_dummy(), false)
    }

    #[test]
    fn normalize_base_url_accepts_urls_and_bare_hosts() {
        let default = default_base_url();
        let cases = [
            ("", default.as_str()),
            ("http://localhost:8080", default.as_str()),
            (
                "  https://owui.example.com  ",
                "https://owui.example.com/api/v1",
            ),
            ("owui.lan", "http://owui.lan:8080/api/v1"),
            ("10.0.2.99/", "http://10.0.2.99:8080/api/v1"),
            ("owui.lan:3000", "http://owui.lan:3000/api/v1"),
            (
                "https://owui.example.com/custom/path",
                "https://owui.example.com/custom/path",
            ),
            ("http://[::1]:8080", "http://[::1]:8080/api/v1"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                normalize_base_url(input).expect("valid").as_str(),
                expected,
                "input: {input}"
            );
        }
    }

    #[test]
    fn normalize_base_url_rejects_bad_input() {
        for input in [
            "ftp://owui.example.com",
            "open webui",
            "http://",
            "host:badport",
        ] {
            assert!(normalize_base_url(input).is_err(), "input: {input}");
        }
    }

    #[test]
    fn connect_screen_shows_only_when_unconfigured() {
        // provider openwebui + nothing configured -> show
        assert!(should_show_connect_screen("openwebui", false, false));
        // already configured via config.toml or env -> skip
        assert!(!should_show_connect_screen("openwebui", true, false));
        assert!(!should_show_connect_screen("openwebui", false, true));
        // other providers never show the connect step
        assert!(!should_show_connect_screen("openai", false, false));
    }

    #[test]
    fn typing_and_enter_saves_config_toml() {
        let codex_home = TempDir::new().expect("temp home");
        let mut widget = ConnectWidget {
            request_frame: FrameRequester::test_dummy(),
            owl: test_owl(),
            codex_home: codex_home.path().to_path_buf(),
            server_input: String::new(),
            token_input: String::new(),
            focused: ConnectField::Server,
            error: None,
            saved: None,
        };

        for c in "owui.lan".chars() {
            widget.handle_key_event(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(widget.focused, ConnectField::Token);
        assert_eq!(widget.saved, None);

        for c in "secret-token".chars() {
            widget.handle_key_event(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        let selection = widget.saved.clone().expect("saved");
        assert_eq!(
            selection,
            ConnectSelection {
                base_url: "http://owui.lan:8080/api/v1".to_string(),
                token: Some("secret-token".to_string()),
            }
        );
        let contents =
            std::fs::read_to_string(codex_home.path().join("config.toml")).expect("read config");
        let expected = format!(
            "[model_providers.openwebui]\nname = \"{OPENWEBUI_DISPLAY_NAME}\"\nbase_url = \"http://owui.lan:8080/api/v1\"\nexperimental_bearer_token = \"secret-token\"\n"
        );
        assert_eq!(contents, expected);
    }

    #[test]
    fn enter_with_empty_token_skips_token() {
        let codex_home = TempDir::new().expect("temp home");
        let mut widget = ConnectWidget {
            request_frame: FrameRequester::test_dummy(),
            owl: test_owl(),
            codex_home: codex_home.path().to_path_buf(),
            server_input: String::new(),
            token_input: String::new(),
            focused: ConnectField::Server,
            error: None,
            saved: None,
        };
        // Enter on the empty server field accepts the default, then Enter on the
        // empty token field saves without a token key.
        widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            widget.saved.clone().expect("saved"),
            ConnectSelection {
                base_url: default_base_url(),
                token: None,
            }
        );
        let contents =
            std::fs::read_to_string(codex_home.path().join("config.toml")).expect("read config");
        let expected = format!(
            "[model_providers.openwebui]\nname = \"{OPENWEBUI_DISPLAY_NAME}\"\nbase_url = \"{}\"\n",
            default_base_url()
        );
        assert_eq!(contents, expected);
    }

    #[test]
    fn bad_input_shows_error_and_backspace_edits() {
        let codex_home = TempDir::new().expect("temp home");
        let mut widget = ConnectWidget {
            request_frame: FrameRequester::test_dummy(),
            owl: test_owl(),
            codex_home: codex_home.path().to_path_buf(),
            server_input: String::from("bad url"),
            token_input: String::new(),
            focused: ConnectField::Server,
            error: None,
            saved: None,
        };
        widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(widget.error.is_some());
        assert_eq!(widget.focused, ConnectField::Server);

        for _ in 0..7 {
            widget.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        }
        assert_eq!(widget.server_input, "");
        assert_eq!(widget.error, None);
    }

    #[test]
    fn renders_snapshot() {
        let codex_home = TempDir::new().expect("temp home");
        let widget = ConnectWidget {
            request_frame: FrameRequester::test_dummy(),
            owl: test_owl(),
            codex_home: codex_home.path().to_path_buf(),
            server_input: "http://owui.lan:8080/api/v1".to_string(),
            token_input: "tok".to_string(),
            focused: ConnectField::Token,
            error: None,
            saved: None,
        };

        let mut terminal = Terminal::new(VT100Backend::new(70, 12)).expect("terminal");
        terminal
            .draw(|f| (&widget).render_ref(f.area(), f.buffer_mut()))
            .expect("draw");

        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn renders_snapshot_with_the_owl_when_there_is_room() {
        let codex_home = TempDir::new().expect("temp home");
        let widget = ConnectWidget {
            request_frame: FrameRequester::test_dummy(),
            owl: LittleOwl::new(OwlState::Idle, FrameRequester::test_dummy(), true),
            codex_home: codex_home.path().to_path_buf(),
            server_input: "http://owui.lan:8080/api/v1".to_string(),
            token_input: "tok".to_string(),
            focused: ConnectField::Token,
            error: None,
            saved: None,
        };

        let mut terminal = Terminal::new(VT100Backend::new(70, 22)).expect("terminal");
        terminal
            .draw(|f| (&widget).render_ref(f.area(), f.buffer_mut()))
            .expect("draw");

        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn saved_connect_step_lets_the_owl_step_aside() {
        let codex_home = TempDir::new().expect("temp home");
        let widget = ConnectWidget {
            request_frame: FrameRequester::test_dummy(),
            owl: LittleOwl::new(OwlState::Idle, FrameRequester::test_dummy(), true),
            codex_home: codex_home.path().to_path_buf(),
            server_input: "http://owui.lan:8080/api/v1".to_string(),
            token_input: "tok".to_string(),
            focused: ConnectField::Token,
            error: None,
            saved: Some(ConnectSelection {
                base_url: "http://owui.lan:8080/api/v1".to_string(),
                token: Some("tok".to_string()),
            }),
        };

        let mut terminal = Terminal::new(VT100Backend::new(70, 22)).expect("terminal");
        terminal
            .draw(|f| (&widget).render_ref(f.area(), f.buffer_mut()))
            .expect("draw");
        let screen = terminal.backend().vt100().screen().contents();

        assert!(
            !screen.contains(",_,"),
            "owl gone once the settings are saved"
        );
        assert!(screen.contains("Saved to config.toml"));
    }
}
