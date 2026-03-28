# Quip Admin Console and User Management

## Reference Documentation

- **Admin API:** <https://quip.com/dev/admin/documentation/current>
- **SCIM API:** <https://quip.com/dev/scim/documentation/current>
- **Automation API:** <https://quip.com/dev/automation/documentation/current>
- **Admin Console Help:** <https://help.salesforce.com/s/articleView?id=000392613&type=1>
- **Admin Roles Help:** <https://help.salesforce.com/s/articleView?id=000393323&type=1>
- **Admin Learning Path:** <https://quip.com/training/work-anywhere-quip-admin-learning-path>
- **SCIM Integration Help:** <https://help.salesforce.com/s/articleView?id=000392638&type=1>

---

## Admin Console Overview

Accessible at `{company}.quip.com/admin`. Key sections:

| Section | Description |
|---------|-------------|
| **People/Members** | List, search, sort, provision, deactivate, reactivate, merge accounts, promote to admin |
| **Settings** | Site Settings, Security, Integrations, User Defaults |
| **Templates** | Template Library with galleries |
| **External Sharing** | Visibility into externally-shared documents, remove external users |
| **Managed Sites** | Create Partner Sites and Testing Sites |
| **Admin Action Log** | Tracks all admin activities |

---

## User Management

### Member Operations

- Sort, filter, and search the member list
- Provision new members (customizable invitation emails)
- Deactivate and reactivate members
- Merge duplicate accounts (SCIM: `POST /2/UserMerges`)
- Transfer content from deactivated users (`POST /2/PrivateContent`)
- Mark users as read-only (`GET/POST /1/admin/users/read-only`)
- Revoke personal access tokens (`POST /1/admin/users/revoke-pat`)
- Revoke sessions (`POST /1/admin/users/revoke-sessions`)
- Create bot/placeholder users for integrations

### Admin Roles

- Default: **Super Admin** (all permissions)
- Custom permission profiles can be created and assigned
- Example permission: "Unredacted Audit Access to All Members & Content"

### Admin Role API

| Method | Path | Description |
|--------|------|-------------|
| GET | `/1/admin/company/admin-roles` | List all roles |
| GET | `/1/admin/users/admin-role` | Get user's admin role |
| POST | `/1/admin/users/admin-role` | Set admin role |
| DELETE | `/1/admin/users/admin-role` | Remove admin role |

---

## User Profiles

### User Object (Automation API)

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | Unique user ID (11-char) |
| `name` | string | Full name |
| `emails` | string[] | Email addresses |
| `company_id` | string | Company ID |
| `disabled` | boolean | Whether deactivated |
| `created_usec` | int64 | Creation timestamp (microseconds) |
| `is_robot` | boolean | Bot user flag |
| `affinity` | double (0-1) | Interaction frequency/recency with authenticated user |
| `chat_thread_id` | string | Direct message thread ID |
| `profile_picture_url` | string | Avatar URL |
| `desktop_folder_id` | string | Home folder (own user only) |
| `archive_folder_id` | string | Archive folder (own user only) |
| `starred_folder_id` | string | Starred folder (own user only) |
| `private_folder_id` | string | Private folder (own user only) |
| `trash_folder_id` | string | Trash folder (own user only) |
| `shared_folder_ids` | string[] | Shared folders (own user only) |
| `group_folder_ids` | string[] | Group folders (own user only) |

### SCIM User Schema (Additional Fields)

- `externalId` -- external identity provider ID
- `userName` -- set by Quip, immutable
- `name` -- structured object: `givenName`, `familyName`, `formatted`

### Update User

`POST /1/users/update` with `picture_url` parameter.

---

## Organization / Company Settings

- **Company ID** -- unique organization identifier
- **Managed Sites** -- create subsidiary sites (Partner Sites, Testing Sites)
  - Partner Site members get blue badges
  - Testing Site members get green badges
- **Collaborating Companies** -- track external organizations sharing content

### Managed Sites API

| Method | Path | Description |
|--------|------|-------------|
| GET | `/1/admin/company/managed-sites` | List managed sites |
| POST | `/1/admin/company/managed-sites` | Create sites (name + subdomain) |
| DELETE | `/1/admin/company/managed-sites` | Delete sites |

### Collaborating Companies API

| Method | Path | Description |
|--------|------|-------------|
| POST | `/1/admin/company/collaborating-companies/list` | List collaborating companies |
| GET | `/1/admin/company/collaborating-companies/{id}` | Get details |

---

## Corporate Colors

- Admin Console > User Defaults tab
- Up to **30 custom colors**
- Appear in color pickers throughout Quip (text, backgrounds, charts)
- Changes take effect immediately (users may need to refresh)
- Company-wide -- all users see the same palette

---

## Template Management

- **Template Library** -- any document marked as template is added
- Galleries: "Created by Me", "Shared with Me", and company-wide
- Admins build galleries from the Admin Console
- Custom slide layout templates configurable company-wide
- Mail-merge via Copy Document API (`POST /1/threads/copy-document` with `values`)
- Template types: documents, spreadsheets, slides
- Automatable via Salesforce Process Builder and Flow
- Admin API: `POST /1/admin/threads/copy-document`

---

## User Provisioning

### Manual

Admins invite users via email from Admin Console with customizable invitation messages.

### SSO (SAML 2.0)

- IdP-initiated SSO, SP-initiated SSO, Just-in-Time (JIT) provisioning
- Compatible with: Okta, Azure AD, JumpCloud, other SAML 2.0 IdPs
- Requires contacting Quip to enable SAML with metadata.xml

### SCIM

- Automatic provisioning, updating, and deprovisioning
- Supports SCIM v1.1 and v2.0
- Token from `{company}.quip.com/business/admin/scim`
- Compatible with: Okta, Azure AD, JumpCloud, OneLogin

---

## SCIM API

**Base URL:** `https://scim.quip.com`
**Auth:** Bearer token
**Rate limit:** 600 requests/minute per company

### Users

| Method | Path | Description |
|--------|------|-------------|
| GET | `/2/Users` | List users (filter, count, startIndex, sortBy, sortOrder) |
| POST | `/2/Users` | Create user (required: name, emails) |
| GET | `/2/Users/{userId}` | Get single user |
| PUT | `/2/Users/{userId}` | Full replace update |
| PATCH | `/2/Users/{userId}` | Partial update |
| DELETE | `/2/Users/{userId}` | Deactivate user |
| DELETE | `/2/Users/{userId}/sharedThreads` | Remove shared threads from disabled user |

### Groups

| Method | Path | Description |
|--------|------|-------------|
| GET | `/2/Groups` | List groups |
| POST | `/2/Groups` | Create group (required: displayName) |
| GET | `/2/Groups/{groupId}` | Get group |
| PATCH | `/2/Groups/{groupId}` | Update group |
| DELETE | `/2/Groups/{groupId}` | Delete group |

### Group Folders

| Method | Path | Description |
|--------|------|-------------|
| GET | `/2/GroupFolders` | List group folders |
| POST | `/2/GroupFolders` | Create group folder (required: displayName, members) |
| GET | `/2/GroupFolders/{id}` | Get group folder |
| PATCH | `/2/GroupFolders/{id}` | Update group folder |

### User Merges and Content Transfer

| Method | Path | Description |
|--------|------|-------------|
| GET | `/2/UserMerges` | Get merge status |
| POST | `/2/UserMerges` | Merge accounts (source_user_id + target_user_id) |
| POST | `/2/PrivateContent` | Transfer private content between users |

---

## Admin API Endpoints

**Base URL:** `https://platform.quip.com`
**Auth:** OAuth 2.0 with scopes: `ADMIN_READ`, `ADMIN_WRITE`, `ADMIN_MANAGE`
**Rate limits:** 100 req/min, 1500 req/hr per user; 600 req/min per company

### Threads (Admin)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/2/admin/threads/{id}` | Get thread |
| GET | `/2/admin/threads/` | Get threads (bulk) |
| POST | `/1/admin/threads/list` | List threads |
| GET | `/2/admin/threads/search` | Search threads |
| POST | `/1/admin/threads/new-document` | Create document |
| POST | `/1/admin/threads/copy-document` | Copy document/template |
| POST | `/1/admin/threads/edit-document` | Edit document |
| POST | `/1/admin/threads/delete` | Delete thread |
| POST | `/1/admin/threads/add-members` | Add members |
| POST | `/1/admin/threads/remove-members` | Remove members |
| POST | `/1/admin/threads/remove-external-members` | Remove all external users |
| POST | `/1/admin/threads/edit-share-link-settings` | Edit link settings |
| POST | `/1/admin/threads/external/list` | List externally shared threads |

### Users (Admin)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/2/admin/users/{id}` | Get user info |
| POST | `/1/admin/users/list` | List users at company |
| GET | `/2/admin/users/{id}/threads` | Get user's threads |
| GET/POST | `/1/admin/users/read-only` | Get/set read-only status |
| POST | `/1/admin/users/revoke-pat` | Revoke personal access tokens |
| POST | `/1/admin/users/revoke-sessions` | Revoke sessions |

### Messages (Admin)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/1/admin/messages/{thread_id}` | Get messages for thread |
| GET | `/1/admin/message/{message_id}` | Get single message |
| POST | `/1/admin/message/delete` | Delete message |

### Events (Add-On)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/1/admin/events/1/cursor/create` | Get event cursor |
| GET | `/1/admin/events/1/cursor/realtime/create` | Get realtime cursor |
| GET | `/1/admin/events/1/events/get` | Get next batch of events |
| GET | `/1/admin/events/1/events/realtime/get` | Get realtime events |

### Event Types Monitored

- Thread: `open-thread`, `create-thread`, `delete-thread`, `share-thread`, `unshare-thread`, `move-thread`, `join-thread`, `joined_by_link-thread`, `receive_in_folder-thread`
- Message: `create-message`, `edit-message`, `delete-message`
- Folder: `create-folder`, `delete-folder`, `open-folder`, `move-folder`, `copy-folder`, `share-folder`, `unshare-folder`
- User: `login`, `create-user`, `disable-user`, `receive-invite`
- Admin: `admin_edit`, `admin_api_call`
- Export: `print-document`, `export-thread`, `upload_blob-thread`

### Quarantine

| Method | Path | Description |
|--------|------|-------------|
| POST | `/1/admin/quarantine` | Quarantine an object |
| DELETE | `/1/admin/quarantine` | Remove from quarantine |

### Governance (Add-On)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/1/admin/data-hold-policy` | Get data holds |
| POST | `/1/admin/data-hold-policy` | Create data hold |
| DELETE | `/1/admin/data-hold-policy` | Retire data hold |
| POST | `/1/admin/data-hold-policy/users` | Add users to hold |
| DELETE | `/1/admin/data-hold-policy/users` | Remove users from hold |
| GET | `/1/admin/retention-policy` | Get retention policies |
| POST | `/1/admin/retention-policy` | Add thread to retention |

### API Keys

| Method | Path | Description |
|--------|------|-------------|
| GET | `/2/admin/api-keys` | List API keys |
| POST | `/1/admin/api-keys` | Create API key |
| DELETE | `/1/admin/api-keys` | Revoke API key |
| GET | `/1/admin/token/get-info` | Get token details |

---

## Security

### Encryption

- AES 256-bit at rest
- TLS 1.2+ in transit

### Quip Shield (Add-On)

| Feature | Description |
|---------|-------------|
| Enterprise Key Management (EKM) | Create, manage, control encryption keys; granular revocation |
| Event Monitoring | Real-time event logs for SIEM/CASB |
| DLP Integration | Scan for sensitive data patterns |
| CASB Integration | Cloud access security broker support |

---

## Compliance and Audit

- **Admin Action Log** -- tracks all admin activities
- **Data Hold policies** -- preserve content for legal holds (states: PENDING, ACTIVE, RETIRED)
- **Data Retention policies** -- configurable periods with actions: No action, Move to trash, Delete immediately
- **eDiscovery** -- via Admin API; partners: Onna, 17a-4 DataParser
- **Quarantine** -- hide content and block edits, reversible

---

## Domain Management

- Custom domain: `{company}.quip.com` or `{company}.quipdomain.com`
- **Domain Authentication** -- Enterprise-only; OAuth 2.0 where admins pre-approve apps
- **VPC** -- custom domains like `customername.onquip.com`

---

## Integration Management

- **API Keys** -- up to 100 per company
- **Scopes**: `ADMIN_READ`, `ADMIN_WRITE`, `ADMIN_MANAGE` (Admin); `USER_READ`, `USER_WRITE`, `USER_MANAGE` (Automation)
- **Salesforce org access** -- allowlist which orgs can connect
- **OAuth 2.0** -- 30-day token expiration with refresh
- **Integrations**: Slack, Google Workspace, GitHub, Jira, Stripe, Dropbox, Box, Zendesk

---

## User API Endpoints (Automation)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/1/users/current` | Get authenticated user (all folder IDs) |
| GET | `/1/users/{id}` | Get user by ID |
| GET | `/1/users/` | Get multiple users (comma-separated IDs) |
| GET | `/1/users/contacts` | Get contacts (affinity-ordered) |
| POST | `/1/users/update` | Update user (picture_url) |
| GET | `/1/users/read-only` | Check read-only status |
| GET | `/1/users/current/threads` | Get current user's threads |
| GET | `/1/users/current/threads-modified-after-usec` | Threads modified after timestamp |
