//! TUI login / register screen. Replaces the old stdin prompts: when no
//! valid token is stored, this form collects server + credentials in the
//! terminal (Discord QR/MFA/captcha have no equivalent here on purpose).

use std::{io::Stdout, time::Duration};

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};
use tuimessager_protocol::User;

use crate::{
    client::Client,
    config::{self, AppOptions},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Server,
    Username,
    Password,
}

impl Field {
    fn next(self) -> Self {
        match self {
            Self::Server => Self::Username,
            Self::Username => Self::Password,
            Self::Password => Self::Server,
        }
    }
    fn prev(self) -> Self {
        match self {
            Self::Server => Self::Password,
            Self::Username => Self::Server,
            Self::Password => Self::Username,
        }
    }
}

struct LoginApp {
    server: String,
    username: String,
    password: String,
    focus: Field,
    register_mode: bool,
    error: String,
    busy: bool,
}

pub async fn login_flow_tui(options: &AppOptions) -> anyhow::Result<(Client, User)> {
    // Fast path: env token or saved token file.
    if let Some(token) = config::load_token(options) {
        let client = Client::new(options.server_url.clone(), token);
        if let Ok(me) = client.me().await {
            return Ok((client, me));
        }
    }

    enable_raw_mode()?;
    let mut out = std::io::stdout();
    execute!(out, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(out))?;

    let mut app = LoginApp {
        server: std::env::var("TUIMESSAGER_URL").unwrap_or_else(|_| options.server_url.clone()),
        username: String::new(),
        password: String::new(),
        focus: Field::Username,
        register_mode: false,
        error: String::new(),
        busy: false,
    };

    let result = run_login(&mut terminal, &mut app).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    result
}

async fn run_login(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut LoginApp,
) -> anyhow::Result<(Client, User)> {
    loop {
        terminal.draw(|f| draw_login(f, app))?;
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let Event::Key(key) = event::read()? else { continue };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => anyhow::bail!("login cancelled"),
            (KeyCode::Tab, m) if m.contains(KeyModifiers::SHIFT) => app.focus = app.focus.prev(),
            (KeyCode::Tab, _) => app.focus = app.focus.next(),
            (KeyCode::F(2), _) => {
                app.register_mode = !app.register_mode;
                app.error.clear();
            }
            (KeyCode::Backspace, _) => {
                app.error.clear();
                match app.focus {
                    Field::Server => app.server.pop(),
                    Field::Username => app.username.pop(),
                    Field::Password => app.password.pop(),
                };
            }
            (KeyCode::Enter, _) => {
                if let Some(done) = submit(app).await {
                    return Ok(done);
                }
            }
            (KeyCode::Char(c), _) => {
                app.error.clear();
                match app.focus {
                    Field::Server => app.server.push(c),
                    Field::Username => app.username.push(c),
                    Field::Password => app.password.push(c),
                };
            }
            _ => {}
        }
    }
}

async fn submit(app: &mut LoginApp) -> Option<(Client, User)> {
    let server = app.server.trim().trim_end_matches('/').to_string();
    if server.is_empty() || app.username.trim().is_empty() || app.password.is_empty() {
        app.error = "server, username and password are all required".to_string();
        return None;
    }
    app.busy = true;
    let outcome = if app.register_mode {
        Client::register(&server, app.username.trim(), &app.password).await
    } else {
        Client::login(&server, app.username.trim(), &app.password).await
    };
    app.busy = false;
    match outcome {
        Ok(auth) => {
            if config::save_token(&auth.token).is_err() {
                app.error = "logged in, but could not save the token file".to_string();
            }
            Some((Client::new(server, auth.token), auth.user))
        }
        Err(e) => {
            let msg = format!("{e:#}");
            app.error = msg.chars().take(90).collect();
            app.password.clear();
            None
        }
    }
}

fn draw_login(f: &mut ratatui::Frame, app: &LoginApp) {
    let area = centered(f.area(), 60, 55);
    f.render_widget(Clear, area);
    let title = if app.register_mode { " Register " } else { " Log in " };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::new().fg(Color::Cyan).bold())
        .title(title);

    let inner = block.inner(area);
    f.render_widget(block, area);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);

    let subtitle = if app.register_mode {
        "create a new account on your server"
    } else {
        "self-hosted server (no Discord)"
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("tuimessager — {subtitle}"),
            Style::new().fg(Color::Cyan).bold(),
        ))),
        rows[0],
    );
    field(f, app, rows[1], Field::Server, "Server", &app.server, false);
    field(f, app, rows[2], Field::Username, "Username", &app.username, false);
    field(f, app, rows[3], Field::Password, "Password", &app.password, true);

    let mode_line = if app.register_mode {
        Line::from(vec![
            Span::raw("Mode: "),
            Span::styled("[Register]", Style::new().bold().fg(Color::Yellow)),
            Span::styled("  (F2 to switch)", Style::new().fg(Color::DarkGray)),
        ])
    } else {
        Line::from(vec![
            Span::raw("Mode: "),
            Span::styled("[Log in]", Style::new().bold().fg(Color::Green)),
            Span::styled("  (F2 to switch)", Style::new().fg(Color::DarkGray)),
        ])
    };
    f.render_widget(Paragraph::new(mode_line), rows[4]);

    if app.busy {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled("working…", Style::new().fg(Color::Yellow)))),
            rows[5],
        );
    } else if !app.error.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(app.error.clone(), Style::new().fg(Color::Red)))),
            rows[5],
        );
    } else {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "Tab move · Enter submit · F2 login/register · Esc quit",
                Style::new().fg(Color::DarkGray),
            ))),
            rows[5],
        );
    }
}

fn field(f: &mut ratatui::Frame, app: &LoginApp, area: Rect, which: Field, label: &str, value: &str, secret: bool) {
    let focused = app.focus == which;
    let shown = if secret {
        "•".repeat(value.chars().count())
    } else {
        value.to_string()
    };
    let cursor = if focused { "▊" } else { "" };
    let style = if focused {
        Style::new().fg(Color::Cyan).bold()
    } else {
        Style::new().fg(Color::DarkGray)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(style)
        .title(format!(" {label} "));
    let inner = block.inner(area);
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(format!("{shown}{cursor}")), inner);
}

fn centered(area: Rect, pct_x: u16, pct_y: u16) -> Rect {
    let h = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - pct_x) / 2),
            Constraint::Percentage(pct_x),
            Constraint::Percentage((100 - pct_x) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - pct_y) / 2),
            Constraint::Percentage(pct_y),
            Constraint::Percentage((100 - pct_y) / 2),
        ])
        .split(h[1])[1]
}
