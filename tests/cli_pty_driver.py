#!/usr/bin/env python3
import fcntl
import os
import pty
import select
import struct
import subprocess
import sys
import termios
import time


def fail(message, process, output, transcript):
    if process.poll() is None:
        process.kill()
        process.wait()
    with open(transcript, "wb") as target:
        target.write(output)
    recent = output[-8000:].decode("utf-8", errors="replace")
    raise SystemExit(f"{message}\n--- recent PTY output ---\n{recent}")


def wait_for(needle, process, master, output, transcript, timeout=10, start=0):
    deadline = time.monotonic() + timeout
    while needle not in output[start:]:
        if process.poll() is not None:
            fail(f"CLI exited before rendering {needle!r}", process, output, transcript)
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            fail(f"timed out waiting for {needle!r}", process, output, transcript)
        readable, _, _ = select.select([master], [], [], min(0.25, remaining))
        if readable:
            try:
                chunk = os.read(master, 65536)
                if not chunk:
                    fail(f"PTY closed before rendering {needle!r}", process, output, transcript)
                output += chunk
            except OSError:
                pass
    return output


def wait_for_exit(process, master, output, transcript, timeout=10):
    """Wait for the CLI while draining redraws so the PTY cannot back up."""
    deadline = time.monotonic() + timeout
    while process.poll() is None:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            fail("CLI did not exit after Ctrl-C", process, output, transcript)
        readable, _, _ = select.select([master], [], [], min(0.25, remaining))
        if readable:
            try:
                chunk = os.read(master, 65536)
                if chunk:
                    output += chunk
            except OSError:
                # Linux PTYs report EIO at EOF. Keep polling the child so a
                # genuine failure still reaches the timeout above.
                pass
    process.wait()
    return output


def main():
    if len(sys.argv) not in (4, 5):
        raise SystemExit(
            "usage: cli_pty_driver.py BINARY TRANSCRIPT create WORKSPACE | restore [TITLE] | picker | slash | resize | scroll GATE | agent WORKSPACE | exit"
        )
    binary, transcript_name, mode = sys.argv[1:4]
    transcript = os.path.abspath(transcript_name)
    master, slave = pty.openpty()
    rows, cols = (20, 120) if mode == "scroll" else (40, 140)
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
    environment = os.environ.copy()
    environment["TERM"] = "xterm-256color"
    command = [binary]
    if mode == "restore":
        title = sys.argv[4] if len(sys.argv) == 5 else "Terminal"
        command.append(f"--resume={title}")
    elif mode in ("picker", "slash", "resize", "scroll", "agent"):
        command.append("--resume=Terminal")
    process = subprocess.Popen(
        command, stdin=slave, stdout=slave, stderr=slave, env=environment, close_fds=True
    )
    os.close(slave)
    output = b""
    try:
        output = wait_for(b"Message S-Code", process, master, output, transcript)
        if mode == "create":
            if len(sys.argv) != 5:
                fail("create mode requires a workspace URI", process, output, transcript)
            output = wait_for(b"Terminal Team Session", process, master, output, transcript)
        elif mode == "restore":
            title = sys.argv[4].encode("utf-8") if len(sys.argv) == 5 else b"Terminal"
            output = wait_for(title, process, master, output, transcript)
        elif mode == "agent":
            if len(sys.argv) != 5:
                fail("agent mode requires a workspace path", process, output, transcript)
            workspace = sys.argv[4]
            os.write(master, b"change the tracked file\r")
            output = wait_for(b"Approval required", process, master, output, transcript, timeout=20)
            output = wait_for(b"Enter confirm", process, master, output, transcript, timeout=20)
            # Approval defaults to Reject. Right wraps to Allow once; send the
            # keys separately so the PTY cannot coalesce escape sequences.
            selection_start = len(output)
            os.write(master, b"\x1b[C")
            output = wait_for(
                b"Allow once",
                process,
                master,
                output,
                transcript,
                start=selection_start,
            )
            os.write(master, b"\r")
            output = wait_for(b"write", process, master, output, transcript, timeout=20)
            # Ratatui redraws only the changed suffix of the status line, so
            # "completed" commonly arrives as the stable suffix "ompleted".
            output = wait_for(b"ompleted", process, master, output, transcript, timeout=20)
            changed = os.path.join(workspace, "tracked.txt")
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                with open(changed, "r", encoding="utf-8") as source:
                    if source.read() == "approved via cli\n":
                        break
                time.sleep(0.05)
            else:
                fail("approved patch was not applied", process, output, transcript)
            # Exercise diff only after the terminal-state event has reached the
            # CLI. The assistant text delta can precede that durable commit.
            os.write(master, b"/diff\r")
            output = wait_for(b"tracked.txt", process, master, output, transcript)
            # The diff body can render before the slash command is fully
            # dismissed. Wait for its completion status before entering the
            # next command so cold runners cannot merge the two interactions.
            output = wait_for(b"ded \xc2\xb7 manual", process, master, output, transcript)
            os.write(master, b"/undo\r")
            output = wait_for(b"Restored:", process, master, output, transcript)
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                with open(changed, "r", encoding="utf-8") as source:
                    if source.read() == "original\n":
                        break
                time.sleep(0.05)
            else:
                fail("Ctrl-U did not restore the tracked file", process, output, transcript)
            os.write(master, b"\x12")
            output = wait_for(b"Ctrl-R", process, master, output, transcript)
            os.write(master, b"tracked")
            output = wait_for(b"change the tracked file", process, master, output, transcript)
            os.write(master, b"\r")
            # Ratatui commonly redraws only the changed suffix of the status
            # line, so the stable bytes arrive as "y restored".
            output = wait_for(b"y restored", process, master, output, transcript)
            clear_start = len(output)
            os.write(master, b"\x03")
            output = wait_for(
                b"input cl", process, master, output, transcript, start=clear_start
            )
            paste_start = len(output)
            os.write(master, b"\x1b[200~first pasted line\nsecond pasted line\x1b[201~")
            output = wait_for(
                b"pasted input inserted without subm",
                process,
                master,
                output,
                transcript,
                start=paste_start,
            )
            clear_start = len(output)
            os.write(master, b"\x03")
            output = wait_for(
                b"input cl", process, master, output, transcript, start=clear_start
            )
        elif mode == "picker":
            os.write(master, b"/model\r")
            output = wait_for(b"Model picker", process, master, output, transcript)
            os.write(master, b"\r")
            output = wait_for(b": gpt-5", process, master, output, transcript)
            os.write(master, b"/permissions\r")
            output = wait_for(b"Permission picker", process, master, output, transcript)
            os.write(master, b"accept")
            output = wait_for(b"Accept edits", process, master, output, transcript)
            os.write(master, b"\r")
            output = wait_for(b"server policy", process, master, output, transcript)
            os.write(master, b"/permissions manual\r")
            output = wait_for(b"Manual", process, master, output, transcript)
            os.write(master, b"\r")
            time.sleep(0.5)
        elif mode == "slash":
            os.write(master, b"/")
            # The menu can be taller than a 24-row PTY. Wait for its visible
            # header instead of a description that may be below the viewport.
            output = wait_for(
                b"Commands ",
                process,
                master,
                output,
                transcript,
            )
            completion_start = len(output)
            os.write(master, b"\x1b[B")
            os.write(master, b"\t")
            # Ratatui preserves the unchanged "s" from the previous
            # "slash command" status and redraws the new text around it.
            # The stable completion suffix therefore arrives separately.
            output = wait_for(
                b"ume selected",
                process,
                master,
                output,
                transcript,
                start=completion_start,
            )
            clear_start = len(output)
            os.write(master, b"\x03")
            output = wait_for(
                b"input cl", process, master, output, transcript, start=clear_start
            )
        elif mode == "resize":
            fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
            time.sleep(0.75)
            os.write(master, b"/status\r")
            output = wait_for(b"Connected", process, master, output, transcript)
            fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 160, 0, 0))
            time.sleep(0.75)
            os.write(master, b"/help\r")
            output = wait_for(b"Commands", process, master, output, transcript)
        elif mode == "scroll":
            if len(sys.argv) != 5:
                fail("scroll mode requires a fixture gate", process, output, transcript)
            gate = sys.argv[4]
            os.write(master, b"stream terminal viewport\r")
            output = wait_for(
                b"PHASE_ONE_TAIL", process, master, output, transcript, timeout=20
            )

            os.write(master, b"\x1b[5~")
            before_anchor = len(output)
            fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", 21, 120, 0, 0))
            output = wait_for(
                b"viewing earlier transcript",
                process,
                master,
                output,
                transcript,
                timeout=10,
                start=before_anchor,
            )
            detached_redraw = output[before_anchor:]
            visible_rows = [
                f"VIEWPORT_ROW_{index:03}".encode()
                for index in range(80)
                if f"VIEWPORT_ROW_{index:03}".encode() in detached_redraw
            ]
            if not visible_rows:
                fail(
                    "scrolling up did not expose an earlier viewport row",
                    process,
                    output,
                    transcript,
                )
            anchor = visible_rows[len(visible_rows) // 2]

            with open(gate, "w", encoding="utf-8") as target:
                target.write("continue\n")
            completion_start = len(output)
            output = wait_for(
                b"ompleted",
                process,
                master,
                output,
                transcript,
                timeout=20,
                start=completion_start,
            )
            after_completion = len(output)
            fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", 22, 120, 0, 0))
            output = wait_for(
                anchor,
                process,
                master,
                output,
                transcript,
                timeout=10,
                start=after_completion,
            )
            completion_redraw = output[after_completion:]
            if b"FINAL_STREAM_TAIL" in completion_redraw:
                fail(
                    "detached viewport unexpectedly jumped to the final stream row",
                    process,
                    output,
                    transcript,
                )

            for _ in range(20):
                os.write(master, b"\x1b[6~")
                time.sleep(0.02)
            tail_redraw_start = len(output)
            fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", 23, 120, 0, 0))
            output = wait_for(
                b"FINAL_STREAM_TAIL",
                process,
                master,
                output,
                transcript,
                timeout=10,
                start=tail_redraw_start,
            )
            output = wait_for(
                b"following latest transcript",
                process,
                master,
                output,
                transcript,
                timeout=10,
                start=tail_redraw_start,
            )
        elif mode == "exit":
            os.write(master, b"exit\r")
            output = wait_for_exit(process, master, output, transcript)
        else:
            fail(f"unknown mode: {mode}", process, output, transcript)
        if mode != "exit":
            os.write(master, b"\x03")
            output = wait_for_exit(process, master, output, transcript)
        while True:
            readable, _, _ = select.select([master], [], [], 0)
            if not readable:
                break
            try:
                chunk = os.read(master, 65536)
                if not chunk:
                    break
                output += chunk
            except OSError:
                break
        with open(transcript, "wb") as target:
            target.write(output)
        if process.returncode != 0:
            raise SystemExit(f"CLI exited with {process.returncode}")
    finally:
        os.close(master)


if __name__ == "__main__":
    main()
