import argparse
import json
import sqlite3
import sys

from . import clock, store


def positive_integer(text):
    try:
        value = int(text)
    except ValueError:
        raise argparse.ArgumentTypeError("value must be an integer") from None
    if value <= 0:
        raise argparse.ArgumentTypeError("value must be positive")
    return value


def main(argv=None):
    parser = argparse.ArgumentParser(description="A durable SQLite task queue")
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("init", "enqueue", "claim", "ack", "fail", "stats"):
        sub = commands.add_parser(name)
        sub.add_argument("database")
        if name in ("enqueue", "ack", "fail"):
            sub.add_argument("task_id")
        if name in ("claim", "ack", "fail"):
            sub.add_argument("worker")
        if name in ("ack", "fail"):
            sub.add_argument("token")
        if name == "enqueue":
            sub.add_argument("payload", type=json.loads)
            sub.add_argument("--priority", type=int, default=0)
            sub.add_argument("--max-attempts", type=positive_integer, default=3)
        if name == "claim":
            sub.add_argument("--lease-seconds", type=clock.lease_seconds, required=True)
        if name == "fail":
            sub.add_argument("--error", required=True)
        if name in ("enqueue", "claim", "fail", "stats"):
            sub.add_argument("--now", type=clock.timestamp)
    args = parser.parse_args(argv)
    try:
        for field in ("task_id", "worker", "token"):
            if hasattr(args, field) and not getattr(args, field):
                raise ValueError(f"{field} must not be empty")
        if args.command == "enqueue":
            # Reject non-standard JSON constants before opening the database.
            json.dumps(args.payload, allow_nan=False)
        now = clock.now(getattr(args, "now", None))
        with store.transaction(args.database) as connection:
            if args.command == "init":
                result = {"initialized": True}
            elif args.command == "enqueue":
                result = store.enqueue(connection, args.task_id, args.payload, args.priority, args.max_attempts, now)
            elif args.command == "claim":
                result = store.claim(connection, args.worker, args.lease_seconds, now)
            elif args.command == "ack":
                result = store.ack(connection, args.task_id, args.worker, args.token)
            elif args.command == "fail":
                result = store.fail(connection, args.task_id, args.worker, args.token, args.error, now)
            else:
                result = store.stats(connection, now)
        print(json.dumps(result, ensure_ascii=False, sort_keys=True, separators=(",", ":")))
    except (OSError, ValueError, sqlite3.Error) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    return 0
