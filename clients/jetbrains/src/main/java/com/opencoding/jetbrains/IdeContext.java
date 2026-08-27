package com.opencoding.jetbrains;

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import com.intellij.openapi.editor.Document;
import com.intellij.openapi.editor.Editor;
import com.intellij.openapi.editor.LogicalPosition;
import com.intellij.openapi.fileEditor.FileDocumentManager;
import com.intellij.openapi.fileEditor.FileEditorManager;
import com.intellij.openapi.project.Project;
import com.intellij.openapi.vfs.VirtualFile;
import java.nio.file.Path;
import java.util.UUID;

final class IdeContext {
    private static final int MAX_DOCUMENT_CHARS = 262_144;
    private static final int MAX_SELECTION_CHARS = 65_536;

    private IdeContext() {}

    static JsonObject capture(Project project, OpencodingSettings settings) {
        String basePath = project.getBasePath();
        if (basePath == null) throw new IllegalStateException("Project has no local workspace");
        String workspaceUri = Path.of(basePath).toUri().toString();
        if (!workspaceUri.endsWith("/")) workspaceUri += "/";
        JsonObject context = new JsonObject();
        context.add("scope", Protocol.scope(settings));
        context.addProperty("protocol_version", Protocol.IDE_PROTOCOL_VERSION);
        context.addProperty("client_instance_id", "jetbrains_" + UUID.nameUUIDFromBytes(basePath.getBytes()));
        context.addProperty("workspace_uri", workspaceUri);
        context.add("diagnostics", new JsonArray());

        Editor editor = FileEditorManager.getInstance(project).getSelectedTextEditor();
        if (editor == null) {
            context.add("active_document", null);
            context.add("selection", null);
            return context;
        }
        Document document = editor.getDocument();
        VirtualFile file = FileDocumentManager.getInstance().getFile(document);
        if (file == null || !file.isInLocalFileSystem()) {
            context.add("active_document", null);
            context.add("selection", null);
            return context;
        }
        String text = document.getText();
        if (text.length() > MAX_DOCUMENT_CHARS) text = text.substring(0, MAX_DOCUMENT_CHARS);
        JsonObject active = new JsonObject();
        active.addProperty("uri", file.toNioPath().toUri().toString());
        active.addProperty("language_id", file.getExtension() == null ? "text" : file.getExtension());
        active.addProperty("version", document.getModificationStamp());
        active.addProperty("text", text);
        context.add("active_document", active);

        String selected = editor.getSelectionModel().getSelectedText();
        if (selected == null) {
            context.add("selection", null);
        } else {
            if (selected.length() > MAX_SELECTION_CHARS) selected = selected.substring(0, MAX_SELECTION_CHARS);
            LogicalPosition start = editor.offsetToLogicalPosition(editor.getSelectionModel().getSelectionStart());
            LogicalPosition end = editor.offsetToLogicalPosition(editor.getSelectionModel().getSelectionEnd());
            JsonObject selection = new JsonObject();
            JsonObject range = new JsonObject();
            range.add("start", position(start));
            range.add("end", position(end));
            selection.add("range", range);
            selection.addProperty("text", selected);
            context.add("selection", selection);
        }
        return context;
    }

    static String workspaceUri(Project project) {
        String basePath = project.getBasePath();
        if (basePath == null) throw new IllegalStateException("Project has no local workspace");
        String uri = Path.of(basePath).toUri().toString();
        return uri.endsWith("/") ? uri : uri + "/";
    }

    private static JsonObject position(LogicalPosition value) {
        JsonObject position = new JsonObject();
        position.addProperty("line", value.line);
        position.addProperty("character", value.column);
        return position;
    }
}
