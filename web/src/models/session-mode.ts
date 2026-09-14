export type ConversationMode = "chat" | "work";

interface SessionModeFields {
  mode?: ConversationMode;
  workspace_uri: string;
}

// Existing sessions predate the explicit mode field and retain their workspace.
export function sessionMode(session: SessionModeFields): ConversationMode {
  return session.mode ?? (session.workspace_uri ? "work" : "chat");
}

export function hasWorkspace(session: SessionModeFields | null): boolean {
  return Boolean(session && sessionMode(session) === "work" && session.workspace_uri);
}

export function newSessionWorkspace(mode: ConversationMode, selectedDirectory: string): string {
  return mode === "chat" ? "" : selectedDirectory.trim();
}

// A transition belongs to one conversation; applying it must never replace the
// selected conversation or its transcript. Work -> Chat is intentionally absent.
export function applyWorkTransition<T extends SessionModeFields & { id: string }>(
  session: T,
  sessionId: unknown,
  payload: { mode?: unknown; workspace_uri?: unknown; reason?: unknown },
): T {
  if (session.id !== sessionId || payload.mode !== "work"
    || typeof payload.workspace_uri !== "string" || !payload.workspace_uri.trim()) return session;
  return { ...session, mode: "work", workspace_uri: payload.workspace_uri, ...(typeof payload.reason === "string" ? { work_reason: payload.reason } : {}) };
}
