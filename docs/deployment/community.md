# Community build, installation and operation

## Source installation

From a clean checkout:

```sh
scripts/install-from-source.sh
```

Set `OPENCODING_INSTALL_DIR` to choose a dedicated binary directory. The script
builds Local Web and the locked Rust workspace, stages the application files,
runs internal self-tests and atomically replaces the installed commands.

## Start

Keep an OpenAI-compatible model API running separately, then start either
client:

```sh
opencoding
opencoding web
```

Local Web binds loopback. The CLI discovers the local service connection
automatically. `OPENCODING_URL` and `OPENCODING_TOKEN` are advanced automation
overrides, not normal setup.

## State and backup

Local sessions use SQLite. Before relying on a candidate build, exercise backup
and integrity verification with a destination outside the repository:

```sh
opencoding web --backup /secure/path/opencoding-backup.sqlite
opencoding web --verify-database
```

Restore only while the daemon is stopped:

```sh
opencoding web --restore /secure/path/opencoding-backup.sqlite
```

## Signed releases

Public release downloads are currently disabled. `scripts/install.sh` and the
updater intentionally require signed checksum metadata and have no unsigned
fallback. Private workflow artifacts are for release-candidate validation and
are not published through the installer.
