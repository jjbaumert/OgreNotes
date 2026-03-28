# Clean Room Quip Application Design

## Overview

This document describes a clean room implementation of a Quip-compatible collaborative
document application. It serves as a reference for agents building the application,
pointing to the public documentation and API specifications that define Quip's behavior.

## Reference Documentation

### API Documentation

- **Quip Automation API (current):** <https://quip.com/dev/automation/documentation/current>
- **OpenAPI Specification (YAML):** <https://quip.com/dev/automation/documentation/current/openapi-specs>
- **OpenAPI Info:** <https://quip.com/dev/automation/documentation/current/openapi-info>
- **All API versions (including deprecated):** <https://quip.com/dev/automation/documentation/all/openapi-specs>
- **Admin API:** <https://quip.com/dev/admin/documentation/current/openapi-specs>
- **SCIM API:** <https://quip.com/dev/scim/documentation/current/openapi-specs>

### Product Documentation

- **Getting Started / Training:** <https://quip.com/training/get-started-with-work-anywhere-quip>
- **Salesforce Quip Usage Guide:** <https://help.salesforce.com/s/articleView?id=xcloud.quip_use.htm&type=5>

### Sample Client Library

- **Quip API client library:** <https://quip.com/dev/automation/documentation/current>

---

## API Summary

### Base URL

- Standard: `https://platform.quip.com`
- VPC: `https://platform.{customer}.onquip.com` or `https://platform.quip-{customer}.com`

### Authentication

- **OAuth 2.0** authorization code flow
  - `GET /1/oauth/login` -- authorization
  - `POST /1/oauth/access_token` -- token exchange (grant types: `authorization_code`, `refresh_token`)
  - `POST /1/oauth/revoke` -- revoke token
  - `GET /1/oauth/verify_token` -- verify token
- **Personal access tokens** for development: <https://quip.com/dev/token>
- **Scopes:** `USER_READ`, `USER_WRITE`, `USER_MANAGE`
- Tokens expire every 30 days; refresh before expiration

### Core Data Model

| Entity    | Description |
|-----------|-------------|
| **Thread**   | Central unit. Contains documents, spreadsheets, or chats. 11-char ID, 12-char URL secret. |
| **Section**  | Sub-unit of a document (paragraph, list item, table cell). Each has a section ID in HTML output. |
| **Folder**   | Tag-like grouping. A thread can belong to multiple folders. Inherits permissions. Special folders: desktop, archive, starred, private, trash. |
| **Member**   | People in threads/folders. Access levels: `own`, `edit`, `comment`, `view`. |
| **Message**  | Chat/comment within a thread. Contains author, text, files, annotations, likes, mentions. |
| **User**     | Has id, name, company_id, emails, profile picture, affinity score, is_robot flag. |
| **Blob**     | Binary data (images, files) attached to threads. |

### API Endpoints

#### Threads

| Method | Path | Operation |
|--------|------|-----------|
| GET    | `/2/threads/{id}` | Get thread |
| GET    | `/2/threads/` | Get threads (batch) |
| GET    | `/1/threads/recent` | Get recent threads |
| GET    | `/1/threads/search` | Search threads |
| POST   | `/1/threads/new-document` | Create document/spreadsheet |
| POST   | `/1/threads/edit-document` | Edit document (insert/replace/delete by section) |
| POST   | `/1/threads/copy-document` | Copy document (v1) |
| POST   | `/2/threads/{id}/copy` | Copy document (v2) |
| POST   | `/1/threads/delete` | Delete thread |
| POST   | `/1/threads/add-members` | Add members to thread |
| POST   | `/1/threads/remove-members` | Remove members from thread |
| POST   | `/1/threads/edit-share-link-settings` | Edit link sharing |
| POST   | `/1/threads/lock-edits` | Lock/unlock thread |
| POST   | `/1/threads/lock-section-edits` | Lock/unlock section |
| POST   | `/1/threads/live-paste` | Create live paste section |
| POST   | `/1/threads/new-chat` | Create chat room |
| GET    | `/2/threads/{id}/html` | Get thread HTML |
| GET    | `/2/threads/{id}/members` | Get thread members |
| GET    | `/2/threads/{id}/invited-members` | Get invited members |
| GET    | `/2/threads/{id}/folders` | Get thread folders |

#### Blobs & Export

| Method | Path | Operation |
|--------|------|-----------|
| POST   | `/1/blob/{thread_id}` | Upload blob |
| GET    | `/1/blob/{thread_id}/{blob_id}` | Download blob |
| GET    | `/1/threads/{id}/export/docx` | Export to .docx |
| GET    | `/1/threads/{id}/export/xlsx` | Export to .xlsx |
| GET    | `/1/threads/{id}/export/pdf` | Export slides to .pdf |
| POST   | `/1/threads/{id}/export/pdf/async` | Start async PDF export |
| GET    | `/1/threads/{id}/export/pdf/async` | Get async PDF export result |
| POST   | `/1/threads/export/async` | Start bulk export |
| GET    | `/1/threads/export/async` | Get bulk export result |

#### Folders

| Method | Path | Operation |
|--------|------|-----------|
| GET    | `/1/folders/{id}` | Get folder |
| GET    | `/1/folders/` | Get folders (batch) |
| POST   | `/1/folders/new` | Create folder |
| POST   | `/1/folders/update` | Update folder |
| POST   | `/1/folders/add-members` | Add members |
| POST   | `/1/folders/remove-members` | Remove members |
| GET    | `/2/folders/{id}/link-sharing-settings` | Get link sharing settings |
| PUT    | `/2/folders/{id}/link-sharing-settings` | Edit link sharing settings |

#### Messages

| Method | Path | Operation |
|--------|------|-----------|
| POST   | `/1/messages/new` | Add message |
| GET    | `/1/messages/{thread_id}` | Get recent messages |

#### Users

| Method | Path | Operation |
|--------|------|-----------|
| GET    | `/1/users/{id}` | Get user (by ID or email) |
| GET    | `/1/users/` | Get users (batch, up to 1000) |
| GET    | `/1/users/current` | Get current user |
| GET    | `/1/users/contacts` | Get contacts |
| GET    | `/1/users/current/threads` | Get current user's threads (paginated) |
| GET    | `/1/users/current/threads-modified-after-usec` | Get threads modified after timestamp |
| GET    | `/1/users/read-only` | Get read-only users |
| POST   | `/1/users/update` | Update user |

#### Realtime

| Method | Path | Operation |
|--------|------|-----------|
| GET    | `/1/websockets/new` | Create websocket connection |

### Rate Limits

| Scope | Limit |
|-------|-------|
| Per-user | 50 requests/minute, 750 requests/hour |
| Per-company | 600 requests/minute |
| Bulk export | 36,000 documents/hour per company |

Rate limit headers: `X-Ratelimit-Limit`, `X-Ratelimit-Remaining`, `X-Ratelimit-Reset`

### Response Format

- JSON responses
- Errors: standard HTTP status codes with JSON body (`error`, `error_code`, `error_description`)
- Pagination: `cursor` parameter, `response_metadata.next_cursor` in response

---

## Product Capabilities

The following capabilities are documented in the Quip product and should inform clean room design:

- Real-time collaborative editing of documents and spreadsheets
- Live Apps: Project Tracker, Kanban Board, Process Bar, Relationship Map
- Inline and pane-based comments/conversations
- Sharing with granular permission controls (own, edit, comment, view)
- Folder-based organization (including special folders: desktop, archive, starred, private, trash)
- Notification customization
- Document export (docx, xlsx, pdf)
- Blob/file attachments
- Websocket-based realtime updates
- Salesforce integration (data mentions, activity logging)
- Slack integration

---

## Notes for Implementation Agents

1. **Start with the OpenAPI spec.** The YAML spec at the OpenAPI URL above is the authoritative, machine-readable API definition. Parse it directly for exact request/response schemas, required fields, and enum values.

2. **Use v2 endpoints where available.** V1 endpoints exist for backwards compatibility; prefer `/2/` paths.

3. **Respect rate limits.** Implement exponential backoff and honor the rate limit headers.

4. **The Salesforce help article** at the URL above may require browser access. If automated fetching fails, reference the Quip training materials and API docs instead.

5. **Thread is the central abstraction.** Documents, spreadsheets, and chats are all threads with different types. Design the data model around this.

6. **Folders are tags, not directories.** A thread can belong to multiple folders. Do not model folders as a strict tree hierarchy.