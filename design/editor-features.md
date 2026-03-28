# Quip Editor Features

## Overall Layout

The Quip editor uses a three-panel layout:

- **Left sidebar** -- collapsible navigation panel with icons for search, updates, tasks, favorites, and chats
- **Center** -- main document editing canvas
- **Right** -- conversation pane (toggleable)

---

## Document Header / Title Bar

- **Sidebar toggle** (hamburger icon, upper left)
- **Menu bar** -- contextual menus (Document, Checklist, or Spreadsheet depending on content type)
- **Star button** -- add/remove from Favorites
- **Share button** (blue, upper right)
- **Compose button**
- **Gear menu** (upper right) -- theme selection, settings
- **View menu** -- toggle Document Outline, conversation pane visibility

---

## Left Sidebar (Navigation)

Collapsible between icon-only and expanded states.

### Sections

| Section | Description |
|---------|-------------|
| **Search** | Global search across documents, folders, conversations (`Ctrl+Alt+O`) |
| **Home Drawer** | Two regions: *Select View* (All Files, Folders, Updates) and *Favorites* |
| **Updates** | Real-time feed of edits, chat activity, document changes |
| **History** | Last 15 documents/spreadsheets/chats viewed chronologically |
| **Favorites** | Starred documents, chats, and folders with unread counts |
| **Frequently Viewed** | Auto-surfaced frequently accessed items |
| **Tasks** | Consolidated assigned checklist items across all documents |
| **Chats** | Chat rooms and 1:1 messages |

### Notification Indicator

Orange glow with count on sidebar button when notifications are pending.

### Tasks Drawer

- Pin high-priority tasks to top
- Hide tasks until updates occur
- Sort by completed or hidden status
- Complete tasks directly from the drawer

---

## Conversation Pane (Right Side)

- Log of every action, edit, and comment on a document
- Document-level messages
- Shows who has viewed/edited, all comments and highlights
- Toggle: Document menu > "Hide Conversation", X button on hover, or `Ctrl+Alt+C`
- Supports `@mentions` of people and documents
- Like button on messages and edits
- `/invite @person` to grant access directly from the pane

---

## Blue Tab / Section Menu

A distinctive Quip UI element: a blue tab follows the cursor on the right margin.

Hovering reveals:

- **Heading options:** Large (H1), Medium (H2), Small (H3), Normal paragraph
- **List options:** Bulleted, Numbered, Checklist
- **Insert options:** Horizontal Rule, other elements
- **Per-section features:** Edit History, Comments, Anchor Link

Accessibility shortcut: `Alt+Shift+C` (Windows) / `Option+Shift+C` (Mac)

---

## Text Formatting

### Inline Styles

| Style | Shortcut | Markdown |
|-------|----------|----------|
| Bold | `Ctrl+B` | |
| Italic | `Ctrl+I` | |
| Underline | `Ctrl+U` | |
| Strikethrough | `Ctrl+Shift+X` | |
| Monospace / Inline code | `Ctrl+Shift+K` | `{{text}}` |
| Link | `Ctrl+K` | |
| Text color | toolbar | |
| Text highlight | toolbar | |

### Block-Level Formatting

| Style | Shortcut | Markdown |
|-------|----------|----------|
| Large heading | `Ctrl+Alt+1` | `#` + space |
| Medium heading | `Ctrl+Alt+2` | `##` + space |
| Small heading | `Ctrl+Alt+3` | `###` + space |
| Normal paragraph | `Ctrl+Alt+0` | |
| Bulleted list | `Ctrl+Shift+L` | `*` or `-` + space |
| Numbered list | toolbar | |
| Checklist | toolbar | `[]` + space |
| Code block | `Ctrl+Alt+K` | 4 spaces |
| Horizontal rule | menu | `---` or `===` |

### Additional Notes

- Backtick (`` ` ``) cycles through all block styles sequentially
- Code blocks have automatic syntax highlighting with language detection
- Anchor links via `Ctrl+Shift+A`
- Line numbers toggleable for documents
- Page breaks supported

---

## The `@` Menu

Typing `@` anywhere in a document opens an insertion menu:

- Mention a person
- Mention/link a document
- Insert a spreadsheet or table
- Insert an image or link
- Insert Code block
- Insert Horizontal Rule
- Insert Math/Equation
- Insert Live Apps by name
- Arrow key navigation, Tab/Enter to select

---

## Document Outline

Auto-generated clickable table of contents:

- Toggle: `Ctrl+Shift+O`
- Shows Large, Medium, Small headings and top-level checklist items
- Fixes to the far right when window is wide enough
- Only users with edit permissions can toggle it

---

## Themes (Typography)

| Theme | Description |
|-------|-------------|
| **Atlas** (default) | Atlas Grotesk headings + Lyon Text body |
| **Modern** | Neue Haas Grotesk (all sans-serif) |
| **Byline** | Publico (editorial serif) |
| **Marseilles** | Duplicate Sans (casual) |
| **Manuscript** | Courier Prime (monospaced, centered headings) |

Additional: **OpenDyslexic** font for accessibility.

### Display Modes

- **Dark Mode** (web, desktop, iOS; can follow device setting)
- **Increased Contrast Mode** (bolder colors)

---

## Comments and Annotations

### Inline Comments

- Highlight text and start a conversation on that selection
- Message bubble appears in the margin
- Shortcut: `//` at end of a line, or `Ctrl+Shift+C`
- Works on paragraphs, spreadsheet cells, images, and code blocks

### Comment Thread Navigation

- "Previous Thread" and "Next Thread" buttons
- Comment menu (`...`) with "Show Comment Source" and "Archive" (resolve) options
- Read/unread tracking for comments

### Document-Level Comments

- Written in the conversation pane
- All edits logged with names and timestamps
- `@mentions` supported
- Like button for lightweight acknowledgment

---

## Edit History and Versioning

### Per-Section History

- Click section menu > "Show Edit History"
- Shows complete revision history back to creation
- Green for additions, red for deletions, with names and timestamps

### Inline Edit Indicators

- Selecting text shows the last editor's name in gray to the left
- Hover for chronological list of all edits
- Tracks content changes only (not formatting)

### Version Restoration

- Click any diff to restore to that version
- Works regardless of version age

### Per-Cell History (Spreadsheets)

- Select a cell, click the clock icon in the spreadsheet menu bar

---

## Spreadsheet Editor

### Core Capabilities

- 400+ functions
- Infinite rows and columns
- Multiple sheets per document (tabs in maximized view)
- Sheets Picker for navigation

### Toolbar

| Control | Description |
|---------|-------------|
| Format Painter | Copy formatting between cells |
| Text Wrap | Toggle text wrapping |
| More Formats | Custom date/time formatting |
| Conditional Formatting | Rule-based cell formatting |
| Text Alignment | Cell alignment options |
| Freeze Rows | Freeze header rows |
| Table Mode | Hide row/column numbers for cleaner embedded view |
| Clock icon | Cell-level edit history |

### Cell Content Types

- Text, numbers, formulas
- `@mentions` of people and documents
- Images and files (`@image`, `@file`)
- Checkboxes
- Dropdowns

### Data Operations

- Sorting by columns
- Filtering with "Update Filter" button
- Conditional formatting (string and numerical rules)
- Data Validation 2.0: attachments, people, dates, numbers, text, URLs, emails, checkboxes
- Transpose data
- Paste values only
- Link cells across tables
- Fill Down (`Ctrl+D`), Fill Right (`Ctrl+R`)
- Pixel-perfect row/column resizing

### Charts

Three types: **Pie**, **Line**, **Bar**

- Created from spreadsheet data ranges
- Live-updating when data changes
- Customizable colors and labels
- Embeddable in slide presentations

### Import/Export

- Import from Excel (.xls, .xlsx), CSV, OpenOffice
- Export to Excel format
- Bulk export of multiple documents/spreadsheets

---

## Live Apps (Embedded Interactive Components)

### Insertion

- From the **Apps** section in the sidebar
- By typing `@` followed by the app name

### Built-in Live Apps

- Kanban boards
- Calendars
- Polls
- Image annotation tools
- Dynamic graphs
- Salesforce record widgets
- Salesforce list views

### Characteristics

- Real-time syncing, editing, and commenting
- Available offline and cross-platform
- Work within templates (account plans, territory plans, roadmaps)

---

## Sharing and Collaboration

### Share Dialog

Accessed via the blue Share button (upper right):

- Search people and folders by name or email
- View all shared individuals, folders, and permissions
- Shareable link generation for external sharing

### Permission Levels

| Level | Capabilities |
|-------|-------------|
| **Full Access** | Share, edit, manage permissions, remove access |
| **Can Edit** | Edit, view collaborators, view history |
| **Can Comment** | View-only + inline comments and conversation messages |
| **Can View** | View-only |

### Real-Time Collaboration

- Multiple simultaneous editors
- Cursor presence (see where others are editing)
- Edit tracking with author attribution
- Diff view (green additions, red deletions)
- Like button on edits and messages

---

## Folder Structure

- Folders act as **tags** (a thread can belong to multiple folders)
- Special folders: **Desktop**, **Archive**, **Starred**, **Private**, **Trash**
- Folder-level sharing with restricted folder support
- List View on desktop (Finder/Explorer-like with sortable columns)
- Permission inheritance from folder to contained documents

---

## Keyboard Shortcuts

### Global

| Shortcut | Action |
|----------|--------|
| `Ctrl+/` | Show all keyboard shortcuts |
| `Ctrl+Alt+Shift+N` | Create new item |
| `Ctrl+Alt+N` | New document |
| `Ctrl+Alt+M` | New message |
| `Ctrl+Shift+D` | Go to desktop/home |
| `Ctrl+Alt+O` | Search documents/folders/conversations |
| `Ctrl+Alt+Left/Right` | Switch tabs (desktop app) |
| `Ctrl+Shift+J` | Open Command Library |
| `Esc` | Stop editing |

### Editing

| Shortcut | Action |
|----------|--------|
| `Ctrl+Z` | Undo |
| `Ctrl+Shift+Z` | Redo |
| `Ctrl+K` | Insert link |
| `Ctrl+P` | Print |
| `Ctrl+Alt+C` | Toggle conversation pane |
| `Ctrl+Shift+C` | Add a comment |
| `Ctrl+Alt+S` | Finish edit session |
| `Ctrl+Shift+A` | Create anchor link |
| `Ctrl+Shift+O` | Toggle document outline |
| `Ctrl+Alt+Up/Down` | Move list item up/down |
| `Ctrl+Enter` | Check a checklist item |
| `Tab` | Indent list item |
| `Shift+Tab` | De-indent list item |
| `Shift+Enter` | Single line break |

### Spreadsheet

| Shortcut | Action |
|----------|--------|
| `Ctrl+Arrows` | Move to edge of data |
| `Ctrl+Shift+Arrows` | Extend selection to edge |
| `Shift+Space` | Select row |
| `Ctrl+Space` | Select column |
| `Ctrl+Enter` | Fill selection with entered text |
| `Ctrl+D` | Fill down |
| `Ctrl+R` | Fill right |
| `Ctrl+Backspace` | Scroll focused cell into view |
| `Ctrl+I` | Insert row/column |
| `Ctrl+-` | Remove selected rows/columns |
| `Ctrl+;` | Insert current time |
| `Ctrl+:` | Insert today's date |
| `Alt+Enter` | Hard return within a cell |

---

## Command Library

Accessed via `Ctrl+Shift+J`: a searchable command palette for invoking any action -- formatting, document creation, navigation, and more.

---

## Mobile vs Desktop

### Desktop-Specific

- Tabbed browsing (`Cmd+T`)
- "Always Show Sidebar" option
- Drag-and-drop file import
- List View for folders
- Full code block insertion
- Full Live App interactivity
- Blue tab section menu

### Mobile-Specific (iOS)

- Formatting menu in gray bar above keyboard
- Paragraph symbol tap for heading/list options
- Custom spreadsheet keyboards (numeric, formula with autocomplete)
- Maximize button for spreadsheet viewing
- Tap-and-hold for cell range selection

### Platform Status (2024-2025)

| Platform | Status |
|----------|--------|
| Mac desktop | Active |
| Web | Active |
| iOS | Active |
| Windows desktop | Retired June 2024 |
| Android | Retired June 2024 |
