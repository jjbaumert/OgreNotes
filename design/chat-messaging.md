# Quip Chat and Messaging

## Reference Documentation

- **Quip Automation API:** <https://quip.com/dev/automation/documentation/current>
- **Quip API Reference:** <https://quip.com/api/reference>

---

## Architectural Foundation

Quip unifies documents and chats into a single **thread** abstraction. A thread is either a pure message list (chat) or a document/spreadsheet with an attached message list. The same API endpoints (`/messages/{thread_id}`, `/messages/new`) work regardless of thread type.

---

## Chat Room Types

### Standalone Chat Rooms

Topic-based channels for ongoing team discussions. Have a title, member list, and persistent message history. Created via the sidebar or API (`POST /1/threads/new-chat`).

### 1:1 Direct Messages

Private conversations between two people. Appear as **Chat Tabs** -- small pop-up windows at the bottom-right of the browser/desktop app. Multiple tabs can stack.

### Group Messages

Multi-person conversations not tied to a document.

### Document-Attached Conversations

Every document and spreadsheet has a built-in conversation pane (right-side panel). This is a message thread attached to the document thread -- shares the same thread ID.

---

## Message Types

| Type | Description |
|------|-------------|
| **Plain text** | Standard text messages |
| **Rich text / styled** | API supports `parts` as JSON array of `[[style, HTML], ...]` with styles: `system`, `body`, `monospace`, `status` |
| **@mentions** | People, documents, or chat rooms; triggers notifications |
| **Emoji** | Type `:` + emoji name (e.g., `:whale:`); standard set based on emoji-cheat-sheet.com |
| **Emoji reactions** | Added to individual messages (celebration, money, chart emojis, etc.) |
| **Images** | Drag-and-drop or upload via blob API |
| **Video** | Drag-and-drop with inline playback |
| **GIFs and stickers** | Thousands built-in; `/giphy` command for search |
| **Custom memes** | `/sayas @Person Top text / Bottom text` creates meme from profile picture |
| **File attachments** | Via blob upload endpoint |
| **System messages** | Automated (member joins, document edits) |
| **Forwarded messages** | Messages/comments forwarded from one conversation to multiple others |

---

## Conversation Pane (Right-Side Panel)

- Located on the right side of document/spreadsheet views
- Toggle: `Ctrl+Alt+C` (Windows) / `Cmd+Option+C` (Mac)
- Complete **activity log**: every action, edit, comment, and message on the document
- Only way to read and write document-level messages
- Shows inline comment threads alongside document-level messages
- Displays read receipts at the bottom
- Shows typing indicators
- Supports @mentions, emoji, slash commands, and file sharing

---

## Inline Comments vs Document-Level Comments

### Inline Comments

- Highlight text to start a threaded conversation about that selection
- Message bubble icon appears in the document margin
- Threaded -- multiple replies can stack
- Can be hidden via View > Show Comments toggle (still visible in conversation pane)
- Users with comment-only permissions can leave inline comments

### Document-Level Comments

- Appear in the conversation pane only
- Not tied to a specific text selection
- General discussion about the document
- Part of the thread message history

---

## @Mention System

### Mentionable Entities

| Entity | Behavior |
|--------|----------|
| **People** | Type `@` + name; autocomplete shows contacts; creates notification; green dot for online status |
| **Documents** | Type `@` + document name; creates clickable link |
| **Chat rooms** | Can @mention a chat room from another conversation |
| **@everyone** | Notifies all members of a chat room |

### UI Behavior

- Autocomplete dropdown appears as you type after `@`
- Arrow keys to navigate, Tab/Enter to select
- Desktop: click `@` button in top-right of document
- Mobile/tablet: tap "Insert..." option

---

## Reactions and Likes

### Like Button

- Single-click positive acknowledgment on messages and edits
- Can like people's **edits** (not just messages)
- Liking a checked-off checklist item is a common pattern
- Triggers push notification to the original author

### Emoji Reactions

- Multiple emoji reactions on a single message
- Examples: celebration, money bag, chart-up, high-five

---

## Chat Room Management

### Creation

- From the sidebar on the Quip homepage
- API: `POST /1/threads/new-chat` with `message`, `title`, `member_ids`

### Member Management

- Add: UI or API (`POST /1/threads/add-members` with `thread_id`, `member_ids`)
- Remove: UI or API (`POST /1/threads/remove-members`)
- `/invite @PersonName` slash command for instant access grant

### Integrations Panel

Click "Add Integrations" on the right side of a chat room to connect: Dropbox, Stripe, Salesforce, Twitter, GitHub, PagerDuty, Jenkins, Crashlytics, JIRA, Zendesk, RSS feeds.

### Document Creation from Chat

Button to create a new document shared with all chat room members.

---

## Message History and Scrollback

- API: `GET /1/messages/{thread_id}` with `max_created_usec` (pagination) and `count`
- Recent threads: `GET /1/threads/recent` with `max_updated_usec` and `count`
- Conversation pane maintains full history with scrollback

---

## Unread Indicators and Badges

| Location | Indicator |
|----------|-----------|
| **Chat Tabs** | Blue circle in top-right with unread count |
| **Sidebar** | Numbered badges next to items with updates; button glows orange with count |
| **Blue dots** | Appear on unread chats and favorites; dismiss by clicking |
| **Bulk mark-as-read** | Checkbox at top of chat/favorites drawers |
| **Inbox filtering** | Filter by "Unread" to see only items with unread activity |

---

## Notifications

### Default Push Notification Triggers

- @mentioned in any document, chat room, or 1:1 message
- Someone shares a document with you
- Someone responds to your comment
- Someone likes your contribution
- Someone opens a document you shared

### Per-Thread Notification Levels (Right-Click Context Menu)

| Level | Description |
|-------|-------------|
| **Notify for all activity** | All updates |
| **Notify for direct responses** | @mentions and replies only (default) |
| **Don't show notifications** | Disabled |

### Notification Channels

- Desktop push (suppressed if app is active)
- Mobile push (separate configuration)
- Email: individual per-mention (throttled) or Daily Digest
- Bell icon for in-app notification center

### Admin Controls

Company-wide default notification levels for new employees.

---

## Message Editing and Deletion

- Messages can be edited after sending
- Thread deletion: `POST /1/threads/delete` with `thread_id`
- Edit history: Document > Document History shows all versions
- Per-paragraph history: select text > menu > "Show Edit History"
- Visual diffs: green = additions, red = deletions
- "Deleted by Others" subfolder in Trash for documents deleted by collaborators

---

## Slash Commands

| Command | Description |
|---------|-------------|
| `/invite @Person` | Add person to document/chat with instant access |
| `/sayas @Person text / text` | Create meme from teammate's profile picture |
| `/giphy [search]` | Insert GIF from Giphy |

---

## Real-Time Delivery

- WebSocket: `GET /1/websockets/new` returns a WebSocket URL for real-time events
- All messages appear in real time across devices
- Chat Tabs auto-appear on incoming direct messages
- Document presence shows avatars of everyone currently viewing

---

## Typing Indicators

Displayed when someone is typing in: document conversation pane, 1:1 messages, chat rooms, and inline comment threads. Visible to all current viewers.

---

## Read Receipts

- Displayed at the bottom of every conversation thread
- Format: "read by [Name]"
- Open notifications: push sent when someone first opens a shared document, including device type

---

## Chat Sidebar (Left Navigation)

- Collapsible sidebar; "Always Show Sidebar" in View menu (desktop)
- Darker color scheme to contrast with document area
- Sections: Updates, History (last 15 items), Favorites, Frequently Viewed, Direct Messages, Chat Rooms
- Updates feed filterable by Favorites, Direct Messages, etc.
- Orange glow with numerical badge on sidebar button for notifications

---

## Chat API Endpoints

| Method | Endpoint | Purpose |
|--------|----------|---------|
| POST | `/1/threads/new-chat` | Create chat room (`message`, `title`, `member_ids`) |
| GET | `/1/threads/{id}` | Get thread (document or chat) |
| POST | `/1/threads/` | Get multiple threads by `ids` |
| GET | `/1/threads/recent` | Recent threads (`max_updated_usec`, `count`) |
| GET | `/1/threads/search` | Search threads (`query`, `count`, `only_match_titles`) |
| POST | `/1/threads/add-members` | Add members |
| POST | `/1/threads/remove-members` | Remove members |
| POST | `/1/threads/delete` | Delete thread |
| GET | `/1/messages/{thread_id}` | Get messages (`max_created_usec`, `count`) |
| POST | `/1/messages/new` | Send message (`thread_id`, `content`, `parts`, `attachments`, `section_id`) |
| GET | `/1/blob/{thread_id}/{blob_id}` | Download attachment |
| POST | `/1/blob/{thread_id}` | Upload file/image |
| GET | `/1/websockets/new` | Get WebSocket URL |

### Message Parts Format

JSON array of `[[style, HTML], ...]` where style is: `system`, `body`, `monospace`, or `status`.

### Edit Location Constants

| Value | Constant |
|-------|----------|
| 0 | APPEND |
| 1 | PREPEND |
| 2 | AFTER_SECTION |
| 3 | BEFORE_SECTION |
| 4 | REPLACE_SECTION |
| 5 | DELETE_SECTION |
