# Read-only deployment moderation dashboard

Buzz can expose a private, deployment-wide read-only dashboard from the existing
relay process. It shows open moderation reports and recent product feedback.

Configure `BUZZ_ADMIN_HOST` to activate the dashboard. A private ingress limits
access to the operator VPN or approved source IPs.

Required configuration:

```text
BUZZ_ADMIN_HOST=admin.example.com
BUZZ_ADMIN_TOKEN=<64 hex characters>
BUZZ_ADMIN_WEB_DIR=/srv/buzz/admin-web
```

## Authentication

Every `/api/admin/v1` request must carry the operator token as a bearer
credential. The relay fails closed:

- `BUZZ_ADMIN_HOST` set without a valid `BUZZ_ADMIN_TOKEN` → the relay refuses
  to start. There is no insecure opt-out.
- `BUZZ_ADMIN_TOKEN` must be exactly 64 hexadecimal characters (32 bytes).
  Surrounding whitespace is trimmed; anything else — empty, non-hex, wrong
  length, non-Unicode — is a startup error.
- `BUZZ_ADMIN_TOKEN` set without `BUZZ_ADMIN_HOST` is ignored: the admin surface
  stays absent and the relay logs a warning at startup.

Generate a token once per deployment and store it with your other secrets:

```bash
openssl rand -hex 32
```

Call the API with it:

```bash
curl -H "Host: admin.example.com" \
     -H "Authorization: Bearer $BUZZ_ADMIN_TOKEN" \
     https://admin.example.com/api/admin/v1/reports
```

A missing, malformed, duplicated, or incorrect credential returns `401` with
`WWW-Authenticate: Bearer` and reveals nothing about the expected `Host`. The
scheme is matched case-insensitively per RFC 9110, and the credential is
compared in constant time. The token never appears in URLs, logs, or traces.

The dashboard prompts for the token on first load and keeps it in
`sessionStorage` for that browser session only; a rejected token is discarded
and re-prompted. Attachment bytes are fetched through the authenticated API and
rendered from object URLs, because `<img src>` and `<a href>` cannot carry an
`Authorization` header.

Every admin-host response that carries the dashboard itself — the SPA document
on each admin route, the hashed `/assets/*` bundle, and admin-host `404`s — is
served with a Content Security Policy response header, `ADMIN_CSP` in
`crates/buzz-relay/src/router.rs`:

```text
default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' blob:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'
```

It blocks inline and third-party script and restricts subresource and request
destinations to the same origin, which closes the direct paths an injected
script would use to exfiltrate the `sessionStorage` token. It does not
constrain top-level navigation, so it is a containment layer, not a substitute
for keeping script off the origin. `blob:` is permitted for images only, for
attachment previews. It is a response header rather than a `<meta>` tag because
`frame-ancestors` is ignored in meta — that directive is the dashboard's
authoritative frame protection, superseding the `X-Frame-Options: DENY` the JSON
API sends. The policy applies to the admin host only; the public web bundle
keeps its own headers.

The exact admin `Host` and matching browser `Origin` are still required, but
they are defense-in-depth behind the credential, not the access control. HTTPS
and a private ingress remain required: the token is a bearer credential in
transit, and `sessionStorage` is readable by any JavaScript injected into the
admin origin.

The shared token authenticates the deployment operator role, not a person. It
carries no per-operator identity, attribution, or individual revocation —
rotating it revokes access for everyone at once.

When the UI runs in a separate pod, proxy `/api/admin/v1/*` to the relay while
preserving the admin `Host` header and the client's `Authorization` header. A
`NetworkPolicy` grants the admin pod access to that relay path.

Read routes:

- `GET /api/admin/v1/reports`
- `GET /api/admin/v1/reports/:id`
- `GET /api/admin/v1/feedback`
- `GET /api/admin/v1/feedback/:id`

Report reads accept optional `communityId`, `status`, `reportType`, `targetKind`,
`after`, `before`, and `limit` parameters. Limits are capped at 200. Feedback is
a bounded newest-first summary from the existing product-feedback repository.

For local review, run `just admin-seed` before `just admin`. `just admin` mints a
throwaway token for that run and prints it — paste it into the dashboard prompt.
The seed command also uploads real image and diagnostic fixtures to local MinIO.
Feedback search and filters run over the bounded browser result set; the
**Acted on** checkbox is stored in that browser's local storage.

## Feedback attachment boundary

Feedback attachment bytes are available only through the feedback-scoped read
route:

- `GET /api/admin/v1/feedback/:id/attachments/:sha256`

The route uses the same bearer credential, private-ingress, exact admin `Host`,
and same-origin boundary as the JSON API. It is not a generic media endpoint.
The relay loads the feedback row, derives its community from server-owned
provenance, verifies that host resolution still maps to the row's
`community_id`, and requires the requested SHA-256 to match both the `x` field
and source-community `/media/` URL in that row's persisted `imeta` tag. It then
reads the tenant-scoped media sidecar before accessing the shared
content-addressed blob. Unknown feedback, unreferenced hashes, malformed paths,
and cross-community substitutions all collapse to `404`.

Only `GET` and `HEAD` are routed. Existing community `/media/*` authorization is
unchanged, including `BUZZ_REQUIRE_MEDIA_GET_AUTH`; the browser receives no
Blossom credential or reusable signed URL. Responses are uncached, `nosniff`,
governed by a restrictive CSP, streamed from object storage, and non-previewable
content retains attachment disposition. Successful reads produce a structured
trace containing feedback ID, community ID, and attachment hash, but no feedback
body or attachment URL.

The human trust boundary remains the shared operator token plus the private
admin ingress. Neither is per-operator identity. Anyone holding the token and
admitted to the dashboard can read attachments for feedback records they can
access. Per-person attribution or revocation requires authenticated operator
identity at ingress/application level; this endpoint deliberately does not claim
to provide it.
