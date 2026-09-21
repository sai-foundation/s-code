export interface TaskFeedback { label: string; terminal: boolean; failure: boolean; waiting: boolean }
export function taskFeedback(kind: string, status = "", local = false): TaskFeedback {
  const value = status || kind.split(".").at(-1) || "";
  if (["failed", "error"].includes(value) || kind.endsWith(".error")) return { label: "Failed", terminal: true, failure: true, waiting: false };
  if (value === "denied") return { label: "Denied", terminal: true, failure: true, waiting: false };
  if (["completed", "complete"].includes(value)) return { label: "Completed", terminal: true, failure: false, waiting: false };
  if (["cancelled", "canceled", "stopped"].includes(value)) return { label: "Stopped", terminal: true, failure: false, waiting: false };
  if (["awaiting_approval", "waiting_approval"].includes(value) || kind === "approval.required") return { label: "Waiting for approval", terminal: false, failure: false, waiting: true };
  if (["awaiting_input", "waiting_input"].includes(value)) return { label: "Waiting for input", terminal: false, failure: false, waiting: true };
  if (local) return { label: "Applying locally", terminal: false, failure: false, waiting: true };
  if (value === "running_tool") return { label: "Running tool", terminal: false, failure: false, waiting: false };
  if (value === "idle") return { label: "Ready", terminal: false, failure: false, waiting: true };
  if (value === "cancelling") return { label: "Stopping…", terminal: false, failure: false, waiting: true };
  if (value === "uploading") return { label: "Uploading attachments", terminal: false, failure: false, waiting: false };
  if (kind.startsWith("tool.") || kind === "mcp.progress") return { label: value === "proposed" ? "Preparing tool" : "Running tool", terminal: false, failure: false, waiting: false };
  return { label: "Generating", terminal: false, failure: false, waiting: false };
}

export function toolFailureCause(payload: Record<string, unknown>, denied = false): string | null {
  for (const key of ["error", "error_code", "reason"]) {
    const value = payload[key];
    if (typeof value === "string" && value.trim()) return value;
  }
  if (!denied && payload.status !== "denied") return null;
  if (typeof payload.policy_reason === "string" && payload.policy_reason.trim()) return payload.policy_reason;
  const policy = payload.policy ?? payload.effective_policy;
  if (policy && typeof policy === "object" && "reason" in policy && typeof policy.reason === "string") return policy.reason;
  return null;
}

export function validatedToolId(value: unknown): string | null {
  return typeof value === "string" && /^[A-Za-z0-9_-]+$/.test(value) ? value : null;
}

// Never follow a supplied detail reference outside this session's guarded route.
export function toolIdFromDetail(href: string | undefined, sessionId: string): string | null {
  if (!href) return null;
  const prefix = `/v1/sessions/${encodeURIComponent(sessionId)}/tools/`;
  if (!href.startsWith(prefix)) return null;
  const suffix = href.slice(prefix.length);
  if (!suffix || /[/?#]/.test(suffix)) return null;
  try { return validatedToolId(decodeURIComponent(suffix)); } catch { return null; }
}

export function matchesToolInspection(call: Record<string, unknown>, toolId: string, sessionId: string, account: { organization_id: string; team_id: string; actor_id: string }) {
  const request = call.request as Record<string, unknown> | undefined;
  const scope = request?.scope as Record<string, unknown> | undefined;
  return request?.id === toolId && request.session_id === sessionId
    && scope?.organization_id === account.organization_id && scope.team_id === account.team_id && scope.actor_id === account.actor_id;
}
