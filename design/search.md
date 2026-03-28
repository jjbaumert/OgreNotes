# Quip Search

## Reference Documentation

- **Quip Automation API:** <https://quip.com/dev/automation/documentation/current>
- **Search Features Blog:** <https://quip.com/blog/5-new-search-features>
- **Search Redesign Blog:** <https://quip.com/blog/3-improvements-to-quip-search>
- **In-Document Search Blog:** <https://quip.com/blog/in-document-search>
- **Salesforce Help - Search:** <https://help.salesforce.com/s/articleView?id=sf.quip_use_search.htm&type=5>

---

## Global Search UI

### Two-Tier Interface

1. **Quick Search Dialog** -- overlay/modal triggered by clicking the magnifying glass icon or keyboard shortcut. Shows 3 most recently viewed documents immediately before typing. Results appear as you type (typeahead).
2. **Full-Screen Search View** -- expanded view with more results and filter toggles. Accessed when quick search is insufficient.

### Keyboard Shortcuts

| Shortcut | Platform | Function |
|----------|----------|----------|
| `Cmd+Option+O` | Mac | Open global search |
| `Ctrl+Alt+O` | Windows | Open global search |
| `Cmd+J` | Mac | Open search (alternative) |
| `Ctrl+J` | Windows | Open search (alternative) |
| `Cmd+F` | Mac | In-document find |
| `Ctrl+F` | Windows | In-document find |

---

## Search Scope

Results are displayed in prioritized order: **folders and people first**, then documents, spreadsheets, and messages.

| Scope | Searchable |
|-------|------------|
| Documents | Full text content and titles |
| Spreadsheets | Cell content and titles |
| Folders | Folder names |
| People | Names |
| Chat/Conversations | Message content |
| Comments | Inline and document-level |
| Edit history | Revision content |
| Tasks | Checklist items |

---

## Full-Text Search

- Indexes all documents and messages automatically
- Content becomes searchable near-instantly after creation
- Supports **fuzzy search** -- variations, approximate matches, misspellings, partial keywords
- Title matches rank higher than body matches

---

## Search Result Display

Each result shows:
- **Title** with keyword matches highlighted
- **Body preview snippet** showing where the match was found, with highlighting
- **Metadata** indicating creator or last modifier
- Results categorized: folders/people first, then documents/spreadsheets, then messages

---

## Search Ranking / Relevance

Ranking algorithm considers:
- **Previously viewed** -- documents you've viewed are prioritized
- **Recency** -- how recently the document was worked on
- **Title vs body** -- title matches rank higher
- **Person-based relevance** -- when searching a person's name, results prioritized by your view history and their recent activity
- Unified ranking across web, desktop, and mobile

---

## Search Filters

### Full-Screen View Filters

| Filter | Description |
|--------|-------------|
| Content type | Toggle docs vs spreadsheets |
| Author / Modified by | Filter by person |
| Date modified | Filter by modification date |
| Recently opened | Filter by documents you recently opened |

### Inline Search Operators

Type these in the search field:

| Operator | Description |
|----------|-------------|
| `from:[user-name]` | Content created/modified by person |
| `by:[user-name]` | Same as `from:` |
| `mention:[user-name]` | Documents that @mention a person |
| `in:[folder-name]` | Limit search to a specific folder |

After typing an operator, Quip **autocompletes** people, documents, and folders.

### Limitations

- No Boolean search (AND/OR/NOT)
- Cannot combine multiple advanced filters

---

## Recent Search History

- Clicking into search field shows **3 most recently viewed documents** immediately
- No stored search query history (past searches typed)
- "Recently opened" filter in full-screen view serves a similar purpose

---

## Typeahead / Autocomplete

- Results appear immediately as you type (no Enter required)
- Search operators trigger autocomplete for people, documents, and folders
- Mobile: dedicated buttons at top of keyboard for adding operators, then autocomplete

---

## In-Document Search (Find and Replace)

- **Find**: `Cmd+F` / `Ctrl+F`
- **Find and Replace**: works in both documents and spreadsheets
- **Unified search**: results span document body, edit/revision history, comments, and chat
- Results highlighted within the document body and diff panel

---

## Search API

### Endpoint

`GET /1/threads/search`

### Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `query` | string | Yes | Search terms |
| `count` | integer | No | Max results (default 10, max 50) |
| `only_match_titles` | boolean | No | If true, only match document titles (faster) |

### Response

List of thread objects sorted by relevance (most to least). Each result includes a `thread` object with `id`, `title`, `type`, etc.

### Authentication

Bearer token via `Authorization` header. Personal API tokens at `quip.com/api/personal-token`.

---

## Search Indexing

### Indexed Content

- Document text (full text)
- Spreadsheet content
- Document titles
- Comments (inline and document-level)
- Chat/message history
- Edit/revision history
- Tasks / checklist items
- Folder names
- People names

### Not Indexed

- Content within embedded Live Apps
- Image content (no OCR)
- Attached file contents

### Platform Integration

- macOS: documents indexed for **Spotlight Search** (works offline)

---

## Highlighting

- Search results: keyword matches highlighted in light green/yellow in titles and body snippets
- In-document search: matches highlighted in the document body and diff panel
- Mobile: same green highlighting

---

## Mobile Search

- Same ranking/relevancy algorithms as web and desktop
- Green keyword highlighting
- **Dedicated operator buttons** at top of keyboard (instead of typing special characters)
- Autocomplete after tapping an operator button
- Offline access ensures search results reflect latest synced content
