package com.scode.jetbrains;

import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import java.net.URI;
import java.nio.file.Path;

/** An IDE session belongs to one account and the open project, even across reconnects. */
final class SessionSelection {
    private SessionSelection() {}

    static JsonObject find(JsonArray sessions, String savedId, JsonObject scope, String workspaceUri) {
        JsonObject first = null;
        for (JsonElement element : sessions) {
            if (!element.isJsonObject()) continue;
            JsonObject session = element.getAsJsonObject();
            if (!matches(session, scope, workspaceUri)) continue;
            if (savedId.equals(string(session, "id"))) return session;
            if (first == null) first = session;
        }
        return first;
    }

    static void bindContext(JsonObject session, JsonObject context) {
        if (!context.has("scope") || !context.get("scope").isJsonObject()
                || !matches(session, context.getAsJsonObject("scope"), string(context, "workspace_uri"))) {
            throw new IllegalArgumentException("Session does not belong to this account and project");
        }
        // Storage checks the exact URI string, including escaping and the final slash.
        context.addProperty("workspace_uri", string(session, "workspace_uri"));
    }

    private static boolean matches(JsonObject session, JsonObject scope, String workspaceUri) {
        if (string(session, "id").isBlank()
                || (session.has("mode") && !"work".equals(string(session, "mode")))
                || !session.has("scope") || !session.get("scope").isJsonObject()) return false;
        JsonObject sessionScope = session.getAsJsonObject("scope");
        for (String field : new String[] {"organization_id", "team_id", "actor_id"}) {
            String expected = string(scope, field);
            if (expected.isBlank() || !expected.equals(string(sessionScope, field))) return false;
        }
        Path expected = localPath(workspaceUri);
        Path actual = localPath(string(session, "workspace_uri"));
        return expected != null && expected.equals(actual);
    }

    private static Path localPath(String raw) {
        try {
            URI uri = URI.create(raw);
            if (!"file".equalsIgnoreCase(uri.getScheme()) || uri.getQuery() != null
                    || uri.getFragment() != null) return null;
            return Path.of(uri).normalize();
        } catch (IllegalArgumentException error) {
            return null;
        }
    }

    private static String string(JsonObject object, String key) {
        JsonElement value = object.get(key);
        return value != null && value.isJsonPrimitive() && value.getAsJsonPrimitive().isString()
                ? value.getAsString() : "";
    }
}
