import { clearToken, getToken } from "./token";

const PREFIX = "/api/admin/v1";

export class ApiFailure extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
  }
}

/// Every admin API call goes through here so the operator token is attached
/// exactly once and a rejected credential is cleared in exactly one place.
/// When no token is present the request is sent without an Authorization
/// header; in insecure_no_auth mode the relay returns 200, while in token
/// mode it returns 401 and the caller discovers that auth is required.
async function send(path: string, accept: string): Promise<Response> {
  const token = getToken();
  const headers: Record<string, string> = { accept };
  if (token) headers.authorization = `Bearer ${token}`;
  const response = await fetch(`${PREFIX}${path}`, {
    credentials: "same-origin",
    headers,
  });
  if (response.status === 401) {
    // Idempotent: concurrent 401s collapse into a single re-prompt.
    clearToken();
    throw new ApiFailure(401, "The admin token was rejected.");
  }
  if (!response.ok) {
    const envelope = await response.json().catch(() => null);
    throw new ApiFailure(
      response.status,
      envelope?.error?.message ?? `Request failed (${response.status})`,
    );
  }
  return response;
}

export async function request<T>(path: string): Promise<T> {
  const response = await send(path, "application/json");
  return response.json() as Promise<T>;
}

/// Probe whether the relay requires a bearer token. Issues one unauthenticated
/// request and returns `true` if the relay accepted it (insecure_no_auth mode)
/// or `false` if a 401 came back (token mode). Used by the App shell to skip
/// the token prompt for deployments that rely on network-layer access control.
export async function probeAuthRequired(): Promise<boolean> {
  try {
    const response = await fetch(`${PREFIX}/reports`, {
      credentials: "same-origin",
      headers: { accept: "application/json" },
    });
    // 200 → insecure_no_auth mode, no token needed.
    // Anything else — including 401, 403, 404, 5xx — means auth is required
    // or the deployment is misconfigured. Show the prompt rather than silently
    // skipping it.
    return response.status !== 200;
  } catch {
    // Network error — treat as auth-required so the prompt is shown.
    return true;
  }
}

/// Attachments cannot be fetched by `<img src>` or `<a href>` because those
/// carry no Authorization header. Callers render the object URL and must
/// revoke it when it is replaced or unmounted.
export async function requestObjectUrl(path: string): Promise<string> {
  const response = await send(path, "*/*");
  return URL.createObjectURL(await response.blob());
}
