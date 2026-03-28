# Quip Notifications and Activity Feed

## Reference Documentation

- **Quip Notifications Blog:** <https://quip.com/blog/notifications>
- **Managing Notifications:** <https://quip.com/blog/manage-quip-notifications>
- **Open Notifications Blog:** <https://quip.com/blog/open-notifications>
- **Salesforce Help - Notifications:** <https://help.salesforce.com/s/articleView?id=000389285&type=1>
- **Salesforce Help - Per-Document Settings:** <https://help.salesforce.com/s/articleView?id=sf.quip_change_your_notification_settings_for_a_document.htm&type=5>

---

## Two-Tier Notification Model

Quip separates notifications into two tiers:

1. **Updates panel (passive)** -- browsable activity feed; you check at your leisure
2. **Push notifications (active)** -- bell icon, native OS/mobile push for urgent items

---

## Updates Panel (Formerly Inbox)

Located at the top of the sidebar. A passive, real-time activity feed showing everything across the organization.

### Content

- Document edits
- Chat room activity
- New comments
- Messages
- Shares

### Filtering Options

| Filter | Description |
|--------|-------------|
| **Starred / Favorites** | Activity on starred documents |
| **Unread** | Only items with unread activity |
| **Private** | Unshared drafts |
| **Direct Messages** | 1:1 and group conversations |
| **Created by Me** | Documents you authored |

---

## Per-Document Activity Feed

Each document's conversation pane serves as a chronological activity log:

- Document edits (shown as diffs)
- Comments (inline and sidebar)
- @mentions
- Shares (who was added)
- Likes/reactions
- Read receipts at the bottom ("read by Kathy")
- Open receipts ("Molly opened the document on her phone")

---

## Notification Triggers

| Event | Notification |
|-------|-------------|
| @mentioned in document, chat, or 1:1 | Yes |
| Someone responds to your comment | Yes |
| Someone likes your message/contribution | Yes |
| Someone shares a document with you | Yes |
| Someone opens a document you shared | Yes (configurable) |
| Document edits | Only if per-document level set to "all activity" |
| New messages in chat rooms | Yes |
| New comments on accessible documents | Yes |

---

## Per-Document Notification Levels

Right-click any item in Updates to access the Notifications menu. Three levels, configurable independently for desktop and mobile:

| Level | Behavior |
|-------|----------|
| **Notify for all activity** | Push for every update: messages, comments, edits |
| **Notify for direct responses** (default) | Push only for @mentions and replies to your contributions |
| **Don't show notifications** | No push notifications for this document |

Additionally, documents can be toggled out of the Updates feed entirely.

---

## Email Notifications

### Individual Notification Emails

- Sent on @mentions, comment replies, etc.
- **Smart suppression**: NOT sent if you're actively using the app or have mobile push enabled
- Capped at **25 per day** (increased from 4 in November 2021)
- Configurable: Profile menu > Notifications > Email tab
- Options: "@mentions and direct responses" or disable entirely

### Daily Digest

- Sent if you haven't used Quip in the past day
- One consolidated email summarizing all updates
- Highlights most important activity while you were away
- Configurable in email preferences

---

## Push Notifications (Mobile)

Default triggers: @mentions, shares, comment responses, likes, document opens.

### Rich Notifications (Android 2.1+)

- Show sender's avatar/identity
- Include images and multiple replies inline
- **Actionable**: Reply or Like directly from notification tray
- Automatic grouping/collapsing when many notifications arrive

### Smart Behavior

- Suppressed when actively using the app
- Capped to prevent flooding during vacations
- Per-document notification level applies independently

---

## Desktop Notifications

- Native OS notifications (macOS Notification Center, Windows)
- Same triggers as mobile push
- Smart suppression when Quip app is active
- Per-document levels configurable independently from mobile

### Sidebar Visual Indicators (When App Is Active)

- Sidebar button **glows orange** with waiting notifications
- **Numeric badge** with notification count
- Visible even when sidebar is collapsed

---

## Bell Icon / Notification Center

- Located at the top of the inbox/sidebar
- Shows **personally relevant** items only (unlike passive Updates feed)
- Notification types: @mentions, comment replies, likes, document opens, shares
- Notifications persist indefinitely

---

## Unread Tracking System

| Indicator | Location | Behavior |
|-----------|----------|----------|
| **Blue dots** | Sidebar (chats, favorites, updates) | Dismiss by clicking without opening |
| **Numeric badges** | Favorites section | Count of updates since last visit |
| **Orange glow + count** | Sidebar button | Visible even when collapsed |

---

## Mark as Read

- **Individual**: Click blue dot on any unread item to dismiss
- **Bulk**: Checkbox/button at top of Chats and Favorites drawers for "mark all as read"

---

## Open Notifications (Document Open Receipts)

When someone opens a shared document for the first time:
- Sharer receives a push notification (e.g., "Molly opened the document on her phone")
- Read receipt appears at the bottom of the conversation thread
- Read receipts appear in: document sidebars, chat rooms, 1:1 messages, inline comments

### Disabling

Profile menu > Notifications > Uncheck "Notify when people open documents you've shared"

---

## Muting / Snoozing

| Method | Description |
|--------|-------------|
| **Per-document mute** | Set notification level to "Don't show notifications" |
| **Remove from inbox** | Toggle document out of Updates feed |
| **Mute open notifications** | Uncheck global setting |
| **Paste Quietly** | When pasting @mentions, choose not to re-notify mentioned users |

No explicit "snooze for X hours" feature.

---

## Admin Controls

- Company-wide default notification levels for new employees
- Events API (add-on) for monitoring activity across entire Quip site
- Admin console notification settings under User Defaults

---

## Notification API

### REST Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/1/messages/{thread_id}` | Get messages for a thread |
| POST | `/1/messages/new` | Add a message |
| GET | `/1/threads/recent` | Get recent threads (activity feed equivalent) |
| GET | `/1/threads/{thread_id}` | Get thread details |
| GET | `/1/users/current/threads-modified-after-usec` | Threads modified after timestamp (polling) |

### WebSocket API (Real-Time)

- Endpoint: `GET /1/websockets/new` returns WebSocket URL
- Persistent connection for real-time updates
- Authentication via personal access token
- Sample: `github.com/quip/quip-api/tree/master/samples/websocket`

### Events API (Admin, Add-On)

- Historical or near-real-time activity monitoring across entire site
- Used for auditing, compliance, content governance
- Distinct from WebSocket (admin-level vs user-level)

### No Outbound Webhooks

Quip does not natively push notification events to external URLs. The API is pull-based (REST + WebSocket).

---

## Rate Limits

- 50 requests/minute per user
- 750 requests/hour per user
- 600 requests/minute per company
