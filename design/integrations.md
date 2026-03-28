# Quip Integrations

## Reference Documentation

- **Quip Automation API:** <https://quip.com/dev/automation/documentation/current>
- **Quip Live Apps API:** <https://quip.com/dev/liveapps/>
- **Quip API Samples (GitHub):** <https://github.com/quip/quip-api>
- **Quip Apps (GitHub):** <https://github.com/quip/quip-apps>

---

## Salesforce Integration

### Data Mentions (Inline Live Data)

- `@mention` any standard or custom Salesforce field inline in a document or spreadsheet
- Mentioned fields become dynamic, live-syncing elements that stay current with Salesforce
- Fields are editable inline -- changes can be saved back to Salesforce bidirectionally
- Pasting a Salesforce record URL auto-converts to a Data Mention or Live App
- **Template Data Mentions** (`@Salesforce Template Data Mention`) allow building templates that auto-populate with record data using picklist-based field selection

### Salesforce Record Live App

- Embeds a full Salesforce record view inside a Quip document
- Team members update and comment on field data directly within Quip
- Changes sync back to Salesforce
- Record relationships visible in Quip navigation (icons next to linked documents)

### Quip Lightning Component

- Embeddable on Salesforce record pages (opportunities, cases, accounts)
- Link, access, create, and edit Quip documents within Salesforce
- "Quip Associated Documents" component shows all linked documents

### Salesforce Reports in Quip

- CRM reports opened directly within Quip documents and spreadsheets
- Data can be analyzed, sorted, and formatted within Quip
- Real-time updates from Salesforce

### Activity Logging

- Log calls from Quip documents back to Salesforce
- Collaborate on notes while reporting progress to Salesforce activity history

### Salesforce Flow / Process Builder Actions

| Action | Description |
|--------|-------------|
| **Create Quip Document** | New document, optionally from template with auto-populated data |
| **Create Quip Spreadsheet** | New spreadsheet |
| **Clone Quip Document** | Duplicate a template document |
| **Store Data in Quip Document** | Insert Flow data into specific spreadsheet cells |
| **Create Chat Room** | New chat room with message to members |
| **Send Message** | Post message (including `@everyone`) to chat room |
| **Remove Users from Chat Room** | Remove members |
| **Create Folder / Add to Folder** | Create folders or organize documents |
| **Add Members** | Add users to documents |
| **Attach Document to Record** | Link Quip document to Salesforce record |
| **Lock Document** | Lock edits for record-keeping |
| **Live Paste** | Copy content between documents with auto-updates |

**Typical pattern**: When opportunity changes stage, Flow auto-creates a templated document, populates it with Salesforce metadata via Data Mentions, shares with the team, and attaches to the record.

---

## Slack Integration

### Document Sharing and Preview

- Pasting Quip document links into Slack generates rich previews
- View document summaries without leaving Slack

### Notifications

- @mentions, messages, and edits auto-post to Slack
- Configurable "Document Updated" trigger for designated channels

### Slash Commands

| Command | Description |
|---------|-------------|
| `/quip new [name]` | Create new document |
| `/quip spreadsheet [name]` | Create new spreadsheet |
| `/quip checklist [name]` | Create new checklist |
| `/quip notepad` | Create channel notepad |
| `/quip note [text]` | Add text to channel notepad |
| `/quip task [text]` | Add task to notepad (supports `@username`) |
| `/quip grab` | Capture last Slack message into notepad |
| `/quip grab [N]` | Capture last N messages (up to 100) |

- Markdown support in notes: `[]` for tasks, `*` for bullets, `#` for headers
- Alternative syntax: `@quip: [text]` at message start
- Slack usernames auto-convert to Quip usernames
- Requires `@quip` bot in channel

---

## Third-Party Live Apps

Interactive, embeddable components inside Quip documents. Inserted via `@` menu.

| Partner | Capability |
|---------|------------|
| **Jira (Atlassian)** | Embed issues, manage sprints, track bugs inline |
| **DocuSign** | Author docs, add signature fields, send for signature, view status |
| **Lucidchart** | Create/embed diagrams and flowcharts via `@Lucidchart Diagram` |
| **Smartsheet** | Embed dashboards and sheets |
| **New Relic** | Real-time performance dashboards |
| **Xactly** | Compensation and performance data |
| **Altify** | Relationship maps, update Salesforce from Quip |
| **draw.io** | Org charts, network diagrams, flowcharts |
| **Elements.cloud** | Requirements, process diagrams, Salesforce config |
| **Diffeo** | Research connections between people/companies |
| **PDFFiller** | eSignature within Quip |
| **Box Files Viewer** | Embed Box files and folders |
| **Dropbox** | Insert folders, navigate, view, download files |

> As of March 5, 2025, Quip no longer allows creation of new Live Apps. Existing apps continue functioning.

---

## Live Apps Platform / API

### Architecture

- Built with **React**, running in isolated **iframes** from `quipelements.com`
- Each app gets its own subdomain (same-origin isolation)
- Communication via bridge APIs between iframe and host document
- CSP headers whitelist external domains
- Manifest JSON defines allowed `script_srcs`, `font_srcs`, `img_srcs`, `connect_srcs`

### Key APIs

| API | Description |
|-----|-------------|
| **Collaboration API** | Automatic real-time syncing, editing, commenting; offline and cross-platform |
| **Auth API** | Generic OAuth2 for authenticating against third-party platforms |
| **Proxy API** | API calls to external services via configured proxy domains |
| **Data Persistence** | App data stored within the document; copies include app data |
| **Preferences** | User-specific data storage |

### Developer Tooling

- `quip-cli`: `login`, `init`, `build`, `test`, `publish`, release updates
- `create-quip-app` NPM module
- `quip-apps-webpack-config` for standardized build
- Apps packaged as `.ele` files, uploaded via Developer Console
- Insertable via `@` menu as first-class document elements

---

## Chat Room Integrations

Real-time notifications from external services into Quip chat rooms:

| Service | Notification Type |
|---------|-------------------|
| **GitHub** | Commit notifications |
| **PagerDuty** | Incident alerts (new and resolved) |
| **Crashlytics** | Issue notifications |
| **Jenkins** | Build status updates |
| **Stripe** | Customer activity |
| **Salesforce** | Customer data updates |
| **Twitter** | Brand mentions |
| **Zendesk** | Ticket updates |
| **JIRA** | Ticket updates |
| **iTunes App Store** | App reviews |
| **RSS feeds** | Content updates |
| **SMS/Phone** | Message and call notifications |

All integrated content becomes permanently searchable.

---

## Cloud Storage Integrations

| Service | Integration |
|---------|-------------|
| **Dropbox** | Insert folders into documents; navigate, view, download files inline |
| **Box** | Box Files Viewer Live App; embed files/folders, manage, preview |
| **Google Drive** | Data sharing and collaboration; available via Live Apps and Zapier/IFTTT |

---

## Webhook Support

Quip provides **inbound webhook infrastructure**:

- Sample code: `github.com/quip/quip-api/tree/master/samples/webhooks`
- Stateless Python App Engine server forwarding messages to Quip threads
- Built-in support: GitHub commits, Crashlytics issues, PagerDuty incidents
- Extensible for any service sending HTTP POST payloads

**No native outbound webhooks** -- Quip does not push events to external URLs. The API is pull-based (REST + WebSocket).

---

## Email Integration

- Email messages to team mailing lists can be piped into Quip chat rooms/threads
- Available through integrations framework and IFTTT/Zapier
- No deep native email client integration

---

## Calendar Integration

- Built-in **Calendar Live App** addable to documents
- One of the core native Live App types alongside Kanban Board, Project Tracker, Salesforce Record, Salesforce List

---

## Automation Connectors

### Zapier Integration (25 Actions)

- Trigger: New Folder
- Actions: Create Document, Add Row to Spreadsheet, Send Message, Add Item to List
- Extends connectivity to 300+ apps

### IFTTT

- Similar automation recipes for connecting Quip to external services

---

## API-Based Integration Patterns

### Client Libraries

| Language | Source |
|----------|--------|
| Python | Official (`quip-api` repo) |
| Node.js | Official |
| Ruby | Community |
| Go | Community |
| Elixir | Community |
| Java | Community |

### Sample Applications

| Sample | Description |
|--------|-------------|
| `baqup` | Export all Quip content to local directory |
| `twitterbot` | Post Twitter messages to Quip threads |
| `webhooks` | Inbound webhook handler |
| `wordpress` | Publish Quip documents to WordPress |
| `websocket` | Receive Quip messages in real-time |

### Content Formats

API supports HTML and Markdown for document content creation and editing.

### Convenience Operations

Add items to lists, add/update rows in spreadsheets, toggle checkmarks, merge comments.
