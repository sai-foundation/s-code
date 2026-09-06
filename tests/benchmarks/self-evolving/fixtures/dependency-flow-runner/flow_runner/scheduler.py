"""Schedule dependency-ready tasks, retaining input identity in final reports."""
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait

from .process import execute


def skipped(task_id, dependency):
    return {"id": task_id, "status": "skipped", "attempts": 0, "exit_code": None, "stdout": "", "stderr": "", "reason": f"dependency {dependency!r} did not succeed"}


def run(tasks, jobs):
    pending = {task["id"]: task for task in tasks}
    finished, running, resources = {}, {}, set()
    with ThreadPoolExecutor(max_workers=jobs) as workers:
        while pending or running:
            for task_id in sorted(list(pending)):
                task = pending[task_id]
                failed = next((dep for dep in task["depends_on"] if dep in finished and finished[dep]["status"] != "succeeded"), None)
                if failed is not None:
                    finished[task_id] = skipped(task_id, failed)
                    del pending[task_id]
            for task_id in sorted(list(pending)):
                if len(running) >= jobs:
                    break
                task = pending[task_id]
                if any(dep not in finished or finished[dep]["status"] != "succeeded" for dep in task["depends_on"]):
                    continue
                if task["resource"] is not None and task["resource"] in resources:
                    continue
                del pending[task_id]
                if task["resource"] is not None:
                    resources.add(task["resource"])
                running[workers.submit(execute, task)] = task
            if running:
                done, _ = wait(running, return_when=FIRST_COMPLETED)
                for future in sorted(done, key=lambda item: running[item]["id"]):
                    task = running.pop(future)
                    resources.discard(task["resource"])
                    finished[task["id"]] = future.result()
            elif pending:
                # Pure validation ruled out a cycle. Another pass propagates
                # skipped state through dependents whose IDs sort earlier.
                if not any(any(dep in finished and finished[dep]["status"] != "succeeded" for dep in task["depends_on"]) for task in pending.values()):
                    raise ValueError("no runnable task in validated plan")
    reports = [finished[task["id"]] for task in tasks]
    return {"status": "succeeded" if all(task["status"] == "succeeded" for task in reports) else "failed", "tasks": reports}
