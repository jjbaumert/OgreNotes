# Quip Presentation Editor

> **Note:** Quip Slides was retired January 31, 2021. No new decks could be created after that date; existing decks became view-only. This document captures the full feature set for clean room reference.

## Reference Documentation

- **Introducing Quip Slides:** <https://quip.com/blog/introducing-quip-slides>
- **Convert PowerPoint to Quip Slides:** <https://quip.com/blog/convert-powerpoint-to-quip-slides>
- **Quip Slides Retirement Notice:** <https://quip.com/blog/quip-slides-is-retiring-on-january-31>
- **Quip Formatting:** <https://quip.com/blog/formatting>
- **Quip Typography Themes:** <https://quip.com/blog/typography>
- **Rich Media Picker:** <https://quip.com/blog/new-feature-rich-media-picker>
- **Salesforce Retirement Article:** <https://help.salesforce.com/s/articleView?id=000389647&type=1>

---

## Overall Layout

- **Left panel** -- slide thumbnail navigation for reordering, adding, duplicating, deleting slides
- **Center** -- slide canvas (editing area)
- **Right** -- conversation pane (toggleable, shows comments, edits, chat)
- **Top** -- persistent toolbar/menu bar with contextual formatting and insertion controls
- **Upper left** -- sidebar toggle (hamburger icon) for navigating to other documents

---

## Slide Canvas

- **Widescreen 16:9** aspect ratio (standard, not configurable)
- Elements placed freely on the canvas: text boxes, images, videos, charts, Live Apps
- Every element has its own **comment button** for contextual feedback
- Single living version -- no file versioning, real-time sync across all editors

---

## Slide Panel (Left Sidebar)

- Thumbnail view of all slides
- **Add slide** -- opens a slide picker with layout templates organized by category
- **Duplicate slide** -- copy existing slide
- **Delete slide** -- remove slide
- **Reorder slides** -- `Ctrl+Shift+Up/Down` or drag-and-drop
- **Add new slide shortcut** -- `Ctrl+M`

---

## Slide Layouts

Five categories of default layouts available in the slide picker:

| Category | Description |
|----------|-------------|
| **Titles** | Title slides and section headers |
| **Text** | Text-heavy layouts |
| **Data** | Chart and data-focused layouts |
| **Media** | Image and video-focused layouts |
| **Diagrams** | Diagram-focused layouts |

### Custom Templates (Admin-Controlled)

- Any existing slide deck can be converted to a template
- Managed in the **Quip Admin Console**
- Each template deck appears as a new section in the slide picker
- Unlimited template sections can be created
- Enforces brand consistency across the organization

---

## Text Editing

### Typography Themes (Document-Level)

Selectable via the gear menu. Five themes:

| Theme | Description |
|-------|-------------|
| **Atlas** (default) | Atlas Grotesk headings + Lyon Text body |
| **Modern** | Neue Haas Grotesk (all sans-serif) |
| **Byline** | Publico (editorial serif) |
| **Marseilles** | Duplicate Sans (casual mid-century) |
| **Manuscript** | Courier Prime (monospaced, centered headings) |

Arbitrary font selection is **not** supported. Font is determined by the chosen theme.

### Text Sizes

- Normal paragraph (`Ctrl+Alt+0`)
- Large heading (`Ctrl+Alt+1`)
- Medium heading (`Ctrl+Alt+2`)
- Small heading (`Ctrl+Alt+3`)

### Inline Styling

| Style | Shortcut |
|-------|----------|
| Bold | `Ctrl+B` |
| Italic | `Ctrl+I` |
| Underline | `Ctrl+U` |
| Strikethrough | `Ctrl+Shift+X` |
| Monospace / code | `Ctrl+Shift+K` |
| Code block | `Ctrl+Alt+K` |
| Link | `Ctrl+K` |

### Per-Character Formatting

Size or color of specific text selections within a text box can be changed independently without affecting all text in the box.

### Text Alignment

Standard alignment options: left, center, right.

---

## Color System

### Standard Palette

Named colors: text, text-secondary, action, selection, red, orange, yellow, green, blue, violet.

### Corporate Colors

- Up to **30 custom corporate colors** configured by admin in the Admin Console
- Available in the color picker alongside standard colors

### Color Application

The color picker applies uniformly to:
- Text color
- Background color
- Border color
- Highlight color (multiple bright options, default yellow)

---

## Background Options

- **Solid color backgrounds** -- selected from the color picker (standard + corporate colors)
- No gradient backgrounds
- No background images

---

## Images and Video

### Image Insertion (Rich Media Picker)

Three sources:
1. **Recently Used / Upload** -- upload new images or reuse recent ones
2. **Stock Image Library** -- curated professional stock photos
3. **GIF Search** -- via Giphy integration

### Video Insertion

- Insert via the same button as images
- Options for **looping** and **autoplay**

### Manipulation

- Basic resize and positioning on canvas
- No advanced crop, filters, or image editing tools

---

## Shapes and Drawing

Quip Slides has **no native shape drawing tools** (rectangles, circles, arrows, lines, etc.).

For shapes and diagrams, users relied on third-party Live App integrations:
- **Lucidchart** -- drag-and-drop shapes, formatting, auto-alignment, diagram templates (via `@Lucidchart`)
- **Draw.io Diagrams** -- alternative diagramming integration

---

## Charts and Data

### Embedded Charts

- Created from **Quip Spreadsheets** embedded in the same document
- Chart types: **Pie**, **Line**, **Bar**
- Customizable colors and labels
- **Live-updating** when source spreadsheet data changes
- Can connect to spreadsheets backed by Salesforce Reports for live CRM data

### Embedded Spreadsheets

- Spreadsheet data can be embedded directly on slides
- Maintains live connection to source data

---

## Live Apps on Slides

Interactive components insertable via `@` command:

| Live App | Description |
|----------|-------------|
| **Calendar** | Project timeline and scheduling |
| **Project Tracker** | Task status tracking |
| **Kanban Board** | Task management with assignees and due dates |
| **Salesforce Record** | Always-current CRM data with bidirectional sync |
| **Salesforce List Views** | Live CRM list data |
| **Lucidchart** | Diagrams and flowcharts |
| **Draw.io** | Diagrams |
| **Polls** | Embedded voting/polling |
| **Feedback Prompts** | Questions and comment prompts |
| **Jira** | Issue tracking (Atlassian) |
| **DocuSign** | Document signing |
| **Box / Dropbox** | File embedding |

Live Apps are real-time syncing, work offline, and function cross-platform.

---

## Comments and Collaboration

### Real-Time Co-Editing

- Multiple simultaneous editors
- Changes sync in real time across all devices
- Single living version (no file branching)

### Commenting

- Every element has its own comment button
- Comments can be placed anywhere on a slide
- Comment highlights support multiple colors (toggle in upper-right of comment)
- Shortcut: `Ctrl+Shift+C`

### Built-in Chat

- Integrated conversation pane for discussing slides
- Toggle: `Ctrl+Alt+C`
- Supports `@mentions` of people and documents
- `/invite @person` to grant access from conversation

### Feedback Prompts

- Embeddable questions, polls, and comment prompts on any slide
- Audience can respond interactively

### Engagement Insights

- Analytics showing which stakeholders viewed the presentation
- Per-slide engagement metrics (most/least viewed slides)

---

## Transitions and Animations

**Quip Slides has no transitions or animations.** This was a deliberate design choice -- the product targeted simple slideshows for team meetings, not keynote presentations.

Animated GIFs and animated stickers can be used as workarounds for visual interest.

---

## Presenter Mode

Limited presenter mode -- slides can be viewed in full-screen format. No dedicated presenter view with:
- No speaker notes
- No timer
- No next-slide preview
- No laser pointer

The emphasis was on collaborative viewing rather than traditional keynote delivery.

---

## Version History

- Available via Document menu > "Show Edit History"
- Timestamps for every previous version, sorted by edit time and editor
- **Restore button** to revert to any previous version
- Green for additions, red for deletions, with names and timestamps

---

## Import and Export

### Import

- **PowerPoint (.pptx)** import supported
- Process: Compose > Upload or Import > drag file or browse
- Imported slides become collaboratively editable

### Export

- **PDF export** -- primary export format
- **No export to PowerPoint (.pptx)** format
- Slides can be copied to Quip documents for continued editing

---

## Keyboard Shortcuts

### Slide-Specific

| Shortcut | Action |
|----------|--------|
| `Ctrl+M` | Add new slide |
| `Ctrl+Shift+Up` | Move slide up |
| `Ctrl+Shift+Down` | Move slide down |

### General (Usable in Slides)

| Shortcut | Action |
|----------|--------|
| `Ctrl+B` | Bold |
| `Ctrl+I` | Italic |
| `Ctrl+U` | Underline |
| `Ctrl+Shift+X` | Strikethrough |
| `Ctrl+Shift+K` | Monospace |
| `Ctrl+Alt+0` | Normal paragraph |
| `Ctrl+Alt+1` | Large heading |
| `Ctrl+Alt+2` | Medium heading |
| `Ctrl+Alt+3` | Small heading |
| `Ctrl+Shift+L` | Bulleted list |
| `Ctrl+Alt+K` | Code block |
| `Ctrl+K` | Insert link |
| `Ctrl+Shift+A` | Anchor link |
| `Ctrl+Z` | Undo |
| `Ctrl+Shift+Z` | Redo |
| `Ctrl+Shift+C` | Add comment |
| `Ctrl+Alt+C` | Toggle conversation pane |
| `Ctrl+/` | Show all shortcuts |
| `Esc` | Stop editing |
| `Tab` | Indent |
| `Shift+Tab` | De-indent |

---

## Mobile

- Available on iOS (Android retired June 2024)
- Designed for **view, comment, and collaborate** rather than full editing
- Formatting via gray bar above keyboard
- Offline access with sync when back online

---

## Object Manipulation

### Documented

- Basic resize and positioning of text boxes and images on canvas
- Per-element commenting

### Not Documented / Not Supported

| Feature | Status |
|---------|--------|
| Object alignment/distribution tools | Not documented |
| Layering / z-order (bring to front/back) | Not documented |
| Object grouping | Not supported |
| Grid display | Not documented |
| Snap-to-grid | Not documented |
| Smart guides | Likely basic support (undocumented) |
| Action buttons / hotspots | Not supported |

---

## Limitations vs PowerPoint / Google Slides

### Missing Features

1. No native shapes or drawing tools
2. No transitions or animations
3. No speaker notes
4. No presenter view (timer, next-slide preview)
5. No slide master / master slides
6. No gradient or image backgrounds
7. No arbitrary font selection (5 themes only)
8. No export to PowerPoint format
9. No custom slide dimensions or aspect ratios
10. No object grouping, layering, or distribution tools
11. No grid or snap-to-grid
12. Limited chart types (pie, line, bar only)
13. No text effects (shadows, outlines, WordArt)
14. No SmartArt or auto-layout diagrams (natively)
15. No audio insertion
16. No table creation on slides (only via embedded spreadsheets)

### Where Quip Slides Excelled

1. Real-time collaboration with built-in chat
2. Live data from Salesforce and spreadsheets
3. Feedback prompts and embedded polls
4. Engagement analytics (viewer tracking, per-slide metrics)
5. Live Apps ecosystem (Calendar, Kanban, Project Tracker, etc.)
6. Corporate template management via Admin Console
7. Custom corporate color palette (up to 30 colors)
8. Cross-platform availability
9. PowerPoint import
10. Per-element commenting
11. Version history with restore

### Design Philosophy

Quip Slides was intentionally designed for **simple slideshows for team meetings**, not for keynote presentations. It prioritized collaboration and live data over design polish.
