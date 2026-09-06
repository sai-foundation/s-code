import argparse
import json
import sys

from .events import read_events
from .io import atomic_write
from .reports import summarize


def main(argv=None):
    parser = argparse.ArgumentParser(description="Summarize JSONL incident events")
    parser.add_argument("input")
    parser.add_argument("--output", required=True)
    args = parser.parse_args(argv)
    try:
        report = summarize(read_events(args.input))
        atomic_write(args.output, json.dumps(report, ensure_ascii=False, separators=(",", ":")) + "\n")
    except (OSError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    return 0
