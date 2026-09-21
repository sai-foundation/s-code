import { isProtectionCommand } from "./composer-routing";
import type { PickerContext } from "./primary-controls";

export function composerControls(content: string, running: boolean, attachments: boolean, supported: boolean, pending: boolean) {
  const protect = isProtectionCommand(content), hasInput = Boolean(content.trim()) || attachments;
  const queue = running && hasInput && !protect;
  const blocked = running && attachments || queue && !supported;
  return {
    label: protect ? "Protect" : queue ? "Queue" : running ? "Stop" : "Send",
    disabled: pending || blocked,
    steer: queue && !attachments && supported,
    stop: running && hasInput,
    hint: protect ? "Applies protection locally; it is never queued as a model instruction."
      : running && attachments ? "Remove attachments before queuing or steering."
      : queue && !supported ? "This server does not support follow-up input while a task runs. Your draft will stay here."
      : running ? "Queue adds a follow-up at the next available step. Steer gives the current task new guidance."
      : "",
  };
}

export function sameComposerContext(expected: PickerContext, current: PickerContext) {
  return expected.generation === current.generation && expected.sessionId === current.sessionId && expected.account === current.account;
}

export function pendingInputLabel(mode: string, index: number) {
  return mode === "steer" ? "Guidance pending" : `Follow-up ${index + 1} · pending`;
}

export interface TurnInputAttemptContext {
  account: string;
  sessionId: string;
  turnId: string;
  mode: string;
  content: string;
}
export interface TurnInputAttempt { identity: string; key: string }

/** An uncertain acknowledgement must retry the same logical request exactly once. */
export function nextTurnInputAttempt(previous: TurnInputAttempt | null, context: TurnInputAttemptContext, createKey: () => string): TurnInputAttempt {
  const identity = JSON.stringify([context.account, context.sessionId, context.turnId, context.mode, context.content]);
  return previous?.identity === identity ? previous : { identity, key: createKey() };
}

export function pendingInputsOwned(inputs: unknown, sessionId: string, scope: { organization_id: string; team_id: string; actor_id: string }): boolean {
  return Array.isArray(inputs) && inputs.every(input => input && typeof input === "object"
    && input.session_id === sessionId && input.scope?.organization_id === scope.organization_id
    && input.scope?.team_id === scope.team_id && input.scope?.actor_id === scope.actor_id);
}
