use anyhow::{Result, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::{
    env,
    io::{self, Write},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

/// Keep terminal escape sequences bounded. Native clipboard transports do not
/// inherit this limit because they do not write the payload into the terminal.
pub(crate) const MAX_OSC52_BYTES: usize = 100 * 1024;
const COPY_COMMAND_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CopyOutcome {
    /// A local process accepted the complete payload and exited successfully.
    Confirmed,
    /// The request was written to the terminal, which may still reject OSC 52.
    Requested,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Platform {
    MacOs,
    Linux,
    Windows,
    Other,
}

impl Platform {
    fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Other
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ClipboardEnvironment {
    wayland: bool,
    x11: bool,
    tmux: bool,
    remote: bool,
}

impl ClipboardEnvironment {
    fn current() -> Self {
        Self {
            wayland: nonempty_environment_variable("WAYLAND_DISPLAY"),
            x11: nonempty_environment_variable("DISPLAY"),
            tmux: nonempty_environment_variable("TMUX"),
            remote: nonempty_environment_variable("SSH_CONNECTION")
                || nonempty_environment_variable("SSH_CLIENT")
                || nonempty_environment_variable("SSH_TTY"),
        }
    }
}

fn nonempty_environment_variable(name: &str) -> bool {
    env::var_os(name).is_some_and(|value| !value.is_empty())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CommandSpec {
    program: &'static str,
    arguments: &'static [&'static str],
}

const PBCOPY: CommandSpec = CommandSpec {
    program: "/usr/bin/pbcopy",
    arguments: &[],
};
const WL_COPY: CommandSpec = CommandSpec {
    program: "wl-copy",
    arguments: &[],
};
const XCLIP: CommandSpec = CommandSpec {
    program: "xclip",
    arguments: &["-selection", "clipboard", "-in"],
};
const XSEL: CommandSpec = CommandSpec {
    program: "xsel",
    arguments: &["--clipboard", "--input"],
};
const POWERSHELL: CommandSpec = CommandSpec {
    program: "powershell.exe",
    arguments: &[
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "$r = [IO.StreamReader]::new([Console]::OpenStandardInput(), [Text.UTF8Encoding]::new($false)); Set-Clipboard -Value $r.ReadToEnd()",
    ],
};
const POWERSHELL_CORE: CommandSpec = CommandSpec {
    program: "pwsh.exe",
    arguments: &[
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "$r = [IO.StreamReader]::new([Console]::OpenStandardInput(), [Text.UTF8Encoding]::new($false)); Set-Clipboard -Value $r.ReadToEnd()",
    ],
};
const TMUX: CommandSpec = CommandSpec {
    program: "tmux",
    arguments: &["load-buffer", "-w", "-"],
};

fn native_commands(platform: Platform, environment: ClipboardEnvironment) -> Vec<CommandSpec> {
    if environment.remote {
        return Vec::new();
    }
    match platform {
        Platform::MacOs => vec![PBCOPY],
        Platform::Linux => {
            let mut commands = Vec::with_capacity(3);
            if environment.wayland {
                commands.push(WL_COPY);
            }
            if environment.x11 {
                commands.extend([XCLIP, XSEL]);
            }
            commands
        }
        Platform::Windows => vec![POWERSHELL, POWERSHELL_CORE],
        Platform::Other => Vec::new(),
    }
}

trait CommandRunner {
    fn run(&mut self, command: CommandSpec, input: &[u8]) -> io::Result<bool>;
}

struct ProcessCommandRunner {
    deadline: Instant,
}

impl ProcessCommandRunner {
    fn new() -> Self {
        Self {
            deadline: Instant::now() + COPY_COMMAND_TIMEOUT,
        }
    }
}

impl CommandRunner for ProcessCommandRunner {
    fn run(&mut self, command: CommandSpec, input: &[u8]) -> io::Result<bool> {
        if Instant::now() >= self.deadline {
            return Ok(false);
        }
        let mut child = Command::new(command.program)
            .args(command.arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "clipboard stdin closed"))?;
        let payload = input.to_vec();
        let (write_tx, write_rx) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let mut stdin = stdin;
            let _ = write_tx.send(stdin.write_all(&payload));
        });

        let mut write_result = None;
        loop {
            if write_result.is_none()
                && let Ok(result) = write_rx.try_recv()
            {
                write_result = Some(result);
            }
            if let Some(status) = child.try_wait()? {
                let write_result = write_result.unwrap_or_else(|| {
                    write_rx
                        .recv_timeout(Duration::from_millis(100))
                        .unwrap_or_else(|_| {
                            Err(io::Error::new(
                                io::ErrorKind::TimedOut,
                                "clipboard writer did not finish",
                            ))
                        })
                });
                return write_result.map(|()| status.success());
            }
            if Instant::now() >= self.deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(false);
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

/// Copies `text` using a confirmed local transport where possible, then falls
/// back to an OSC 52 request written through `terminal`.
pub(crate) async fn copy_text(terminal: impl Write, text: &str) -> Result<CopyOutcome> {
    if text.is_empty() {
        return Err(anyhow!("cannot copy an empty selection"));
    }
    let platform = Platform::current();
    let environment = ClipboardEnvironment::current();
    let payload = text.to_owned();
    let copied = tokio::task::spawn_blocking(move || {
        let mut runner = ProcessCommandRunner::new();
        try_command_transports(&payload, platform, environment, &mut runner)
    })
    .await
    .map_err(|error| anyhow!("clipboard worker failed: {error}"))?;
    if let Some(outcome) = copied {
        return Ok(outcome);
    }
    write_terminal_clipboard(terminal, text, environment.tmux)
}

fn try_command_transports(
    text: &str,
    platform: Platform,
    environment: ClipboardEnvironment,
    runner: &mut impl CommandRunner,
) -> Option<CopyOutcome> {
    for command in native_commands(platform, environment) {
        if runner.run(command, text.as_bytes()).unwrap_or(false) {
            return Some(CopyOutcome::Confirmed);
        }
    }
    if environment.tmux && runner.run(TMUX, text.as_bytes()).unwrap_or(false) {
        // tmux accepted the buffer and requested a clipboard write, but the
        // outer terminal may still reject that request.
        return Some(CopyOutcome::Requested);
    }
    None
}

#[cfg(test)]
fn copy_text_with(
    terminal: impl Write,
    text: &str,
    platform: Platform,
    environment: ClipboardEnvironment,
    runner: &mut impl CommandRunner,
) -> Result<CopyOutcome> {
    if text.is_empty() {
        return Err(anyhow!("cannot copy an empty selection"));
    }

    if let Some(outcome) = try_command_transports(text, platform, environment, runner) {
        return Ok(outcome);
    }
    write_terminal_clipboard(terminal, text, environment.tmux)
}

fn write_terminal_clipboard(terminal: impl Write, text: &str, tmux: bool) -> Result<CopyOutcome> {
    if tmux {
        write_tmux_osc52(terminal, text)
    } else {
        write_osc52(terminal, text)
    }
}

/// Requests a terminal clipboard write. `Requested` deliberately does not
/// claim success because terminals may ignore or prompt for OSC 52 access.
pub(crate) fn write_osc52(mut writer: impl Write, text: &str) -> Result<CopyOutcome> {
    validate_osc52_text(text)?;
    let encoded = STANDARD.encode(text.as_bytes());
    writer.write_all(b"\x1b]52;c;")?;
    writer.write_all(encoded.as_bytes())?;
    writer.write_all(b"\x07")?;
    writer.flush()?;
    Ok(CopyOutcome::Requested)
}

fn write_tmux_osc52(mut writer: impl Write, text: &str) -> Result<CopyOutcome> {
    validate_osc52_text(text)?;
    let encoded = STANDARD.encode(text.as_bytes());
    writer.write_all(b"\x1bPtmux;\x1b\x1b]52;c;")?;
    writer.write_all(encoded.as_bytes())?;
    writer.write_all(b"\x07\x1b\\")?;
    writer.flush()?;
    Ok(CopyOutcome::Requested)
}

fn validate_osc52_text(text: &str) -> Result<()> {
    if text.is_empty() {
        return Err(anyhow!("cannot copy an empty selection"));
    }
    if text.len() > MAX_OSC52_BYTES {
        return Err(anyhow!(
            "selection exceeds the 100 KiB OSC 52 limit and no local clipboard backend accepted it"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Default)]
    struct FakeRunner {
        outcomes: VecDeque<io::Result<bool>>,
        calls: Vec<(CommandSpec, Vec<u8>)>,
    }

    impl FakeRunner {
        fn with_outcomes(outcomes: impl IntoIterator<Item = io::Result<bool>>) -> Self {
            Self {
                outcomes: outcomes.into_iter().collect(),
                calls: Vec::new(),
            }
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(&mut self, command: CommandSpec, input: &[u8]) -> io::Result<bool> {
            self.calls.push((command, input.to_vec()));
            self.outcomes.pop_front().unwrap_or(Ok(false))
        }
    }

    #[test]
    fn native_command_order_is_platform_and_display_aware() {
        let both_displays = ClipboardEnvironment {
            wayland: true,
            x11: true,
            tmux: false,
            remote: false,
        };
        assert_eq!(
            native_commands(Platform::Linux, both_displays),
            vec![WL_COPY, XCLIP, XSEL]
        );
        assert_eq!(
            native_commands(
                Platform::Linux,
                ClipboardEnvironment {
                    wayland: false,
                    x11: true,
                    tmux: false,
                    remote: false,
                }
            ),
            vec![XCLIP, XSEL]
        );
        assert_eq!(
            native_commands(Platform::MacOs, ClipboardEnvironment::default()),
            vec![PBCOPY]
        );
        assert_eq!(
            native_commands(Platform::Windows, ClipboardEnvironment::default()),
            vec![POWERSHELL, POWERSHELL_CORE]
        );
        assert!(
            native_commands(
                Platform::MacOs,
                ClipboardEnvironment {
                    wayland: false,
                    x11: true,
                    tmux: false,
                    remote: true,
                }
            )
            .is_empty(),
            "remote sessions must not write to the remote host clipboard"
        );
    }

    #[test]
    fn confirmed_native_copy_stops_before_tmux_or_terminal_fallback() {
        let mut runner = FakeRunner::with_outcomes([Ok(false), Ok(true)]);
        let mut terminal = Vec::new();
        let environment = ClipboardEnvironment {
            wayland: true,
            x11: true,
            tmux: true,
            remote: false,
        };

        assert_eq!(
            copy_text_with(
                &mut terminal,
                "exact 🧡 payload",
                Platform::Linux,
                environment,
                &mut runner,
            )
            .unwrap(),
            CopyOutcome::Confirmed
        );
        assert_eq!(
            runner
                .calls
                .iter()
                .map(|(command, _)| *command)
                .collect::<Vec<_>>(),
            vec![WL_COPY, XCLIP]
        );
        assert!(
            runner
                .calls
                .iter()
                .all(|(_, input)| input == "exact 🧡 payload".as_bytes())
        );
        assert!(terminal.is_empty());
    }

    #[test]
    fn tmux_is_tried_after_native_transports_and_is_requested() {
        let unavailable = io::Error::new(io::ErrorKind::NotFound, "not installed");
        let mut runner =
            FakeRunner::with_outcomes([Err(unavailable), Ok(false), Ok(false), Ok(true)]);
        let mut terminal = Vec::new();
        let environment = ClipboardEnvironment {
            wayland: true,
            x11: true,
            tmux: true,
            remote: false,
        };

        assert_eq!(
            copy_text_with(
                &mut terminal,
                "tmux payload",
                Platform::Linux,
                environment,
                &mut runner,
            )
            .unwrap(),
            CopyOutcome::Requested
        );
        assert_eq!(
            runner
                .calls
                .iter()
                .map(|(command, _)| *command)
                .collect::<Vec<_>>(),
            vec![WL_COPY, XCLIP, XSEL, TMUX]
        );
        assert!(terminal.is_empty());
    }

    #[test]
    fn failed_tmux_command_uses_a_tmux_passthrough_request() {
        let mut runner = FakeRunner::with_outcomes([Ok(false)]);
        let mut terminal = Vec::new();
        let text = "tmux fallback";

        assert_eq!(
            copy_text_with(
                &mut terminal,
                text,
                Platform::Other,
                ClipboardEnvironment {
                    wayland: false,
                    x11: false,
                    tmux: true,
                    remote: true,
                },
                &mut runner,
            )
            .unwrap(),
            CopyOutcome::Requested
        );
        assert_eq!(runner.calls.len(), 1);
        assert_eq!(runner.calls[0].0, TMUX);
        assert_eq!(
            terminal,
            format!(
                "\u{1b}Ptmux;\u{1b}\u{1b}]52;c;{}\u{7}\u{1b}\\",
                STANDARD.encode(text.as_bytes())
            )
            .into_bytes()
        );
    }

    #[test]
    fn osc52_fallback_is_exact_but_only_reports_a_request() {
        let mut runner = FakeRunner::default();
        let mut terminal = Vec::new();
        let text = "你好, S-Code";

        assert_eq!(
            copy_text_with(
                &mut terminal,
                text,
                Platform::Other,
                ClipboardEnvironment::default(),
                &mut runner,
            )
            .unwrap(),
            CopyOutcome::Requested
        );
        assert_eq!(
            String::from_utf8(terminal).unwrap(),
            format!("\u{1b}]52;c;{}\u{7}", STANDARD.encode(text.as_bytes()))
        );
        assert!(runner.calls.is_empty());
    }

    #[test]
    fn osc52_limit_does_not_restrict_confirmed_transports() {
        let text = "x".repeat(MAX_OSC52_BYTES + 1);
        let mut confirmed = FakeRunner::with_outcomes([Ok(true)]);
        let mut terminal = Vec::new();
        assert_eq!(
            copy_text_with(
                &mut terminal,
                &text,
                Platform::MacOs,
                ClipboardEnvironment::default(),
                &mut confirmed,
            )
            .unwrap(),
            CopyOutcome::Confirmed
        );
        assert!(terminal.is_empty());

        let mut unavailable = FakeRunner::default();
        let error = copy_text_with(
            Vec::new(),
            &text,
            Platform::Other,
            ClipboardEnvironment::default(),
            &mut unavailable,
        )
        .unwrap_err();
        assert!(error.to_string().contains("100 KiB OSC 52 limit"));
    }

    #[test]
    fn empty_text_never_invokes_a_transport() {
        let mut runner = FakeRunner::with_outcomes([Ok(true)]);
        let error = copy_text_with(
            Vec::new(),
            "",
            Platform::MacOs,
            ClipboardEnvironment {
                wayland: false,
                x11: false,
                tmux: true,
                remote: false,
            },
            &mut runner,
        )
        .unwrap_err();
        assert!(error.to_string().contains("empty selection"));
        assert!(runner.calls.is_empty());
    }
}
