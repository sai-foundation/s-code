package com.scode.jetbrains;

import com.google.gson.JsonObject;
import com.intellij.openapi.actionSystem.AnAction;
import com.intellij.openapi.actionSystem.AnActionEvent;
import com.intellij.openapi.project.Project;

public final class SendContextAction extends AnAction {
    @Override
    public void actionPerformed(AnActionEvent event) {
        Project project = event.getProject();
        if (project == null) return;
        SCodeSettings settings = new SCodeSettings();
        JsonObject context = IdeContext.capture(project, settings);
        ActionSupport.background(project, () -> {
            DaemonClient client = new DaemonClient(settings);
            client.connect();
            String session = ActionSupport.ensureSession(client, settings, project);
            client.updateContext(session, context);
            return "Editor context sent to shared session " + session;
        });
    }
}
