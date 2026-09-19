package com.scode.jetbrains;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertThrows;

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import java.nio.file.Path;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.ValueSource;

final class SessionSelectionTest {
    private static final String WORKSPACE = Path.of(System.getProperty("java.io.tmpdir"),
            "s-code project").toUri().toString().replaceAll("/$", "");

    @Test
    void aNewWebChatDoesNotReplaceTheOpenProjectsWorkSession() {
        JsonObject chat = session("recent-chat", "chat", "");
        JsonObject otherProject = session("other-project", "work", WORKSPACE + "-other/");
        JsonObject work = session("project-work", "work", WORKSPACE);
        assertSame(work, SessionSelection.find(list(chat, otherProject, work), "", scope(), WORKSPACE));
    }

    @Test
    void aValidSavedSessionIsPreferredOverTheMostRecentWorkSession() {
        JsonObject recent = session("recent", "work", WORKSPACE);
        JsonObject saved = session("saved", "work", WORKSPACE);
        assertSame(saved, SessionSelection.find(list(recent, saved), "saved", scope(), WORKSPACE));
    }

    @Test
    void aSavedChatOrDeletedSessionFallsBackToMatchingWork() {
        JsonObject chat = session("chat", "chat", "");
        JsonObject work = session("work", "work", WORKSPACE);
        for (String savedId : new String[] {"chat", "deleted"}) {
            assertSame(work, SessionSelection.find(list(chat, work), savedId, scope(), WORKSPACE));
        }
    }

    @ParameterizedTest
    @ValueSource(strings = {"organization_id", "team_id", "actor_id"})
    void aSavedSessionFromAnotherScopeIsNeverReused(String field) {
        JsonObject wrongScope = session("saved", "work", WORKSPACE);
        wrongScope.getAsJsonObject("scope").addProperty(field, "different");
        JsonObject work = session("current", "work", WORKSPACE);
        assertSame(work, SessionSelection.find(list(wrongScope, work), "saved", scope(), WORKSPACE));
        assertNull(SessionSelection.find(list(wrongScope), "saved", scope(), WORKSPACE));
    }

    @Test
    void aSavedSessionFromAnotherProjectCannotBindEditorContext() {
        JsonObject wrongProject = session("saved", "work", WORKSPACE + "/other");
        JsonObject context = context();
        assertNull(SessionSelection.find(list(wrongProject), "saved", scope(), WORKSPACE));
        assertThrows(IllegalArgumentException.class, () -> SessionSelection.bindContext(wrongProject, context));
        assertEquals(WORKSPACE, context.get("workspace_uri").getAsString());
    }

    @Test
    void noMatchingWorkSessionRequestsCreationInsteadOfSelectingChat() {
        assertNull(SessionSelection.find(new JsonArray(), "", scope(), WORKSPACE));
        JsonObject chat = session("chat", "chat", "");
        JsonObject promotedChat = session("promoted", "work", WORKSPACE + "/managed-task");
        assertNull(SessionSelection.find(list(chat, promotedChat), "chat", scope(), WORKSPACE));
    }

    @Test
    void olderDaemonsMayOmitModeButStillNeedMatchingScopeAndWorkspace() {
        JsonObject legacy = session("legacy", "work", WORKSPACE);
        legacy.remove("mode");
        assertSame(legacy, SessionSelection.find(list(legacy), "", scope(), WORKSPACE));
        legacy.addProperty("workspace_uri", "");
        assertNull(SessionSelection.find(list(legacy), "", scope(), WORKSPACE));
    }

    @Test
    void equivalentDirectoryUrisUseTheStoredSpellingForTheBackend() {
        String stored = WORKSPACE.replace("file:///", "file:/").replace("s-code", "%73-code") + "/";
        JsonObject work = session("work", "work", stored);
        assertSame(work, SessionSelection.find(list(work), "", scope(), WORKSPACE));
        JsonObject context = context();
        SessionSelection.bindContext(work, context);
        assertEquals(stored, context.get("workspace_uri").getAsString());
    }

    @ParameterizedTest
    @ValueSource(strings = {"", "not-a-uri", "https://example.com/project", "file:relative", "file:///tmp?query", "file:///tmp#fragment"})
    void malformedOrNonLocalWorkspaceUrisAreSkipped(String uri) {
        assertNull(SessionSelection.find(list(session("bad", "work", uri)), "bad", scope(), WORKSPACE));
    }

    @Test
    void unknownModesAndMalformedSessionsFailClosed() {
        JsonObject unknown = session("future", "future-mode", WORKSPACE);
        JsonObject missingScope = session("missing-scope", "work", WORKSPACE);
        missingScope.remove("scope");
        JsonArray sessions = list(unknown, missingScope);
        sessions.add("invalid");
        assertNull(SessionSelection.find(sessions, "future", scope(), WORKSPACE));
    }

    private static JsonObject context() {
        JsonObject context = new JsonObject();
        context.add("scope", scope());
        context.addProperty("workspace_uri", WORKSPACE);
        return context;
    }

    private static JsonObject scope() {
        JsonObject scope = new JsonObject();
        scope.addProperty("organization_id", "org");
        scope.addProperty("team_id", "team");
        scope.addProperty("actor_id", "actor");
        return scope;
    }

    private static JsonObject session(String id, String mode, String workspace) {
        JsonObject session = new JsonObject();
        session.addProperty("id", id);
        session.addProperty("mode", mode);
        session.addProperty("workspace_uri", workspace);
        session.add("scope", scope());
        return session;
    }

    private static JsonArray list(JsonObject... sessions) {
        JsonArray list = new JsonArray();
        for (JsonObject session : sessions) list.add(session);
        return list;
    }
}
