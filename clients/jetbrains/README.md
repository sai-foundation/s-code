# Opencoding JetBrains client

This IntelliJ Platform plugin is a first-class IDE Capability Protocol v1
client. It connects only to an explicit loopback daemon, stores the bearer token
in JetBrains `PasswordSafe`, negotiates required capabilities, restores or
creates a shared Team session, submits bounded active-document/selection
context, and starts Agent Turns from the IDE.

The plugin targets IntelliJ Platform 2025.1+ and uses the IntelliJ Platform
Gradle Plugin 2.x. Build and test with Java 17+ and Gradle 9+:

```sh
gradle test buildPlugin verifyPlugin
```

Use **Tools → Opencoding → Connect to Daemon**, then **Send Editor Context** or
**Ask Agent**. Session state remains in the daemon; the IDE never implements a
separate Agent loop.
