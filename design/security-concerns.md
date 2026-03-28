# Security Considerations

---

## Authentication & Session Management
- **Token security** — access tokens should be short-lived (15–60 min) with refresh tokens stored in `HttpOnly`, `Secure`, `SameSite=Strict` cookies. Never store tokens in `localStorage` — they're accessible to any JavaScript running on the page.
- **WebSocket auth** — WebSocket connections don't send cookies automatically in all clients. Pass a short-lived, single-use token as a query parameter on the upgrade request, validated immediately and discarded. Never accept a long-lived token on a WebSocket URL.
- **SSO/OIDC** — use PKCE flow (no client secret in the browser). Validate issuer, audience, and expiry on every token.

---

## Authorization & Access Control
- **Document-level ACLs** — every API call and every WebSocket message must verify the requesting user has permission for that specific document. Authorization checks belong deep in the service layer, not just at the route level.
- **Workspace hierarchy** — permissions cascade (workspace → folder → document) but must be explicitly enforced at each level, not assumed from the parent.
- **Principle of least privilege** — presigned storage URLs should be scoped to exactly one object. Never issue a wildcard or prefix-level credential to a client.
- **Role enforcement** — viewer/commenter/editor/admin roles must be checked on every write operation, including WebSocket updates, not just on initial page load.

---

## Transport Security
- **TLS everywhere** — all HTTP, WebSocket (`wss://`), and cache connections must use TLS. Cloud storage and database services are HTTPS-only by default; don't override that.
- **HSTS** — set `Strict-Transport-Security` with a long `max-age` and `includeSubDomains`.
- **Certificate pinning** — worth considering for any native mobile client.

---

## Input Validation & Injection
- **CRDT update validation** — malformed or oversized update payloads should be rejected before being applied to the server-side document state or persisted. A corrupt update can bring down the document for all collaborators.
- **Content sanitization** — if you render HTML from document content, sanitize on the way out. A collaborator with edit access could inject script content that executes for other viewers.
- **File upload validation** — validate MIME type server-side (not just the file extension), enforce size limits, and scan for malware before making attachments accessible to other users. Presigned upload URLs should be constrained with explicit content type and length conditions.

---

## Data Security
- **Encryption at rest** — enable encryption on your key-value store and object storage. For highly sensitive workspaces, consider client-side encryption before upload.
- **Encryption in transit** — covered by TLS, but ensure cache/pubsub traffic is also encrypted if running on a managed service.
- **Key management** — use a dedicated secrets manager for all secrets (signing keys, OAuth client secrets). Never commit secrets to source control; local development values should be clearly non-production.
- **PII minimization** — store only what you need. Prefer referencing user IDs over duplicating names or email addresses across records.

---

## Infrastructure & Cloud-Specific
- **IAM least privilege** — the API server's cloud role should have access only to the specific database table and storage bucket it needs, with no wildcard actions. Use separate roles for different services.
- **Storage bucket policy** — block all public access at the bucket level. All access should flow through presigned URLs or a CDN with signed cookies.
- **Network isolation** — cache and pubsub services should never be internet-accessible. Run them in private subnets with no public endpoints.
- **VPC endpoints** — route traffic to managed cloud services (object storage, key-value store) through private network endpoints so it never traverses the public internet.

---

## Real-Time / WebSocket Specific
- **Rate limiting** — apply per-user rate limits on WebSocket messages to prevent a single client from flooding the update pipeline. This check should happen before updates are persisted.
- **Room authorization on reconnect** — when a client reconnects and requests a document, re-validate permissions from the database. Don't trust cached room membership from a previous session.
- **Awareness data** — cursor positions and presence are broadcast to all room participants. Ensure a user can't inject arbitrary data into the awareness payload that other clients will render without validation.

---

## Operational Security
- **Audit logging** — log document access, permission changes, and sharing events to an append-only store. This is a compliance requirement in most enterprise contexts.
- **Dependency supply chain** — audit third-party dependencies in CI. Flag known vulnerabilities before they reach production.
- **Secrets rotation** — design for signing key rotation from day one. It is painful and risky to retrofit.
- **DDoS / abuse** — WebSocket connections are more expensive to maintain than HTTP requests. Enforce connection limits per user and per IP at the load balancer before traffic reaches your application layer.