mod config;
mod converter;
mod error;
mod steam_registry;

use std::collections::VecDeque;
use std::io::{self, Stdout};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, TryRecvError},
    Arc,
};
use std::time::Duration;

use chrono::Local;
use converter::Converter;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

const MAX_LOG_LINES: usize = 250;
const VOICE_PROVIDER_OWNER: &str = "sipdetect";
const VOICE_PROVIDER_REPO: &str = "LimbusDialogueBoxes_ZH";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Ready,
    Running,
    Success,
    Failed,
}

#[derive(Debug, Clone, Copy)]
enum LogLevel {
    Info,
    Success,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
struct LogEntry {
    timestamp: String,
    level: LogLevel,
    message: String,
}

#[derive(Debug, Clone)]
struct ProgressUpdate {
    progress: f64,
    message: String,
    current_file: Option<String>,
    total_files: Option<usize>,
    processed_files: Option<usize>,
}

enum WorkerEvent {
    Progress(ProgressUpdate),
    Finished(Result<(), String>),
}

#[derive(Debug, Clone, Default)]
struct GamePathInfo {
    found: bool,
    game_path: Option<String>,
    output_path: Option<String>,
}

struct App {
    phase: Phase,
    progress: f64,
    message: String,
    error: Option<String>,
    current_file: Option<String>,
    total_files: Option<usize>,
    processed_files: Option<usize>,
    logs: VecDeque<LogEntry>,
    log_scroll: usize,
    worker_rx: Option<Receiver<WorkerEvent>>,
    cancel_flag: Option<Arc<AtomicBool>>,
    config: Option<config::Config>,
    game_info: GamePathInfo,
    should_quit: bool,
    last_logged_message: Option<String>,
}

impl App {
    fn new() -> Self {
        let mut app = Self {
            phase: Phase::Ready,
            progress: 0.0,
            message: "按 S 開始轉換，按 Q 離開".to_string(),
            error: None,
            current_file: None,
            total_files: None,
            processed_files: None,
            logs: VecDeque::with_capacity(MAX_LOG_LINES),
            log_scroll: 0,
            worker_rx: None,
            cancel_flag: None,
            config: None,
            game_info: detect_game_path_info(),
            should_quit: false,
            last_logged_message: None,
        };

        app.reload_config();
        if app.game_info.found {
            app.push_log(LogLevel::Success, "已偵測到 Limbus Company 安裝路徑");
        } else {
            app.push_log(
                LogLevel::Warn,
                "找不到 Limbus Company 路徑，將使用預設輸出資料夾",
            );
        }

        app.push_log(LogLevel::Info, "TUI 已就緒，可開始執行轉換");
        app
    }

    fn push_log(&mut self, level: LogLevel, message: impl Into<String>) {
        let message = message.into();
        let timestamp = Local::now().format("%H:%M:%S").to_string();

        if self.logs.len() >= MAX_LOG_LINES {
            self.logs.pop_front();
        }

        self.logs.push_back(LogEntry {
            timestamp,
            level,
            message,
        });
    }
    fn reload_config(&mut self) {
        match config::load_config() {
            Ok(cfg) => {
                self.config = Some(cfg);
                self.push_log(LogLevel::Info, "設定檔已載入");
            }
            Err(err) => {
                self.config = None;
                self.error = Some(err.to_string());
                self.phase = Phase::Failed;
                self.push_log(LogLevel::Error, format!("設定檔載入失敗: {err}"));
            }
        }
    }

    fn running(&self) -> bool {
        self.phase == Phase::Running
    }

    fn start_conversion(&mut self) {
        if self.running() {
            self.push_log(LogLevel::Warn, "轉換已在執行中");
            return;
        }

        let (tx, rx) = mpsc::channel::<WorkerEvent>();
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let worker_cancel_flag = Arc::clone(&cancel_flag);

        self.worker_rx = Some(rx);
        self.cancel_flag = Some(cancel_flag);
        self.phase = Phase::Running;
        self.progress = 0.0;
        self.error = None;
        self.message = "啟動轉換工作...".to_string();
        self.current_file = None;
        self.total_files = None;
        self.processed_files = None;
        self.last_logged_message = None;
        self.push_log(LogLevel::Info, "開始執行轉換流程");

        std::thread::spawn(move || {
            let progress_tx = tx.clone();
            let callback = Box::new(
                move |progress: f64,
                      message: String,
                      current_file: Option<String>,
                      total_files: Option<usize>,
                      processed_files: Option<usize>| {
                    let _ = progress_tx.send(WorkerEvent::Progress(ProgressUpdate {
                        progress,
                        message,
                        current_file,
                        total_files,
                        processed_files,
                    }));
                },
            );

            let result = Converter::new_with_callback_and_cancel(callback, worker_cancel_flag)
                .and_then(|mut converter| converter.run())
                .map_err(|e| e.to_string());

            let _ = tx.send(WorkerEvent::Finished(result));
        });
    }

    fn request_cancel(&mut self) {
        if !self.running() {
            self.push_log(LogLevel::Warn, "目前沒有進行中的轉換");
            return;
        }

        if let Some(cancel_flag) = &self.cancel_flag {
            cancel_flag.store(true, Ordering::Relaxed);
            self.push_log(LogLevel::Warn, "已送出取消請求，等待安全停止...");
            self.message = "正在取消轉換...".to_string();
        }
    }

    fn handle_worker_event(&mut self, event: WorkerEvent) {
        match event {
            WorkerEvent::Progress(update) => {
                self.progress = update.progress.clamp(0.0, 100.0);
                self.message = update.message.clone();
                self.current_file = update.current_file;
                self.total_files = update.total_files;
                self.processed_files = update.processed_files;

                if self.last_logged_message.as_deref() != Some(update.message.as_str()) {
                    self.last_logged_message = Some(update.message.clone());
                    self.push_log(LogLevel::Info, update.message);
                }
            }
            WorkerEvent::Finished(result) => {
                self.worker_rx = None;
                self.cancel_flag = None;
                self.last_logged_message = None;

                match result {
                    Ok(()) => {
                        self.phase = Phase::Success;
                        self.progress = 100.0;
                        self.message = "轉換完成".to_string();
                        self.error = None;
                        self.push_log(LogLevel::Success, "轉換成功完成");
                        self.reload_config();
                    }
                    Err(err) => {
                        if err.to_ascii_lowercase().contains("cancel") {
                            self.phase = Phase::Ready;
                            self.progress = 0.0;
                            self.message = "轉換已取消".to_string();
                            self.error = None;
                            self.push_log(LogLevel::Warn, "轉換已取消");
                        } else {
                            self.phase = Phase::Failed;
                            self.progress = 0.0;
                            self.message = "轉換失敗".to_string();
                            self.error = Some(err.clone());
                            self.push_log(LogLevel::Error, format!("轉換失敗: {err}"));
                        }
                    }
                }
            }
        }
    }

    fn poll_worker_events(&mut self) {
        let mut events = Vec::new();

        if let Some(rx) = &self.worker_rx {
            loop {
                match rx.try_recv() {
                    Ok(event) => events.push(event),
                    Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
                }
            }
        }

        for event in events {
            self.handle_worker_event(event);
        }
    }

    fn tick(&mut self) {
        self.poll_worker_events();
    }

    fn current_step_index(&self) -> usize {
        if self.phase == Phase::Success {
            return 4;
        }

        let p = self.progress;
        if p < 10.0 {
            0
        } else if p < 40.0 {
            1
        } else if p < 60.0 {
            2
        } else if p < 99.0 {
            3
        } else {
            4
        }
    }
}

fn detect_game_path_info() -> GamePathInfo {
    match steam_registry::find_limbus_company_path() {
        Ok(game_path) => {
            let lang_path = game_path.join("LimbusCompany_Data").join("Lang");
            let output_path = lang_path.join("LLC_zh-Hant");

            GamePathInfo {
                found: true,
                game_path: Some(game_path.display().to_string()),
                output_path: Some(output_path.display().to_string()),
            }
        }
        Err(_) => GamePathInfo::default(),
    }
}

fn main() {
    if let Err(err) = run_tui() {
        eprintln!("[ERROR] {err}");
        std::process::exit(1);
    }
}

fn run_tui() -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let result = run_app(&mut terminal);

    disable_raw_mode()?;
    terminal.backend_mut().execute(LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result.map_err(|e| e.into())
}

fn run_app(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    let mut app = App::new();

    loop {
        app.tick();
        terminal.draw(|frame| draw_ui(frame, &app))?;

        if app.should_quit {
            break;
        }

        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind != KeyEventKind::Press && key.kind != KeyEventKind::Repeat {
                        continue;
                    }

                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => {
                            if app.running() {
                                app.request_cancel();
                            } else {
                                app.should_quit = true;
                            }
                        }
                        KeyCode::Char('s') => app.start_conversion(),
                        KeyCode::Char('x') => app.request_cancel(),
                        KeyCode::Char('r') => {
                            app.reload_config();
                            app.game_info = detect_game_path_info();
                            app.push_log(LogLevel::Info, "已重新載入設定與路徑資訊");
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    Ok(())
}

fn draw_ui(frame: &mut Frame, app: &App) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(20), Constraint::Length(3)])
        .split(frame.area());

    let content_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(46), Constraint::Length(1), Constraint::Min(20)])
        .split(root[0]);

    let left_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(10),
            Constraint::Length(6),
            Constraint::Length(3),
            Constraint::Min(8),
        ])
        .split(content_chunks[0]);

    render_header(frame, left_chunks[0], app);
    render_status_panel(frame, left_chunks[1], app);
    render_steps_panel(frame, left_chunks[2], app);
    render_info_panel(frame, left_chunks[3], app);

    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(8)])
        .split(content_chunks[2]);

    render_log_panel(frame, right_chunks[0], app);
    render_detail_panel(frame, right_chunks[1], app);

    render_footer(frame, root[1], app);
}

fn translation_provider_parts(app: &App) -> (String, String) {
    if let Some(cfg) = &app.config {
        (
            cfg.repo_owner.clone(),
            cfg.repo_name.clone(),
        )
    } else {
        ("設定讀取失敗".to_string(), "-".to_string())
    }
}

fn normalize_release_date(raw: &str) -> Option<String> {
    let value = raw.trim();
    if value.len() >= 10 {
        let date = &value[..10];
        if date.chars().all(|c| c.is_ascii_digit() || c == '-') {
            return Some(date.to_string());
        }
    }
    None
}

fn release_date_from_tag(tag: &str) -> Option<String> {
    let digits: String = tag.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() < 8 {
        return None;
    }
    let d = &digits[..8];
    Some(format!("{}-{}-{}", &d[0..4], &d[4..6], &d[6..8]))
}

fn installed_release_date(cfg: &config::Config) -> Option<String> {
    cfg.last_release_date
        .as_deref()
        .and_then(normalize_release_date)
        .or_else(|| cfg.last_release_tag.as_deref().and_then(release_date_from_tag))
        .or_else(|| cfg.last_commit_hash.as_deref().and_then(release_date_from_tag))
}

fn installed_voice_update_date(cfg: &config::Config) -> Option<String> {
    cfg.last_voice_update_date
        .as_deref()
        .and_then(normalize_release_date)
}

fn detected_system_label() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "⊞ Windows"
    }
    #[cfg(target_os = "linux")]
    {
        "🐧 Linux"
    }
    #[cfg(target_os = "macos")]
    {
        " macOS"
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        "• Unknown"
    }
}

fn render_header(frame: &mut Frame, area: ratatui::layout::Rect, _app: &App) {
    let content_lines = vec![
        Line::from(vec![
            Span::styled(
                "Limbus",
                Style::default()
                    .fg(Color::Rgb(239, 68, 68))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                "Company",
                Style::default()
                    .fg(Color::Rgb(245, 158, 11))
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(Span::raw("")),
        Line::from(Span::styled(
            "零協會文本正體中文化轉換工具",
            Style::default()
                .fg(Color::Rgb(248, 250, 252))
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::raw("")),
        Line::from(Span::raw("")),
        Line::from(Span::styled(
            format!("系統偵測 : {}", detected_system_label()),
            Style::default()
                .fg(Color::Rgb(148, 163, 184))
                .add_modifier(Modifier::BOLD),
        )),
    ];

    let inner_height = area.height.saturating_sub(2) as usize;
    let top_padding = inner_height.saturating_sub(content_lines.len()) / 2;
    let mut lines = Vec::with_capacity(top_padding + content_lines.len());
    for _ in 0..top_padding {
        lines.push(Line::from(Span::raw("")));
    }
    lines.extend(content_lines);

    let header = Paragraph::new(lines)
        .alignment(ratatui::layout::Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(51, 65, 85))),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(header, area);
}

fn render_status_panel(frame: &mut Frame, area: ratatui::layout::Rect, app: &App) {
    let voice_date = app
        .config
        .as_ref()
        .and_then(installed_voice_update_date)
        .unwrap_or_else(|| "尚未記錄".to_string());
    let text_date = app
        .config
        .as_ref()
        .and_then(installed_release_date)
        .unwrap_or_else(|| "尚未記錄".to_string());

    let content_lines = vec![
        Line::from(vec![
            Span::styled(
                "戰鬥氣泡更新日期 : ",
                Style::default()
                    .fg(Color::Rgb(148, 163, 184))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                voice_date,
                Style::default()
                    .fg(Color::Rgb(96, 165, 250))
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(Span::raw("")),
        Line::from(vec![
            Span::styled(
                "遊戲文本更新日期 : ",
                Style::default()
                    .fg(Color::Rgb(148, 163, 184))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                text_date,
                Style::default()
                    .fg(Color::Rgb(74, 222, 128))
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
    ];

    let inner_height = area.height.saturating_sub(2) as usize;
    let top_padding = inner_height.saturating_sub(content_lines.len()) / 2;
    let mut lines = Vec::with_capacity(top_padding + content_lines.len());
    for _ in 0..top_padding {
        lines.push(Line::from(Span::raw("")));
    }
    lines.extend(content_lines);

    let status = Paragraph::new(lines)
        .alignment(ratatui::layout::Alignment::Center)
        .block(
            Block::default()
                .title(" Status ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(51, 65, 85))),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(status, area);
}

fn render_steps_panel(frame: &mut Frame, area: ratatui::layout::Rect, app: &App) {
    let steps = ["初始化", "版本檢查", "下載/解壓", "文字轉換", "語音同步"];
    let current = app.current_step_index();

    let mut spans = Vec::new();
    for (index, step) in steps.iter().enumerate() {
        let style = if index < current {
            Style::default()
                .fg(Color::Rgb(16, 185, 129))
                .add_modifier(Modifier::BOLD)
        } else if index == current {
            Style::default()
                .fg(Color::Rgb(245, 158, 11))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Rgb(100, 116, 139))
        };

        let dot = if index < current {
            "●"
        } else if index == current {
            "◉"
        } else {
            "○"
        };

        spans.push(Span::styled(format!("{dot} {step}"), style));
        if index != steps.len() - 1 {
            spans.push(Span::styled(
                "  ─  ",
                Style::default().fg(Color::Rgb(71, 85, 105)),
            ));
        }
    }

    let panel = Paragraph::new(Line::from(spans)).block(
        Block::default()
            .title(" Pipeline ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(51, 65, 85))),
    );

    frame.render_widget(panel, area);
}
fn render_info_panel(frame: &mut Frame, area: ratatui::layout::Rect, app: &App) {
    let (translation_owner, translation_repo) = translation_provider_parts(app);
    let translation_owner = ellipsize(&translation_owner, 36);
    let translation_repo = ellipsize(&translation_repo, 36);
    let section_style = Style::default()
        .fg(Color::Rgb(148, 163, 184))
        .add_modifier(Modifier::BOLD);
    let voice_owner_style = Style::default().fg(Color::Rgb(56, 189, 248));
    let voice_repo_style = Style::default().fg(Color::Rgb(125, 211, 252));
    let trans_owner_style = Style::default().fg(Color::Rgb(74, 222, 128));
    let trans_repo_style = Style::default().fg(Color::Rgb(190, 242, 100));

    let lines = vec![
        Line::from(Span::styled("◆ 語音氣泡文本提供者", section_style)),
        Line::from(Span::styled(format!("  ├ {}", VOICE_PROVIDER_OWNER), voice_owner_style)),
        Line::from(Span::styled(format!("  └ {}", VOICE_PROVIDER_REPO), voice_repo_style)),
        Line::from(Span::styled("◆ 翻譯來源提供者", section_style)),
        Line::from(Span::styled(format!("  ├ {}", translation_owner), trans_owner_style)),
        Line::from(Span::styled(format!("  └ {}", translation_repo), trans_repo_style)),
    ];

    let info = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Provider ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(51, 65, 85))),
        )
        .wrap(Wrap { trim: true });

    frame.render_widget(info, area);
}
fn render_log_panel(frame: &mut Frame, area: ratatui::layout::Rect, app: &App) {
    let visible_lines = area.height.saturating_sub(2) as usize;
    let total_logs = app.logs.len();

    let max_scroll = total_logs.saturating_sub(visible_lines);
    let scroll = app.log_scroll.min(max_scroll);
    let start_index = total_logs.saturating_sub(visible_lines.saturating_add(scroll));

    let items: Vec<ListItem> = app
        .logs
        .iter()
        .skip(start_index)
        .take(visible_lines)
        .map(|entry| {
            let (icon, color) = match entry.level {
                LogLevel::Info => ("•", Color::Rgb(148, 163, 184)),
                LogLevel::Success => ("✓", Color::Rgb(16, 185, 129)),
                LogLevel::Warn => ("⚠", Color::Rgb(245, 158, 11)),
                LogLevel::Error => ("✗", Color::Rgb(239, 68, 68)),
            };

            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("[{}] ", entry.timestamp),
                    Style::default().fg(Color::Rgb(100, 116, 139)),
                ),
                Span::styled(
                    format!("{icon} "),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(entry.message.clone(), Style::default().fg(color)),
            ]))
        })
        .collect();

    let shown_start = if total_logs == 0 { 0 } else { start_index + 1 };
    let shown_end = if total_logs == 0 {
        0
    } else {
        (start_index + visible_lines).min(total_logs)
    };

    let title = if scroll > 0 {
        format!(
            " Logs {shown_start}-{shown_end}/{total_logs} (↑{scroll}) "
        )
    } else {
        format!(" Logs {shown_start}-{shown_end}/{total_logs} ")
    };

    let logs = List::new(items).block(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(51, 65, 85))),
    );

    frame.render_widget(logs, area);
}
fn render_detail_panel(frame: &mut Frame, area: ratatui::layout::Rect, app: &App) {
    let current_file = app
        .current_file
        .as_deref()
        .map(|s| ellipsize(s, 48))
        .unwrap_or_else(|| "-".to_string());

    let file_counter = match (app.processed_files, app.total_files) {
        (Some(done), Some(total)) => format!("{done}/{total}"),
        _ => "-".to_string(),
    };

    let mut lines = vec![
        kv_line("目前檔案", &current_file),
        kv_line("處理進度", &file_counter),
    ];

    if let Some(err) = &app.error {
        lines.push(kv_line("錯誤", &ellipsize(err, 48)));
    } else {
        lines.push(kv_line("錯誤", "-"));
    }

    if let Some(output) = &app.game_info.output_path {
        lines.push(kv_line("輸出目標", &ellipsize(output, 48)));
    }

    if let Some(game_path) = &app.game_info.game_path {
        lines.push(kv_line("遊戲目錄", &ellipsize(game_path, 48)));
    }

    let panel = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Details ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(51, 65, 85))),
        )
        .wrap(Wrap { trim: true });

    frame.render_widget(panel, area);
}

fn render_footer(frame: &mut Frame, area: ratatui::layout::Rect, app: &App) {
    let (button_text, button_color) = if matches!(app.phase, Phase::Success | Phase::Failed) {
        ("[按 Q 離開]", Color::Rgb(248, 113, 113))
    } else {
        ("[按 S 開始轉換]", Color::Rgb(56, 189, 248))
    };

    let footer = Paragraph::new(Line::from(vec![Span::styled(
        button_text,
        Style::default()
            .fg(button_color)
            .add_modifier(Modifier::BOLD),
    )]))
    .alignment(ratatui::layout::Alignment::Center)
    .block(
        Block::default()
            .title(" Controls ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(51, 65, 85))),
    );

    frame.render_widget(footer, area);
}
fn kv_line(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label:<10} "),
            Style::default()
                .fg(Color::Rgb(148, 163, 184))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            value.to_string(),
            Style::default().fg(Color::Rgb(226, 232, 240)),
        ),
    ])
}

fn ellipsize(input: &str, max_chars: usize) -> String {
    let total = input.chars().count();
    if total <= max_chars {
        return input.to_string();
    }

    let take_tail = max_chars.saturating_sub(3);
    let tail: String = input
        .chars()
        .skip(total.saturating_sub(take_tail))
        .collect();
    format!("...{tail}")
}













