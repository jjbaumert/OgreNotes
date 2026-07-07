# OgreNotes Comments Specification

**Version:** 1.1  
**Status:** Draft  
**Purpose:** Comprehensive reference for all comment modes available in OgreNotes documents, spreadsheets, and presentations

---

## 1. Overview

OgreNotes integrates commenting directly into its collaborative editing model. Comments are **threaded conversations** anchored to specific content units — what OgreNotes calls *sections*. Every comment mode shares a common thread model: a comment opens a thread, collaborators reply within it, and the thread persists alongside the content it annotates.

### 1.1 The Section Model

OgreNotes' core abstraction for commenting is the **section**. A section is the smallest addressable unit of content in an OgreNotes document or spreadsheet:

| Context | Section |
|---|---|
| Document | One paragraph, one list item, one checklist item |
| Table | One table cell |
| Spreadsheet | One spreadsheet cell |
| Slides | One slide or one content block on a slide |

### 1.2 Comment Threading Rules by Mode

Not all comment modes share the same threading constraint. The rules differ between block and inline comments:

| Mode | Threading Rule |
|---|---|
| Block comment | One thread per paragraph (section). A second block comment cannot be added while one is open. |
| Inline comment | Multiple threads per paragraph are permitted. Each distinct text selection carries its own independent thread. |
| Table cell comment | One thread per cell. |
| Spreadsheet cell comment | One thread per cell. |
| Slide comment | One thread per slide or content block. |

### 1.3 Common Thread Behaviors

All comment modes share these behaviors:

- **Threaded replies** — Any collaborator can reply within an existing comment thread.
- **@mentions** — Type `@` followed by a collaborator's name to notify them and pull them into the thread.
- **Read/unread tracking** — Comment indicators display in yellow when unread; the indicator turns white once the thread has been opened and read.
- **Sidebar visibility** — All comment threads appear in the document's right-hand sidebar. The sidebar provides a chronological view of all threads in the document.
- **Typing presence** — OgreNotes shows a typing indicator when another collaborator is composing a reply in the same thread.
- **Read receipts** — At the bottom of any comment thread, read receipts indicate which collaborators have seen the thread.
- **Edit history** — Every comment and edit is tracked; the sidebar doubles as an activity log for the document.

---

## 2. Comment Modes

### 2.1 Block Comment (Paragraph-Level)

**Applies to:** Documents  
**Anchor:** A full paragraph, list item, or checklist item

A block comment is attached to an entire paragraph or block-level element. It is triggered via the comment bubble that appears in the margin when a user hovers over or selects a paragraph.

**Behavior:**

- The comment bubble (chat icon) appears to the right of any paragraph when the cursor is placed within it or the paragraph is selected.
- Clicking the bubble opens a comment composer anchored to that paragraph.
- The paragraph is highlighted (typically in yellow) to indicate an active comment thread.
- Only one block comment thread is permitted per paragraph. A second block comment cannot be opened on the same paragraph while a thread is active.

**Use cases:**

- Reviewing a requirements statement and flagging an ambiguity.
- Leaving approval or sign-off on a design paragraph.
- Asking a clarifying question about a planning assumption.

**Constraints:**

- One thread per paragraph. This constraint applies only to block comments — inline comments on the same paragraph are still permitted and operate independently.
- Applies to checklist items individually (each item is its own section).

---

### 2.2 Inline Comment (Selected Text)

**Applies to:** Documents  
**Anchor:** A selected range of text within a paragraph

An inline comment is attached to a specific selection of text — one or more words, a phrase, or a sentence — within a paragraph. The selected text is highlighted, and the comment is anchored to that highlight. Multiple inline comment threads can exist on the same paragraph, each anchored to a different text selection.

**Behavior:**

- The user selects text (click and drag, or double-click a word).
- A formatting toolbar appears above the selection; a comment option is available in this toolbar.
- Once submitted, the selected text is underlined or highlighted in yellow to indicate an active inline comment thread.
- The inline comment thread appears in the sidebar and is linked visually to its highlighted text range.
- A second (or third, etc.) inline comment can be added to the same paragraph by selecting a different range of text and opening a new thread. Each selection carries its own independent thread.

**Relationship to block comments:**

A block comment and one or more inline comments can coexist on the same paragraph. The block comment is anchored to the paragraph as a whole; each inline comment is anchored to a specific text range. They are independent threads and do not interfere with one another.

**Use cases:**

- Flagging multiple distinct terms or clauses in a single requirements paragraph.
- Commenting on a specific identifier while separately noting an issue with the surrounding sentence.
- Suggesting rewording of a phrase while a reviewer has an open block comment on the broader paragraph.

**Constraints:**

- Selections cannot span multiple paragraphs; each inline comment must be anchored within a single paragraph.
- Each text selection within a paragraph supports one thread. The same text range cannot have two simultaneous inline comment threads.

---

### 2.3 Table Cell Comment

**Applies to:** Tables embedded in OgreNotes documents  
**Anchor:** A single table cell

OgreNotes documents can contain embedded tables. Each cell in a table is a discrete section, and each cell can carry its own independent comment thread.

**Behavior:**

- Click into a table cell to place the cursor. The comment bubble appears to the right of the cell row.
- The comment is anchored to the specific cell, not the whole table or the row.
- Multiple cells in the same table can each have active comment threads simultaneously.
- The highlighted cell indicator follows the same yellow/white read-state behavior as other comment modes.

**Use cases:**

- Commenting on a specific value in a decision matrix.
- Flagging a requirement status in a traceability table.
- Asking about a specific entry in a planning timeline table.

**Constraints:**

- One thread per cell. A cell is its own section.
- Comments cannot be placed on the table as a whole — only on individual cells.
- An empty cell can receive a comment.

---

### 2.4 Spreadsheet Cell Comment

**Applies to:** Standalone OgreNotes spreadsheets and spreadsheets embedded in OgreNotes documents  
**Anchor:** A single spreadsheet cell

Spreadsheet cell commenting follows the same model as table cell commenting but applies to OgreNotes' full spreadsheet context (with formulas, data validation, and so on). Each cell is a section; each cell supports one comment thread.

**Behavior:**

- Select a spreadsheet cell. The comment bubble appears in the margin alongside the selected row.
- The comment is anchored to the cell address (e.g., B4) and persists even if the cell content changes.
- Comment indicators appear on the cell itself in the spreadsheet view, making it easy to see which cells have active threads without opening the sidebar.
- Replies, @mentions, and read-state tracking all behave identically to other comment modes.

**Embedded spreadsheets:**

When a spreadsheet is embedded within an OgreNotes document, cell comments on the spreadsheet are tracked separately from any comments on the surrounding document content. The sidebar shows both in chronological order.

**Use cases:**

- Questioning a formula or value in a planning estimate.
- Flagging a data input that needs verification.
- Discussing a specific budget line or schedule entry.
- Noting a dependency for a particular cell in a project tracker.

**Constraints:**

- One thread per cell.
- Comments are anchored to cell addresses; if rows or columns are inserted and the cell shifts position, the comment follows the cell, not the address.
- Charts embedded in spreadsheets do not support cell-level comments.

---

### 2.5 Slide Comment (OgreNotes Slides)

**Applies to:** OgreNotes Slides (presentations)  
**Anchor:** A slide or a content element on a slide

OgreNotes Slides supports commenting at the slide level. Comments allow collaborators to give feedback on specific slides during collaborative presentation development.

**Behavior:**

- A comment icon is available when viewing a slide in OgreNotes Slides.
- Comments are displayed in the sidebar, organized by slide.
- Collaborators can reply, @mention, and track read state in the same way as other comment modes.
- OgreNotes Slides supports **interactive feedback prompts** — a distinct mechanism from standard comments that allows the presenter to pose questions to viewers during or after a presentation, collecting responses in a structured way.

**Distinction between comments and feedback prompts:**

| Feature | Comment | Feedback Prompt |
|---|---|---|
| Initiated by | Any collaborator | Presenter / slide author |
| Appears on | Sidebar | Embedded in the slide |
| Response collection | Threaded reply | Structured poll / open response |
| Typical use | Review feedback | Audience engagement |

**Use cases:**

- Reviewing draft slides before a design review presentation.
- Flagging a slide that needs updated data.
- Collecting stakeholder input on a proposed architecture diagram in slide form.

**Constraints:**

- OgreNotes Slides is a separate document type from OgreNotes documents and spreadsheets; slides cannot be embedded within a standard OgreNotes document and receive inline comments.
- One thread per slide or content block, depending on how OgreNotes resolves section boundaries within a slide layout.

---

## 3. Cross-Cutting Behaviors

### 3.1 Sidebar as Unified Comment View

Regardless of comment mode, all threads for a given document appear in the right-hand sidebar in chronological order. The sidebar shows:

- The author and timestamp of the opening comment and each reply.
- The content anchor (which paragraph, which cell, which slide).
- Read/unread state per thread.
- The full reply history for each thread.

### 3.2 Notifications and @mentions

When a collaborator is @mentioned in any comment thread, they receive a notification. OgreNotes also sends notifications when:

- A new comment is added to a document the user is a member of.
- A reply is posted in a thread the user has participated in.

### 3.3 Permissions

Comment access is controlled by the document's sharing settings. The relevant permission levels are:

| Access Level | Can Read Comments | Can Comment | Can Edit Document |
|---|---|---|---|
| Full Access | ✓ | ✓ | ✓ |
| Edit | ✓ | ✓ | ✓ |
| Comment | ✓ | ✓ | ✗ |
| View only | ✓ | ✗ | ✗ |

### 3.4 Resolving and Deleting Comments

Comment threads can be resolved (marking the discussion as concluded) or deleted (removing the thread entirely). Resolving preserves the thread history; deleting removes it. Both actions are available to collaborators with appropriate permissions.

### 3.5 Export Behavior

When an OgreNotes document is exported:

- **DOCX export:** Comments are included in the exported Word document as standard Word comment annotations.
- **PDF export:** Comments may or may not be rendered depending on the export configuration. Sidebar comments are generally not embedded in the PDF.
- **HTML export:** Comment content is included in the export and can be retrieved programmatically for archival or search purposes.
- **Bulk export (eDiscovery):** The bulk export API includes comment content alongside document content, enabling full-corpus search across document text and comments.

---

## 4. API Access

The OgreNotes Automation API exposes comment threads as part of the thread model. Key behaviors:

- Each document (thread) has an associated list of messages, which includes both chat-style sidebar messages and comments anchored to sections.
- Section IDs (the internal identifiers for paragraphs, cells, etc.) are exposed in the HTML representation of the document, allowing API clients to correlate comments with specific content sections.
- Comments can be posted via API to specific sections using the section ID.
- The bulk export API includes comment content for compliance and archival use cases.

---

## 5. Summary Reference

| Mode | Anchor | Multiple Threads Per Anchor? | Context |
|---|---|---|---|
| Block comment | Paragraph / list item | No — one thread per paragraph | Documents |
| Inline comment | Selected text range | Yes — each selection is independent | Documents |
| Table cell comment | Table cell | No — one thread per cell | Tables in documents |
| Spreadsheet cell comment | Spreadsheet cell | No — one thread per cell | Standalone or embedded spreadsheets |
| Slide comment | Slide / content block | No — one thread per slide or block | OgreNotes Slides |

---

## 6. Open Questions

- [ ] **Slide section granularity** — Confirm the exact section boundary behavior within OgreNotes Slides (whole slide vs. individual content blocks).
- [ ] **Comment export in HTML** — Confirm exact HTML structure of comment annotations in HTML export for programmatic parsing.
- [ ] **Resolved comment visibility** — Confirm whether resolved comment threads remain visible in exports and API responses.