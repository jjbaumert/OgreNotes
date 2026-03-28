# Quip Sharing and Permissions

## Reference Documentation

- **Granular Permissions Blog:** <https://quip.com/blog/granular-permissions-for-quip-docs>
- **External Sharing Blog:** <https://quip.com/blog/external-sharing-partners-vendors-quip>
- **Restricted Folders Blog:** <https://quip.com/blog/restricted-folder>
- **Shared with Me Blog:** <https://quip.com/blog/shared-with-me>
- **Salesforce Help - Sharing:** <https://help.salesforce.com/s/articleView?id=000389898&type=1>
- **Salesforce Help - External Sharing:** <https://help.salesforce.com/s/articleView?id=sf.quip_restrict_external_sharing.htm&type=5>

---

## Share Dialog UI

- Blue **Share** button in the document header (upper right)
- Opens a dialog with:
  - Search field for people and folders (by name or email, with autocomplete)
  - Centralized list of all shared individuals, folders, and permission levels
  - Permission level dropdown next to each collaborator (Full Access only can change these)
  - Shareable Link settings section
  - "Share" confirmation button
- Users with Edit or lesser permissions see a read-only collaborator list

---

## Permission Levels

Four levels, introduced September 2019:

| Level | API Value | Numeric | Description |
|-------|-----------|---------|-------------|
| **Full Access** | `OWN` | 0 | Share, edit, manage permissions, remove access, change link settings |
| **Can Edit** | `EDIT` | 1 | Edit content, comment, view history and collaborators. Cannot share or manage permissions. |
| **Can Comment** | `COMMENT` | 2 | Read and add comments (inline and conversation pane). Cannot edit content. |
| **Can View** | `VIEW` | 3 | Read-only. Cannot comment, edit, or share. |

---

## Folder-Level Permission Inheritance

- Documents in a folder inherit the folder's permissions
- A thread can be shared with **multiple folders AND individual users simultaneously**
- Users can access a thread if they have access to **any** folder containing it, or are individually added
- New subfolders initially inherit parent folder members
- Can be overridden with restricted folders (see below)

### API Response Fields

| Field | Description |
|-------|-------------|
| `shared_folder_ids` | Folder IDs containing the thread |
| `user_ids` | Users individually added to the thread |
| `expanded_user_ids` | Full union of all users with access (individual + all folder members) |
| `access_levels` | Map of user ID to `{"access_level": "OWN"|"EDIT"|"COMMENT"|"VIEW"}` |

---

## Link Sharing Settings

### Company Link Mode

| Mode | Description |
|------|-------------|
| `edit` | Anyone in the company with the link can view and edit (default) |
| `view` | Anyone in the company with the link can view only |
| `none` | Link sharing disabled; only explicitly shared users/folders have access |

### External Access Toggle

"Allow access from outside" -- extends link permissions to people outside the organization. Warning: effectively makes the document a public web page.

### View-Only Sub-Options (When Mode Is `view`)

| Option | Description |
|--------|-------------|
| Show conversation | Shows activity bar/conversation history |
| Show Diffs | Shows document edit history |
| Allow New Messages | Lets viewers post chat messages |
| Allow Comments | Lets viewers add inline comments |
| Allow Requests to Edit | Lets viewers request edit permissions |

### API

- Link URL: `thread["thread"]["link"]`
- Modify: `POST /1/threads/edit-share-link-settings` with `thread_id`, `mode` (`edit`, `view`, `none`)
- Admin override: `POST /1/admin/threads/edit-share-link-settings`
- Response includes: `link_sharing_mode`, `allow_access_outside_domain`, `allow_comments`, `show_conversation`, `show_diff`, `enable_request_access`, `allow_messages`

---

## External Sharing

### Methods

1. **Shareable link with external access** -- toggle "Allow access from outside"
2. **Individual email** -- enter external person's email in share dialog
3. **Shared folders** -- add external users to a folder
4. **Chat rooms** -- add external users to chats

### Visual Indicators

Documents shared externally are automatically flagged to distinguish internal-only vs externally-shared.

### Admin Controls (Enterprise)

| Control | Description |
|---------|-------------|
| Disable public link sharing | Site-wide setting |
| Restrict external sharing | `only_able_to_share_with_main_site` parameter |
| External sharing allowlist | `add_site_to_external_sharing_allowlist` for specific domains |
| Remove external members | From any document, even as non-member admin |
| Stop all sharing with a company | Bulk revocation |
| External sharing section | Admin console view of flagged documents |

---

## Sharing API Endpoints

### Thread Sharing

| Method | Path | Description |
|--------|------|-------------|
| POST | `/1/threads/add-members` | Share with users/folders (`thread_id`, `member_ids`) |
| POST | `/1/threads/remove-members` | Remove users/folders |
| GET | `/1/threads/{id}` | Get thread with sharing metadata |
| POST | `/1/threads/edit-share-link-settings` | Change link sharing mode |

### Folder Sharing

| Method | Path | Description |
|--------|------|-------------|
| POST | `/1/folders/add-members` | Add users to folder |
| POST | `/1/folders/remove-members` | Remove users from folder |
| GET | `/1/folders/{id}` | Get folder with member list |

### Admin Sharing

| Method | Path | Description |
|--------|------|-------------|
| POST | `/1/admin/threads/edit-share-link-settings` | Admin override for link settings |
| POST | `/1/admin/threads/remove-external-members` | Remove all external users |
| POST | `/1/admin/threads/external/list` | List externally shared threads |

---

## Share Notifications

- Push notification when someone shares a document with you
- Notification when someone opens a document you shared
- Notifications for comment responses, likes, @mentions
- **Shared with Me** tab aggregates all directly shared documents (excludes folder-only access)
- Sorted by share date with manual sort by title, starred status, or author

---

## Restricted Folders

- A subfolder within a shared parent can be restricted to explicit-only access
- Creation: create subfolder > sharing settings > click red circle next to parent name
- **Lock icon** replaces standard sharing icon on folder thumbnail
- Only explicitly added members can access
- Members of restricted folder do NOT need to be members of parent
- Unlocking: click lock icon > "Unlock" to restore inheritance

---

## Bulk Sharing

- Sharing a folder effectively bulk-shares all documents within it
- `add-members` API accepts comma-separated lists of multiple user/folder IDs
- Moving a document into a shared folder grants access to all folder members
- No explicit "bulk share" UI button

---

## Permission Changes

- Full Access users can change permission levels from the share dialog dropdown
- `access_levels` API field provides complete map of users and current levels
- Document history tracks sharing events in the activity log

---

## Transfer Ownership

- When a user is deactivated, admins use **"Transfer Content"** in admin console
- Transfers contents of deactivated user's private folder to another user
- Available for deactivated members only
- No self-service transfer for individual documents; Full Access users can add others with Full Access and remove themselves

---

## Revoking Access

| Method | Description |
|--------|-------------|
| Share dialog | Full Access users remove others via UI |
| `POST /1/threads/remove-members` | API removal with `thread_id` and `member_ids` |
| `POST /1/folders/remove-members` | Folder-level removal |
| Admin console | Remove external members from any document |
| Quip Shield | Granular key revocation for specific content |
| Bulk revocation | Stop all sharing with a specific external company |
