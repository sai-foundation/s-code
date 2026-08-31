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

## Start a client

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

## Update

Community releases publish reviewed Git tags and GitHub-generated source
archives only. They do not publish precompiled executables, a binary installer
or an automatic updater. To update an installation, switch the checkout to the
desired reviewed revision or tag and run `scripts/install-from-source.sh` again.
