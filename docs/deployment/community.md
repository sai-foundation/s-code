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

Use macOS or Linux. The installer checks prerequisites and asks before
installing anything missing. You do not need to prepare Rust or Node yourself.

| Dependency | Purpose | How the installer prepares it |
| --- | --- | --- |
| Rust 1.89 | Build the CLI and local service | Reuse the pinned toolchain or install it with official rustup |
| Node.js 22.12+ (22.x) and npm | Build Local Web | Reuse a compatible installation or download Node 22.23.2 with a pinned SHA-256 checksum |
| Python 3.9+ | Installation locking and atomic replacement | System package manager; existing Homebrew on macOS |
| Git | Clone the source and operate on repositories | System package manager or Apple Command Line Tools |
| C/C++ compiler and make | Compile native dependencies | System build packages or Apple Command Line Tools |
| Bubblewrap (Linux) | Isolate agent commands at runtime | Distribution package manager |

Automatic Linux system-package installation supports `apt-get`, `dnf` and
`pacman`; other distributions must prepare the listed system packages manually.
Automatic Node downloads support macOS and glibc-based Linux on x86_64 and
arm64. Other architectures can use an existing compatible Node installation.
The installer does not change Linux kernel or sandbox settings. Bubblewrap
must be permitted by your system's user-namespace policy.

On macOS, the script opens Apple's Command Line Tools installer when needed.
Complete that system dialog and rerun the script. If Python 3.9+ is still
missing, the script can use an existing Homebrew installation; otherwise
install Python from [python.org](https://www.python.org/downloads/macos/).
The script does not install Homebrew itself.

New managed Rust/Node build tools live in `~/.cache/s-code/build-tools`; the
installer does not put these build tools on your shell path or replace your
existing Node. It adds only S-Code's installation directory to shell
configuration; use `--no-modify-path` to opt out.
An existing rustup installation receives the pinned toolchain without changing
its default. Managed tools are for building S-Code; configure the languages
and tools needed by your own projects separately. Rust, Node, Python and the
compiler are not needed merely to launch an already-built S-Code.

```sh
scripts/install-from-source.sh --check-deps       # Check only; no downloads/build
scripts/install-from-source.sh --yes              # Approve missing dependencies
scripts/install-from-source.sh --no-install-deps  # Build with existing tools only
scripts/install-from-source.sh --no-modify-path   # Leave shell configuration alone
```

Non-interactive installs fail with a dependency list unless `--yes` is supplied.
System package installation can still require an administrator password.

## Source installation

Clone a reviewed S-Code revision and install from a clean checkout:

```sh
git clone https://github.com/sl-7qx/s-code.git
cd s-code
scripts/install-from-source.sh
```

If Git is not installed yet, download and extract the repository's source ZIP
instead, then run `scripts/install-from-source.sh` inside the extracted source.
The installer can then prepare Git along with other system dependencies.

Set `S_CODE_INSTALL_DIR` to choose a dedicated binary directory. The script
builds Local Web and the locked Rust workspace, stages the application files,
runs internal self-tests and atomically replaces the installed commands.

The default destination is `~/.local/bin`. After installation, `./s-code` in
the checkout launches the installed application immediately, even if your
current terminal has not loaded the new command path. With a custom destination,
keep `S_CODE_INSTALL_DIR` exported when using this checkout launcher.

The installer configures zsh (`.zshrc`, respecting `ZDOTDIR`), bash (`.bashrc`
and the existing login profile, or `.bash_profile`), and fish
(`conf.d/s-code-path.fish`, respecting `XDG_CONFIG_HOME`). Existing settings are
preserved and repeat installs do not duplicate the configuration. New terminals
can run `s-code` from any project. For the current terminal, use the activation
command printed by the installer, or keep using `./s-code` from the checkout.
Unknown shells and configuration write failures receive an absolute launch
command. `--no-modify-path` can be combined with a dependency option above.

## Configure a model

Run the first-use assistant after installation:

```sh
./s-code setup
./s-code doctor
```

The assistant writes a private `~/.s-code/config.toml`, containing the
provider, model, endpoint and credential environment-variable name. It never
writes the credential value. Non-interactive automation can use, for example:

```sh
export OPENROUTER_API_KEY='your-key'
s-code setup --provider openrouter --model z-ai/glm-5.3 --yes
```

## Start a client

Start either client:

```sh
s-code
s-code web
```

The first client starts one shared loopback daemon in the background and
discovers its private local connection automatically. `s-code web` opens
the stable loopback application. The launcher authenticates with the private
connection file, or another authenticated local client, to mint a single-use
bootstrap; the page removes its fragment
immediately and exchanges it for a private browser cookie. Opening the bare
loopback URL cannot create an authorized browser session. In a headless
environment, set `S_CODE_NO_BROWSER=1` to print a single-use launch URL;
do not share or log it. Closing the browser does not stop the daemon.
Use `s-code stop` and `s-code restart` for lifecycle control. Logs are
written below the configured state directory at `logs/daemon.log` and rotate at
5 MiB with two backups. `S_CODE_URL` and `S_CODE_TOKEN` are advanced
automation overrides, not normal setup.

## State and backup

Fresh local sessions use SQLite under `~/.s-code/state`. Model and user
message content, turn inputs and checkpoints, attachments, artifacts, extension
and MCP configuration, audit payloads and retained background-terminal output
are encrypted with a generated 32-byte managed key. IDs, timestamps, statuses
and indexes remain plaintext. Team planning records also remain plaintext,
including goal and task titles, criteria, evidence, blockers, outcome and
pull-request URLs, ownership resource URIs and on-call labels, knowledge titles
and source URIs, model labels and budgets. Do not put secrets in those
operational fields. Both the state directory and key file use private
permissions.
Before relying on a candidate build, stop the daemon and exercise backup and
integrity verification with a destination outside the repository. All offline
database maintenance commands acquire the same instance lock as the daemon:

```sh
s-code stop
s-code web --backup /secure/path/s-code-backup.sqlite
s-code web --verify-database
```

For managed encryption, backup also creates a private sibling named
`.s-code-backup.sqlite.storage-key`. Protect and move the database and key
together. Restore discovers that sibling automatically. A database copied
without its key remains unreadable, while possession of the full state
directory is equivalent to local-user access and is outside this protection.

Restore also requires the daemon to remain stopped:

```sh
s-code web --restore /secure/path/s-code-backup.sqlite
```

## Update

S-Code releases publish reviewed Git tags and GitHub-generated source
archives only. They do not publish precompiled executables, a binary installer
or an automatic updater. To update an installation, switch the checkout to the
desired reviewed revision or tag, then run:

```sh
s-code stop
scripts/install-from-source.sh
s-code
```

Stopping first prevents an older already-running daemon from being reused with
the newly installed CLI. The installer deliberately never kills active work;
if it was run while the service remained active, run `s-code restart`
before continuing.

## Moving from Opencoding Community

The S-Code rename requires a fresh installation and profile. This includes
Opencoding Community candidates labeled `v0.1.0-preview.1` and earlier private
candidates. Encryption identifiers and historical database migrations changed;
old databases and backups are not compatible with S-Code.

Stop the old daemon using its original launcher and preserve the entire old
`~/.opencoding` directory with private permissions. Keep the old binary if you
need to read its local history. Install S-Code and run `s-code setup` to create
a new `~/.s-code` profile. The rename does not move or delete old state.
Do not copy old databases, backups, keys or configuration into the new profile,
or point S-Code at the old state directory.

Update automation to use `s-code` and `S_CODE_*` environment variables. Install
the renamed IDE clients and connect them to the new daemon; saved connections,
browser sessions, URI handlers and credentials use new namespaces. Old client
and daemon versions must not be mixed. Future S-Code database migrations must
be explicit and tested before a stable release.
