package com.scode.jetbrains;

import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import java.net.URI;
import java.util.HashSet;
import java.util.Set;

final class Protocol {
    static final String PROTOCOL_VERSION = "1.0";
    static final String IDE_PROTOCOL_VERSION = "1.0";

    private Protocol() {}

    static URI loopbackBase(String raw) {
        URI uri = URI.create(raw.endsWith("/") ? raw.substring(0, raw.length() - 1) : raw);
        String host = uri.getHost();
        boolean loopback = "localhost".equalsIgnoreCase(host)
                || "127.0.0.1".equals(host)
                || "::1".equals(host)
                || "[::1]".equals(host);
        if (!"http".equals(uri.getScheme()) || !loopback || uri.getUserInfo() != null
                || uri.getPort() < 1) {
            throw new IllegalArgumentException("Daemon URL must be an explicit loopback HTTP endpoint");
        }
        return uri;
    }

    static Set<String> negotiate(JsonObject manifest, Set<String> required) {
        if (major(manifest.get("protocol_version")) != major(PROTOCOL_VERSION)) {
            throw new IllegalArgumentException("Incompatible daemon protocol");
        }
        Set<String> enabled = new HashSet<>();
        JsonArray capabilities = manifest.has("capabilities")
                ? manifest.getAsJsonArray("capabilities") : new JsonArray();
        for (JsonElement element : capabilities) {
            JsonObject capability = element.getAsJsonObject();
            if (capability.has("enabled") && capability.get("enabled").getAsBoolean()
                    && major(capability.get("version")) == 1 && capability.has("id")) {
                enabled.add(capability.get("id").getAsString());
            }
        }
        if (!enabled.containsAll(required)) {
            Set<String> missing = new HashSet<>(required);
            missing.removeAll(enabled);
            throw new IllegalArgumentException("Daemon is missing required capabilities: " + missing);
        }
        return enabled;
    }

    static JsonObject scope(SCodeSettings settings) {
        JsonObject scope = new JsonObject();
        scope.addProperty("organization_id", settings.organizationId());
        scope.addProperty("team_id", settings.teamId());
        scope.addProperty("actor_id", settings.actorId());
        return scope;
    }

    private static int major(JsonElement value) {
        return major(value == null || value.isJsonNull() ? "" : value.getAsString());
    }

    private static int major(String raw) {
        try {
            return Integer.parseInt(raw.split("\\.", 2)[0]);
        } catch (NumberFormatException error) {
            return -1;
        }
    }
}
