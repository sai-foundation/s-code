"""A task executes argv directly, retrying whole attempts when requested."""
import subprocess


def text(value):
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    return value or ""


def execute(task):
    for attempt in range(1, task["retries"] + 2):
        try:
            result = subprocess.run(task["command"], capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=task["timeout_seconds"], shell=False)
            code, stdout, stderr = result.returncode, result.stdout, result.stderr
        except subprocess.TimeoutExpired as error:
            code, stdout, stderr = 124, text(error.stdout), text(error.stderr) + "timeout exceeded\n"
        except OSError as error:
            code, stdout, stderr = 127, "", str(error) + "\n"
        if code == 0:
            break
    return {"id": task["id"], "status": "succeeded" if code == 0 else "failed", "attempts": attempt, "exit_code": code, "stdout": stdout, "stderr": stderr}
