package com.scode.jetbrains;

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
                        () -> Messages.showInfoMessage(project, message, "S-Code"));
            } catch (Exception error) {
                ApplicationManager.getApplication().invokeLater(
                        () -> Messages.showErrorDialog(project, safe(error), "S-Code"));
            }
        });
    }

    static String ensureSession(DaemonClient client, SCodeSettings settings, Project project,
            JsonObject context)
            throws Exception {
        JsonArray sessions = client.sessions(settings);
        String workspaceUri = IdeContext.workspaceUri(project);
        JsonObject session = SessionSelection.find(
                sessions, settings.sessionId(), Protocol.scope(settings), workspaceUri);
        if (session == null) {
            settings.sessionId("");
            session = client.createSession(settings, workspaceUri, project.getName());
        }
        SessionSelection.bindContext(session, context);
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
