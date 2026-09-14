export interface AccountScope {
  organization_id: string;
  team_id: string;
  actor_id: string;
}

// Encoding the tuple keeps separator characters in account IDs unambiguous.
export function accountKey(scope: AccountScope): string {
  return JSON.stringify([scope.organization_id, scope.team_id, scope.actor_id]);
}

export function accountDraftContext(account: string, sessionId?: string | null): string {
  return JSON.stringify([account, sessionId ?? "new"]);
}

export function accountPermissionKey(account: string): string {
  return `oc.permission-mode:${account}`;
}

export function ownsSession(session: { scope: AccountScope }, current: AccountScope | null): boolean {
  return current !== null && accountKey(session.scope) === accountKey(current);
}

// Every API response crosses this boundary before its caller can update UI.
export async function guardAccountResponse<T>(
  response: Promise<T>,
  generation: number,
  currentGeneration: () => number,
): Promise<T> {
  const assertCurrent = () => {
    if (generation !== currentGeneration()) {
      throw new DOMException("Account or connection changed while the request was running", "AbortError");
    }
  };
  try {
    const result = await response;
    assertCurrent();
    return result;
  } catch (error) {
    assertCurrent();
    throw error;
  }
}
