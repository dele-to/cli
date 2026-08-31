use std::io::{self, stdout};
use std::time::Duration;

use anyhow::Result;
use arboard::Clipboard;
use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use deleto::share::ViewedShare;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Padding, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use tui_textarea::{Input, Key, TextArea};

use crate::{analytics, share_from_tui, ShareArgs};
use serde_json::json;

// Dark landing palette: near-black surfaces, red-600 brand, zinc type.
const BG: Color = Color::Rgb(10, 10, 10); // hsl(0 0% 3.9%)
const PANEL: Color = Color::Rgb(18, 18, 18);
const BORDER: Color = Color::Rgb(38, 38, 38); // hsl(0 0% 14.9%)
const ACCENT: Color = Color::Rgb(220, 38, 38); // red-600 / #DC2626
const MUTED: Color = Color::Rgb(163, 163, 163); // hsl(0 0% 63.9%)
const TEXT: Color = Color::Rgb(250, 250, 250);
const DANGER: Color = Color::Rgb(239, 68, 68); // red-500
const TITLE: Color = Color::Rgb(250, 250, 250);

enum Screen {
    Compose,
    Created,
    Viewed,
}

struct App {
    screen: Screen,
    textarea: TextArea<'static>,
    expires: String,
    views: String,
    file_path: String,
    focus: Focus,
    status: String,
    error: Option<String>,
    created_url: String,
    delete_capability: String,
    reveal_delete: bool,
    viewed: Option<ViewedShare>,
    args: ShareArgs,
    copied: Option<&'static str>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Secret,
    Expires,
    Views,
    File,
}

pub fn run(args: ShareArgs) -> Result<()> {
    let mut app = App::new(args);
    if let Some(path) = app.args.file.clone() {
        app.file_path = path.display().to_string();
        if let Ok(contents) = std::fs::read_to_string(&path) {
            app.textarea = textarea_from(contents);
        }
    }
    with_terminal(|terminal| loop {
        terminal.draw(|frame| app.draw(frame))?;
        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        let Event::Key(key) = event::read()? else { continue };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if app.handle_key(key.code, key.modifiers)? {
            break Ok(());
        }
    })
}

pub fn show_viewed(viewed: &ViewedShare) -> Result<()> {
    let mut app = App::new(ShareArgs {
        file: None,
        expires: "1h".into(),
        views: 1,
        json: false,
        receipt: false,
        tui: true,
        api: crate::ApiArgs {
            api_url: DEFAULT_PLACEHOLDER.into(),
            api_key: None,
        },
    });
    app.screen = Screen::Viewed;
    app.viewed = Some(viewed.clone());
    with_terminal(|terminal| loop {
        terminal.draw(|frame| app.draw(frame))?;
        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        let Event::Key(key) = event::read()? else { continue };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
            break Ok(());
        }
        if matches!(key.code, KeyCode::Char('c')) {
            if let Some(viewed) = &app.viewed {
                copy(&viewed.plaintext);
                app.copied = Some("copied plaintext");
            }
        }
    })
}

const DEFAULT_PLACEHOLDER: &str = "https://dele.to";

impl App {
    fn new(args: ShareArgs) -> Self {
        let expires = args.expires.clone();
        let views = args.views.to_string();
        let mut textarea = textarea_from(String::new());
        textarea.set_placeholder_text("Paste or type a secret. Encrypted on this machine before it is sent.");
        Self {
            screen: Screen::Compose,
            textarea,
            expires,
            views,
            file_path: String::new(),
            focus: Focus::Secret,
            status: compose_status().into(),
            error: None,
            created_url: String::new(),
            delete_capability: String::new(),
            reveal_delete: false,
            viewed: None,
            args,
            copied: None,
        }
    }

    fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<bool> {
        self.copied = None;
        match self.screen {
            Screen::Compose => self.handle_compose(code, modifiers),
            Screen::Created => {
                match code {
                    KeyCode::Char('q') | KeyCode::Esc => Ok(true),
                    KeyCode::Char('c') => {
                        copy(&self.created_url);
                        self.copied = Some("copied share URL");
                        Ok(false)
                    }
                    KeyCode::Char('d') => {
                        self.reveal_delete = !self.reveal_delete;
                        Ok(false)
                    }
                    KeyCode::Char('n') => {
                        *self = App::new(self.args.clone());
                        Ok(false)
                    }
                    _ => Ok(false),
                }
            }
            Screen::Viewed => Ok(matches!(code, KeyCode::Char('q') | KeyCode::Esc)),
        }
    }

    fn handle_compose(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<bool> {
        if modifiers.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('c') | KeyCode::Char('q')) {
            return Ok(true);
        }
        if code == KeyCode::Esc {
            return Ok(true);
        }
        if is_share_shortcut(code, modifiers) {
            self.submit()?;
            return Ok(false);
        }
        if modifiers.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('o') {
            self.load_file();
            return Ok(false);
        }
        if code == KeyCode::Tab {
            self.focus = match self.focus {
                Focus::Secret => Focus::Expires,
                Focus::Expires => Focus::Views,
                Focus::Views => Focus::File,
                Focus::File => Focus::Secret,
            };
            return Ok(false);
        }
        if code == KeyCode::BackTab {
            self.focus = match self.focus {
                Focus::Secret => Focus::File,
                Focus::Expires => Focus::Secret,
                Focus::Views => Focus::Expires,
                Focus::File => Focus::Views,
            };
            return Ok(false);
        }
        if self.focus == Focus::Secret {
            self.textarea.input(to_input(code, modifiers));
            return Ok(false);
        }
        let field = match self.focus {
            Focus::Expires => &mut self.expires,
            Focus::Views => &mut self.views,
            Focus::File => &mut self.file_path,
            Focus::Secret => unreachable!(),
        };
        match code {
            KeyCode::Char(c) if !modifiers.contains(KeyModifiers::CONTROL) => field.push(c),
            KeyCode::Backspace => {
                field.pop();
            }
            KeyCode::Enter if self.focus == Focus::File => self.load_file(),
            _ => {}
        }
        Ok(false)
    }

    fn load_file(&mut self) {
        let path = self.file_path.trim();
        if path.is_empty() {
            self.error = Some("enter a file path first".into());
            return;
        }
        match std::fs::read_to_string(path) {
            Ok(contents) => {
                self.textarea = textarea_from(contents);
                self.focus = Focus::Secret;
                self.error = None;
                self.status = format!("loaded {path}");
            }
            Err(error) => self.error = Some(format!("could not read {path}: {error}")),
        }
    }

    fn submit(&mut self) -> Result<()> {
        let plaintext = self.textarea.lines().join("\n");
        if plaintext.trim().is_empty() {
            self.error = Some("write a secret first".into());
            return Ok(());
        }
        let mut args = self.args.clone();
        args.expires = self.expires.clone();
        args.views = self.views.parse().unwrap_or(1);
        match share_from_tui(&plaintext, &args) {
            Ok(created) => {
                analytics::track(
                    "share_created_successfully",
                    analytics::props(&[
                        ("source", json!("tui")),
                        ("input", json!("tui")),
                        ("max_views", json!(args.views)),
                        ("expires", json!(args.expires.clone())),
                        ("content_length", json!(plaintext.len())),
                    ]),
                );
                self.created_url = created.share_url;
                self.delete_capability = created.delete_capability;
                self.screen = Screen::Created;
                self.error = None;
                self.status = format!("expires {}", created.expires_at);
            }
            Err(error) => {
                let error = display_error(&error, args.api.api_key.is_some());
                analytics::track(
                    "cli_command_failed",
                    analytics::props(&[
                        ("source", json!("tui")),
                        ("command", json!("create")),
                        ("reason", json!(analytics::error_reason(&error))),
                    ]),
                );
                self.error = Some(error);
            }
        }
        Ok(())
    }

    fn draw(&self, frame: &mut Frame) {
        let area = frame.area();
        frame.render_widget(Block::default().style(Style::default().bg(BG)), area);
        match self.screen {
            Screen::Compose => self.draw_compose(frame, area),
            Screen::Created => self.draw_created(frame, area),
            Screen::Viewed => self.draw_viewed(frame, area),
        }
    }

    fn draw_compose(&self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(1)
            .constraints([
                Constraint::Length(4),
                Constraint::Min(8),
                Constraint::Length(3),
                Constraint::Length(2),
            ])
            .split(area);

        frame.render_widget(
            header("Share secrets that disappear. The server never sees plaintext."),
            chunks[0],
        );

        let secret_focused = self.focus == Focus::Secret;
        let mut textarea = self.textarea.clone();
        textarea.set_block(card(" secret ", secret_focused));
        frame.render_widget(&textarea, chunks[1]);

        let fields = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(28), Constraint::Percentage(22), Constraint::Percentage(50)])
            .split(chunks[2]);
        field(frame, fields[0], " expires ", &self.expires, self.focus == Focus::Expires);
        field(frame, fields[1], " views ", &self.views, self.focus == Focus::Views);
        field(frame, fields[2], " file path ", &self.file_path, self.focus == Focus::File);
        frame.render_widget(status_bar(&self.status, self.error.as_deref(), self.copied), chunks[3]);
    }

    fn draw_created(&self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(1)
            .constraints([
                Constraint::Length(4),
                Constraint::Length(6),
                Constraint::Length(5),
                Constraint::Min(1),
                Constraint::Length(2),
            ])
            .split(area);
        frame.render_widget(
            header("Share created. Give the URL to a recipient. Keep the delete capability."),
            chunks[0],
        );
        frame.render_widget(
            Paragraph::new(self.created_url.clone())
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(ACCENT))
                .block(card(" share url ", true).border_style(Style::default().fg(ACCENT))),
            chunks[1],
        );
        let delete = if self.reveal_delete {
            self.delete_capability.clone()
        } else {
            "••••••••  press d to reveal".into()
        };
        frame.render_widget(
            Paragraph::new(delete)
                .wrap(Wrap { trim: true })
                .style(Style::default().fg(if self.reveal_delete { DANGER } else { MUTED }))
                .block(card(" delete capability ", false)),
            chunks[2],
        );
        frame.render_widget(Paragraph::new(created_actions_line()), chunks[3]);
        frame.render_widget(status_bar(&self.status, self.error.as_deref(), self.copied), chunks[4]);
    }

    fn draw_viewed(&self, frame: &mut Frame, area: Rect) {
        let Some(viewed) = &self.viewed else { return };
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(1)
            .constraints([Constraint::Length(4), Constraint::Min(6), Constraint::Length(2)])
            .split(area);
        frame.render_widget(
            header(&format!(
                "Decrypted locally.  {} views remaining   expires {}",
                viewed.remaining_views, viewed.expires_at
            )),
            chunks[0],
        );
        frame.render_widget(
            Paragraph::new(viewed.plaintext.clone())
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(TEXT))
                .block(card(" plaintext ", true).padding(Padding::uniform(1))),
            chunks[1],
        );
        frame.render_widget(status_bar("c copy   q quit", None, self.copied), chunks[2]);
    }
}

fn header(subtitle: &str) -> Paragraph<'static> {
    Paragraph::new(vec![
        Line::from(vec![
            Span::styled("DELE", Style::default().fg(TITLE).add_modifier(Modifier::BOLD)),
            Span::styled(".TO", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(Span::styled(subtitle.to_string(), Style::default().fg(MUTED))),
    ])
}

fn label(focused: bool) -> Style {
    if focused {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(MUTED)
    }
}

fn focus_border(focused: bool) -> Style {
    Style::default().fg(if focused { ACCENT } else { BORDER })
}

fn card(title: &str, focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(focus_border(focused))
        .title(title.to_string())
        .title_style(label(focused))
        .padding(Padding::horizontal(1))
        .style(Style::default().bg(PANEL).fg(TEXT))
}

fn field(frame: &mut Frame, area: Rect, title: &str, value: &str, focused: bool) {
    frame.render_widget(
        Paragraph::new(value)
            .style(Style::default().fg(TEXT))
            .block(card(title, focused)),
        area,
    );
}

fn display_error(error: &anyhow::Error, has_api_key: bool) -> String {
    let message = format!("{error:#}");
    let lower = message.to_ascii_lowercase();
    if !lower.contains("quota exceeded") && !lower.contains("rate limit") && !lower.contains("429") {
        return message;
    }
    if has_api_key {
        format!("{message}. Review API key usage and plan limits at https://dele.to/developers")
    } else {
        format!("{message}. Get an API key at https://dele.to/developers, then set DELETO_API_KEY=dlt_v1_...")
    }
}

fn shortcut_key(value: &'static str) -> Span<'static> {
    Span::styled(
        format!(" {value} "),
        Style::default()
            .fg(Color::Rgb(248, 113, 113))
            .bg(Color::Rgb(55, 15, 15))
            .add_modifier(Modifier::BOLD),
    )
}

fn compose_status_line() -> Line<'static> {
    #[cfg(target_os = "macos")]
    let create = "Option+Enter / Ctrl+S";
    #[cfg(not(target_os = "macos"))]
    let create = "Ctrl+Enter / Ctrl+S";
    let label = Style::default().fg(TEXT);
    let separator = Style::default().fg(BORDER);
    Line::from(vec![
        Span::styled("Create: ", label),
        shortcut_key(create),
        Span::styled("  ·  ", separator),
        Span::styled("Navigate: ", label),
        shortcut_key("Tab"),
        Span::styled("  ·  ", separator),
        Span::styled("Load file: ", label),
        shortcut_key("Ctrl+O"),
        Span::styled("  ·  ", separator),
        Span::styled("Quit: ", label),
        shortcut_key("Ctrl+Q"),
    ])
}

fn created_actions_line() -> Line<'static> {
    let label = Style::default().fg(TEXT);
    let separator = Style::default().fg(BORDER);
    Line::from(vec![
        Span::styled("Copy URL: ", label),
        shortcut_key("c"),
        Span::styled("  ·  ", separator),
        Span::styled("Reveal delete capability: ", label),
        shortcut_key("d"),
        Span::styled("  ·  ", separator),
        Span::styled("New share: ", label),
        shortcut_key("n"),
        Span::styled("  ·  ", separator),
        Span::styled("Quit: ", label),
        shortcut_key("q"),
    ])
}

fn status_bar(status: &str, error: Option<&str>, copied: Option<&str>) -> Paragraph<'static> {
    let line = if let Some(error) = error {
        Line::styled(error.to_string(), Style::default().fg(DANGER))
    } else if let Some(copied) = copied {
        Line::styled(copied.to_string(), Style::default().fg(ACCENT))
    } else if status == compose_status() {
        compose_status_line()
    } else {
        Line::styled(status.to_string(), Style::default().fg(MUTED))
    };
    Paragraph::new(line)
        .alignment(Alignment::Left)
        .wrap(Wrap { trim: false })
}

fn textarea_from(contents: String) -> TextArea<'static> {
    let mut textarea = if contents.is_empty() {
        TextArea::default()
    } else {
        TextArea::new(contents.lines().map(str::to_string).collect())
    };
    textarea.set_cursor_line_style(Style::default());
    textarea.set_cursor_style(Style::default().bg(ACCENT).fg(TEXT));
    textarea.set_placeholder_style(Style::default().fg(MUTED).bg(PANEL));
    textarea.set_style(Style::default().fg(TEXT).bg(PANEL));
    textarea
}

fn compose_status() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "Create: Option+Enter / Ctrl+S   Navigate: Tab   Load file: Ctrl+O   Quit: Ctrl+Q"
    }
    #[cfg(not(target_os = "macos"))]
    {
        "Create: Ctrl+Enter / Ctrl+S   Navigate: Tab   Load file: Ctrl+O   Quit: Ctrl+Q"
    }
}

/// Share submit: Ctrl/Cmd/Alt+Enter, Ctrl+S, and the sequences Mac terminals
/// actually emit for Ctrl+Enter when the kitty keyboard protocol is off.
fn is_share_shortcut(code: KeyCode, modifiers: KeyModifiers) -> bool {
    let enter_mod =
        modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER | KeyModifiers::ALT);
    match code {
        KeyCode::Enter | KeyCode::Char('\n') | KeyCode::Char('\r') if enter_mod => true,
        KeyCode::Char('s' | 'S')
            if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) =>
        {
            true
        }
        // Ctrl+J is the traditional encoding of Ctrl+Enter (LF).
        KeyCode::Char('j' | 'J') if modifiers.contains(KeyModifiers::CONTROL) => true,
        _ => false,
    }
}

fn to_input(code: KeyCode, modifiers: KeyModifiers) -> Input {
    let ctrl = modifiers.contains(KeyModifiers::CONTROL);
    let alt = modifiers.contains(KeyModifiers::ALT);
    let shift = modifiers.contains(KeyModifiers::SHIFT);
    let key = match code {
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Enter => Key::Enter,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Delete => Key::Delete,
        KeyCode::Tab => Key::Tab,
        KeyCode::Esc => Key::Esc,
        _ => Key::Null,
    };
    Input { key, ctrl, alt, shift }
}

fn copy(value: &str) {
    if let Ok(mut clipboard) = Clipboard::new() {
        let _ = clipboard.set_text(value.to_string());
    }
}

fn with_terminal<F>(mut body: F) -> Result<()>
where
    F: FnMut(&mut Terminal<ratatui::backend::CrosstermBackend<io::Stdout>>) -> Result<()>,
{
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    // Distinguishes Ctrl+Enter from Enter on terminals that speak the kitty
    // keyboard protocol (Ghostty, Kitty, WezTerm, iTerm2 3.5+). Others ignore it.
    let _ = stdout().execute(PushKeyboardEnhancementFlags(
        KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES,
    ));
    let mut terminal = Terminal::new(ratatui::backend::CrosstermBackend::new(stdout()))?;
    terminal.clear()?;
    let result = body(&mut terminal);
    let _ = stdout().execute(PopKeyboardEnhancementFlags);
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creation_error_includes_the_api_reason() {
        let error = anyhow::Error::msg("Creation quota exceeded").context("failed to create share");
        assert_eq!(
            display_error(&error, false),
            "failed to create share: Creation quota exceeded. Get an API key at https://dele.to/developers, then set DELETO_API_KEY=dlt_v1_..."
        );
        assert_eq!(
            display_error(&error, true),
            "failed to create share: Creation quota exceeded. Review API key usage and plan limits at https://dele.to/developers"
        );
    }

    #[test]
    fn share_shortcut_accepts_ctrl_enter_and_mac_fallbacks() {
        assert!(is_share_shortcut(KeyCode::Enter, KeyModifiers::CONTROL));
        assert!(is_share_shortcut(KeyCode::Enter, KeyModifiers::SUPER));
        assert!(is_share_shortcut(KeyCode::Enter, KeyModifiers::ALT));
        assert!(is_share_shortcut(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert!(is_share_shortcut(KeyCode::Char('s'), KeyModifiers::SUPER));
        assert!(is_share_shortcut(KeyCode::Char('j'), KeyModifiers::CONTROL));
        assert!(is_share_shortcut(KeyCode::Char('\n'), KeyModifiers::CONTROL));
        assert!(!is_share_shortcut(KeyCode::Enter, KeyModifiers::NONE));
        assert!(!is_share_shortcut(KeyCode::Char('s'), KeyModifiers::NONE));
        assert!(!is_share_shortcut(KeyCode::Char('j'), KeyModifiers::NONE));
    }
}