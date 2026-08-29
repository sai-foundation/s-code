use crate::state::{App, InputMode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SlashCommand {
    pub(crate) name: &'static str,
    pub(crate) detail: &'static str,
}

pub(crate) const SLASH_COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        name: "/new",
        detail: "Create a new Session",
    },
    SlashCommand {
        name: "/resume",
        detail: "Resume another Session",
    },
    SlashCommand {
        name: "/fork",
        detail: "Fork the current Session",
    },
    SlashCommand {
        name: "/retry",
        detail: "Edit and retry an earlier Turn",
    },
    SlashCommand {
        name: "/checkpoints",
        detail: "List restorable checkpoints",
    },
    SlashCommand {
        name: "/rename",
        detail: "Rename the current Session",
    },
    SlashCommand {
        name: "/alias",
        detail: "Change the assistant name",
    },
    SlashCommand {
        name: "/goal",
        detail: "Create or inspect the Session Goal",
    },
    SlashCommand {
        name: "/goal-run",
        detail: "Inspect a persistent Goal Run",
    },
    SlashCommand {
        name: "/terminal",
        detail: "Start a background terminal",
    },
    SlashCommand {
        name: "/ps",
        detail: "List background terminals",
    },
    SlashCommand {
        name: "/stop",
        detail: "Stop a background terminal",
    },
    SlashCommand {
        name: "/side",
        detail: "Start a side conversation",
    },
    SlashCommand {
        name: "/btw",
        detail: "Ask in the side conversation",
    },
    SlashCommand {
        name: "/agents",
        detail: "List Agent runs",
    },
    SlashCommand {
        name: "/agent",
        detail: "Inspect an Agent run",
    },
    SlashCommand {
        name: "/subagents",
        detail: "List child Agent runs",
    },
    SlashCommand {
        name: "/follow-up",
        detail: "Send follow-up work to an Agent",
    },
    SlashCommand {
        name: "/wait",
        detail: "Wait for Agent activity",
    },
    SlashCommand {
        name: "/interrupt",
        detail: "Interrupt an Agent run",
    },
    SlashCommand {
        name: "/close-agent",
        detail: "Close a finished Agent run",
    },
    SlashCommand {
        name: "/archive",
        detail: "Archive the current Session",
    },
    SlashCommand {
        name: "/unarchive",
        detail: "Restore an archived Session",
    },
    SlashCommand {
        name: "/delete",
        detail: "Delete the current Session",
    },
    SlashCommand {
        name: "/steer",
        detail: "Steer the running Turn",
    },
    SlashCommand {
        name: "/queue",
        detail: "Queue input for the running Turn",
    },
    SlashCommand {
        name: "/dequeue",
        detail: "Remove queued Turn input",
    },
    SlashCommand {
        name: "/attach",
        detail: "Attach a file to the next message",
    },
    SlashCommand {
        name: "/detach",
        detail: "Remove a pending attachment",
    },
    SlashCommand {
        name: "/diff",
        detail: "Show current Git changes",
    },
    SlashCommand {
        name: "/undo",
        detail: "Restore the last Turn checkpoint",
    },
    SlashCommand {
        name: "/copy",
        detail: "Copy the latest answer",
    },
    SlashCommand {
        name: "/raw",
        detail: "Show the latest raw answer",
    },
    SlashCommand {
        name: "/output",
        detail: "Expand or collapse tool output",
    },
    SlashCommand {
        name: "/links",
        detail: "Open a safe answer link",
    },
    SlashCommand {
        name: "/usage",
        detail: "Show Session token usage",
    },
    SlashCommand {
        name: "/editor",
        detail: "Open the prompt in an editor",
    },
    SlashCommand {
        name: "/keymap",
        detail: "Choose Emacs or Vim keys",
    },
    SlashCommand {
        name: "/vim",
        detail: "Switch Vim input mode",
    },
    SlashCommand {
        name: "/theme",
        detail: "Choose the CLI theme",
    },
    SlashCommand {
        name: "/statusline",
        detail: "Configure the status line",
    },
    SlashCommand {
        name: "/context",
        detail: "Inspect active context",
    },
    SlashCommand {
        name: "/compact",
        detail: "Compact Session context",
    },
    SlashCommand {
        name: "/memory",
        detail: "Manage durable memory",
    },
    SlashCommand {
        name: "/init",
        detail: "Initialize repository guidance",
    },
    SlashCommand {
        name: "/answer",
        detail: "Answer a pending question",
    },
    SlashCommand {
        name: "/artifact",
        detail: "Open a Session Artifact",
    },
    SlashCommand {
        name: "/model",
        detail: "Choose the Session model",
    },
    SlashCommand {
        name: "/permissions",
        detail: "Choose the permission mode",
    },
    SlashCommand {
        name: "/status",
        detail: "Show connection status",
    },
    SlashCommand {
        name: "/clear",
        detail: "Clear the local transcript view",
    },
    SlashCommand {
        name: "/exit",
        detail: "Exit the CLI",
    },
    SlashCommand {
        name: "/help",
        detail: "Show command help",
    },
];

fn slash_query(input: &str) -> Option<&str> {
    let query = input.strip_prefix('/')?;
    (!query.chars().any(char::is_whitespace)).then_some(query)
}

pub(crate) fn slash_command_menu_visible(app: &App) -> bool {
    app.input_mode == InputMode::Prompt
        && !app.slash_command_dismissed
        && slash_query(app.input.as_str()).is_some()
}

pub(crate) fn matching_slash_commands(input: &str) -> Vec<&'static SlashCommand> {
    let Some(query) = slash_query(input) else {
        return Vec::new();
    };
    let query = query.to_ascii_lowercase();
    let mut matches = SLASH_COMMANDS
        .iter()
        .filter(|command| query.is_empty() || command.name[1..].contains(&query))
        .collect::<Vec<_>>();
    // Keep a fully typed command ahead of broader substring matches such as
    // `/statusline`, so Enter preserves the established execute behavior.
    matches.sort_by_key(|command| command.name != input);
    matches
}

pub(crate) fn selected_slash_command(app: &App) -> Option<&'static SlashCommand> {
    let matches = matching_slash_commands(app.input.as_str());
    matches
        .get(
            app.slash_command_selected
                .min(matches.len().saturating_sub(1)),
        )
        .copied()
}

pub(crate) fn slash_command_input_is_exact(app: &App) -> bool {
    selected_slash_command(app).is_some_and(|command| command.name == app.input.as_str())
}

pub(crate) fn move_slash_command_selection(app: &mut App, direction: isize) {
    let count = matching_slash_commands(app.input.as_str()).len();
    if count == 0 {
        app.slash_command_selected = 0;
        return;
    }
    app.slash_command_selected =
        (app.slash_command_selected as isize + direction).rem_euclid(count as isize) as usize;
}

pub(crate) fn complete_slash_command(app: &mut App) -> bool {
    let Some(command) = selected_slash_command(app) else {
        app.status = "no slash command matches the current input".into();
        return false;
    };
    app.input.replace(&format!("{} ", command.name));
    app.slash_command_selected = 0;
    app.slash_command_dismissed = false;
    app.status = format!("{} selected · press Enter to run", command.name);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_moves_and_completes_slash_commands() {
        let mut app = App::new(Vec::new(), true, true);
        app.input.replace("/arch");
        assert_eq!(
            matching_slash_commands(app.input.as_str())
                .iter()
                .map(|command| command.name)
                .collect::<Vec<_>>(),
            ["/archive", "/unarchive"]
        );
        move_slash_command_selection(&mut app, 1);
        assert_eq!(selected_slash_command(&app).unwrap().name, "/unarchive");
        assert!(!slash_command_input_is_exact(&app));
        assert!(complete_slash_command(&mut app));
        assert_eq!(app.input.as_str(), "/unarchive ");
        assert!(!slash_command_menu_visible(&app));

        app.input.replace("/model");
        assert!(slash_command_input_is_exact(&app));

        app.input.replace("/status");
        assert_eq!(selected_slash_command(&app).unwrap().name, "/status");
        assert!(slash_command_input_is_exact(&app));
    }
}
