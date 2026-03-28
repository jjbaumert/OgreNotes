# Quip Authentication

## Reference Documentation

- **Quip Automation API:** <https://quip.com/dev/automation/documentation/current>
- **Quip Admin API:** <https://quip.com/dev/admin/documentation/current>
- **OAuth2 Example (GitHub):** <https://github.com/quip/quip-apps/tree/master/examples/quip-automation-api-oauth2>
- **Okta SAML for Quip:** <https://saml-doc.okta.com/SAML_Docs/How-to-Configure-SAML-2.0-for-Quip.html>
- **MFA Help:** <https://help.salesforce.com/s/articleView?id=000390822&type=1>
- **Security Settings Help:** <https://help.salesforce.com/s/articleView?id=000392626&type=1>

---

## OAuth 2.0 Authorization Code Flow

### Endpoints

| Endpoint | URL |
|----------|-----|
| Authorization | `GET https://platform.quip.com/1/oauth/login` |
| Token | `POST https://platform.quip.com/1/oauth/access_token` |
| Verify | `GET https://platform.quip.com/1/oauth/verify_token` |
| Revoke | `POST https://platform.quip.com/1/oauth/revoke` |

For VPC customers: `https://platform.quip-{customername}.com/1/...`

### Authorization Request

`GET /1/oauth/login` with parameters:

| Parameter | Description |
|-----------|-------------|
| `client_id` | Client ID from API key |
| `redirect_uri` | Redirect URL; receives `code` on success or `error`, `error_description`, `error_uri` on failure |
| `response_type` | `code` |
| `scope` | Space-delimited scopes |
| `state` | Anti-CSRF opaque value |

### Token Exchange

`POST /1/oauth/access_token` with parameters:

| Parameter | Required | Description |
|-----------|----------|-------------|
| `grant_type` | Yes | `authorization_code` or `refresh_token` |
| `client_id` | Yes | Client ID |
| `client_secret` | Yes | Client secret |
| `code` | When grant_type=authorization_code | Verification code from redirect |
| `redirect_uri` | When grant_type=authorization_code | Must match original |
| `refresh_token` | When grant_type=refresh_token | Refresh token from prior exchange |

### Token Response

```json
{
  "access_token": "string",
  "refresh_token": "string",
  "expires_in": 2592000,
  "token_type": "Bearer"
}
```

Follows RFC 6749 (OAuth 2.0) and RFC 6750 (Bearer Token).

---

## Personal Access Tokens

- Generated at `https://{domain}.quip.com/dev/token`
- Single-click generation
- **Generating a new token invalidates all previous personal tokens**
- Same `Authorization: Bearer <token>` header format as OAuth tokens
- Subject to same 30-day expiration
- Intended for testing, personal automation, individual integrations
- Enterprise users get full API access; free/individual users get a subset

---

## Token Lifecycle

| Operation | Details |
|-----------|---------|
| **Expiration** | Tokens expire every **30 days** |
| **Refresh** | `POST /1/oauth/access_token` with `grant_type=refresh_token` before expiration |
| **Verify** | `GET /1/oauth/verify_token` returns expiry status and applicable scopes |
| **Revoke** | `POST /1/oauth/revoke` -- does NOT require Authorization header |
| **Admin revoke** | Admin Console > Settings > Integrations > Revoke API key |

Tokens are not refreshed automatically; applications must call refresh explicitly.

---

## OAuth Scopes

Selected when creating an API key in Admin Console (Settings > Integrations > API Keys).

### Automation API Scopes

| Scope | Description |
|-------|-------------|
| `USER_READ` | GET calls that read data (documents, folders, users) |
| `USER_WRITE` | POST, DELETE, PATCH calls that edit data |
| `USER_MANAGE` | Full thread/folder access (add/remove access, lock/unlock) |

### Admin API Scopes

| Scope | Description |
|-------|-------------|
| `ADMIN_READ` | Read-only admin operations |
| `ADMIN_WRITE` | Admin write operations |
| `ADMIN_MANAGE` | Full admin actions (user management, quarantine, data holds) |

Admin API access also requires the admin to be added to Admin API Users list (Settings > Site Settings).

---

## SSO / SAML 2.0

### Supported Features

| Feature | Description |
|---------|-------------|
| **IdP-initiated SSO** | User starts login at the identity provider |
| **SP-initiated SSO** | User navigates to Quip, enters email, redirected to IdP |
| **JIT Provisioning** | Users auto-provisioned on first SAML login |

### SAML Attribute Mapping

| SAML Attribute | Quip Field |
|----------------|------------|
| `User.FirstName` | `user.firstName` |
| `User.LastName` | `user.lastName` |

### Configuration

- Admin Console > Settings > Authentication > New Configuration
- Upload IdP metadata XML or configure manually
- Some setups require emailing Quip with metadata.xml

### Supported Identity Providers

Okta, Azure AD, JumpCloud, Citrix, OneLogin, Salesforce (direct SSO federation)

### SCIM Integration

SCIM 2.0 for automated user provisioning/deprovisioning. Changes in IdP propagate automatically. Compatible with Okta, Active Directory, OneLogin.

---

## Session Management

- Admins set **session lengths per platform** (web, desktop, mobile) from Admin Console
- Admins can **restrict which platforms** are allowed
- Session timeout controls re-authentication frequency
- Force-invalidate sessions via SCIM V2 Patch: set `active` to `false`, then `true`
- API: `POST /1/admin/users/revoke-sessions` to revoke all sessions for a user
- API: `POST /1/admin/users/revoke-pat` to revoke personal access tokens

---

## Login UI Flow

1. User navigates to `https://quip.com/account/login` (or `https://{subdomain}.quip.com/`)
2. User enters email address
3. If domain is SSO-enabled: redirect to IdP
4. If not SSO: enter password
5. If MFA enabled: prompted for verification code
6. For Live Apps: login in popup window (web) or overlay (native); closes automatically on completion

---

## Multi-Factor Authentication (MFA)

Enabled in Admin Console > Accounts & Access > Multi Factor Authentication.

### Supported Methods

| Method | Description |
|--------|-------------|
| Salesforce Authenticator | Salesforce's authenticator app |
| Third-party authenticator | Google Authenticator, Microsoft Authenticator, Authy |
| Hardware security keys | YubiKey, Google Titan Security Key |

Can be enforced at the profile level or selectively with permission sets.

---

## Domain Authentication (Enterprise Only)

- Standard OAuth 2.0 with admin **pre-approved applications**
- End users skip individual authorization prompts
- Setup: create OAuth 2.0 API key per application, configure with standard endpoints
- Best practice: separate API key per app (named after the app) for easy revocation

---

## API Authentication

All API calls use the same header format:

```
Authorization: Bearer <access_token>
```

Base URL: `https://platform.quip.com/1/` (or `/2/` for v2 endpoints)

Tokens from OAuth flow, personal access tokens, or domain authentication are used identically.

---

## Rate Limits on Auth and API Endpoints

### Per-User (Automation API)

- 50 requests/minute
- 750 requests/hour (some sources report 900/hour)
- Headers: `X-Ratelimit-Limit`, `X-Ratelimit-Remaining`, `X-Ratelimit-Reset`

### Per-User (Admin API)

- 100 requests/minute
- 1,500 requests/hour
- Same `X-Ratelimit-*` headers

### Per-Company (All APIs)

- 600 requests/minute
- Headers: `X-Company-RateLimit-Limit`, `X-Company-RateLimit-Remaining`, `X-Company-RateLimit-Reset`, `X-Company-Retry-After`

### Rate Limit Error

HTTP `503: Over Rate Limit` with reset timestamp for backoff implementation.
