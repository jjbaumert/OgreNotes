# Quip Templates

## Reference Documentation

- **Quip Template Gallery:** <https://quip.com/templates>
- **Quip Automation API:** <https://quip.com/dev/automation/documentation/current>
- **Quip Admin Learning Path:** <https://quip.com/training/work-anywhere-quip-admin-learning-path>

---

## Template Model

Templates are not a separate data type -- they are **regular documents with a boolean `is_template` flag**. Any document, spreadsheet, or slide deck can be marked as a template. The Template Library is a filtered view of flagged documents organized into galleries.

---

## Template Types

| Type | Description |
|------|-------------|
| **Documents** | Rich text documents with embedded components (primary template type) |
| **Spreadsheets** | Spreadsheets with 400+ functions, cell references |
| **Slides** | Slide decks with embedded charts (retired January 2021) |

---

## Creating Templates

1. Open any existing Quip document
2. Go to **Document menu > Mark as Template**
3. Document automatically appears in the Template Library
4. **Lock** the template to prevent accidental edits (unlock to modify later)
5. Optionally add Salesforce Live Apps and mail merge fields
6. Share with collaborators and set access levels
7. Best practice: change collaborator access to "comment only" after publishing

---

## Template Library UI

### Gallery Types

| Gallery | Description |
|---------|-------------|
| **Created by Me** | Templates the user authored |
| **Shared with Me** | Templates shared by others |
| **Sample Templates** | Pre-built templates from Quip |
| **Company Galleries** | Admin-created organization-wide curated galleries |

### Browsing

Users browse galleries, select a template, and click **"Try Template"** to create a copy.

---

## Admin Template Management

- Admins publish company-wide templates via the **Quip Admin Console**
- Company galleries allow thematic organization of templates
- Admins can create, name, and curate custom galleries
- Feature introduced March 2021 (Web and Desktop v7.32.0+)

---

## Template Usage

### From Within Quip

1. Open Template Library
2. Browse galleries
3. Select template and click "Try Template"
4. Copy created in user's Private folder by default
5. Copy can be moved to other folders

### From Within Salesforce

- **Quip Document Component** on Lightning record pages
- When configured with a template, auto-creates a document associated with the current record
- Mail merge fields and Live Apps auto-populate with record data

### Via Automation

- Salesforce Flow or Process Builder triggers document creation from templates
- Default destination: running user's Private folder
- Flow can then move, share, and attach to records

---

## Template Variables / Mail Merge

### API-Level Variables (copy-document endpoint)

Uses `[[variable_name]]` double-bracket syntax in document body.

When calling `POST /1/threads/copy-document` with a `values` JSON parameter, the API scans for `[[varname]]` patterns and replaces them.

| Feature | Details |
|---------|---------|
| Syntax | `[[variable_name]]` |
| Nested/dot notation | `[[user.name]]` looks up `values["user"]["name"]` |
| Allowed key characters | `A-Z, a-z, 0-9, . (dot), _ (underscore)` |
| Value types | Strings, numbers, nested dictionaries |
| Behavior without `values` | Plain copy, no variable substitution |

**Example API call:**

```bash
curl https://platform.quip.com/1/threads/copy-document \
  -d 'thread_id=LeSAAAqaCfc' \
  -d 'values={"user": {"name": "Arnie", "age": "34"}}' \
  -d 'folder_ids=UGaAOAjCHcK' \
  -H 'Authorization: Bearer ${ACCESS_TOKEN}'
```

### Salesforce Mail Merge Fields

- Syntax: `[[Object.FieldName]]` (e.g., `[[Account.Name]]`, `[[Account.Id]]`)
- Double brackets, period separator, proper capitalization
- Pulls **static** data from Salesforce record at creation time
- Works with standard and custom fields

### Salesforce Template Data Mentions

- Type `@Salesforce` in the template to insert a live data mention
- **Dynamic/live** -- stays updated as Salesforce record changes
- Bidirectional sync -- edits in Quip write back to Salesforce

---

## Template Sharing and Permissions

| Level | Description |
|-------|-------------|
| **Full Access** | Share, edit, manage permissions |
| **Can Edit** | Edit and comment, cannot share |
| **Can Comment** | View and comment, no editing |
| **Can View** | Read-only |
| **Must Request Access** | Restricted, requires approval |

Best practices:
- Set "Can Edit" for collaborative team members
- After publishing, change most collaborators to "Can Comment" to protect the template
- Use template locking as additional safeguard

---

## Built-In / Default Templates (~45+)

Organized by category at `quip.com/templates`:

### Account Strategy & Planning
- Account Plan, Enterprise Account Plan, Territory Plan

### Deal Qualification
- Discovery Call Notes, Lead Handover, Sales Playbook

### Closing the Deal
- Sales Battlecard, Mutual Close Plan, Collaborative Pricing Proposal, Sales Team Deal Room, Executive Briefing

### Post Sales & Customer Success
- Account Management Plan, Account Transition Plan, Project Hub, Success Review, QBR, Renewal Plan

### Service
- Case Swarm, Root Cause Analysis

### Marketing
- Campaign Plan, Pipeline Review, Event Plan, Marketing Playbook

### Human Resources
- Onboarding Checklist, Employee Coaching Guide, Performance Review, Individual Development Plan

### General
- Meeting Notes, Client Notes, 1:1 Meeting Notes, Company Wiki, Team Hub, Project Hub

### Financial Services
- Client Review, Proposal Plan, Commercial Banking Relationship Plan, Commercial Loan Close Plan

### Manufacturing
- Account Plan, Pursuit Plan

---

## Salesforce Automation with Templates

### Flow Builder Actions

| Action | Description |
|--------|-------------|
| **Create Quip Document** | From template or blank |
| **Clone Quip Document** | Duplicate named template |
| **Edit Quip Document** | Modify existing documents |
| **Store Data in Quip Document** | Insert up to 10 values into specific cells |
| **Get Quip Sheet Data** | Retrieve spreadsheet data |
| **AddRowToQuipSheet** | Add/update rows with Salesforce data |
| **Add User to Folder** | Manage access |
| **Add Document to Folder** | Organize documents |

### Cell Targeting Methods (Store Data)

| Method | Description |
|--------|-------------|
| Label reference | Right of or below a label |
| Column/row intersection | Column header + row label |
| Absolute cell address | E.g., B2 |

### Common Patterns

- Opportunity stage change triggers templated document creation
- Auto-populate with record data via Data Mentions
- Auto-add team members and share
- Export to PDF and attach to Opportunity on close

---

## Template Adoption Metrics

- **Quip Engagement Metric Objects** in Salesforce track template usage
- Metrics: document views, comments, edits, Lightning Charts created
- Enable: Setup > Quip > Turn On Quip Metrics
- Build Lightning Reports and Dashboards on metric objects
- Create Tableau CRM dashboards for deeper analytics

---

## Template API Endpoints

### Copy Document (Primary Template Endpoint)

`POST /1/threads/copy-document`

| Parameter | Required | Description |
|-----------|----------|-------------|
| `thread_id` | Yes | Source document/template to copy |
| `values` | No | JSON dictionary for `[[variable]]` substitution |
| `folder_ids` | No | Comma-separated folder IDs for placement |
| `member_ids` | No | Comma-separated user IDs to add as members |
| `title` | No | Title for the new document |

**Response:** `{ "id": "<new_thread_id>" }`

### V2 Copy Document

`POST /2/threads/{threadIdOrSecretPath}/copy`

| Parameter | Required | Description |
|-----------|----------|-------------|
| `values` | No | JSON for variable substitution |
| `folder_ids` | No | Folder placement |
| `member_ids` | No | User sharing |
| `title` | No | New title |

### Admin Copy Document

`POST /1/admin/threads/copy-document` -- admin-level variant.

### Related Endpoints

| Method | Path | Description |
|--------|------|-------------|
| POST | `/1/threads/new-document` | Create from scratch (HTML/Markdown) |
| POST | `/1/threads/edit-document` | Edit existing document |
| GET | `/1/threads/{id}` | Retrieve document metadata and content |

### Live Apps Template Parameters

- `quip.apps.updateTemplateParams(params, isTemplate)` -- set template parameters with mail-merge values
- `quip.apps.getTemplateParams()` -- retrieve template parameters
