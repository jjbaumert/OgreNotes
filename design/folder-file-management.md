# Quip Folder and File Management

## Reference Documentation

- **Quip Automation API:** <https://quip.com/dev/automation/documentation/current>
- **Quip API Reference:** <https://quip.com/api/reference>

---

## Folder Model

Quip folders are **tag-based**, not file-system directories. A document (thread) can exist in **multiple folders** simultaneously -- there is only one version of the document regardless of access path. The API allows adding a thread to a maximum of 100 folders per request and 2,500 folders across multiple requests.

---

## Special / System Folders

Each user has system folders returned by `GET /1/users/current`:

| Folder | API Field | Behavior |
|--------|-----------|----------|
| **Desktop** | `desktop_folder_id` | Cannot be deleted, shared, or renamed. User's home/root folder. |
| **Archive** | `archive_folder_id` | Cannot be deleted, shared, or renamed. Holds archived content. |
| **Starred** | `starred_folder_id` | Contains all starred/favorited items. |
| **Private** | `private_folder_id` | Personal workspace for drafts before sharing. |
| **Trash** | `trash_folder_id` | Deleted documents recoverable for 30 days. Contains "Deleted by Others" subfolder. |

Additional user fields:
- `shared_folder_ids` -- array of shared folder IDs
- `group_folder_ids` -- array of group folder IDs

---

## File Browser UI

### Home Page Views

Three buttons at the top of the home page:

| View | Description |
|------|-------------|
| **All Files** | Three tabs: Recent, Frequent, Shared |
| **Folders** | Browse folder tree |
| **Updates** | Real-time feed of recently updated documents |

### "Shared with Me" Tab

Shows documents shared directly with the user (via Share menu, shared links, or @mentions). Excludes documents accessible only through shared folder membership.

---

## List View vs Grid View

| Mode | Description |
|------|-------------|
| **Grid view** | Original default. Folders and documents as tiles/cards. |
| **List view** | Current default. Resembles Finder/Explorer. Column headers for sorting, expandable/collapsible folders via triangle icons, bulk selection. |

---

## Sorting Options

In list view, click column headers to sort by:

- **Title** (document name)
- **Last Updated** (date modified)
- **Author**
- **Created date** (optional column)
- **Unread dot** (optional column -- indicates unread changes)

Columns can be shown/hidden by right-clicking any column header.

---

## Folder Operations

### Creation

- API: `POST /1/folders/new` with `title`, `parent_id`, `color`, `member_ids`
- UI: Create from sidebar or file browser

### Rename / Recolor

- API: `POST /1/folders/update` with `folder_id`, `title`, `color`

### Deletion

- No explicit delete endpoint in API; folders are removed by removing all members or via UI
- System folders cannot be deleted

---

## Moving Documents Between Folders

Since documents are tag-based, "moving" is:
1. Add the thread to the destination folder (`POST /1/threads/add-members` with folder ID)
2. Remove the thread from the source folder (`POST /1/threads/remove-members`)

UI methods:
- Breadcrumb dropdown menu at top-left of document
- Drag-and-drop in list view
- Item menu for bulk operations
- Right-click context menu

---

## Drag-and-Drop

Supported in list view:
- Drag individual documents/folders to new locations
- Hold **Shift** to select multiple items, then drag all to a destination
- Available on desktop apps and web

---

## Folder Hierarchy (Nesting)

- Arbitrary nesting supported (subfolders within subfolders)
- API uses `parent_id` when creating folders
- Folder response includes `children` array with either `folder_id` or `thread_id` entries
- No documented depth limit
- List view supports expanding/collapsing via triangle toggles

---

## Folder Colors

12 colors available (originally 5):

| Value | Color |
|-------|-------|
| 0 | Manila (default) |
| 1 | Red |
| 2 | Orange |
| 3 | Green |
| 4 | Blue |
| 5-11 | Additional colors (light variants, yellow, purple, etc.) |

Set via folder gear menu in UI, or `color` parameter in API.

---

## Folder Sharing and Permissions

### Permission Levels

| Level | Description |
|-------|-------------|
| **Full Access** | Can share and edit |
| **Can Edit** | Can edit but not share |
| **Can Comment** | View and comment only |
| **Can View** | Read-only |

### Inheritance

- Folder has `inherit_mode` field (e.g., `"inherit"`)
- `sharing` object contains `company_mode` and `company_id`
- New subfolders initially inherit parent's sharing/members
- Link sharing settings: `GET /2/folders/{id}/link-sharing-settings`

### API

- Add members: `POST /1/folders/add-members` with `folder_id`, `member_ids`
- Remove members: `POST /1/folders/remove-members` with `folder_id`, `member_ids`

---

## Restricted Folders

- A subfolder within a shared parent can be restricted to limit access
- In sharing settings, click the red circle next to the parent folder name
- A **lock icon** replaces the standard sharing icon on the folder thumbnail
- Only explicitly added people can access documents in the restricted folder
- Members of restricted folders do not need to be members of the parent folder
- Unlocking restores access to all parent folder members plus specifically invited members

---

## Recently Viewed / Frequently Viewed

| Feature | Description |
|---------|-------------|
| **History** | Sidebar section showing last 15 documents/spreadsheets/chats viewed chronologically |
| **Search quick access** | Clicking search shows 3 most recently viewed documents |
| **Frequently Viewed** | Auto-generated section; appears at bottom of Favorites once user has enough favorites |
| **All Files > Recent** | Recently accessed documents tab |
| **All Files > Frequent** | Frequently accessed documents tab |
| **API** | `GET /1/threads/recent` with `count`, `max_updated_usec` |

---

## Favorites / Starred System

- Click star icon at top of any document, chat, or folder to add to Favorites
- **Collections**: When starring, optionally add to a Collection (personal organizational groups within Favorites)
- Dedicated sidebar section with notification badges for updates since last visit
- Stored in user's `starred_folder_id` system folder

---

## Trash / Deletion and Recovery

- API: `POST /1/threads/delete` with `thread_id`
- Deleted documents move to user's Trash folder
- Recoverable for **30 days**
- **"Deleted by Others"** subfolder: shows documents deleted by collaborators with info about who deleted them
- After 30 days, permanently deleted

---

## Archive Behavior

- Permanent `archive_folder_id` (cannot be deleted, shared, or renamed)
- Archiving removes document from Desktop/active view without deleting
- Archived documents remain accessible and searchable
- Can be moved back to active folders

---

## Breadcrumb Navigation

- Located at top-left of every document
- Shows folder hierarchy path
- Clicking opens dropdown menu for:
  - Viewing folder hierarchy
  - Moving to a different folder
  - Adding to additional folders
- Since documents can be in multiple folders, breadcrumbs may show multiple paths

---

## Folder Sidebar Navigation

- Collapsible panel on the left side
- Closed by default when opening documents (focus mode)
- "Always Show Sidebar" option in View menu (desktop app)
- Sections: Updates, History, Favorites (with Collections), Folders
- Orange glow with numerical badge for notifications
- Home button leads to three-view home page

---

## Folder API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/1/folders/{id}` | Get folder (metadata, children, member_ids) |
| POST | `/1/folders/` | Get multiple folders (`ids`, comma-separated) |
| POST | `/1/folders/new` | Create folder (`title`, `parent_id`, `color`, `member_ids`) |
| POST | `/1/folders/update` | Update folder (`folder_id`, `title`, `color`) |
| POST | `/1/folders/add-members` | Add members (`folder_id`, `member_ids`) |
| POST | `/1/folders/remove-members` | Remove members (`folder_id`, `member_ids`) |
| GET | `/2/folders/{id}/link-sharing-settings` | Get link sharing settings |

### Related Thread Endpoints

| Method | Path | Description |
|--------|------|-------------|
| POST | `/1/threads/add-members` | Add thread to folders or share with users |
| POST | `/1/threads/remove-members` | Remove thread from folders |
| POST | `/1/threads/delete` | Delete thread (moves to trash) |
| GET | `/1/threads/recent` | Recently accessed threads |

---

## Data Models

### Folder Object

```json
{
  "folder": {
    "id": "string",
    "title": "string",
    "color": 0,
    "parent_id": "string",
    "creator_id": "string",
    "created_usec": 0,
    "updated_usec": 0,
    "folder_type": "PRIVATE | SHARED",
    "inherit_mode": "inherit",
    "sharing": {
      "company_mode": "EDIT | VIEW",
      "company_id": "string"
    }
  },
  "member_ids": ["string"],
  "children": [
    {"folder_id": "string"},
    {"thread_id": "string"}
  ]
}
```

### User Object (Folder-Related Fields)

```json
{
  "id": "string",
  "desktop_folder_id": "string",
  "archive_folder_id": "string",
  "starred_folder_id": "string",
  "private_folder_id": "string",
  "trash_folder_id": "string",
  "shared_folder_ids": ["string"],
  "group_folder_ids": ["string"]
}
```
