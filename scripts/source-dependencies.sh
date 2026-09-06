#!/bin/sh
# Sourced by install-from-source.sh. Keep this bootstrap usable before Python,
# Rust or Node exists; only the post-system-install steps need Python.

source_python_ready() {
  command -v python3 >/dev/null 2>&1 &&
    python3 -c 'import sys; sys.exit(sys.version_info < (3, 9))' >/dev/null 2>&1
}

source_node_ready() {
  command -v node >/dev/null 2>&1 && command -v npm >/dev/null 2>&1 &&
    node -e 'const [major, minor] = process.versions.node.split(".").map(Number); process.exit(major === 22 && minor >= 12 ? 0 : 1)' >/dev/null 2>&1 &&
    npm --version >/dev/null 2>&1
}

source_rust_ready() {
  command -v cargo >/dev/null 2>&1 && command -v rustc >/dev/null 2>&1 &&
    rustc --version 2>/dev/null | grep -q "^rustc $SOURCE_RUST_VERSION " &&
    cargo --version >/dev/null 2>&1
}

source_system_missing() {
  source_python_ready || printf '%s\n' 'Python 3.9+'
  for source_command in git curl cc c++ make; do
    command -v "$source_command" >/dev/null 2>&1 || printf '%s\n' "$source_command"
  done
  if [ "$SOURCE_OS" = Darwin ]; then
    xcode-select -p >/dev/null 2>&1 || printf '%s\n' 'Apple Command Line Tools'
  else
    command -v bwrap >/dev/null 2>&1 || printf '%s\n' Bubblewrap
  fi
}

source_as_root() {
  if [ "$(id -u)" = 0 ]; then
    "$@"
  elif command -v sudo >/dev/null 2>&1; then
    sudo "$@"
  else
    echo "System packages need an administrator. Run the listed package command as root, then retry." >&2
    return 1
  fi
}

source_system_install() {
  case "$SOURCE_PACKAGE_MANAGER" in
    apt-get)
      source_as_root apt-get update
      source_as_root apt-get install -y python3 git curl ca-certificates build-essential bubblewrap
      ;;
    dnf)
      source_as_root dnf install -y python3 git curl ca-certificates gcc gcc-c++ make bubblewrap
      ;;
    pacman)
      # Never do a partial system upgrade with pacman -Sy.
      source_as_root pacman -S --needed --noconfirm python git curl ca-certificates base-devel bubblewrap
      ;;
    macos)
      if ! xcode-select -p >/dev/null 2>&1; then
        xcode-select --install
        echo "Complete Apple's Command Line Tools installer, then rerun scripts/install-from-source.sh." >&2
        return 1
      fi
      if ! source_python_ready; then
        if ! command -v brew >/dev/null 2>&1; then
          echo "Python 3.9+ is missing. Install it from python.org or Homebrew, then retry. Homebrew itself is not installed automatically." >&2
          return 1
        fi
        brew install python
        # Homebrew's Python may be keg-only or absent from the caller's PATH.
        PATH="$(brew --prefix python)/libexec/bin:$(brew --prefix python)/bin:$PATH"
        export PATH
      fi
      ;;
  esac
}

source_download() {
  curl --fail --silent --show-error --location --retry 2 \
    --connect-timeout 15 --max-time 300 --proto '=https' --proto-redir '=https' \
    "$1" --output "$2"
  python3 - "$2" "$3" <<'PY'
import hashlib, pathlib, sys
path = pathlib.Path(sys.argv[1])
with path.open('rb') as stream:
    digest = hashlib.sha256()
    for block in iter(lambda: stream.read(1024 * 1024), b''):
        digest.update(block)
if digest.hexdigest() != sys.argv[2]:
    raise SystemExit('Dependency checksum mismatch; refusing to use the download')
PY
}

source_install_node() (
  set -eu
  umask 077
  source_stage="$(mktemp -d "$SOURCE_TOOLS/.node.XXXXXX")"
  trap 'rm -rf "$source_stage"' EXIT
  trap 'exit 1' HUP INT TERM
  source_download "https://nodejs.org/dist/v$SOURCE_NODE_VERSION/$SOURCE_NODE_ARCHIVE.tar.gz" \
    "$source_stage/node.tar.gz" "$SOURCE_NODE_SHA256"
  tar -xzf "$source_stage/node.tar.gz" -C "$source_stage"
  "$source_stage/$SOURCE_NODE_ARCHIVE/bin/node" --version
  # The outer installer holds the dependency lock. Never merge a partial tree
  # into a previous installation or follow an existing destination symlink.
  if [ -e "$SOURCE_NODE_HOME" ] || [ -L "$SOURCE_NODE_HOME" ]; then
    echo "Managed Node installation is incomplete: $SOURCE_NODE_HOME. Move it aside and retry." >&2
    exit 1
  fi
  mv "$source_stage/$SOURCE_NODE_ARCHIVE" "$SOURCE_NODE_HOME"
)

source_install_rust() (
  set -eu
  if command -v rustup >/dev/null 2>&1; then
    rustup toolchain install "$SOURCE_RUST_VERSION" --profile minimal --no-self-update
  else
    umask 077
    source_stage="$(mktemp -d "$SOURCE_TOOLS/.rustup.XXXXXX")"
    trap 'rm -rf "$source_stage"' EXIT
    trap 'exit 1' HUP INT TERM
    source_download https://raw.githubusercontent.com/rust-lang/rustup/1.28.2/rustup-init.sh \
      "$source_stage/rustup-init.sh" 17247e4bcacf6027ec2e11c79a72c494c9af69ac8d1abcc1b271fa4375a106c2
    # Isolate newly bootstrapped Rust from the user's default toolchain and
    # shell profiles. Existing rustup installations are reused above.
    export CARGO_HOME="$SOURCE_TOOLS/cargo" RUSTUP_HOME="$SOURCE_TOOLS/rustup"
    RUSTUP_VERSION=1.28.2 sh "$source_stage/rustup-init.sh" -y --no-modify-path \
      --profile minimal --default-toolchain "$SOURCE_RUST_VERSION"
  fi
)

prepare_source_dependencies() {
  case "${HOME:-}" in
    /|*/../*|*/..|*/./*|*/.) echo 'HOME must be a dedicated, normalized absolute user directory.' >&2; return 1 ;;
    /*) ;;
    *) echo 'HOME must be an absolute user directory.' >&2; return 1 ;;
  esac
  SOURCE_OS="$(uname -s)"
  case "$SOURCE_OS" in
    Darwin|Linux) ;;
    *) echo "Source installation supports macOS and Linux only." >&2; return 1 ;;
  esac
  SOURCE_RUST_VERSION="$(sed -n 's/^channel = "\([^"]*\)"/\1/p' "$ROOT/rust-toolchain.toml")"
  [ -n "$SOURCE_RUST_VERSION" ] || { echo 'Missing pinned Rust toolchain' >&2; return 1; }
  # Probes must not cause rustup to download anything before consent.
  export RUSTUP_TOOLCHAIN="$SOURCE_RUST_VERSION" RUSTUP_AUTO_INSTALL=0
  SOURCE_TOOLS="$HOME/.cache/s-code/build-tools"
  SOURCE_NODE_VERSION=22.23.2
  case "$SOURCE_OS:$(uname -m)" in
    Darwin:arm64) SOURCE_NODE_PLATFORM=darwin-arm64; SOURCE_NODE_SHA256=61130f394c1630d211dd50aecc4353d379480f36d3ac913cd85dbba1aed585c6 ;;
    Darwin:x86_64) SOURCE_NODE_PLATFORM=darwin-x64; SOURCE_NODE_SHA256=58e99022c2ff89395576cc7fd4d98cea24bb68081475d5f88b801ee8729fb026 ;;
    Linux:aarch64|Linux:arm64) SOURCE_NODE_PLATFORM=linux-arm64; SOURCE_NODE_SHA256=013b59cfd2819703a6f4a14ab891fc46fc2a4e3f5bcd92de3fb4929b43e35b30 ;;
    Linux:x86_64) SOURCE_NODE_PLATFORM=linux-x64; SOURCE_NODE_SHA256=b294a556e639d64338823920e5866c21c02741742d2e1529ee1a225c1ec9252a ;;
    *) SOURCE_NODE_PLATFORM=unsupported; SOURCE_NODE_SHA256= ;;
  esac
  SOURCE_NODE_ARCHIVE="node-v$SOURCE_NODE_VERSION-$SOURCE_NODE_PLATFORM"
  SOURCE_NODE_HOME="$SOURCE_TOOLS/$SOURCE_NODE_ARCHIVE"
  # Only add managed tools if the caller's tool does not meet the requirement.
  if ! source_node_ready && [ -x "$SOURCE_NODE_HOME/bin/node" ]; then
    PATH="$SOURCE_NODE_HOME/bin:$PATH"
  fi
  if ! source_rust_ready && ! command -v rustup >/dev/null 2>&1; then
    if [ -x "${CARGO_HOME:-$HOME/.cargo}/bin/rustup" ]; then
      PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
    elif [ -x "$SOURCE_TOOLS/cargo/bin/rustup" ]; then
      export CARGO_HOME="$SOURCE_TOOLS/cargo" RUSTUP_HOME="$SOURCE_TOOLS/rustup"
      PATH="$CARGO_HOME/bin:$PATH"
    fi
  fi
  export PATH
  SOURCE_SYSTEM_MISSING="$(source_system_missing)"
  SOURCE_NEED_RUST=0; SOURCE_NEED_NODE=0
  source_rust_ready || SOURCE_NEED_RUST=1
  source_node_ready || SOURCE_NEED_NODE=1
  if [ -z "$SOURCE_SYSTEM_MISSING" ] && [ "$SOURCE_NEED_RUST$SOURCE_NEED_NODE" = 00 ]; then
    echo 'Source dependencies are ready.'
    return 0
  fi
  echo 'Missing source-build dependencies:'
  [ -z "$SOURCE_SYSTEM_MISSING" ] || printf '%s\n' "$SOURCE_SYSTEM_MISSING"
  [ "$SOURCE_NEED_RUST" = 0 ] || echo "Rust $SOURCE_RUST_VERSION (official rustup; existing default unchanged)"
  [ "$SOURCE_NEED_NODE" = 0 ] || echo "Node.js $SOURCE_NODE_VERSION + npm (private build-tools directory)"
  SOURCE_PACKAGE_MANAGER=macos
  if [ -n "$SOURCE_SYSTEM_MISSING" ] && [ "$SOURCE_OS" = Linux ]; then
    SOURCE_PACKAGE_MANAGER=
    for source_manager in apt-get dnf pacman; do
      if command -v "$source_manager" >/dev/null 2>&1; then
        SOURCE_PACKAGE_MANAGER="$source_manager"
        break
      fi
    done
    [ -n "$SOURCE_PACKAGE_MANAGER" ] || {
      echo 'Install the listed system dependencies using your distribution, then retry.' >&2
      return 1
    }
    echo "System packages will be installed with $SOURCE_PACKAGE_MANAGER (administrator access may be required)."
    case "$SOURCE_PACKAGE_MANAGER" in
      apt-get) echo 'Packages: python3 git curl ca-certificates build-essential bubblewrap' ;;
      dnf) echo 'Packages: python3 git curl ca-certificates gcc gcc-c++ make bubblewrap' ;;
      pacman) echo 'Packages: python git curl ca-certificates base-devel bubblewrap' ;;
    esac
  fi
  if [ "$SOURCE_NEED_NODE" = 1 ] && [ "$SOURCE_NODE_PLATFORM" = unsupported ]; then
    echo 'Automatic Node installation supports x86_64 and arm64. Install Node.js 22 and npm manually on this architecture.' >&2
    return 1
  fi
  case "$SOURCE_DEPENDENCY_MODE" in
    check|never)
      echo 'Rerun scripts/install-from-source.sh interactively, or use --yes to install missing dependencies.' >&2
      return 1
      ;;
    prompt)
      if [ ! -t 0 ]; then
        echo 'Non-interactive installation needs --yes to install missing dependencies.' >&2
        return 1
      fi
      printf 'Install the missing dependencies and build S-Code? [y/N] '
      read -r source_answer
      case "$source_answer" in y|Y|yes|YES) ;; *) echo 'Installation cancelled.'; return 1 ;; esac
      ;;
  esac
  [ -z "$SOURCE_SYSTEM_MISSING" ] || source_system_install
  SOURCE_SYSTEM_MISSING="$(source_system_missing)"
  [ -z "$SOURCE_SYSTEM_MISSING" ] || {
    printf 'Still missing:\n%s\n' "$SOURCE_SYSTEM_MISSING" >&2
    return 1
  }
  if [ "${S_CODE_SOURCE_DEPENDENCY_LOCK_HELD:-0}" = 1 ]; then
    finish_source_dependencies
    return 0
  fi
  # One dependency bootstrap at a time, including concurrent installs into
  # different application directories. Recheck after acquiring the lock.
  exec python3 - "$SOURCE_TOOLS" "$ROOT/scripts/install-from-source.sh" <<'PY'
import fcntl, os, pathlib, stat, sys
root = pathlib.Path(sys.argv[1])
root.mkdir(mode=0o700, parents=True, exist_ok=True)
info = root.lstat()
if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o022:
    raise SystemExit('Build-tools directory must be owned by you and not writable by other users')
flags = os.O_CREAT | os.O_RDWR | getattr(os, 'O_NOFOLLOW', 0)
fd = os.open(root / '.install.lock', flags, 0o600)
if not stat.S_ISREG(os.fstat(fd).st_mode):
    raise SystemExit('Dependency lock must be a regular file')
fcntl.flock(fd, fcntl.LOCK_EX)
os.set_inheritable(fd, True)
environment = os.environ.copy()
environment['S_CODE_SOURCE_DEPENDENCY_LOCK_HELD'] = '1'
os.execve(sys.argv[2], [sys.argv[2], '--yes'], environment)
PY
}

finish_source_dependencies() {
  # Called only while holding the bootstrap lock acquired above.
  if ! source_node_ready; then
    source_install_node
    PATH="$SOURCE_NODE_HOME/bin:$PATH"
    export PATH
  fi
  if ! source_rust_ready; then
    source_install_rust
    if ! command -v rustup >/dev/null 2>&1; then
      export CARGO_HOME="$SOURCE_TOOLS/cargo" RUSTUP_HOME="$SOURCE_TOOLS/rustup"
      PATH="$CARGO_HOME/bin:$PATH"
      export PATH
    fi
  fi
  source_node_ready && source_rust_ready || {
    echo 'Dependency installation did not produce the required tool versions.' >&2
    return 1
  }
}
