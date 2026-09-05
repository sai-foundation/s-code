#!/usr/bin/env python3
"""Loopback fixture that accepts connections but never sends an HTTP response."""

from pathlib import Path
import fcntl
import os
import socket
import sys
import time


def main() -> None:
    address_file = Path(sys.argv[1])
    lock_file = Path(sys.argv[2])
    lock_file.parent.mkdir(parents=True, exist_ok=True)
    lock = lock_file.open("w+", encoding="utf-8")
    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    lock.write(f"{os.getpid()}\n")
    lock.flush()
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", 0))
    listener.listen(16)
    address_file.write_text(f"127.0.0.1:{listener.getsockname()[1]}\n", encoding="utf-8")
    while True:
        connection, _ = listener.accept()
        try:
            time.sleep(60)
        finally:
            connection.close()


if __name__ == "__main__":
    main()
