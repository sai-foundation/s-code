use crossterm::style::{Color, Stylize};
use std::io::{self, IsTerminal};

pub(super) fn enabled() -> bool {
    io::stdout().is_terminal()
        && std::env::var_os("NO_COLOR").is_none()
        && std::env::var("CLICOLOR").as_deref() != Ok("0")
        && std::env::var("TERM").is_ok_and(|term| term != "dumb")
}

pub(super) fn paint(text: &str, color: Color) -> String {
    if enabled() {
        text.with(color).bold().to_string()
    } else {
        text.into()
    }
}

pub(super) fn step(number: usize, title: &str, emoji: &str) {
    let icon = if enabled() {
        format!("{emoji} ")
    } else {
        String::new()
    };
    println!(
        "\n  {}",
        paint(&format!("{icon}{number:02} / 03 · {title}"), Color::Cyan)
    );
    if enabled() {
        let dots: Vec<_> = (1..=3)
            .map(|n| {
                if n < number {
                    "●"
                } else if n == number {
                    "◉"
                } else {
                    "○"
                }
            })
            .collect();
        println!("  {}", paint(&dots.join(" ─── "), Color::DarkCyan));
    }
}
