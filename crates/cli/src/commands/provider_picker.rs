//! Keyboard provider selection with a line-oriented fallback.
use anyhow::{Result, ensure};
use crossterm::{
    cursor::Show,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use s_code_model_gateway::onboarding::{Preset, Promotion};
use std::io::{self, IsTerminal, Write};

struct Screen;
impl Drop for Screen {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), LeaveAlternateScreen, Show);
        let _ = terminal::disable_raw_mode();
    }
}

pub(super) fn choose(providers: &[Preset], offers: &[Promotion]) -> Result<Option<usize>> {
    ensure!(!providers.is_empty(), "No providers are available");
    if !io::stdin().is_terminal()
        || !io::stdout().is_terminal()
        || std::env::var("TERM").as_deref() == Ok("dumb")
    {
        return choose_plain(providers, offers);
    }
    terminal::enable_raw_mode()?;
    let _screen = Screen;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let colorful = super::setup_style::enabled();
    let accent = if colorful {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    };
    let mut selected = 0;
    loop {
        terminal.draw(|frame| {
            let [heading, body, details, help] = Layout::vertical([
                Constraint::Length(4), Constraint::Min(1), Constraint::Length(3), Constraint::Length(2),
            ]).areas(frame.area());
            frame.render_widget(Paragraph::new(if colorful {
                "✨ S-CODE · Safe · Speedy · Self-evolving\n\n🧭 01 / 03 · Choose your provider\n◉ ─── ○ ─── ○"
            } else {
                "S-Code · Safe. Speedy. Self-evolving.\n\n01 / 03 · Choose your provider"
            }).style(accent), heading);
            let rows = usize::from(body.height).max(1);
            let start = selected / rows * rows;
            let lines: Vec<Line<'_>> = providers.iter().enumerate().skip(start).take(rows).map(|(index, provider)| {
                let label = format!("{} {}  {}{}", if selected == index { ">" } else { " " }, index + 1,
                    provider.name, if provider.id == "sai" { " · api.sai.foundation" } else { "" });
                if index == selected {
                    Line::from(Span::styled(label, accent.add_modifier(Modifier::BOLD)))
                } else { Line::from(label) }
            }).collect();
            frame.render_widget(Paragraph::new(lines), body);
            let provider = &providers[selected];
            let endpoint = if provider.base_url.is_empty() { "Enter your own API endpoint next" } else { provider.base_url };
            let mut detail = format!("{} · {}/{}\n{}", provider.name, selected + 1, providers.len(), endpoint);
            if provider.id == "sai" && let Some(offer) = offers.first() {
                detail.push_str(&format!("\n{} · Terms shown after selection", offer.title));
            }
            frame.render_widget(Paragraph::new(detail), details);
            frame.render_widget(Paragraph::new("↑/↓ move · Home/End first/last · 1–8 jump\nEnter select · Esc cancel"), help);
        })?;
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Release {
                continue;
            }
            match key.code {
                KeyCode::Esc => return Ok(None),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(None);
                }
                KeyCode::Up => selected = selected.saturating_sub(1),
                KeyCode::Down => selected = (selected + 1).min(providers.len() - 1),
                KeyCode::Home => selected = 0,
                KeyCode::End => selected = providers.len() - 1,
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    if let Some(index) = c.to_digit(10).and_then(|n| n.checked_sub(1))
                        && (index as usize) < providers.len()
                    {
                        selected = index as usize;
                    }
                }
                KeyCode::Enter => return Ok(Some(selected)),
                _ => {}
            }
        }
    }
}

fn choose_plain(providers: &[Preset], offers: &[Promotion]) -> Result<Option<usize>> {
    for (index, provider) in providers.iter().enumerate() {
        println!("  {}  {}", index + 1, provider.name);
        if provider.id == "sai" {
            for offer in offers {
                println!(
                    "      {}\n      {}\n      {}",
                    offer.title, offer.terms, offer.url
                );
            }
        }
    }
    loop {
        print!("Provider (number or ID, q cancel) [1]: ");
        io::stdout().flush()?;
        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            return Ok(None);
        }
        let choice = input.trim();
        if choice.eq_ignore_ascii_case("q") {
            return Ok(None);
        }
        let choice = if choice.is_empty() { "1" } else { choice };
        let index = choice
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .filter(|n| *n < providers.len())
            .or_else(|| providers.iter().position(|p| p.id == choice));
        if let Some(index) = index {
            return Ok(Some(index));
        }
        println!("Choose a listed provider number or ID.");
    }
}
