// Generated from s-code-protocol. Do not edit by hand.
import { requestEndpoint } from "./client";
import type {
  CancelTurn,
  CapabilityManifest,
  ClientPresence,
  CreateDurableTask,
  CreateSession,
  StartSessionWork,
  CreateTurn,
  DurableTask,
  DurableTaskSummary,
  RemoveClientPresence,
  PrivacyPage,
  ResolveApproval,
  Session,
  TeamGovernanceSummary,
  TranscriptSnapshot,
  Turn,
  UpdateClientPresence,
  UpdateSession,
} from "../protocol";

export interface ScopeQuery {
  organization_id: string;
  team_id: string;
  actor_id: string;
}

function scopeQuery(scope: ScopeQuery): string {
  return new URLSearchParams({ ...scope }).toString();
}

function encoded(value: string): string {
  return encodeURIComponent(value);
}

/** Typed v1 client for the shared Session/Turn/Item/Approval/Task protocol. */
export class SCodeClient {
  privacy(sessionId: string, scope: ScopeQuery, before?: number): Promise<PrivacyPage> {
    const suffix = before == null ? "" : `&before=${before}`;
    return requestEndpoint(`/v1/sessions/${encoded(sessionId)}/privacy?${scopeQuery(scope)}${suffix}`);
  }

  capabilities(): Promise<CapabilityManifest> {
    return requestEndpoint("/v1/capabilities");
  }

  listSessions(scope: ScopeQuery): Promise<Session[]> {
    return requestEndpoint(`/v1/sessions?${scopeQuery(scope)}`);
  }

  createSession(input: CreateSession): Promise<Session> {
    return requestEndpoint("/v1/sessions", {
      method: "POST",
      body: JSON.stringify(input),
    });
  }

  startSessionWork(id: string, input: StartSessionWork): Promise<Session> {
    return requestEndpoint(`/v1/sessions/${encoded(id)}/work`, { method: "POST", body: JSON.stringify(input) });
  }

  updateSession(id: string, input: UpdateSession): Promise<Session> {
    return requestEndpoint(`/v1/sessions/${encoded(id)}`, {
      method: "PATCH",
      body: JSON.stringify(input),
    });
  }

  transcriptSnapshot(
    sessionId: string,
    scope: ScopeQuery,
    before?: string,
    limit = 500,
  ): Promise<TranscriptSnapshot> {
    const query = new URLSearchParams({ ...scope, limit: String(limit) });
    if (before) query.set("before", before);
    return requestEndpoint(`/v1/sessions/${encoded(sessionId)}/snapshot?${query}`);
  }

  createTurn(sessionId: string, input: CreateTurn): Promise<Turn> {
    return requestEndpoint(`/v1/sessions/${encoded(sessionId)}/turns`, {
      method: "POST",
      body: JSON.stringify(input),
    });
  }

  cancelTurn(turnId: string, input: CancelTurn): Promise<Turn> {
    return requestEndpoint(`/v1/turns/${encoded(turnId)}/cancel`, {
      method: "POST",
      body: JSON.stringify(input),
    });
  }

  resolveApproval(approvalId: string, input: ResolveApproval): Promise<unknown> {
    return requestEndpoint(`/v1/approvals/${encoded(approvalId)}`, {
      method: "POST",
      body: JSON.stringify(input),
    });
  }

  listDurableTasks(scope: ScopeQuery): Promise<DurableTaskSummary[]> {
    return requestEndpoint(`/v1/durable-task-summaries?${scopeQuery(scope)}`);
  }

  createDurableTask(input: CreateDurableTask): Promise<DurableTask> {
    return requestEndpoint("/v1/durable-tasks", {
      method: "POST",
      body: JSON.stringify(input),
    });
  }

  listClientPresence(scope: ScopeQuery): Promise<ClientPresence[]> {
    return requestEndpoint(`/v1/client-presence?${scopeQuery(scope)}`);
  }

  updateClientPresence(input: UpdateClientPresence): Promise<ClientPresence[]> {
    return requestEndpoint("/v1/client-presence", {
      method: "PUT",
      body: JSON.stringify(input),
    });
  }

  removeClientPresence(
    clientId: string,
    input: RemoveClientPresence,
  ): Promise<ClientPresence[]> {
    return requestEndpoint(`/v1/client-presence/${encoded(clientId)}`, {
      method: "DELETE",
      body: JSON.stringify(input),
    });
  }

  teamGovernance(
    teamId: string,
    scope: Pick<ScopeQuery, "organization_id" | "actor_id">,
  ): Promise<TeamGovernanceSummary> {
    const query = new URLSearchParams({ ...scope });
    return requestEndpoint(
      `/v1/teams/${encoded(teamId)}/governance?${query}`,
    );
  }

  eventsUrl(scope: ScopeQuery, after = 0): `/v1/${string}` {
    return `/v1/events?${new URLSearchParams({
      ...scope,
      after: String(after),
    })}`;
  }
}
