"""Validate the complete plan as a pure operation before starting workers."""
import math

TASK_KEYS = {"id", "command", "depends_on", "resource", "retries", "timeout_seconds"}


def validate(value):
    if not isinstance(value, dict) or set(value) != {"tasks"} or not isinstance(value["tasks"], list):
        raise ValueError("plan must contain only a tasks array")
    tasks, identifiers = [], set()
    for item in value["tasks"]:
        if not isinstance(item, dict) or set(item) - TASK_KEYS:
            raise ValueError("task must be an object with known keys")
        task_id = item.get("id")
        if not isinstance(task_id, str) or not task_id or task_id in identifiers:
            raise ValueError("task id must be a unique non-empty string")
        identifiers.add(task_id)
        command = item.get("command")
        if not isinstance(command, list) or not command or any(not isinstance(arg, str) or "\0" in arg for arg in command) or not command[0]:
            raise ValueError(f"task {task_id!r}: command must be a non-empty argv array")
        dependencies = item.get("depends_on", [])
        if not isinstance(dependencies, list) or any(not isinstance(dep, str) or not dep for dep in dependencies):
            raise ValueError(f"task {task_id!r}: dependencies must be non-empty strings")
        resource = item.get("resource")
        if "resource" in item and (not isinstance(resource, str) or not resource):
            raise ValueError(f"task {task_id!r}: resource must be a non-empty string")
        retries = item.get("retries", 0)
        if type(retries) is not int or retries < 0:
            raise ValueError(f"task {task_id!r}: retries must be a non-negative integer")
        timeout = item.get("timeout_seconds")
        if "timeout_seconds" in item and (type(timeout) not in (int, float) or not math.isfinite(timeout) or timeout <= 0):
            raise ValueError(f"task {task_id!r}: timeout_seconds must be positive and finite")
        tasks.append({"id": task_id, "command": command[:], "depends_on": dependencies[:], "resource": resource, "retries": retries, "timeout_seconds": timeout})
    for task in tasks:
        for dep in task["depends_on"]:
            if dep not in identifiers:
                raise ValueError(f"task {task['id']!r}: missing dependency {dep!r}")
    unresolved = {task["id"]: set(task["depends_on"]) for task in tasks}
    while unresolved:
        ready = {task_id for task_id, deps in unresolved.items() if not deps}
        if not ready:
            raise ValueError("plan contains a dependency cycle")
        unresolved = {task_id: deps - ready for task_id, deps in unresolved.items() if task_id not in ready}
    return tasks
