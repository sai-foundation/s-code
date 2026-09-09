#!/usr/bin/env python3
"""Make the installed launcher discoverable and print usable next steps."""

import argparse
import fcntl
import os
from pathlib import Path
import shlex


def shell_configuration(home, shell, directory):
    quoted = shlex.quote(str(directory))
    if shell == "zsh":
        paths = [Path(os.environ.get("ZDOTDIR") or home) / ".zshrc"]
    elif shell == "bash":
        login = next((home / name for name in (".bash_profile", ".bash_login", ".profile")
                      if (home / name).exists()), home / ".bash_profile")
        paths = [home / ".bashrc", login]
    elif shell == "fish":
        config = Path(os.environ.get("XDG_CONFIG_HOME") or home / ".config")
        # Fish single quotes escape backslashes and quotes, unlike POSIX sh.
        quoted = "'" + str(directory).replace("\\", "\\\\").replace("'", "\\'") + "'"
        return [config / "fish" / "conf.d" / "s-code-path.fish"], (
            f"if not contains -- {quoted} $PATH\n"
            f"    set -gx PATH {quoted} $PATH\nend\n"
        ), f"set -gx PATH {quoted} $PATH"
    else:
        return [], "", f'export PATH={quoted}:"$PATH"'
    return paths, (
        f'case ":$PATH:" in\n'
        f'    *:{quoted}:*) ;;\n'
        f'    *) export PATH={quoted}:"$PATH" ;;\nesac\n'
    ), f'export PATH={quoted}:"$PATH"'


def append_configuration(path, stanza):
    path.parent.mkdir(parents=True, exist_ok=True)
    # Append under a lock; preserve existing bytes, permissions and dotfile links.
    with path.open("a+", encoding="utf-8", errors="surrogateescape") as output:
        fcntl.flock(output, fcntl.LOCK_EX)
        output.seek(0)
        if stanza in output.read():
            return
        output.write("\n# S-Code command path\n" + stanza)
    print(f"Configured command path in {path}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--install-dir", required=True, type=Path)
    parser.add_argument("--no-modify-path", action="store_true")
    args = parser.parse_args()
    directory = args.install_dir
    if not directory.is_absolute() or any(c in str(directory) for c in ":\n"):
        parser.error("installation path must be absolute and contain no colons or newlines")
    shell = Path(os.environ.get("SHELL", "")).name
    paths, stanza, activate = shell_configuration(Path.home(), shell, directory)
    configured = bool(paths) and not args.no_modify_path
    if configured:
        for path in paths:
            try:
                append_configuration(path, stanza)
            except OSError as error:
                configured = False
                print(f"Could not configure {path}: {error}")
    if configured:
        print("New terminals can run: s-code")
    elif not args.no_modify_path and not paths:
        print("Shell not recognized; command path was not changed.")
    # A child process cannot update the invoking terminal's environment.
    if str(directory) not in os.environ.get("PATH", "").split(os.pathsep):
        print("To enable s-code in this terminal, run:")
        print(f"  {activate}")
    command = shlex.quote(str(directory / "s-code"))
    print("Start now (works without changing PATH):")
    print(f"  {command}")
    print(f"First use: {command} setup")
    print("If upgrading a running service, finish active tasks, then run:")
    print(f"  {command} restart")


if __name__ == "__main__":
    main()
