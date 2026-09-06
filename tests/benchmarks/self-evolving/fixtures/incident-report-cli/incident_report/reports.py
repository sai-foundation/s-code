"""Aggregation has no filesystem or CLI side effects."""
from collections import Counter

from .events import SEVERITIES


def summarize(events):
    severities = Counter(event["severity"] for event in events)
    services = Counter(event["service"] for event in events)
    messages = Counter(event["message"] for event in events)
    return {
        "total": len(events),
        "by_severity": {severity: severities[severity] for severity in SEVERITIES},
        "by_service": dict(sorted(services.items())),
        "top_messages": [
            {"message": message, "count": count}
            for message, count in sorted(messages.items(), key=lambda item: (-item[1], item[0]))[:3]
        ],
    }
