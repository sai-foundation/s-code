package com.opencoding.jetbrains;

import com.intellij.openapi.actionSystem.AnAction;
import com.intellij.openapi.actionSystem.AnActionEvent;
import com.intellij.openapi.project.Project;
import com.intellij.openapi.ui.Messages;

public final class ConnectAction extends AnAction {
    @Override
    public void actionPerformed(AnActionEvent event) {
        Project project = event.getProject();
        OpencodingSettings settings = new OpencodingSettings();
        String url = input(project, "Loopback daemon URL", settings.daemonUrl());
        if (url == null) return;
        String token = input(project, "Startup bearer token", "");
        if (token == null || token.isBlank()) return;
        String organization = input(project, "Organization ID", settings.organizationId());
        String team = input(project, "Team ID", settings.teamId());
        String actor = input(project, "Actor ID", settings.actorId());
        String model = input(project, "Default model", settings.model());
        if (organization == null || team == null || actor == null || model == null) return;
        settings.save(url, organization, team, actor, model);
        settings.token(token);
        settings.sessionId("");
        ActionSupport.background(project, () -> {
            int count = new DaemonClient(settings).connect().size();
            return "Connected with " + count + " enabled capabilities";
        });
    }

    private static String input(Project project, String label, String initial) {
        return Messages.showInputDialog(project, label, "Opencoding", null, initial, null);
    }
}
