import argparse
import json
import sys

from .plan import validate
from .report import publish
from .scheduler import run


def positive_integer(text):
    try:
        value = int(text)
    except ValueError:
        raise argparse.ArgumentTypeError("jobs must be an integer") from None
    if value <= 0:
        raise argparse.ArgumentTypeError("jobs must be positive")
    return value


def main(argv=None):
    parser = argparse.ArgumentParser(description="Execute a validated dependency graph")
    parser.add_argument("plan")
    parser.add_argument("--jobs", type=positive_integer, default=1)
    parser.add_argument("--output", required=True)
    args = parser.parse_args(argv)
    try:
        with open(args.plan, encoding="utf-8") as stream:
            tasks = validate(json.load(stream))
        report = run(tasks, args.jobs)
        publish(args.output, report)
    except (OSError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    return 0 if report["status"] == "succeeded" else 1
