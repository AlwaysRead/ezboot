use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::prelude::Stylize;
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};
use regex::Regex;
use std::{
    io::{self, Write},
    process::{Command, Stdio},
    time::Duration,
};

#[derive(Clone)]
struct BootEntry {
    id: String,
    name: String,
}

#[derive(Clone, Copy)]
enum Focus {
    Priority,
    BootOnce,
}

#[derive(Clone)]
enum Action {
    None,
    SetOrder(Vec<String>),
    BootOnce(String),
}

enum UIState {
    Main,
    AskPassword,
    ConfirmReboot,
    Rebooting,
}

// ---------- Layout helpers ----------

fn center(area: Rect, width: u16, height: u16) -> Rect {
    Rect::new(
        area.x + area.width / 2 - width / 2,
        area.y + area.height / 2 - height / 2,
        width,
        height,
    )
}

fn centered_area(area: Rect, width_pct: u16, height_pct: u16) -> Rect {
    let w = area.width * width_pct / 100;
    let h = area.height * height_pct / 100;
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

// ---------- EFI helpers ----------

fn fetch_boot_entries() -> Vec<BootEntry> {
    let output = Command::new("efibootmgr")
        .arg("-v")
        .output()
        .expect("failed to run efibootmgr -v");

    let text = String::from_utf8_lossy(&output.stdout);
    let regex = Regex::new(r"Boot(?P<id>[0-9A-Fa-f]{4})\*?\s+(?P<name>[^\t\(]+)").unwrap();

    text.lines()
        .filter_map(|line| {
            regex.captures(line).map(|cap| BootEntry {
                id: cap["id"].trim().to_string(),
                name: cap["name"].trim().to_string(),
            })
        })
        .collect()
}

fn fetch_boot_order() -> Vec<String> {
    let output = Command::new("efibootmgr")
        .output()
        .expect("failed to run efibootmgr");
    let text = String::from_utf8_lossy(&output.stdout);

    text.lines()
        .find(|l| l.starts_with("BootOrder:"))
        .map(|l| {
            l["BootOrder:".len()..]
                .trim()
                .split(',')
                .map(|s| s.trim().to_string())
                .collect()
        })
        .unwrap_or_default()
}

// ---------- Draw main UI ----------

fn draw_main_ui(
    f: &mut ratatui::Frame,
    area: Rect,
    entries: &[BootEntry],
    focus: Focus,
    selected_priority: usize,
    selected_boot_once: usize,
) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(10),
            Constraint::Percentage(40),
            Constraint::Percentage(40),
            Constraint::Percentage(10),
        ])
        .split(area);

    // Title
    f.render_widget(
        Paragraph::new("Boot Switcher")
            .style(Style::default().fg(Color::Cyan).bold())
            .alignment(Alignment::Center),
        layout[0],
    );

    // Priority panel
    let priority_items: Vec<ListItem> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let style = if matches!(focus, Focus::Priority) && i == selected_priority {
                Style::default().bg(Color::Cyan).fg(Color::Black).bold()
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(format!(" {}. {}", i + 1, e.name)).style(style)
        })
        .collect();

    f.render_widget(
        List::new(priority_items).block(
            Block::default()
                .title(" Boot Priority (default order) ")
                .borders(Borders::ALL),
        ),
        layout[1],
    );

    // Boot once panel
    let boot_once_items: Vec<ListItem> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let style = if matches!(focus, Focus::BootOnce) && i == selected_boot_once {
                Style::default().bg(Color::Cyan).fg(Color::Black).bold()
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(format!(" {}", e.name)).style(style)
        })
        .collect();

    f.render_widget(
        List::new(boot_once_items)
            .block(Block::default().title(" Boot Once ").borders(Borders::ALL)),
        layout[2],
    );

    // Footer keybind bar
    let footer = "Tab: Switch panel  |  ↑/↓: Move  |  u/d: Reorder priority  |  Enter: Apply/Boot  |  q: Quit";
    f.render_widget(
        Paragraph::new(footer)
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::DarkGray)),
        layout[3],
    );
}

// ---------- Password modal (transparent, thin bar) ----------

fn draw_password_popup(f: &mut ratatui::Frame, area: Rect, password: &str, show: bool) {
    let popup_width = area.width * 3 / 4;
    let popup_height = 5;
    let popup = center(area, popup_width, popup_height);

    f.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .title(" Authentication "),
        popup,
    );

    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // label
            Constraint::Length(1), // thin input
            Constraint::Length(1), // help
        ])
        .split(Rect {
            x: popup.x + 1,
            y: popup.y + 1,
            width: popup.width - 2,
            height: popup.height - 2,
        });

    f.render_widget(
        Paragraph::new("Enter sudo password")
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::Yellow)),
        inner[0],
    );

    let displayed = if show {
        password.to_string()
    } else {
        "*".repeat(password.len())
    };

    let bar_width = popup_width / 2;
    let bar_area = Rect {
        x: popup.x + (popup.width - bar_width) / 2,
        y: inner[1].y,
        width: bar_width,
        height: 1,
    };

    f.render_widget(
        Paragraph::new(format!(" {}", displayed))
            .style(
                Style::default()
                    .bg(Color::Rgb(200, 180, 140))
                    .fg(Color::Black),
            )
            .alignment(Alignment::Left),
        bar_area,
    );

    f.render_widget(
        Paragraph::new("Enter = Confirm   •   Esc = Cancel   •   Tab = Show/Hide")
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::Gray)),
        inner[2],
    );
}

// ---------- Reboot confirm modal ----------

fn draw_reboot_popup(f: &mut ratatui::Frame, area: Rect, yes_selected: bool) {
    let popup_width = area.width / 3;
    let popup_height = 7;
    let popup = center(area, popup_width, popup_height);

    f.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .title(" Apply Complete "),
        popup,
    );

    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Length(2)])
        .split(Rect {
            x: popup.x + 1,
            y: popup.y + 1,
            width: popup.width - 2,
            height: popup.height - 2,
        });

    f.render_widget(
        Paragraph::new("Reboot now?")
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::White)),
        inner[0],
    );

    let buttons = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(inner[1]);

    let yes_style = if yes_selected {
        Style::default().bg(Color::Green).fg(Color::Black).bold()
    } else {
        Style::default().fg(Color::White)
    };

    let no_style = if !yes_selected {
        Style::default().bg(Color::Red).fg(Color::Black).bold()
    } else {
        Style::default().fg(Color::White)
    };

    f.render_widget(
        Paragraph::new("[ Yes ]")
            .alignment(Alignment::Center)
            .style(yes_style),
        buttons[0],
    );
    f.render_widget(
        Paragraph::new("[ No ]")
            .alignment(Alignment::Center)
            .style(no_style),
        buttons[1],
    );
}

// ---------- Rebooting screen ----------

fn draw_rebooting_screen(f: &mut ratatui::Frame, area: Rect) {
    f.render_widget(
        Block::default().style(Style::default().bg(Color::Black)),
        area,
    );
    let popup = center(area, area.width / 3, 5);

    f.render_widget(
        Paragraph::new("Rebooting…")
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::Cyan).bold())
            .block(Block::default().borders(Borders::ALL)),
        popup,
    );
}

// ---------- main ----------

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut entries = fetch_boot_entries();
    let order = fetch_boot_order();

    if !order.is_empty() {
        entries.sort_by_key(|e| {
            order
                .iter()
                .position(|id| id == &e.id)
                .unwrap_or(usize::MAX)
        });
    }

    let mut selected_priority = 0usize;
    let mut selected_boot_once = 0usize;
    let mut focus = Focus::Priority;

    let mut state = UIState::Main;
    let mut password = String::new();
    let mut show_password = false;
    let mut pending_action = Action::None;
    let mut reboot_yes = true;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    while !matches!(state, UIState::Rebooting) {
        terminal.draw(|f| {
            // shrink + center the whole UI (like the WiFi dialog)
            let area = centered_area(f.area(), 65, 60);

            match state {
                UIState::Main => draw_main_ui(
                    f,
                    area,
                    &entries,
                    focus,
                    selected_priority,
                    selected_boot_once,
                ),
                UIState::AskPassword => draw_password_popup(f, area, &password, show_password),
                UIState::ConfirmReboot => draw_reboot_popup(f, area, reboot_yes),
                UIState::Rebooting => draw_rebooting_screen(f, area),
            }
        })?;

        if matches!(state, UIState::Rebooting) {
            break;
        }

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                match state {
                    UIState::Main => match key.code {
                        KeyCode::Char('q') => break,

                        KeyCode::Tab => {
                            focus = match focus {
                                Focus::Priority => Focus::BootOnce,
                                Focus::BootOnce => Focus::Priority,
                            }
                        }

                        KeyCode::Up => match focus {
                            Focus::Priority if selected_priority > 0 => selected_priority -= 1,
                            Focus::BootOnce if selected_boot_once > 0 => selected_boot_once -= 1,
                            _ => {}
                        },

                        KeyCode::Down => match focus {
                            Focus::Priority if selected_priority + 1 < entries.len() => {
                                selected_priority += 1
                            }
                            Focus::BootOnce if selected_boot_once + 1 < entries.len() => {
                                selected_boot_once += 1
                            }
                            _ => {}
                        },

                        KeyCode::Char('u') if matches!(focus, Focus::Priority) => {
                            if selected_priority > 0 {
                                entries.swap(selected_priority, selected_priority - 1);
                                selected_priority -= 1;
                            }
                        }

                        KeyCode::Char('d') if matches!(focus, Focus::Priority) => {
                            if selected_priority + 1 < entries.len() {
                                entries.swap(selected_priority, selected_priority + 1);
                                selected_priority += 1;
                            }
                        }

                        KeyCode::Enter => {
                            pending_action = match focus {
                                Focus::Priority => {
                                    let ids =
                                        entries.iter().map(|e| e.id.clone()).collect::<Vec<_>>();
                                    Action::SetOrder(ids)
                                }
                                Focus::BootOnce => {
                                    let id = entries[selected_boot_once].id.clone();
                                    Action::BootOnce(id)
                                }
                            };
                            password.clear();
                            state = UIState::AskPassword;
                        }

                        _ => {}
                    },

                    UIState::AskPassword => match key.code {
                        KeyCode::Esc => {
                            password.clear();
                            pending_action = Action::None;
                            state = UIState::Main;
                        }
                        KeyCode::Tab => {
                            show_password = !show_password;
                        }
                        KeyCode::Backspace => {
                            password.pop();
                        }
                        KeyCode::Enter => match pending_action.clone() {
                            Action::SetOrder(order_ids) => {
                                let mut child = Command::new("sudo")
                                    .arg("-S")
                                    .arg("efibootmgr")
                                    .arg("-o")
                                    .arg(order_ids.join(","))
                                    .stdin(Stdio::piped())
                                    .stdout(Stdio::null())
                                    .stderr(Stdio::null())
                                    .spawn()?;
                                if let Some(mut stdin) = child.stdin.take() {
                                    stdin.write_all(format!("{password}\n").as_bytes())?;
                                }
                                state = UIState::ConfirmReboot;
                            }
                            Action::BootOnce(id) => {
                                let mut child = Command::new("sudo")
                                    .arg("-S")
                                    .arg("efibootmgr")
                                    .arg("-n")
                                    .arg(&id)
                                    .stdin(Stdio::piped())
                                    .stdout(Stdio::null())
                                    .stderr(Stdio::null())
                                    .spawn()?;
                                if let Some(mut stdin) = child.stdin.take() {
                                    stdin.write_all(format!("{password}\n").as_bytes())?;
                                }
                                state = UIState::Rebooting;
                                let mut reboot = Command::new("sudo")
                                    .arg("-S")
                                    .arg("reboot")
                                    .stdin(Stdio::piped())
                                    .stdout(Stdio::null())
                                    .stderr(Stdio::null())
                                    .spawn()?;
                                if let Some(mut stdin) = reboot.stdin.take() {
                                    stdin.write_all(format!("{password}\n").as_bytes())?;
                                }
                            }
                            Action::None => state = UIState::Main,
                        },
                        KeyCode::Char(c) => password.push(c),
                        _ => {}
                    },

                    UIState::ConfirmReboot => match key.code {
                        KeyCode::Esc => {
                            state = UIState::Main;
                        }
                        KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                            reboot_yes = !reboot_yes;
                        }
                        KeyCode::Enter => {
                            if reboot_yes {
                                state = UIState::Rebooting;
                                let mut reboot = Command::new("sudo")
                                    .arg("-S")
                                    .arg("reboot")
                                    .stdin(Stdio::piped())
                                    .stdout(Stdio::null())
                                    .stderr(Stdio::null())
                                    .spawn()?;
                                if let Some(mut stdin) = reboot.stdin.take() {
                                    stdin.write_all(format!("{password}\n").as_bytes())?;
                                }
                            } else {
                                state = UIState::Main;
                            }
                        }
                        _ => {}
                    },

                    UIState::Rebooting => {}
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}
