package com.scode.jetbrains;

import com.intellij.credentialStore.CredentialAttributes;
import com.intellij.credentialStore.Credentials;
import com.intellij.ide.passwordSafe.PasswordSafe;
import com.intellij.ide.util.PropertiesComponent;

final class SCodeSettings {
    private static final String PREFIX = "s-code.";
    private static final CredentialAttributes TOKEN =
            new CredentialAttributes("S-Code Team Agent — daemon token");

    String daemonUrl() { return value("daemonUrl", "http://127.0.0.1:4096"); }
    String organizationId() { return value("organizationId", "org_local"); }
    String teamId() { return value("teamId", "team_local"); }
    String actorId() { return value("actorId", "user_local"); }
    String model() { return value("model", "deepseek/deepseek-v4-flash"); }
    String sessionId() { return value("sessionId", ""); }

    void save(String daemonUrl, String organizationId, String teamId, String actorId, String model) {
        PropertiesComponent values = PropertiesComponent.getInstance();
        values.setValue(PREFIX + "daemonUrl", daemonUrl);
        values.setValue(PREFIX + "organizationId", organizationId);
        values.setValue(PREFIX + "teamId", teamId);
        values.setValue(PREFIX + "actorId", actorId);
        values.setValue(PREFIX + "model", model);
    }

    void sessionId(String value) {
        PropertiesComponent.getInstance().setValue(PREFIX + "sessionId", value);
    }

    String token() {
        Credentials credentials = PasswordSafe.getInstance().get(TOKEN);
        return credentials == null ? "" : credentials.getPasswordAsString();
    }

    void token(String value) {
        PasswordSafe.getInstance().set(TOKEN, new Credentials("daemon", value));
    }

    private String value(String key, String fallback) {
        return PropertiesComponent.getInstance().getValue(PREFIX + key, fallback);
    }
}
