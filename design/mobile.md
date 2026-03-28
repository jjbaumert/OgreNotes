# Quip Mobile App

## Reference Documentation

- **Quip Blog - Mobile:** <https://quip.com/blog/mobile>
- **Quip Blog - iOS:** <https://quip.com/blog/ios>
- **Quip Accessibility:** <https://quip.com/training/accessibility-for-quip>

---

## Platform Availability

| Platform | Status |
|----------|--------|
| **iOS** | Active (current version 8.40.0, iPhone and iPad) |
| **Android** | Retired June 6, 2024 |
| **Windows Desktop** | Retired June 6, 2024 |
| **Mac Desktop** | Active |
| **Web** | Active |

---

## Design Philosophy

Quip was designed mobile-first. Documents are responsive by default, auto-formatting to screen size. No pinch-to-zoom required.

---

## Mobile Formatting Toolbar

On iPhone and iPad, the formatting menu appears as a **gray bar at the top of the keyboard** (replaces the desktop blue tab on the right side).

- Tap the **gray paragraph symbol** to open formatting options
- Available: large, medium, small headers, normal text
- List types: bulleted, numbered, checklists
- Text styling: highlight text to see popup with bold, italic, underline
- The bar follows the cursor position as you write and edit

### Desktop vs Mobile Comparison

| Feature | Desktop | Mobile |
|---------|---------|--------|
| Formatting access | Blue tab on right side (hover) | Gray bar above keyboard |
| Text formatting | Keyboard shortcuts | Highlight text, select from popup |
| Section menu | Hover blue tab | Tap paragraph symbol |

---

## Custom Spreadsheet Keyboards

Three distinct keyboard modes for mobile spreadsheet editing:

| Mode | Description |
|------|-------------|
| **Standard** | Device built-in keyboard for text entry |
| **Numeric keypad** | Dedicated number pad for numeric values |
| **Formula keyboard** | Specialized keys for operators, parentheses, cell references, function names |

On **tablets**, these combine into a single keyboard merging numeric and formula keys.

### Features

- **Formula autocomplete** -- no need to memorize 400+ function names
- **Maximize button** -- expands embedded spreadsheet to full-screen
- **Cell selection** -- tap and hold first cell until it "glows", then tap final cell for range

---

## Offline Mode and Sync

- Works on **native apps only** (not web browser)
- Transition between online/offline is **seamless** -- no user action required
- While offline: create, edit, comment, and send messages -- full functionality
- All changes saved locally on device
- When connectivity restored: changes **automatically sync** to server

### Unsaved Changes Indicators

- Visual indicator at bottom of documents showing pending changes
- Sidebar counter shows total unsaved changes across all items
- Click counter to see which documents have pending changes
- Indicator disappears after successful sync

---

## Mobile Gestures

| Gesture | Action |
|---------|--------|
| **Swipe on lock screen** | iOS Handoff -- continue editing from another device |
| **Tap and hold cell** | Activate spreadsheet cell selection (cell "glows") |
| **Tap paragraph symbol** | Open formatting options |
| **Highlight text** | Standard text selection for formatting popup |
| **Tap contacts** | Quick-add frequent contacts in chat |
| **Press and hold notification** | Reply from notification (iPhone/Apple Watch) |

Pinch-to-zoom is NOT required due to responsive document design.

---

## Mobile Document Editing

- Create new documents directly on mobile
- Real-time collaborative editing across devices
- Documents auto-format to screen size
- `@` symbol provides shortcuts for mentioning people, inserting files/images, linking documents, creating spreadsheets/tables
- `//` at end of a line creates inline comments
- `=` enables calculations and cross-references to spreadsheets
- Complete change history maintained
- Export to PDF and Word (.docx)
- Import from Dropbox, Google Drive, Evernote, Box, iCloud (iOS)
- **Document outline**: tap outline icon to jump to specific headings
- **Pin documents** to home screen for one-tap access

---

## Mobile Spreadsheet Editing

- Full spreadsheet functionality with 400+ functions
- Custom keyboards (numeric, formula, standard)
- Formula autocomplete
- Cell-by-cell commenting and annotations
- @mention people and documents within cells (triggers notifications)
- Insert images into cells
- Tap-and-hold for cell range selection
- Maximize button for embedded spreadsheets
- Paste options: transpose, values only, link cells
- Freeze rows and conditional formatting
- Collaborative editing with conversation panel

---

## Mobile Chat and Messaging

- 1:1 direct messages and group chat rooms
- @mention documents in messages
- Create new documents from within chat
- Emoji, custom meme creator, slash commands
- GIF and sticker support
- Third-party integrations: Dropbox, Stripe, Salesforce, GitHub, PagerDuty, RSS, Jira, Zendesk
- Separate mobile and desktop notification levels

---

## Mobile Search

- Full-text search across documents, messages, spreadsheets, chat
- **Dedicated operator buttons** above keyboard (`from:`, `mention:`, `in:`) -- no special character typing needed
- Autocomplete for people, documents, folders
- Green keyword highlighting in results
- Ranking/relevancy matches desktop quality
- Execute a search in as few as three taps

---

## Mobile Notifications

### Push Notification Triggers

- @mentioned in any document, chat, or 1:1
- Someone shares a document with you
- Someone responds to your comment
- Someone likes your message
- Someone opens a shared document (open receipts)

### Notification Customization

- Separate notification levels for **desktop and mobile**
- Per-document: all activity / direct responses / none
- Email suppressed if push active or app in use

### Rich Notifications (Android, Pre-Retirement)

- Sender identity with avatar
- Images and multiple replies inline
- Individual conversations as separate entries
- Collapsed/stacked when multiple arrive
- Reply or like directly from notification tray

---

## Mobile Sharing

- Share via phone contact list
- Shareable links (no Quip account required to view)
- Private editable links or view-only published versions
- iOS share sheet integration
- Export as PDF for email attachment
- Export to Word (.docx)
- Open receipt notifications

---

## Handoff Between Devices

- **Apple Handoff**: start editing on one Apple device, continue on another
- Quip icon appears on lock screen of nearby devices
- Works across all platforms via cloud sync (iOS, Mac, web)
- Offline changes sync when any device reconnects
- Every change synced instantly across desktop, phone, and tablet

---

## Mobile Live Apps

- Live Apps are **automatically mobile** -- work on every device, online or offline
- API: `quip.apps.isMobile()` for mobile detection
- `quip.apps.getContainerWidth()` for responsive layout
- `CONTAINER_SIZE_UPDATE` event for dynamic adjustment
- Live App Menus supported on mobile for insert/delete operations
- Reduced interactivity for some apps (e.g., cannot move Kanban cards between columns)

---

## Mobile Accessibility

- Quip recommends **web browser** for accessible use; mobile/desktop apps "not reliably accessible"
- Screen reader support (VoiceOver/NVDA) optimized for web only
- "Improve Screen Reader Support" setting available
- Mobile accessibility is a known gap in the product

---

## Tablet-Specific Behavior

- Expanded desktop-like view with inbox visible (iPad)
- Combined numeric/formula keyboard for spreadsheets
- Larger canvas for document editing
- Handoff support between iPad, iPhone, and Mac
