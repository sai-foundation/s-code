package com.scode.jetbrains;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import java.util.Set;
import org.junit.jupiter.api.Test;

final class ProtocolTest {
    @Test
    void loopbackTransportRejectsRemoteHostsAndEmbeddedCredentials() {
        assertEquals("127.0.0.1", Protocol.loopbackBase("http://127.0.0.1:4096/").getHost());
        assertEquals(4096, Protocol.loopbackBase("http://[::1]:4096").getPort());
        assertThrows(IllegalArgumentException.class, () -> Protocol.loopbackBase("https://daemon.example"));
        assertThrows(IllegalArgumentException.class, () -> Protocol.loopbackBase("http://user:secret@127.0.0.1:4096"));
        assertThrows(IllegalArgumentException.class, () -> Protocol.loopbackBase("http://127.0.0.1"));
    }

    @Test
    void capabilityNegotiationRequiresProtocolAndIdeContractButIgnoresUnknowns() {
        JsonObject manifest = new JsonObject();
        manifest.addProperty("protocol_version", "1.9");
        JsonArray capabilities = new JsonArray();
        capabilities.add(capability("scope.team", "1", true));
        capabilities.add(capability("session.persistence", "1", true));
        capabilities.add(capability("ide.context.v1", "1", true));
        capabilities.add(capability("future.unknown", "99", true));
        manifest.add("capabilities", capabilities);
        Set<String> enabled = Protocol.negotiate(
                manifest, Set.of("scope.team", "session.persistence", "ide.context.v1"));
        assertTrue(enabled.contains("ide.context.v1"));
        assertTrue(!enabled.contains("future.unknown"));
        assertThrows(IllegalArgumentException.class,
                () -> Protocol.negotiate(manifest, Set.of("approval.once")));
        manifest.addProperty("protocol_version", "2.0");
        assertThrows(IllegalArgumentException.class,
                () -> Protocol.negotiate(manifest, Set.of("scope.team")));
    }

    private static JsonObject capability(String id, String version, boolean enabled) {
        JsonObject value = new JsonObject();
        value.addProperty("id", id);
        value.addProperty("version", version);
        value.addProperty("enabled", enabled);
        return value;
    }
}
