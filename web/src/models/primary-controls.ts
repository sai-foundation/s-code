import type { PermissionMode } from "./protocol";
import type { ConversationMode } from "./session-mode";

export const permissionLabels: Record<PermissionMode, string> = {
  manual: "Manual approval",
  accept_edits: "Accept edits",
  workspace: "Workspace autonomy",
  plan: "Plan only",
};

export function changesAvailability(mode: ConversationMode, workspace: boolean, connected: boolean, protectedFiles: boolean, protectionState: "ready" | "loading" | "error" = "ready") {
  if (mode !== "work") return { visible: false, reason: "", action: null } as const;
  if (!connected) return { visible: true, reason: "Reconnect to view changes.", action: "connect" } as const;
  if (!workspace) return { visible: true, reason: "Start Work in a project to view changes.", action: "project" } as const;
  if (protectionState === "loading") return { visible: true, reason: "Loading file protections before viewing changes…", action: null } as const;
  if (protectionState === "error") return { visible: true, reason: "File protections could not be verified.", action: "retry-protections" } as const;
  if (protectedFiles) return { visible: true, reason: "Changes are unavailable while file protection blocks Git.", action: "protections" } as const;
  return { visible: true, reason: "", action: null } as const;
}

export function presenceVisibility(connected: boolean, supported: boolean, count: number, inspecting: boolean) {
  return connected && supported && (count > 1 || inspecting);
}

export function composerScopes(existingSession: boolean) {
  return existingSession
    ? { model: "Model · this session", permission: "Permissions · this session" }
    : { model: "Model · default", permission: "Permissions · next Work" };
}

export interface PickerContext { generation: number; sessionId: string | null; account: string }

// Both catalog responses and selection callbacks must still belong to their origin.
export function pickerContextMatches(expected: PickerContext, current: PickerContext, connected: boolean, running: boolean) {
  return connected && !running && expected.generation === current.generation
    && expected.sessionId === current.sessionId && expected.account === current.account;
}
