package com.scode.jetbrains;

import com.google.gson.JsonObject;
import com.intellij.openapi.actionSystem.AnAction;
import com.intellij.openapi.actionSystem.AnActionEvent;
import com.intellij.openapi.project.Project;
import com.intellij.openapi.ui.Messages;

public final class AskAction extends AnAction {
    @Override
    public void actionPerformed(AnActionEvent event) {
        Project project = event.getProject();
        if (project == null) return;
        String prompt = Messages.showInputDialog(project, "Ask the Team Agent", "S-Code", null);
        if (prompt == null || prompt.isBlank()) return;
        SCodeSettings settings = new SCodeSettings();
        JsonObject context = IdeContext.capture(project, settings);
        ActionSupport.background(project, () -> {
            DaemonClient client = new DaemonClient(settings);
            client.connect();
            String session = ActionSupport.ensureSession(client, settings, project, context);
            client.updateContext(session, context);
            JsonObject turn = client.startTurn(session, settings, prompt);
            return "Agent Turn started: " + turn.get("id").getAsString();
        });
    }
}
