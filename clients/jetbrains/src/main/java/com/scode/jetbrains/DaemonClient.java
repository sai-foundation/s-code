package com.scode.jetbrains;

import com.google.gson.Gson;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import java.io.IOException;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.time.Duration;
import java.util.Set;

final class DaemonClient {
    private static final Gson GSON = new Gson();
    private final HttpClient http = HttpClient.newBuilder()
            .connectTimeout(Duration.ofSeconds(5)).build();
    private final URI base;
    private final String token;

    DaemonClient(SCodeSettings settings) {
        base = Protocol.loopbackBase(settings.daemonUrl());
        token = settings.token();
        if (token.isBlank()) throw new IllegalArgumentException("Daemon token is not configured");
    }

    Set<String> connect() throws IOException, InterruptedException {
        JsonObject manifest = get("/v1/capabilities").getAsJsonObject();
        return Protocol.negotiate(manifest, Set.of("scope.team", "session.persistence", "ide.context.v1"));
    }

    JsonArray sessions(SCodeSettings settings) throws IOException, InterruptedException {
        return get("/v1/sessions?organization_id=" + encode(settings.organizationId())
                + "&team_id=" + encode(settings.teamId())
                + "&actor_id=" + encode(settings.actorId())).getAsJsonArray();
    }

    JsonObject createSession(SCodeSettings settings, String workspaceUri, String title)
            throws IOException, InterruptedException {
        JsonObject body = new JsonObject();
        body.add("scope", Protocol.scope(settings));
        body.addProperty("mode", "work");
        body.addProperty("workspace_uri", workspaceUri);
        body.addProperty("title", title);
        body.addProperty("model", settings.model());
        return post("/v1/sessions", body).getAsJsonObject();
    }

    JsonObject updateContext(String sessionId, JsonObject context)
            throws IOException, InterruptedException {
        return post("/v1/sessions/" + encode(sessionId) + "/editor-context", context).getAsJsonObject();
    }

    JsonObject startTurn(String sessionId, SCodeSettings settings, String prompt)
            throws IOException, InterruptedException {
        JsonObject body = new JsonObject();
        body.add("scope", Protocol.scope(settings));
        body.addProperty("content", prompt);
        return post("/v1/sessions/" + encode(sessionId) + "/turns", body).getAsJsonObject();
    }

    private com.google.gson.JsonElement get(String path) throws IOException, InterruptedException {
        return request(HttpRequest.newBuilder(base.resolve(path)).GET(), null);
    }

    private com.google.gson.JsonElement post(String path, JsonObject body)
            throws IOException, InterruptedException {
        return request(HttpRequest.newBuilder(base.resolve(path)), GSON.toJson(body));
    }

    private com.google.gson.JsonElement request(HttpRequest.Builder builder, String body)
            throws IOException, InterruptedException {
        builder.timeout(Duration.ofSeconds(30))
                .header("authorization", "Bearer " + token)
                .header("accept", "application/json");
        if (body != null) {
            builder.header("content-type", "application/json")
                    .POST(HttpRequest.BodyPublishers.ofString(body));
        }
        HttpResponse<String> response = http.send(builder.build(), HttpResponse.BodyHandlers.ofString());
        if (response.statusCode() < 200 || response.statusCode() >= 300) {
            throw new IOException("Daemon returned HTTP " + response.statusCode());
        }
        if (response.body().length() > 8 * 1024 * 1024) throw new IOException("Daemon response is too large");
        return GSON.fromJson(response.body(), com.google.gson.JsonElement.class);
    }

    private static String encode(String value) {
        return java.net.URLEncoder.encode(value, java.nio.charset.StandardCharsets.UTF_8);
    }
}
