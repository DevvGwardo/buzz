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
async function send(path: string, accept: string): Promise<Response> {
  const token = getToken();
  if (!token) throw new ApiFailure(401, "An admin token is required.");
  const response = await fetch(`${PREFIX}${path}`, {
    credentials: "same-origin",
    headers: { accept, authorization: `Bearer ${token}` },
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

/// Attachments cannot be fetched by `<img src>` or `<a href>` because those
/// carry no Authorization header. Callers render the object URL and must
/// revoke it when it is replaced or unmounted.
export async function requestObjectUrl(path: string): Promise<string> {
  const response = await send(path, "*/*");
  return URL.createObjectURL(await response.blob());
}
