# Quip Import and Export

## Reference Documentation

- **Quip Automation API:** <https://quip.com/dev/automation/documentation/current>
- **Quip Admin API:** <https://quip.com/dev/admin/documentation/current>
- **Import Documents Blog:** <https://quip.com/blog/import-documents>
- **PDF and Printing Blog:** <https://quip.com/blog/pdf-printing>
- **Markdown Blog:** <https://quip.com/blog/markdown>
- **Salesforce Help - Import/Export:** <https://help.salesforce.com/s/articleView?id=000389292&type=1>

---

## Document Import

### Supported Formats (UI)

| Format | Notes |
|--------|-------|
| Microsoft Word (.doc, .docx) | Converted to native Quip documents |
| PDF | Pages converted to **images** (no OCR, not editable text) |
| Dropbox files | Direct import integration |
| Google Drive files | Direct import integration |
| Box files | Direct import integration |
| Evernote files | Direct import integration |
| Email attachments | Mobile import |

### Import Process UI

- **Desktop/Web**: Drag-and-drop files onto Quip, or Gear menu > "Import Documents"
- **Mobile (iOS)**: Tap "Import Document" when creating new document; import from email, iCloud, cloud services
- **Admin controls**: Admins can restrict importing/exporting documents site-wide

### API Document Creation

`POST /1/threads/new-document` accepts content in **HTML** or **Markdown** format:

| Parameter | Description |
|-----------|-------------|
| `content` | HTML or Markdown content (max **1 MB**) |
| `format` | `html` (default) or `markdown` |
| `title` | Document title (max **10 KB**) |
| `type` | `document` or `spreadsheet` |
| `member_ids` | Users/folders to share with |

For spreadsheets, content must be wrapped in `<table>` tags.

`POST /1/threads/edit-document` also accepts HTML or Markdown with same 1 MB limit.

**Important**: There is no API endpoint for importing binary files (.docx, .xlsx, .pptx). File import is UI-only. The API only supports HTML or Markdown content.

---

## Spreadsheet Import

### Supported Formats

| Format | Notes |
|--------|-------|
| Microsoft Excel (.xls, .xlsx) | Primary format |
| CSV | Basic tabular data |
| OpenOffice (ODS) | Open format |

### What Is Preserved (.xlsx Import)

- Hyperlinks
- Images (placed in a separate tab since Quip requires images to be in-cell)
- Merged cells
- Currency number formatting
- Cell borders
- Text color
- Filters
- Data validation

### What Is Not Preserved

- Charts (not mentioned as preserved)
- Conditional formatting (not mentioned)
- Macros/VBA (not supported)
- Formula compatibility varies (Quip supports 400+ functions natively)

---

## Presentation Import

- PowerPoint (.pptx) import was supported for Quip Slides
- Process: Compose > Upload or Import > drag .pptx or browse
- **Quip Slides retired January 31, 2021** -- `new-document` endpoint explicitly states slides are no longer supported

---

## Export Formats (UI)

Available from the gear menu at the top-right of a document:

| Format | Applies To |
|--------|------------|
| Microsoft Word (.docx) | Documents |
| PDF | Documents, spreadsheets |
| Markdown | Documents (copies to clipboard) |
| HTML | Documents |
| LaTeX | Documents |
| Microsoft Excel (.xlsx) | Spreadsheets |

---

## Export API Endpoints

### Synchronous Document Export (DOCX)

`GET /1/threads/{thread_id}/export/docx`

- Auth: OAuth2 (`USER_READ` scope)
- Response: Binary DOCX content
- Admin variant: `GET /1/admin/threads/{thread_id}/export/docx`

### Synchronous Spreadsheet Export (XLSX)

`GET /1/threads/{thread_id}/export/xlsx`

- Auth: OAuth2 (`USER_READ` scope)
- Response: Binary XLSX (`application/vnd.openxmlformats-officedocument.spreadsheetml.sheet`)
- Admin variant: `GET /1/admin/threads/{thread_id}/export/xlsx`

### XLSX Export Preserves

Hyperlinks, images, merged cells, column width, row height, text alignment, text color, time format.

### Synchronous Slides Export (PDF)

`GET /1/threads/{thread_id}/export/pdf`

- For slides only (now retired)
- Response: Binary PDF

---

## Async PDF Export (Documents and Spreadsheets)

### Create Export Request

`POST /1/threads/{thread_id}/export/pdf/async`

| Parameter | Type | Description |
|-----------|------|-------------|
| `destination_thread_id` | optional | Attach PDF to another Quip document |
| `salesforce_org_id` | optional | Attach to Salesforce record |
| `salesforce_record_id` | optional | Attach to Salesforce record |
| `sheet_name` | optional | Which tab for multi-tab spreadsheets (defaults to first) |
| `apply_print_options` | boolean | When true: includes headers/footers, shows collapsed content, removes web formatting |

Response: `{ "request_id": "...", "status": "PROCESSING" }`

### Retrieve Export Result

`GET /1/threads/{thread_id}/export/pdf/async`

| Parameter | Description |
|-----------|-------------|
| `request_id` | From create response |
| `thread_id` | Thread ID |

Response statuses: `PROCESSING`, `SUCCESS`, `PARTIAL_SUCCESS`, `FAILURE`

On SUCCESS: returns `pdf_url` (available for **3 days** after creation).

Processing time: up to **10 minutes** depending on thread size.

### PDF Export Limitations

- Maximum **40,000 cells** for spreadsheet export
- Charts **excluded** from PDF
- Standalone spreadsheets only (not embedded in documents)
- Known issues: bullets appear larger, emojis replaced with boxes, large spreadsheets may have hidden data at page breaks, Live Apps may be distorted

---

## Bulk Export API

### Automation API Bulk Export

**Create**: `POST /1/threads/export/async`

```json
{
  "threads": [
    { "thread_id": "UcZ9AAhvqrc", "format": "XLSX" },
    { "thread_id": "AVN9AAeqq5w", "format": "DOCX" }
  ],
  "include_conversations": false,
  "omit_if_unchanged_since_usec": 1750375408000000,
  "locale": "en-US"
}
```

Supported formats: `XLSX`, `DOCX`, `HTML`

`include_conversations`: includes conversation history (DOCX and XLSX only).

`omit_if_unchanged_since_usec`: skip unchanged documents.

**Retrieve**: `GET /1/threads/export/async?request_id=...`

```json
{
  "completed": false,
  "results": [
    { "thread_id": "...", "format": "DOCX", "status": "EXPORTED", "file_url": "..." },
    { "thread_id": "...", "format": "HTML", "status": "PROCESSING" }
  ]
}
```

### Admin API Bulk Export

Same endpoints under `/1/admin/threads/export/async` with additional support for `PDF` format.

Admin PDF bulk export: `POST /1/admin/threads/export/pdf/async` with `company_id` and `ids` array. All tabs of multi-tab spreadsheets exported into one PDF.

---

## Export Rate Limits

### Standard API Rate Limits

- 50 requests/minute per user
- 750 requests/hour per user
- 600 requests/minute per company

### Bulk Export Rate Limit

- **36,000 documents per hour** per company
- Headers: `X-Documentbulkexport-RateLimit-Limit`, `X-Documentbulkexport-RateLimit-Remaining`, `X-Documentbulkexport-RateLimit-Reset`, `X-Documentbulkexport-Retry-After`
- If remaining count < requested batch size, request fails until reset
- Rate limits can be raised by contacting Quip support

---

## Markdown Export

- **UI**: Document menu > Export > "Export as Markdown" -- copies to clipboard
- **API**: No dedicated markdown export endpoint. `GET /2/threads/{id}/html` returns HTML for client-side conversion
- **Markdown input**: `format: "markdown"` supported for `new-document` and `edit-document` (Python-Markdown implementation)
- **Editor shortcuts**: `#`/`##`/`###` for headings, `*`/`-` for bullets, `[]` for checklists, 4 spaces for code, `{{text}}` for inline code

---

## Print Functionality

- **AirPrint** support for wireless printing on iOS
- **Print options**: Document menu > "Print Options..." for page layout and scaling
- PDF export serves as the primary print pathway
- Spreadsheets have scaling options to fit all content without cutoff

---

## Copy/Paste Between Quip and External Apps

| Direction | Behavior |
|-----------|----------|
| **Copy from Quip** | Rich text formatting preserved (HTML clipboard format) |
| **Paste into Quip** | Rich text interpreted from HTML clipboard content |
| **Markdown clipboard** | "Export as Markdown" puts Markdown text on clipboard |
| **API paste** | `edit-document` accepts HTML or Markdown content |
| **Live Paste** | `POST /1/threads/live-paste` creates synced content between documents |
