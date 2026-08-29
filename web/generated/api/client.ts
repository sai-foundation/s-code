// Generated from opencoding-protocol. Do not edit by hand.
export interface ApiRequestOptions extends RequestInit {
  allowDisconnected?: boolean;
}

export interface ApiErrorPayload {
  detail?: unknown;
}

/**
 * Versioned same-origin daemon client. Callers that do not supply a generated
 * response type receive `unknown`; credentials are always the HttpOnly cookie.
 */
export async function requestEndpoint<T = unknown>(
  path: `/v1/${string}`,
  options: ApiRequestOptions = {},
): Promise<T> {
  const { allowDisconnected: _allowDisconnected, ...requestOptions } = options;
  const response = await fetch(path, {
    ...requestOptions,
    cache: "no-store",
    credentials: "same-origin",
    referrerPolicy: "no-referrer",
    headers: {
      "x-opencoding-csrf": "1",
      ...(requestOptions.body ? { "content-type": "application/json" } : {}),
      ...(requestOptions.headers || {}),
    },
  });
  if (!response.ok) {
    let detail = `${response.status}`;
    const contentType = response.headers.get("content-type") || "";
    try {
      if (contentType.includes("application/json")) {
        const payload = await response.json() as ApiErrorPayload;
        if (typeof payload.detail === "string") detail = payload.detail;
      } else {
        const text = (await response.text()).trim();
        if (text) detail = text.slice(0, 4096);
      }
    } catch {
      // The status code remains the safe fallback for a malformed response.
    }
    throw new Error(detail);
  }
  if (response.status === 204) return undefined as T;
  return await response.json() as T;
}

export const requestJson = requestEndpoint;
