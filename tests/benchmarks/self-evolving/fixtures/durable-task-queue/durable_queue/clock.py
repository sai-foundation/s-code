"""Logical time parsing shared by the queue's command-line operations."""
import argparse
import time


def timestamp(text):
    try:
        return float(text)
    except ValueError:
        raise argparse.ArgumentTypeError("time must be a number") from None


def lease_seconds(text):
    value = timestamp(text)
    if not value > 0:
        raise argparse.ArgumentTypeError("lease duration must be positive")
    return value


def now(override):
    return time.time() if override is None else override
