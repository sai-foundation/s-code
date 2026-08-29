package com.opencoding.jetbrains;

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import com.intellij.openapi.application.ApplicationManager;
import com.intellij.openapi.project.Project;
import com.intellij.openapi.ui.Messages;

final class ActionSupport {
    interface CheckedTask { String run() throws Exception; }

    private ActionSupport() {}

    static void background(Project project, CheckedTask task) {
        ApplicationManager.getApplication().executeOnPooledThread(() -> {
            try {
                String message = task.run();
                ApplicationManager.getApplication().invokeLater(
                        () -> Messages.showInfoMessage(project, message, "Opencoding"));
            } catch (Exception error) {
                ApplicationManager.getApplication().invokeLater(
                        () -> Messages.showErrorDialog(project, safe(error), "Opencoding"));
            }
        });
    }

    static String ensureSession(DaemonClient client, OpencodingSettings settings, Project project)
            throws Exception {
        if (!settings.sessionId().isBlank()) return settings.sessionId();
        JsonArray sessions = client.sessions(settings);
        JsonObject session;
        if (!sessions.isEmpty()) {
            session = sessions.get(0).getAsJsonObject();
        } else {
            session = client.createSession(settings, IdeContext.workspaceUri(project), project.getName());
        }
        String id = session.get("id").getAsString();
        settings.sessionId(id);
        return id;
    }

    private static String safe(Exception error) {
        String message = error.getMessage();
        if (message == null || message.isBlank()) message = error.getClass().getSimpleName();
        return message.length() > 1024 ? message.substring(0, 1024) : message;
    }
}
