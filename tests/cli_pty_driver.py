#!/usr/bin/env python3
import base64
import fcntl
import os
import pty
import re
import select
import struct
import subprocess
import sys
import termios
import time


# Ratatui writes differential screen updates, so raw-output membership cannot
# prove where text ended up. This models the cursor/control subset emitted by
# the Crossterm backend and lets the scroll test compare the visible rows.
class TerminalScreen:
    def __init__(self, rows, cols):
        self.rows = rows
        self.cols = cols
        self.cells = [[" "] * cols for _ in range(rows)]
        self.row = 0
        self.col = 0
        self.saved = (0, 0)
        self.mode = "normal"
        self.sequence = bytearray()
        self.utf8_remaining = 0
        self.output_offset = 0

    def feed_new(self, output):
        chunk = output[self.output_offset :]
        self.output_offset = len(output)
        for byte in chunk:
            self.feed_byte(byte)

    def feed_byte(self, byte):
        if self.mode == "normal":
            if self.utf8_remaining:
                self.utf8_remaining -= 1
            elif byte == 0x1B:
                self.mode = "escape"
            elif byte == 0x0D:
                self.col = 0
            elif byte == 0x0A:
                self.row = min(self.row + 1, self.rows - 1)
            elif byte == 0x08:
                self.col = max(0, self.col - 1)
            elif byte == 0x09:
                self.col = min(self.cols, ((self.col // 8) + 1) * 8)
            elif 0xC0 <= byte < 0xF8:
                self.utf8_remaining = 1 if byte < 0xE0 else 2 if byte < 0xF0 else 3
                if self.row < self.rows and self.col < self.cols:
                    self.cells[self.row][self.col] = "?"
                    self.col += 1
            elif 0x20 <= byte < 0x7F and self.row < self.rows and self.col < self.cols:
                self.cells[self.row][self.col] = chr(byte)
                self.col += 1
            return
        if self.mode == "escape":
            if byte == ord("["):
                self.mode = "csi"
                self.sequence.clear()
            elif byte == ord("]"):
                self.mode = "osc"
            elif byte in (ord("("), ord(")")):
                self.mode = "charset"
            elif byte == ord("7"):
                self.saved = (self.row, self.col)
                self.mode = "normal"
            elif byte == ord("8"):
                self.row, self.col = self.saved
                self.mode = "normal"
            else:
                self.mode = "normal"
            return
        if self.mode == "charset":
            self.mode = "normal"
            return
        if self.mode == "osc":
            if byte == 0x07:
                self.mode = "normal"
            elif byte == 0x1B:
                self.mode = "osc_escape"
            return
        if self.mode == "osc_escape":
            self.mode = "normal" if byte == ord("\\") else "osc"
            return
        if self.mode == "csi":
            if 0x40 <= byte <= 0x7E:
                self.apply_csi(chr(byte), self.sequence.decode("ascii", errors="ignore"))
                self.mode = "normal"
            else:
                self.sequence.append(byte)

    def apply_csi(self, command, raw_parameters):
        private = raw_parameters[:1] in "?><!"
        raw_parameters = raw_parameters.lstrip("?><!")
        parameters = []
        for value in raw_parameters.split(";"):
            try:
                parameters.append(int(value) if value else 0)
            except ValueError:
                parameters.append(0)

        def parameter(index, default=1):
            if index >= len(parameters) or parameters[index] == 0:
                return default
            return parameters[index]

        if command in ("H", "f"):
            self.row = min(self.rows - 1, parameter(0) - 1)
            self.col = min(self.cols, parameter(1) - 1)
        elif command == "A":
            self.row = max(0, self.row - parameter(0))
        elif command == "B":
            self.row = min(self.rows - 1, self.row + parameter(0))
        elif command == "C":
            self.col = min(self.cols, self.col + parameter(0))
        elif command == "D":
            self.col = max(0, self.col - parameter(0))
        elif command == "E":
            self.row = min(self.rows - 1, self.row + parameter(0))
            self.col = 0
        elif command == "F":
            self.row = max(0, self.row - parameter(0))
            self.col = 0
        elif command == "G":
            self.col = min(self.cols, parameter(0) - 1)
        elif command == "d":
            self.row = min(self.rows - 1, parameter(0) - 1)
        elif command == "J":
            mode = parameter(0, 0)
            if mode in (2, 3):
                self.cells = [[" "] * self.cols for _ in range(self.rows)]
            elif mode == 0:
                self.cells[self.row][self.col :] = [" "] * (self.cols - self.col)
                for row in range(self.row + 1, self.rows):
                    self.cells[row] = [" "] * self.cols
        elif command == "K":
            mode = parameter(0, 0)
            if mode == 0:
                self.cells[self.row][self.col :] = [" "] * (self.cols - self.col)
            elif mode == 1:
                self.cells[self.row][: self.col + 1] = [" "] * (self.col + 1)
            elif mode == 2:
                self.cells[self.row] = [" "] * self.cols
        elif command == "X":
            count = min(parameter(0), self.cols - self.col)
            self.cells[self.row][self.col : self.col + count] = [" "] * count
        elif command == "s" and not private:
            self.saved = (self.row, self.col)
        elif command == "u" and not private:
            self.row, self.col = self.saved

    def text(self):
        return "\n".join("".join(row) for row in self.cells)


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


def read_for(process, master, output, transcript, seconds=0.35):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if process.poll() is not None:
            fail("CLI exited while draining a redraw", process, output, transcript)
        remaining = deadline - time.monotonic()
        readable, _, _ = select.select(
            [master], [], [], max(0.0, min(0.05, remaining))
        )
        if not readable:
            continue
        try:
            chunk = os.read(master, 65536)
            if chunk:
                output += chunk
        except OSError:
            pass
    return output


def release_gate(path):
    with open(path, "w", encoding="utf-8") as target:
        target.write("continue\n")


def sgr_mouse(button, column, row, release=False):
    terminator = "m" if release else "M"
    return f"\x1b[<{button};{column + 1};{row + 1}{terminator}".encode("ascii")


def osc52(text):
    encoded = base64.b64encode(text.encode("utf-8"))
    return b"\x1b]52;c;" + encoded + b"\x07"


def wait_for_screen(needle, screen, process, master, output, transcript, timeout=10):
    deadline = time.monotonic() + timeout
    while needle not in screen.text():
        if process.poll() is not None:
            fail(f"CLI exited before rendering {needle!r}", process, output, transcript)
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            fail(f"timed out waiting for visible {needle!r}", process, output, transcript)
        output = read_for(
            process, master, output, transcript, seconds=min(0.1, remaining)
        )
        screen.feed_new(output)
    return output


def wait_for_screen_without(
    needle, screen, process, master, output, transcript, timeout=10
):
    initial_offset = screen.output_offset
    deadline = time.monotonic() + timeout
    while screen.output_offset == initial_offset or needle in screen.text():
        if process.poll() is not None:
            fail(f"CLI exited before clearing {needle!r}", process, output, transcript)
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            fail(f"timed out waiting for cleared {needle!r}", process, output, transcript)
        output = read_for(
            process, master, output, transcript, seconds=min(0.1, remaining)
        )
        screen.feed_new(output)
    return output


def wait_for_cursor(
    expected, screen, process, master, output, transcript, timeout=10
):
    initial_offset = screen.output_offset
    deadline = time.monotonic() + timeout
    while screen.output_offset == initial_offset or (screen.row, screen.col) != expected:
        if process.poll() is not None:
            fail(
                f"CLI exited before moving the cursor to {expected}",
                process,
                output,
                transcript,
            )
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            fail(
                "timed out waiting for cursor movement: "
                f"expected {expected}, got {(screen.row, screen.col)}",
                process,
                output,
                transcript,
            )
        output = read_for(
            process, master, output, transcript, seconds=min(0.1, remaining)
        )
        screen.feed_new(output)
    return output


def marker_rows(screen, pattern):
    visible_rows = []
    for screen_row, text in enumerate(screen.text().splitlines()):
        for match in re.finditer(pattern, text):
            visible_rows.append((int(match.group(1)), screen_row))
    return visible_rows


def visible_viewport_rows(screen, process, output, transcript):
    visible_rows = marker_rows(screen, r"VIEWPORT_ROW_(\d{3})")
    if len(visible_rows) < 3:
        fail(
            f"detached redraw exposed too few viewport rows: {visible_rows}",
            process,
            output,
            transcript,
        )
    row_ids = [row_id for row_id, _ in visible_rows]
    if row_ids != list(range(row_ids[0], row_ids[-1] + 1)):
        fail(
            f"detached redraw rows were not contiguous: {visible_rows}",
            process,
            output,
            transcript,
        )
    return visible_rows


def contiguous_marker_rows(screen, pattern):
    visible_rows = marker_rows(screen, pattern)
    row_ids = [row_id for row_id, _ in visible_rows]
    if len(row_ids) < 3 or row_ids != list(range(row_ids[0], row_ids[-1] + 1)):
        return []
    return visible_rows


def wait_for_marker_rows(
    pattern,
    predicate,
    description,
    screen,
    process,
    master,
    output,
    transcript,
    timeout=10,
):
    initial_offset = screen.output_offset
    deadline = time.monotonic() + timeout
    latest = contiguous_marker_rows(screen, pattern)
    while screen.output_offset == initial_offset or not latest or not predicate(latest):
        if process.poll() is not None:
            fail(
                f"CLI exited before {description}: {latest}",
                process,
                output,
                transcript,
            )
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            fail(
                f"timed out waiting for {description}: {latest}",
                process,
                output,
                transcript,
            )
        output = read_for(
            process, master, output, transcript, seconds=min(0.1, remaining)
        )
        screen.feed_new(output)
        latest = contiguous_marker_rows(screen, pattern)
    return output, latest


def flood_mouse_moves_until_marker_rows(
    pattern,
    predicate,
    description,
    screen,
    process,
    master,
    output,
    transcript,
    column,
    row,
    timeout=2,
):
    deadline = time.monotonic() + timeout
    next_move = time.monotonic()
    latest = contiguous_marker_rows(screen, pattern)
    move_count = 0
    while not latest or not predicate(latest):
        if process.poll() is not None:
            fail(
                f"CLI exited before {description}: {latest}",
                process,
                output,
                transcript,
            )
        now = time.monotonic()
        if now >= next_move:
            # Cb 35 is an SGR motion event with no pressed button. Alternate
            # columns so this is genuine unrelated pointer activity, not a
            # repeated drag update for the selection owner.
            move_column = column - move_count % 2
            os.write(master, sgr_mouse(35, move_column, row))
            move_count += 1
            next_move = now + 0.01
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            fail(
                f"timed out waiting for {description} while flooding "
                f"{move_count} mouse moves: {latest}",
                process,
                output,
                transcript,
            )
        readable, _, _ = select.select(
            [master], [], [], min(0.01, max(0.0, next_move - time.monotonic()), remaining)
        )
        if readable:
            try:
                chunk = os.read(master, 65536)
                if not chunk:
                    fail(
                        f"PTY closed before {description}",
                        process,
                        output,
                        transcript,
                    )
                output += chunk
                screen.feed_new(output)
            except OSError:
                pass
        latest = contiguous_marker_rows(screen, pattern)
    return output, latest


def visible_row(screen, needle, process, output, transcript):
    matches = [
        row
        for row, text in enumerate(screen.text().splitlines())
        if needle in text
    ]
    if len(matches) != 1:
        fail(
            f"expected one visible {needle!r} row, found {matches}",
            process,
            output,
            transcript,
        )
    return matches[0]


def main():
    if len(sys.argv) not in (4, 5):
        raise SystemExit(
            "usage: cli_pty_driver.py BINARY TRANSCRIPT create WORKSPACE | restore [TITLE] | picker | slash | resize | composer | scroll GATE | selection | agent WORKSPACE | exit"
        )
    binary, transcript_name, mode = sys.argv[1:4]
    transcript = os.path.abspath(transcript_name)
    master, slave = pty.openpty()
    if mode in ("scroll", "selection"):
        rows, cols = 20, 120
    elif mode == "composer":
        rows, cols = 20, 40
    else:
        rows, cols = 40, 140
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
    environment = os.environ.copy()
    environment["TERM"] = "xterm-256color"
    if mode == "selection":
        # Exercise the terminal clipboard request without mutating the host
        # clipboard or inheriting a developer's tmux session.
        environment["SSH_CONNECTION"] = "s-code-e2e"
        environment.pop("TMUX", None)
    command = [binary]
    if mode == "restore":
        title = sys.argv[4] if len(sys.argv) == 5 else "Terminal"
        command.append(f"--resume={title}")
    elif mode in (
        "picker",
        "slash",
        "resize",
        "composer",
        "scroll",
        "selection",
        "agent",
    ):
        command.append("--resume=" + environment.get("S_CODE_E2E_SESSION_ID", "Terminal"))
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
            # Reconstruct the visible screen: Ratatui may split "completed"
            # across several cursor updates when replacing an animated status.
            screen = TerminalScreen(rows, cols)
            screen.feed_new(output)
            output = wait_for_screen(
                "completed", screen, process, master, output, transcript, timeout=20
            )
            changed = os.path.join(workspace, "tracked.txt")
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                with open(changed, "r", encoding="utf-8") as source:
                    if source.read() == "approved via cli\n":
                        break
                time.sleep(0.05)
            else:
                fail("approved patch was not applied", process, output, transcript)
            # Privacy is a separate page, backed by the same durable records as Web.
            privacy_start = len(output)
            os.write(master, b"/privacy\r")
            output = wait_for(b"FILES & MODEL REQUESTS", process, master, output, transcript, start=privacy_start)
            privacy_screen = TerminalScreen(rows, cols)
            output = wait_for_screen("Accepted by endpoint", privacy_screen, process, master, output, transcript)
            # The privacy page owns input while open. Paste and ordinary keys
            # must not leak into the composer when the main UI is restored.
            os.write(master, b"\x1b[200~must-not-enter-composer\x1b[201~x")
            close_start = len(output)
            os.write(master, b"q")
            output = wait_for(b"manual", process, master, output, transcript, start=close_start)
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

            # Modifier-aware reporting distinguishes Ctrl-I from Tab. Preserve
            # the legacy Ctrl-I alias through the shared input dispatch.
            menu_start = len(output)
            os.write(master, b"/res")
            output = wait_for(
                b"Commands ",
                process,
                master,
                output,
                transcript,
                start=menu_start,
            )
            completion_start = len(output)
            os.write(master, b"\x1b[105;5u")
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

            # Ctrl-M likewise arrived as Enter before enhanced reporting.
            screen = TerminalScreen(rows, cols)
            screen.feed_new(output)
            os.write(master, b"/keymap emacs\x1b[109;5u")
            output = wait_for_screen(
                "keymap: emacs", screen, process, master, output, transcript
            )

            # Ctrl-[ was indistinguishable from Escape in legacy reporting and
            # remains the conventional Vim escape chord.
            os.write(master, b"/")
            output = wait_for_screen(
                "Commands ", screen, process, master, output, transcript
            )
            os.write(master, b"\x1b[91;5u")
            output = wait_for_screen(
                "slash command menu closed",
                screen,
                process,
                master,
                output,
                transcript,
            )
            os.write(master, b"\x03")
            output = wait_for_screen(
                "input cleared", screen, process, master, output, transcript
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
        elif mode == "composer":
            if b"\x1b[>1u" not in output:
                fail(
                    "CLI did not enable modifier-aware keyboard reporting",
                    process,
                    output,
                    transcript,
                )
            screen = TerminalScreen(rows, cols)
            screen.feed_new(output)

            os.write(master, b"first line\x1b[13;2usecond line")
            output = wait_for_screen(
                "second line", screen, process, master, output, transcript
            )
            composer_row = visible_row(
                screen, "Message S-Code", process, output, transcript
            )
            first_row = visible_row(screen, "first line", process, output, transcript)
            second_row = visible_row(screen, "second line", process, output, transcript)
            if (first_row, second_row) != (composer_row + 1, composer_row + 2):
                fail(
                    "Shift-Enter did not create a visible composer line",
                    process,
                    output,
                    transcript,
                )

            second_cursor = (screen.row, screen.col)
            first_line = screen.text().splitlines()[first_row]
            expected_up = (
                first_row,
                first_line.index("first line") + len("first line"),
            )
            os.write(master, b"\x1b[A")
            output = wait_for_cursor(
                expected_up, screen, process, master, output, transcript
            )
            os.write(master, b"\x1b[B")
            output = wait_for_cursor(
                second_cursor, screen, process, master, output, transcript
            )

            os.write(master, b"\x03")
            output = wait_for_screen_without(
                "first line", screen, process, master, output, transcript
            )

            wrapped = b"alpha beta gamma delta epsilon zeta eta theta WRAP_TAIL"
            os.write(master, wrapped)
            output = wait_for_screen(
                "WRAP_TAIL", screen, process, master, output, transcript
            )
            output = read_for(process, master, output, transcript, seconds=0.2)
            screen.feed_new(output)
            composer_row = visible_row(
                screen, "Message S-Code", process, output, transcript
            )
            tail_row = visible_row(screen, "WRAP_TAIL", process, output, transcript)
            if tail_row <= composer_row + 1:
                fail(
                    "long composer input did not wrap onto a visible later row",
                    process,
                    output,
                    transcript,
                )
            tail_line = screen.text().splitlines()[tail_row]
            expected_cursor = (tail_row, tail_line.index("WRAP_TAIL") + len("WRAP_TAIL"))
            if (screen.row, screen.col) != expected_cursor:
                fail(
                    "wrapped composer cursor did not follow the rendered text: "
                    f"expected {expected_cursor}, got {(screen.row, screen.col)}",
                    process,
                    output,
                    transcript,
                )

            os.write(master, b"\x1b[A")
            expected_wrapped_up = (tail_row - 1, expected_cursor[1])
            output = wait_for_cursor(
                expected_wrapped_up, screen, process, master, output, transcript
            )
            os.write(master, b"\x1b[B")
            output = wait_for_cursor(
                expected_cursor, screen, process, master, output, transcript
            )

            os.write(master, b"\x03")
            output = wait_for_screen_without(
                "WRAP_TAIL", screen, process, master, output, transcript
            )

            os.write(
                master,
                b"ROW_ONE\x1b[13;2uROW_TWO\x1b[13;2uROW_THREE\x1b[13;2uROW_FOUR",
            )
            output = wait_for_screen(
                "ROW_FOUR", screen, process, master, output, transcript
            )
            composer_row = visible_row(
                screen, "Message S-Code", process, output, transcript
            )
            lines = screen.text().splitlines()
            visible_composer = lines[composer_row + 1 : composer_row + 5]
            if not (
                "ROW_ONE" in visible_composer[0]
                and "ROW_TWO" in visible_composer[1]
                and "ROW_THREE" in visible_composer[2]
                and "ROW_FOUR" in visible_composer[3]
            ):
                fail(
                    f"composer did not grow across four input rows: {visible_composer}",
                    process,
                    output,
                    transcript,
                )

            os.write(
                master,
                b"\x1b[13;2uROW_FIVE\x1b[13;2uROW_SIX\x1b[13;2uROW_SEVEN",
            )
            output = wait_for_screen(
                "ROW_SEVEN", screen, process, master, output, transcript
            )
            composer_row = visible_row(
                screen, "Message S-Code", process, output, transcript
            )
            lines = screen.text().splitlines()
            visible_composer = lines[composer_row + 1 : composer_row + 7]
            if not (
                "ROW_ONE" not in "\n".join(visible_composer)
                and all(
                    marker in visible_composer[index]
                    for index, marker in enumerate(
                        [
                            "ROW_TWO",
                            "ROW_THREE",
                            "ROW_FOUR",
                            "ROW_FIVE",
                            "ROW_SIX",
                            "ROW_SEVEN",
                        ]
                    )
                )
            ):
                fail(
                    f"capped composer did not follow the cursor: {visible_composer}",
                    process,
                    output,
                    transcript,
                )

            os.write(master, b"\x03")
            output = wait_for_screen_without(
                "ROW_SEVEN", screen, process, master, output, transcript
            )
        elif mode == "scroll":
            if len(sys.argv) != 5:
                fail("scroll mode requires a fixture gate", process, output, transcript)
            gate = sys.argv[4]
            os.write(master, b"stream terminal viewport\r")
            output = wait_for(
                b"PHASE_ONE_TAIL", process, master, output, transcript, timeout=20
            )

            screen = TerminalScreen(20, 120)
            screen.feed_new(output)
            os.write(master, b"\x1b[5~")
            output = wait_for_screen(
                "viewing earlier transcript",
                screen,
                process,
                master,
                output,
                transcript,
                timeout=10,
            )
            baseline_rows = visible_viewport_rows(
                screen, process, output, transcript
            )
            if baseline_rows[0][0] == 0:
                fail(
                    "scrolling up did not place the viewport inside the streamed response",
                    process,
                    output,
                    transcript,
                )
            if "FINAL_STREAM_TAIL" in screen.text():
                fail(
                    "detached baseline unexpectedly showed the stream tail",
                    process,
                    output,
                    transcript,
                )

            release_gate(f"{gate}.phase-two")
            # PageUp detached by 10 rows; phase two appends 16 rendered rows.
            output = wait_for_screen(
                "history -26",
                screen,
                process,
                master,
                output,
                transcript,
                timeout=20,
            )
            streaming_rows = visible_viewport_rows(
                screen, process, output, transcript
            )
            if streaming_rows != baseline_rows:
                fail(
                    "detached viewport moved during streaming: "
                    f"{baseline_rows} -> {streaming_rows}",
                    process,
                    output,
                    transcript,
                )
            if "FINAL_STREAM_TAIL" in screen.text():
                fail(
                    "detached viewport unexpectedly showed the stream tail",
                    process,
                    output,
                    transcript,
                )

            release_gate(f"{gate}.finish")
            output = wait_for_screen(
                "completed", screen, process, master, output, transcript, timeout=20
            )
            for _ in range(4):
                output = read_for(process, master, output, transcript)
                screen.feed_new(output)
                completed_rows = visible_viewport_rows(
                    screen, process, output, transcript
                )
                if completed_rows != baseline_rows:
                    fail(
                        "detached viewport moved after completion: "
                        f"{baseline_rows} -> {completed_rows}",
                        process,
                        output,
                        transcript,
                    )
                if "FINAL_STREAM_TAIL" in screen.text():
                    fail(
                        "detached viewport unexpectedly jumped to the final stream row",
                        process,
                        output,
                        transcript,
                    )

            for _ in range(20):
                os.write(master, b"\x1b[6~")
                time.sleep(0.02)
            output = wait_for_screen(
                "FINAL_STREAM_TAIL",
                screen,
                process,
                master,
                output,
                transcript,
                timeout=10,
            )
            output = wait_for_screen(
                "following latest transcript",
                screen,
                process,
                master,
                output,
                transcript,
                timeout=10,
            )
            # Canonical order places this reasoning summary above the tail.
            if "Checked the terminal viewport fixture." in screen.text():
                fail(
                    "returning to the tail did not apply the canonical transcript order",
                    process,
                    output,
                    transcript,
                )
        elif mode == "selection":
            screen = TerminalScreen(rows, cols)
            screen.feed_new(output)
            output = wait_for_screen(
                "FINAL_STREAM_TAIL",
                screen,
                process,
                master,
                output,
                transcript,
                timeout=20,
            )
            if b"\x1b[?1002h" not in output or b"\x1b[?1006h" not in output:
                fail(
                    "CLI did not enable button-drag and SGR mouse reporting",
                    process,
                    output,
                    transcript,
            )

            baseline = visible_viewport_rows(screen, process, output, transcript)
            # Keep exact-copy away from the viewport boundaries so this
            # scenario remains independent of autoscroll behavior.
            marker_id, marker_row = baseline[len(baseline) // 2]
            marker = f"VIEWPORT_ROW_{marker_id:03}"
            marker_column = screen.text().splitlines()[marker_row].index(marker)
            os.write(master, sgr_mouse(64, marker_column, marker_row))
            output = wait_for_screen(
                "viewing earlier transcript",
                screen,
                process,
                master,
                output,
                transcript,
            )
            moved = visible_viewport_rows(screen, process, output, transcript)
            moved_row = dict(moved).get(marker_id)
            if moved_row != marker_row + 3:
                fail(
                    f"mouse wheel did not move the transcript by three rows: "
                    f"{marker_row} -> {moved_row}",
                    process,
                    output,
                    transcript,
                )

            os.write(master, sgr_mouse(65, marker_column, moved_row))
            output = wait_for_screen(
                "following latest transcript",
                screen,
                process,
                master,
                output,
                transcript,
            )
            restored = dict(visible_viewport_rows(screen, process, output, transcript))
            if restored.get(marker_id) != marker_row:
                fail(
                    "mouse wheel round trip did not restore the viewport",
                    process,
                    output,
                    transcript,
                )

            # A held edge must keep autoscrolling even when unrelated input
            # arrives faster than the 50 ms scheduler period. The initial
            # outside Drag advances one row immediately; requiring three rows
            # proves that later scheduler ticks survive the MouseMoved flood.
            scheduler_rows = visible_viewport_rows(
                screen, process, output, transcript
            )
            scheduler_id, scheduler_row = scheduler_rows[len(scheduler_rows) // 2]
            scheduler_marker = f"VIEWPORT_ROW_{scheduler_id:03}"
            scheduler_column = screen.text().splitlines()[scheduler_row].index(
                scheduler_marker
            )
            scheduler_first = scheduler_rows[0][0]
            os.write(master, sgr_mouse(0, scheduler_column, scheduler_row))
            output = wait_for_screen(
                "selecting transcript",
                screen,
                process,
                master,
                output,
                transcript,
            )
            os.write(master, sgr_mouse(32, scheduler_column, 2))
            output, _ = flood_mouse_moves_until_marker_rows(
                r"VIEWPORT_ROW_(\d{3})",
                lambda rows: rows[0][0] <= scheduler_first - 3,
                "held-edge autoscroll to advance multiple transcript rows",
                screen,
                process,
                master,
                output,
                transcript,
                cols - 1,
                rows - 1,
            )

            # Enhanced Ctrl-[ is an unambiguous Escape. End this synthetic
            # drag and return to the tail before the exact wheel/copy case.
            os.write(master, b"\x1b[91;5u")
            output = wait_for_screen(
                "transcript selection cleared",
                screen,
                process,
                master,
                output,
                transcript,
            )
            os.write(master, b"\x1b[6~" * 4)
            output = wait_for_screen(
                "following latest transcript",
                screen,
                process,
                master,
                output,
                transcript,
            )
            output = wait_for_screen(
                "FINAL_STREAM_TAIL",
                screen,
                process,
                master,
                output,
                transcript,
            )

            # Move the viewport into one uninterrupted fixture range. Keeping
            # both endpoints below row 80 makes the expected copied bytes
            # independent of the phase-tail sentinels between stream chunks.
            active_rows = visible_viewport_rows(screen, process, output, transcript)
            while active_rows[len(active_rows) // 2][0] > 70:
                previous_first = active_rows[0][0]
                wheel_id, wheel_row = active_rows[len(active_rows) // 2]
                wheel_marker = f"VIEWPORT_ROW_{wheel_id:03}"
                wheel_column = screen.text().splitlines()[wheel_row].index(wheel_marker)
                os.write(master, sgr_mouse(64, wheel_column, wheel_row))
                output, active_rows = wait_for_marker_rows(
                    r"VIEWPORT_ROW_(\d{3})",
                    lambda rows, before=previous_first: rows[0][0] < before,
                    "the transcript viewport to move to an earlier fixture range",
                    screen,
                    process,
                    master,
                    output,
                    transcript,
                )

            origin_candidates = [
                item for item in active_rows if 2 <= item[0] and item[0] + 3 < 80
            ]
            if not origin_candidates:
                fail(
                    f"no safe transcript selection origin was visible: {active_rows}",
                    process,
                    output,
                    transcript,
                )
            origin_id, origin_row = origin_candidates[len(origin_candidates) // 2]
            origin = f"VIEWPORT_ROW_{origin_id:03}"
            origin_column = screen.text().splitlines()[origin_row].index(origin)
            origin_last = origin_column + len(origin) - 1
            os.write(master, sgr_mouse(0, origin_column, origin_row))
            output = wait_for_screen(
                "selecting transcript",
                screen,
                process,
                master,
                output,
                transcript,
            )
            output = read_for(process, master, output, transcript, seconds=0.15)
            screen.feed_new(output)
            held_rows = dict(visible_viewport_rows(screen, process, output, transcript))
            if held_rows.get(origin_id) != origin_row:
                fail(
                    "holding a transcript click scrolled before a drag",
                    process,
                    output,
                    transcript,
                )

            # Establish a non-empty selection, then keep that same mouse
            # owner alive while the wheel moves the transcript in both
            # directions. The intermediate drag is backwards; the final drag
            # crosses the origin again and proves direction reversal.
            os.write(master, sgr_mouse(32, origin_last, origin_row))
            before_up = visible_viewport_rows(screen, process, output, transcript)[0][0]
            os.write(master, sgr_mouse(64, origin_last, origin_row))
            output, earlier_rows = wait_for_marker_rows(
                r"VIEWPORT_ROW_(\d{3})",
                lambda rows: rows[0][0] < before_up,
                "an active transcript selection to wheel upward",
                screen,
                process,
                master,
                output,
                transcript,
            )
            older = [item for item in earlier_rows if item[0] < origin_id]
            if not older:
                fail(
                    f"wheel-up exposed no row before selection origin {origin_id}: "
                    f"{earlier_rows}",
                    process,
                    output,
                    transcript,
                )
            older_id, older_row = older[len(older) // 2]
            older_marker = f"VIEWPORT_ROW_{older_id:03}"
            older_column = screen.text().splitlines()[older_row].index(older_marker)
            os.write(master, sgr_mouse(32, older_column, older_row))

            target_id = origin_id + 3
            before_down = earlier_rows[0][0]
            os.write(master, sgr_mouse(65, older_column, older_row))
            output, later_rows = wait_for_marker_rows(
                r"VIEWPORT_ROW_(\d{3})",
                lambda rows: rows[0][0] > before_down,
                "an active transcript selection to reverse wheel direction",
                screen,
                process,
                master,
                output,
                transcript,
            )
            while target_id not in dict(later_rows):
                previous_first = later_rows[0][0]
                os.write(master, sgr_mouse(65, older_column, older_row))
                output, later_rows = wait_for_marker_rows(
                    r"VIEWPORT_ROW_(\d{3})",
                    lambda rows, before=previous_first: rows[0][0] > before,
                    "the transcript selection target to enter the viewport",
                    screen,
                    process,
                    master,
                    output,
                    transcript,
                )

            target_row = dict(later_rows)[target_id]
            target = f"VIEWPORT_ROW_{target_id:03}"
            target_column = screen.text().splitlines()[target_row].index(target)
            target_last = target_column + len(target) - 1
            os.write(master, sgr_mouse(32, target_last, target_row))
            copy_start = len(output)
            os.write(master, sgr_mouse(0, target_last, target_row, release=True))
            expected_transcript = "\n".join(
                f"VIEWPORT_ROW_{row_id:03}"
                for row_id in range(origin_id, target_id + 1)
            )
            output = wait_for(
                osc52(expected_transcript),
                process,
                master,
                output,
                transcript,
                start=copy_start,
            )
            output = wait_for_screen(
                "selection sent to the terminal clipboard",
                screen,
                process,
                master,
                output,
                transcript,
            )

            composer_lines = [f"COMPOSER_ROW_{index:02}" for index in range(12)]
            os.write(
                master,
                b"\x1b[200~"
                + b"\n".join(line.encode("ascii") for line in composer_lines)
                + b"\x1b[201~",
            )
            output = wait_for_screen(
                composer_lines[-1], screen, process, master, output, transcript
            )
            composer_rows = contiguous_marker_rows(screen, r"COMPOSER_ROW_(\d{2})")
            if not composer_rows or composer_rows[-1][0] != len(composer_lines) - 1:
                fail(
                    f"pasted composer did not render its tail: {composer_rows}",
                    process,
                    output,
                    transcript,
                )
            transcript_before_passive_scroll = visible_viewport_rows(
                screen, process, output, transcript
            )

            # With no button held, the pane under the pointer owns the wheel.
            # Scroll the pasted composer to both boundaries and verify that
            # neither movement nor an extra boundary event reaches transcript.
            while composer_rows[0][0] > 0:
                previous_first = composer_rows[0][0]
                hover_id, hover_row = composer_rows[len(composer_rows) // 2]
                hover_marker = composer_lines[hover_id]
                hover_column = screen.text().splitlines()[hover_row].index(hover_marker)
                os.write(master, sgr_mouse(64, hover_column, hover_row))
                output, composer_rows = wait_for_marker_rows(
                    r"COMPOSER_ROW_(\d{2})",
                    lambda visible, before=previous_first: visible[0][0] < before,
                    "the passively hovered composer to wheel upward",
                    screen,
                    process,
                    master,
                    output,
                    transcript,
                )
                if (
                    visible_viewport_rows(screen, process, output, transcript)
                    != transcript_before_passive_scroll
                ):
                    fail(
                        "passive composer wheel-up changed the transcript viewport",
                        process,
                        output,
                        transcript,
                    )

            top_composer_rows = composer_rows
            hover_id, hover_row = composer_rows[len(composer_rows) // 2]
            hover_marker = composer_lines[hover_id]
            hover_column = screen.text().splitlines()[hover_row].index(hover_marker)
            os.write(master, sgr_mouse(64, hover_column, hover_row))
            os.write(master, sgr_mouse(65, hover_column, hover_row))
            output, composer_rows = wait_for_marker_rows(
                r"COMPOSER_ROW_(\d{2})",
                lambda visible: visible[0][0] > top_composer_rows[0][0],
                "an opposite wheel event after the composer top boundary",
                screen,
                process,
                master,
                output,
                transcript,
            )
            if (
                visible_viewport_rows(screen, process, output, transcript)
                != transcript_before_passive_scroll
            ):
                fail(
                    "composer top-boundary wheel fell through to transcript",
                    process,
                    output,
                    transcript,
                )

            previous_first = composer_rows[0][0]
            hover_id, hover_row = composer_rows[len(composer_rows) // 2]
            hover_marker = composer_lines[hover_id]
            hover_column = screen.text().splitlines()[hover_row].index(hover_marker)
            os.write(master, sgr_mouse(64, hover_column, hover_row))
            output, composer_rows = wait_for_marker_rows(
                r"COMPOSER_ROW_(\d{2})",
                lambda visible, before=previous_first: visible[0][0] < before,
                "the composer to return to its top boundary",
                screen,
                process,
                master,
                output,
                transcript,
            )
            if composer_rows != top_composer_rows:
                fail(
                    f"composer did not return to its top boundary: {composer_rows}",
                    process,
                    output,
                    transcript,
                )

            while composer_rows[-1][0] < len(composer_lines) - 1:
                previous_first = composer_rows[0][0]
                hover_id, hover_row = composer_rows[len(composer_rows) // 2]
                hover_marker = composer_lines[hover_id]
                hover_column = screen.text().splitlines()[hover_row].index(hover_marker)
                os.write(master, sgr_mouse(65, hover_column, hover_row))
                output, composer_rows = wait_for_marker_rows(
                    r"COMPOSER_ROW_(\d{2})",
                    lambda visible, before=previous_first: visible[0][0] > before,
                    "the passively hovered composer to wheel downward",
                    screen,
                    process,
                    master,
                    output,
                    transcript,
                )
                if (
                    visible_viewport_rows(screen, process, output, transcript)
                    != transcript_before_passive_scroll
                ):
                    fail(
                        "passive composer wheel-down changed the transcript viewport",
                        process,
                        output,
                        transcript,
                    )

            bottom_composer_rows = composer_rows
            hover_id, hover_row = composer_rows[len(composer_rows) // 2]
            hover_marker = composer_lines[hover_id]
            hover_column = screen.text().splitlines()[hover_row].index(hover_marker)
            os.write(master, sgr_mouse(65, hover_column, hover_row))
            os.write(master, sgr_mouse(64, hover_column, hover_row))
            output, composer_rows = wait_for_marker_rows(
                r"COMPOSER_ROW_(\d{2})",
                lambda visible: visible[0][0] < bottom_composer_rows[0][0],
                "an opposite wheel event after the composer bottom boundary",
                screen,
                process,
                master,
                output,
                transcript,
            )
            if (
                visible_viewport_rows(screen, process, output, transcript)
                != transcript_before_passive_scroll
            ):
                fail(
                    "composer bottom-boundary wheel fell through to transcript",
                    process,
                    output,
                    transcript,
                )

            previous_first = composer_rows[0][0]
            hover_id, hover_row = composer_rows[len(composer_rows) // 2]
            hover_marker = composer_lines[hover_id]
            hover_column = screen.text().splitlines()[hover_row].index(hover_marker)
            os.write(master, sgr_mouse(65, hover_column, hover_row))
            output, composer_rows = wait_for_marker_rows(
                r"COMPOSER_ROW_(\d{2})",
                lambda visible, before=previous_first: visible[0][0] > before,
                "the composer to return to its bottom boundary",
                screen,
                process,
                master,
                output,
                transcript,
            )
            if composer_rows != bottom_composer_rows:
                fail(
                    f"composer did not return to its bottom boundary: {composer_rows}",
                    process,
                    output,
                    transcript,
                )

            composer_candidates = [
                item
                for item in composer_rows
                if item[0] >= 2 and item[0] + 2 < len(composer_lines)
            ]
            if not composer_candidates:
                fail(
                    f"no safe multiline composer selection origin was visible: "
                    f"{composer_rows}",
                    process,
                    output,
                    transcript,
                )
            composer_id, composer_row = composer_candidates[
                len(composer_candidates) // 2
            ]
            composer_origin = composer_lines[composer_id]
            composer_column = screen.text().splitlines()[composer_row].index(composer_origin)
            composer_last = composer_column + len(composer_origin) - 1
            transcript_before_composer = visible_viewport_rows(
                screen, process, output, transcript
            )
            os.write(master, sgr_mouse(0, composer_column, composer_row))
            output = wait_for_screen(
                "selecting input", screen, process, master, output, transcript
            )
            os.write(master, sgr_mouse(32, composer_last, composer_row))

            composer_first = composer_rows[0][0]
            os.write(master, sgr_mouse(64, composer_last, composer_row))
            output, earlier_composer_rows = wait_for_marker_rows(
                r"COMPOSER_ROW_(\d{2})",
                lambda rows: rows[0][0] < composer_first,
                "an active composer selection to wheel upward",
                screen,
                process,
                master,
                output,
                transcript,
            )
            if (
                visible_viewport_rows(screen, process, output, transcript)
                != transcript_before_composer
            ):
                fail(
                    "composer-owned wheel motion changed the transcript viewport",
                    process,
                    output,
                    transcript,
                )
            older_composer = [
                item for item in earlier_composer_rows if item[0] < composer_id
            ]
            if not older_composer:
                fail(
                    f"wheel-up exposed no composer row before {composer_id}: "
                    f"{earlier_composer_rows}",
                    process,
                    output,
                    transcript,
                )
            older_composer_id, older_composer_row = older_composer[
                len(older_composer) // 2
            ]
            older_composer_marker = composer_lines[older_composer_id]
            older_composer_column = screen.text().splitlines()[
                older_composer_row
            ].index(older_composer_marker)
            os.write(
                master,
                sgr_mouse(32, older_composer_column, older_composer_row),
            )

            composer_target_id = composer_id + 2
            before_composer_down = earlier_composer_rows[0][0]
            os.write(
                master,
                sgr_mouse(65, older_composer_column, older_composer_row),
            )
            output, later_composer_rows = wait_for_marker_rows(
                r"COMPOSER_ROW_(\d{2})",
                lambda rows: rows[0][0] > before_composer_down,
                "an active composer selection to reverse wheel direction",
                screen,
                process,
                master,
                output,
                transcript,
            )
            while composer_target_id not in dict(later_composer_rows):
                previous_first = later_composer_rows[0][0]
                os.write(
                    master,
                    sgr_mouse(65, older_composer_column, older_composer_row),
                )
                output, later_composer_rows = wait_for_marker_rows(
                    r"COMPOSER_ROW_(\d{2})",
                    lambda rows, before=previous_first: rows[0][0] > before,
                    "the composer selection target to enter the viewport",
                    screen,
                    process,
                    master,
                    output,
                    transcript,
                )
            if (
                visible_viewport_rows(screen, process, output, transcript)
                != transcript_before_composer
            ):
                fail(
                    "composer wheel reversal changed the transcript viewport",
                    process,
                    output,
                    transcript,
                )

            composer_target_row = dict(later_composer_rows)[composer_target_id]
            composer_target = composer_lines[composer_target_id]
            composer_target_column = screen.text().splitlines()[composer_target_row].index(
                composer_target
            )
            composer_target_last = composer_target_column + len(composer_target) - 1
            os.write(
                master,
                sgr_mouse(32, composer_target_last, composer_target_row),
            )
            copy_start = len(output)
            os.write(
                master,
                sgr_mouse(
                    0,
                    composer_target_last,
                    composer_target_row,
                    release=True,
                ),
            )
            expected_composer = "\n".join(
                composer_lines[composer_id : composer_target_id + 1]
            )
            output = wait_for(
                osc52(expected_composer),
                process,
                master,
                output,
                transcript,
                start=copy_start,
            )
            output = wait_for_screen(
                "selection sent to the terminal clipboard",
                screen,
                process,
                master,
                output,
                transcript,
            )
            os.write(master, b"\x03")
            output = wait_for_screen(
                "input cleared", screen, process, master, output, transcript
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
        if mode == "composer" and b"\x1b[<1u" not in output:
            fail(
                "CLI did not restore keyboard reporting on exit",
                process,
                output,
                transcript,
            )
        if mode == "selection":
            restored_modes = (b"\x1b[?1002l", b"\x1b[?1006l", b"\x1b[?1004l")
            if not all(sequence in output for sequence in restored_modes):
                fail(
                    "CLI did not restore mouse and focus reporting on exit",
                    process,
                    output,
                    transcript,
                )
        with open(transcript, "wb") as target:
            target.write(output)
        if process.returncode != 0:
            raise SystemExit(f"CLI exited with {process.returncode}")
    finally:
        os.close(master)


if __name__ == "__main__":
    main()
