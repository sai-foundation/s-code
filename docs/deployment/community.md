---
site: true
slug: quick-start
title: Quick start
short_title: Quick start
group: Start here
order: 20
description: Install from source and start your first CLI or Local Web session.
keywords:
  - install
  - setup
  - start
  - CLI
  - Local Web
---

# Quick start

## Prerequisites

Use macOS or Linux with Rust 1.89, Node.js 22, npm, Python 3, Git,
ripgrep (`rg`) and the platform build toolchain.

## Source installation

Clone a reviewed Community revision and install from a clean checkout:

```sh
git clone https://github.com/shilongliu-iteria/opencoding-community.git
cd opencoding-community
scripts/install-from-source.sh
```

Set `OPENCODING_INSTALL_DIR` to choose a dedicated binary directory. The script
builds Local Web and the locked Rust workspace, stages the application files,
runs internal self-tests and atomically replaces the installed commands.

## Configure a model

Run the first-use assistant after installation:

```sh
opencoding setup
opencoding doctor
```

The assistant writes a private `~/.opencoding/config.toml`, containing the
provider, model, endpoint and credential environment-variable name. It never
writes the credential value. Non-interactive automation can use, for example:

```sh
export OPENROUTER_API_KEY='your-key'
opencoding setup --provider openrouter --model z-ai/glm-5.3 --yes
```

## Start a client

Start either client:

```sh
opencoding
opencoding web
```

The CLI starts the loopback service when needed and discovers its private local
connection automatically. Local Web remains a foreground service and binds
loopback. `OPENCODING_URL` and `OPENCODING_TOKEN` are advanced automation
overrides, not normal setup.

## State and backup

Fresh local sessions use SQLite under `~/.opencoding/state`. Sensitive content
is encrypted with a generated 32-byte managed key; both the state directory and
key file use private permissions. Before relying on a candidate build, exercise
backup and integrity verification with a destination outside the repository:

```sh
opencoding web --backup /secure/path/opencoding-backup.sqlite
opencoding web --verify-database
```

For managed encryption, backup also creates a private sibling named
`.opencoding-backup.sqlite.storage-key`. Protect and move the database and key
together. Restore discovers that sibling automatically. A database copied
without its key remains unreadable, while possession of the full state
directory is equivalent to local-user access and is outside this protection.

Restore only while the daemon is stopped:

```sh
opencoding web --restore /secure/path/opencoding-backup.sqlite
```

## Update

Community releases publish reviewed Git tags and GitHub-generated source
archives only. They do not publish precompiled executables, a binary installer
or an automatic updater. To update an installation, switch the checkout to the
desired reviewed revision or tag and run `scripts/install-from-source.sh` again.
