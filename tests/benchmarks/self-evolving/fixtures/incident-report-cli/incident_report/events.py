"""Parse and validate all events before the report is published."""
import json

SEVERITIES = ("info", "warning", "error")


def read_events(path):
    events = []
    with open(path, encoding="utf-8") as stream:
        for number, line in enumerate(stream, 1):
            if not line.strip():
                continue
            try:
                event = json.loads(line)
                if not isinstance(event, dict):
                    raise ValueError("event must be an object")
                if any(not isinstance(event.get(key), str) for key in ("service", "severity", "message")):
                    raise ValueError("service, severity and message must be strings")
                if event["severity"] not in SEVERITIES:
                    raise ValueError("unknown severity")
            except ValueError as error:
                raise ValueError(f"{path}: line {number}: {error}") from None
            events.append({key: event[key] for key in ("service", "severity", "message")})
    return events
