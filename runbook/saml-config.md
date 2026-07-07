# SAML SSO — configure and verify

Phase 4 M-E4 ships workspace-level SAML 2.0 SSO. A workspace admin
points their IdP (Okta, Entra ID, Google Workspace, ...) at our SP
metadata, then members of that workspace sign in via the IdP.

This runbook walks through configuring and verifying SAML against a
real IdP. An automated happy-path test now exists
(`crates/api/tests/test_saml_acs_happy.rs`) but **skips on every CI
and dev runtime we have** — samael's in-process signature verify is
broken on all of them (see "Why no automated happy-path test" at the
bottom). Until a runtime with a working in-process xmlsec exists,
this manual run against a real IdP is the effective happy-path
verification.

## Prerequisites

- A workspace where the operator is owner or workspace-admin.
- An IdP tenant where the operator can register a new application
  (Okta org admin, Entra ID app admin, etc.).
- The OgreNotes deploy URL (e.g. `https://ogrenotes.example.com`).

## 1. Get the SP metadata URL

In OgreNotes, open `/workspaces/<workspace-id>/saml`. The page
shows an SP-metadata URL like:

```
https://ogrenotes.example.com/api/v1/auth/saml/metadata
```

Copy it. You can also fetch the XML directly:

```
curl -s https://ogrenotes.example.com/api/v1/auth/saml/metadata
```

The body declares `WantAssertionsSigned="true"` (assertions MUST be
signed) and a single HTTP-POST AssertionConsumerService at
`/api/v1/auth/saml/acs`.

## 2. Configure the IdP

### Okta

1. **Applications → Create App Integration → SAML 2.0 → Next**.
2. **General Settings**: name = "OgreNotes (<workspace name>)".
3. **Configure SAML**:
   - **Single sign-on URL**: `https://ogrenotes.example.com/api/v1/auth/saml/acs`
   - **Audience URI (SP Entity ID)**: `https://ogrenotes.example.com/api/v1/auth/saml/metadata`
   - **Name ID format**: `EmailAddress`
   - **Application username**: `Email`
   - **Signature Algorithm**: **RSA-SHA256** (NOT SHA-1 — see "Common errors")
   - **Digest Algorithm**: **SHA-256**
   - **Attribute Statements**:
     - Name `email`, value `user.email`
     - Name `name`, value `user.displayName` (or `user.firstName + " " + user.lastName`)
4. **Finish**. On the "Sign On" tab, click **View SAML setup
   instructions** and copy the **Identity Provider metadata** XML
   from the bottom of that page.

### Entra ID (Azure AD)

1. **Enterprise applications → New application → Create your own
   application → Integrate any other application you don't find in
   the gallery**.
2. **Single sign-on → SAML**.
3. **Basic SAML Configuration**:
   - **Identifier (Entity ID)**: `https://ogrenotes.example.com/api/v1/auth/saml/metadata`
   - **Reply URL (ACS)**: `https://ogrenotes.example.com/api/v1/auth/saml/acs`
4. **Attributes & Claims**: add `email` (source `user.mail`) and
   `name` (source `user.displayname`). Remove the default
   `name`/`givenname`/`emailaddress` claims if you only want clean
   short names.
5. **SAML Certificates**: confirm the signing algorithm is
   **SHA-256** (Entra ID's default; an old tenant may need the
   "SAML Signing Certificate → Edit → Signing Algorithm" knob).
6. Copy the **App Federation Metadata URL** and `curl` it to get
   the XML, or download "Federation Metadata XML" directly.

## 3. Save the IdP config in OgreNotes

Back on `/workspaces/<id>/saml`:

1. Paste the IdP's `<EntityDescriptor>` XML into "IdP Metadata XML".
2. Set "IdP Entity ID" to the `entityID` attribute of that
   `EntityDescriptor` root (Okta: `http://www.okta.com/<id>`;
   Entra: `https://sts.windows.net/<tenant>/`).
3. Leave "Email attribute name" = `email`, "Name attribute name" =
   `name` unless you used different attribute names above.
4. **Save**.

## 4. Test login

In a fresh browser tab (or incognito, to avoid an existing
session):

```
https://ogrenotes.example.com/api/v1/auth/saml/login?workspace=<workspace-id>
```

You should be 302'd to the IdP, complete the IdP's challenge, and
land at `/auth/complete`. After hydration you end up at `/`.

## 5. Verify

### Audit event in CloudWatch

Search the API log group for:

```
SamlAssertionAccepted
```

You should see a structured event with `workspace_id=<your ws>` and
`name_id=<your IdP NameID>`. The event fires before the session
cookie is minted, so its presence is the durable record that the
signed assertion passed every check.

### User row

The JIT-created user has `provider=saml` and `external_id=<NameID>`.
Find them in DynamoDB:

```
aws dynamodb query \
  --table-name ogrenotes-test \
  --index-name GSI6-external-id \
  --key-condition-expression "external_id_gsi = :nid" \
  --expression-attribute-values '{":nid":{"S":"<NameID>"}}'
```

### A second login

Sign out (DELETE /auth/session). Hit the login URL again. The
audit event fires a second time but **no new user row** is created
— the second-stage external_id lookup hits the row from the first
login.

## Verify MFA enforcement (gap-003)

OgreNotes enforces SP-side MFA on SAML logins. To prove it:

1. Log into OgreNotes via OAuth (or SAML for a non-MFA user).
2. Enroll TOTP at `/auth/mfa-enroll`.
3. Sign out.
4. Hit `/api/v1/auth/saml/login?workspace=<id>` and complete the
   IdP's flow as the same user.
5. Instead of landing at `/`, you should be redirected to
   `/auth/mfa-challenge?handle=…`. Enter your TOTP code to finish.

If step 5 lands at `/` without an MFA prompt, the gate is broken
— file a P0.

## Common errors → what they mean

The ACS handler collapses every rejection into `401 Unauthorized`
on the wire so an attacker can't tell which check failed. The
structured `tracing::warn!` line in the API log distinguishes them:

| Log message | Cause |
|-------------|-------|
| `SAML ACS hit with no RelayState` | IdP didn't echo RelayState. Re-check the IdP config or use SP-initiated login instead of IdP-initiated. |
| `ACS hit for workspace with no SAML config` | The RelayState workspace_id has no DDB row. Either the admin deleted the config or the IdP is sending the wrong RelayState. |
| `SAML response is not valid base64` | IdP sent malformed body. Check the POST body and the IdP's binding setting. |
| `SAML response uses SHA-1 algorithm` | Production rejects SHA-1 digests / signatures. Switch the IdP to RSA-SHA256 + SHA-256. |
| `SAML response contains DTD construct` | Response includes a `<!DOCTYPE>` or `<!ENTITY>` declaration. Real IdPs don't emit DTDs in SAML responses; either a misconfigured IdP or an XXE/SSRF probe. |
| `stored IdP metadata exposes no signing certs` | Workspace SAML config was written without a signing cert. PUT a fresh metadata blob via `/workspaces/:id/saml`. |
| `SAML response failed validation` | xmlsec1 rejected the signature, OR Destination / Issuer / Audience / NotBefore / NotOnOrAfter failed. Check the assertion XML against the signing cert in the IdP metadata. |
| `SAML assertion replay detected` | A second SAMLResponse with the same assertion ID arrived within the 90-second TTL. Expected if you double-submit; suspicious otherwise. |
| `SAML assertion missing Subject NameID` | IdP's NameID format isn't set. Force `EmailAddress` (or `unspecified` with a non-empty value). |
| `SAML assertion missing email attribute` | The configured email attribute name doesn't match what the IdP sends. Adjust either side; the page lets you change attribute names without re-uploading metadata. |

## Cleanup

Remove the SAML config on the SP side:

```
DELETE /api/v1/workspaces/<id>/saml-config
```

(Or click **Remove** on the workspace SAML page.) Deletes the DDB
row but leaves any JIT-created users in place — those users can no
longer log in via SAML (the workspace has no config) but their
data is intact.

To fully remove a user, delete them via the admin UI.

## Why no automated happy-path test

There is one — `crates/api/tests/test_saml_acs_happy.rs` mints a
SHA-256-signed assertion, stores a pending `AuthnRequest`, POSTs it
to the real `/auth/saml/acs`, and asserts `200` + session cookie
(plus a replay test asserting the second POST is `401`). It just
**can't run on any runtime we have**, so it skips.

The blocker is NOT what it looks like. The obvious story —
samael 0.0.18's `Signature::template()` hardcodes
`DigestAlgorithm::Sha1`, production rejects SHA-1 (commit
`0013eb5`), and Fedora's crypto-policies block SHA-1 signing — is a
red herring. The test sidesteps all of it by signing through the
`xmlsec1` CLI, which mints a perfectly valid SHA-256 assertion
(verified by a CLI sign→verify round-trip) with no samael involvement.

The real blocker is on the **verify** side, which is the production
code itself: samael 0.0.18's in-process xmlsec bindings abort at
`xmldsig.c:442` with `signValueNode == NULL` — on BOTH signing and
verifying, *before any crypto runs*. It's a binding/ABI
incompatibility with the host xmlsec1, not a signature problem: the
`xmlsec1` CLI verifies the same assertion fine on the same host.
Confirmed broken on the Fedora 43 dev box (xmlsec1 1.2.41) **and**
on GitHub's `ubuntu-latest` CI runner (we enforced the test there
once via `OGRE_REQUIRE_SAML_HAPPY=1` and it hard-failed with exactly
that error). So samael's verify path can't be exercised in-process
anywhere available — which is why the happy path lands here as a
manual check.

The test is wired with a capability probe (sign via CLI → verify via
samael): it **runs and asserts for real on any runtime where
in-process verify works**, and skips otherwise. The day a runner
ships a working xmlsec, it self-activates with no code change; set
`OGRE_REQUIRE_SAML_HAPPY=1` there to make the skip a hard failure and
lock in the coverage. Until then, this runbook against a real
production-grade IdP IS the happy-path verification; the rejection-
branch ACS tests (`test_saml_acs.rs`) plus this test's always-on
signing smoke test cover everything that can run automatically.
