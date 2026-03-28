# Rich Text Editor -- Pure Rust Implementation Reference

This document captures the complete feature set of TipTap (built on ProseMirror) as a reference for building an equivalent rich text editor in pure Rust/WASM.

## Reference Documentation

- **ProseMirror Guide:** <https://prosemirror.net/docs/guide/>
- **ProseMirror Reference:** <https://prosemirror.net/docs/ref/>
- **TipTap Documentation:** <https://tiptap.dev/docs/editor/getting-started/overview>
- **yrs (Yjs Rust port):** <https://docs.rs/yrs/latest/yrs/>
- **Leptos:** <https://leptos.dev>

---

## Architecture Overview

ProseMirror is a toolkit, not a monolithic editor. TipTap wraps ProseMirror with a higher-level API and extension system. The architecture has four layers:

1. **Model** -- immutable document tree with typed nodes, marks, and schema validation
2. **State** -- document + selection + plugin states; updated via transactions
3. **Transform** -- operations (steps) that modify the document with position mapping
4. **View** -- contenteditable-based rendering, DOM-to-model reconciliation, input handling

---

## 1. Document Model

### Tree Structure

```
Document (root)
  -> Block Nodes (paragraph, heading, blockquote, codeBlock, table, list, ...)
    -> Inline Content (text nodes with marks, hard breaks, images, mentions, ...)
```

Documents are **immutable trees**. Every modification creates a new tree (structural sharing for efficiency).

### Node

The fundamental unit. Every node has:

| Property | Type | Description |
|----------|------|-------------|
| `type` | NodeType | Schema-defined type (paragraph, heading, etc.) |
| `attrs` | Map<String, Value> | Type-specific attributes (e.g., heading level, image src) |
| `content` | Fragment | Ordered list of child nodes |
| `marks` | Vec<Mark> | Marks applied to this node (for inline nodes only) |
| `text` | Option<String> | Text content (only for text nodes) |

### Fragment

An ordered sequence of child nodes. Provides:
- `size` -- total content size (including all descendants)
- `child_count` -- number of direct children
- `child(index)` -- access by index
- `cut(from, to)` -- extract sub-fragment
- `replace_child(index, node)` -- produce new fragment with replaced child
- Iteration, mapping, equality checking

### Mark

Inline formatting metadata attached to text nodes. A mark has:
- `type` -- MarkType (bold, italic, link, etc.)
- `attrs` -- type-specific attributes (e.g., href for links)

Multiple marks can coexist on the same text (bold + italic). Marks have a schema-defined **order** and **exclusion rules** (e.g., `code` excludes all other marks).

### Text Nodes

Leaf nodes containing string content. Adjacent text nodes with identical mark sets are always merged (normalization). A text node has:
- `text` -- the string content
- `marks` -- marks applied to this text

### Position System

Positions are **integer offsets** into a flat token stream of the document:
- Position 0 is before the document's first child
- Each node boundary counts as 1 (open tag + close tag)
- Each character in a text node counts as 1
- Leaf nodes (images, hard breaks) count as 1

Example: `<p>Hello</p>` has positions 0 (before p), 1-5 (within "Hello"), 6 (after p content), 7 (after p).

### ResolvedPos

A position resolved against the document, providing:
- `depth` -- nesting depth at this position
- `parent` -- the parent node
- `node(depth)` -- ancestor at given depth
- `index(depth)` -- child index at given depth
- `textOffset` -- offset within the text node (if in text)
- `nodeAfter` / `nodeBefore` -- adjacent nodes
- Navigation to shared ancestors, block start/end

### Slice

A cut-out piece of a document, used for clipboard and replace operations:
- `content` -- the Fragment
- `openStart` -- depth of the opening side
- `openEnd` -- depth of the closing side

Open depths indicate how many node boundaries were crossed to create the slice (important for paste fitting).

### NodeRange

A flat range of sibling nodes within a parent, used for wrapping/lifting operations:
- `$from`, `$to` -- resolved positions
- `depth` -- the common ancestor depth
- `parent` -- the common ancestor node
- `startIndex`, `endIndex` -- range of sibling indices

---

## 2. Schema System

The schema defines what documents are valid. It specifies:
- Which node types exist and what content they may contain
- Which mark types exist and where they can appear
- How nodes/marks serialize to/from HTML

### NodeSpec

| Field | Type | Description |
|-------|------|-------------|
| `content` | String | Content expression (e.g., `"block+"`, `"inline*"`, `"paragraph block*"`) |
| `group` | String | Group membership (e.g., `"block"`, `"inline"`) |
| `marks` | String | Allowed marks (`"_"` for all, `""` for none) |
| `inline` | bool | Whether this is an inline node |
| `atom` | bool | Single unit, not directly editable (e.g., image, mention) |
| `selectable` | bool | Whether the node can be selected as a unit |
| `draggable` | bool | Whether the node can be dragged |
| `code` | bool | Whether content is code (preserves whitespace) |
| `whitespace` | String | `"normal"` or `"pre"` |
| `defining` | bool | Preserved during content replacement |
| `isolating` | bool | Editing operations don't cross boundaries |
| `attrs` | Map | Attribute definitions with defaults |
| `toDOM` | fn | Renders node to DOM specification |
| `parseDOM` | Vec<ParseRule> | Rules for parsing HTML into this node type |

### MarkSpec

| Field | Type | Description |
|-------|------|-------------|
| `inclusive` | bool | Whether the mark applies at boundary positions |
| `excludes` | String | Marks that can't coexist (`"_"` for all) |
| `spanning` | bool | Whether mark spans across multiple nodes |
| `group` | String | Group membership |
| `attrs` | Map | Attribute definitions |
| `toDOM` | fn | Renders mark to DOM specification |
| `parseDOM` | Vec<ParseRule> | Rules for parsing HTML into this mark type |

### Content Expressions

A formal grammar for specifying valid child content:

| Expression | Meaning |
|------------|---------|
| `block+` | One or more block nodes |
| `inline*` | Zero or more inline nodes |
| `paragraph block*` | A paragraph followed by zero or more blocks |
| `(paragraph \| heading)+` | One or more paragraphs or headings |
| `text*` | Zero or more text nodes (no other inline content) |
| `listItem+` | One or more list items |
| `tableRow+` | One or more table rows |
| `(tableCell \| tableHeader)*` | Zero or more cells or headers |

Content expressions compile to a **deterministic finite automaton (DFA)** (`ContentMatch`) that validates content at every step.

### DOMOutputSpec

How nodes/marks render to DOM. Returns either:
- A string (text content)
- An array: `[tagName, attrs?, ...children]` where a `0` placeholder indicates "render children here"

Example: heading renders as `["h" + level, attrs, 0]` meaning `<h1 attrs>...children...</h1>`

### ParseRule

Rules for parsing DOM/HTML into model nodes/marks:

| Field | Description |
|-------|-------------|
| `tag` | CSS selector to match (e.g., `"p"`, `"h1"`, `"strong"`) |
| `style` | CSS property to match (e.g., `"font-weight"`) |
| `priority` | Higher priority rules checked first |
| `consuming` | Whether to consume matched content |
| `context` | Required parent context |
| `getAttrs` | Extract attributes from DOM element |
| `contentElement` | Which child element contains the content |

---

## 3. State and Transactions

### EditorState

The complete editor state at a point in time:

| Property | Description |
|----------|-------------|
| `doc` | The current document (Node) |
| `selection` | Current selection (TextSelection, NodeSelection, etc.) |
| `storedMarks` | Marks to apply to next input (set by toggle commands) |
| `schema` | The document schema |
| `plugins` | Array of active plugins |

State is immutable. New states are produced by applying transactions.

### Transaction

A transaction describes a state change. It extends Transform (document changes) with:

| Property | Description |
|----------|-------------|
| `steps` | Array of Steps applied to the document |
| `mapping` | Position mapping from old to new document |
| `selection` | New selection (if changed) |
| `storedMarks` | New stored marks (if changed) |
| `meta` | Metadata map (arbitrary key-value pairs) |
| `scrollIntoView` | Whether to scroll to selection |
| `docChanged` | Whether the document was modified |

### Step Types

Steps are the atomic units of document transformation:

| Step | Description |
|------|-------------|
| `ReplaceStep(from, to, slice)` | Replace content between positions with a slice |
| `ReplaceAroundStep(from, to, gapFrom, gapTo, slice, insert, structure)` | Replace while preserving a gap (used for wrapping) |
| `AddMarkStep(from, to, mark)` | Add a mark to a range |
| `RemoveMarkStep(from, to, mark)` | Remove a mark from a range |
| `AddNodeMarkStep(pos, mark)` | Add a mark to a node |
| `RemoveNodeMarkStep(pos, mark)` | Remove a mark from a node |
| `AttrStep(pos, attr, value)` | Set a node attribute |
| `DocAttrStep(attr, value)` | Set a document-level attribute |

Every step can be **inverted** (producing an undo step) and **mapped** through position changes.

### StepMap

Records position changes from a step as ranges: `[start, oldSize, newSize]`. Used to map positions from old to new document (or vice versa) after changes.

### Transform Methods (~25 chainable methods)

| Method | Description |
|--------|-------------|
| `insert(pos, content)` | Insert content at position |
| `delete(from, to)` | Delete content between positions |
| `replace(from, to, slice)` | Replace range with slice |
| `replaceWith(from, to, node)` | Replace range with node |
| `insertText(text, from, to?)` | Insert/replace text |
| `addMark(from, to, mark)` | Add mark to range |
| `removeMark(from, to, mark?)` | Remove mark from range |
| `clearIncompatible(pos, type, marks?)` | Clear content incompatible with new type |
| `setBlockType(from, to, type, attrs?)` | Change block type |
| `setNodeMarkup(pos, type?, attrs?, marks?)` | Change node type/attrs/marks |
| `setNodeAttribute(pos, attr, value)` | Change single node attribute |
| `split(pos, depth?, typesAfter?)` | Split node at position |
| `join(pos, depth?)` | Join nodes at position |
| `wrap(range, wrappers)` | Wrap range in node(s) |
| `lift(range, target)` | Lift range out of parent |
| `setDocAttribute(attr, value)` | Set document attribute |

---

## 4. Selection Model

### TextSelection

The most common selection type. Has:
- `anchor` -- where the selection started (fixed end)
- `head` -- where the selection ends (movable end)
- `from` -- the lesser of anchor/head
- `to` -- the greater of anchor/head
- `$anchor`, `$head`, `$from`, `$to` -- resolved position equivalents
- `empty` -- true when anchor equals head (cursor, no range)

### NodeSelection

Selects an entire node (e.g., an image, a horizontal rule). Has:
- `node` -- the selected node
- `from`, `to` -- positions before and after the node

### AllSelection

Selects the entire document. `from` = 0, `to` = doc content size.

### GapCursor

A cursor in positions that don't normally allow text (e.g., between two tables, at the edge of an isolating node). Rendered as a visual line via CSS.

### Selection Resolution

`Selection.findFrom(resolvedPos, dir, textOnly?)` -- find the nearest valid selection from a position. Handles cases where a position falls inside a node that can't contain a cursor.

---

## 5. Plugin System

### Plugin Interface

```rust
struct PluginSpec {
    // Custom state management
    state: Option<PluginStateSpec>,
    // Key for accessing this plugin's state
    key: Option<PluginKey>,
    // EditorView props to set
    props: Option<EditorProps>,
    // Called when view is created
    view: Option<fn(EditorView) -> PluginView>,
    // Filter transactions before application
    filter_transaction: Option<fn(Transaction, EditorState) -> bool>,
    // Append transactions after others
    append_transaction: Option<fn(Vec<Transaction>, EditorState, EditorState) -> Option<Transaction>>,
}
```

### PluginStateSpec

```rust
struct PluginStateSpec<T> {
    init: fn(config, EditorState) -> T,
    apply: fn(Transaction, T, EditorState, EditorState) -> T,
}
```

### Decoration System

Decorations modify rendering without changing the document:

| Type | Description |
|------|-------------|
| `Inline(from, to, attrs)` | Add attributes/classes to a range of inline content |
| `Widget(pos, dom_or_fn)` | Insert a DOM element at a position |
| `Node(from, to, attrs)` | Add attributes/classes to a node |

Decorations are stored in a `DecorationSet` -- a tree-shaped structure that maps efficiently through document changes.

### Plugin Lifecycle (PluginView)

```rust
trait PluginView {
    fn update(&mut self, view: &EditorView, prev_state: &EditorState);
    fn destroy(&mut self);
}
```

---

## 6. Commands

### Command Signature

```rust
type Command = fn(state: &EditorState, dispatch: Option<&dyn Fn(Transaction)>, view: Option<&EditorView>) -> bool;
```

When `dispatch` is `None`, the command checks applicability without executing. When provided, the command creates and dispatches a transaction.

### Built-In Commands

| Command | Description | Shortcut |
|---------|-------------|----------|
| `toggleMark(type, attrs?)` | Toggle a mark on selection | varies |
| `setBlockType(type, attrs?)` | Change selected block type | varies |
| `wrapIn(type, attrs?)` | Wrap selection in a node | varies |
| `lift` | Lift selection out of wrapping node | |
| `splitBlock` | Split the block at cursor | `Enter` |
| `joinBackward` | Join with block before | `Backspace` |
| `joinForward` | Join with block after | `Delete` |
| `selectAll` | Select entire document | `Ctrl+A` |
| `exitCode` | Exit a code block | `Enter` (in code) |
| `liftEmptyBlock` | Lift empty block | `Enter` (at list end) |
| `createParagraphNear` | Create paragraph adjacent to non-text block | |
| `deleteSelection` | Delete selected content | |
| `newlineInCode` | Insert newline in code block | |
| `chainCommands(...)` | Try commands in sequence until one succeeds | |

### TipTap Command Chaining

TipTap extends commands with a chainable API:

```
editor.chain()
  .focus()
  .toggleBold()
  .run()
```

And a `.can()` method to check applicability without executing.

---

## 7. Input Rules

Text patterns that trigger document transformations while typing.

### InputRule Types

| Type | Description | Example |
|------|-------------|---------|
| `textblockTypeInputRule(regexp, type, attrs?)` | Convert current block on match | `# ` -> heading |
| `wrappingInputRule(regexp, type, attrs?)` | Wrap in node on match | `> ` -> blockquote |
| `markInputRule(regexp, type, attrs?)` | Apply mark on match | `**text**` -> bold |
| `textInputRule(regexp, text)` | Replace text on match | `--` -> em dash |

### Standard Markdown-Style Rules

| Input | Result |
|-------|--------|
| `# ` | Heading level 1 |
| `## ` | Heading level 2 |
| `### ` | Heading level 3 |
| `> ` | Blockquote |
| `* `, `- `, `+ ` | Bullet list |
| `1. ` | Ordered list |
| `[ ] `, `[x] ` | Task list |
| ```` ``` ```` | Code block |
| `---`, `___` | Horizontal rule |
| `**text**`, `__text__` | Bold |
| `*text*`, `_text_` | Italic |
| `` `text` `` | Inline code |
| `~~text~~` | Strikethrough |
| `==text==` | Highlight |

---

## 8. Key Bindings

### Default Keymap

| Key | Command |
|-----|---------|
| `Enter` | splitBlock / newlineInCode / createParagraphNear / liftEmptyBlock / splitListItem |
| `Backspace` | deleteSelection / joinBackward / selectNodeBackward |
| `Delete` | deleteSelection / joinForward / selectNodeForward |
| `Ctrl+B` | toggleBold |
| `Ctrl+I` | toggleItalic |
| `Ctrl+U` | toggleUnderline |
| `Ctrl+E` | toggleCode |
| `Ctrl+Shift+S` | toggleStrike |
| `Ctrl+Shift+H` | toggleHighlight |
| `Ctrl+Shift+8` | toggleBulletList |
| `Ctrl+Shift+7` | toggleOrderedList |
| `Ctrl+Shift+9` | toggleTaskList |
| `Ctrl+Shift+B` | toggleBlockquote |
| `Ctrl+Alt+C` | toggleCodeBlock |
| `Ctrl+Alt+0` | setParagraph |
| `Ctrl+Alt+1-6` | setHeading(level) |
| `Ctrl+Z` | undo |
| `Ctrl+Shift+Z` | redo |
| `Ctrl+A` | selectAll |
| `Shift+Enter` | setHardBreak |
| `Tab` | indent / sinkListItem / goToNextCell |
| `Shift+Tab` | outdent / liftListItem / goToPreviousCell |
| `Ctrl+Shift+L` | textAlign left |
| `Ctrl+Shift+E` | textAlign center |
| `Ctrl+Shift+R` | textAlign right |
| `Ctrl+Shift+J` | textAlign justify |

Key specification supports modifiers: `Mod` (Ctrl on PC, Cmd on Mac), `Ctrl`, `Alt`, `Shift`, `Meta`.

Multiple keymaps are supported with priority ordering -- first match wins.

---

## 9. Clipboard / Copy-Paste

### Copy Pipeline

1. Serialize selection to HTML using schema's `toDOM`
2. Attach ProseMirror-specific metadata (open depth info as `data-pm-slice` attribute)
3. Set both `text/html` and `text/plain` on the clipboard

### Paste Pipeline

1. Read clipboard data (`text/html` preferred, fallback to `text/plain`)
2. Run `transformPastedHTML(html)` hook
3. Parse HTML through `clipboardParser` (derived from schema's `parseDOM`)
4. Run `transformPasted(slice)` hook
5. Fit the parsed Slice to the current schema context
6. Apply the fitted slice via a replace transaction

### Schema Conformance

Pasted content is automatically adjusted to fit the schema. Unknown elements are stripped, invalid nesting is flattened, and marks that aren't allowed in the target context are removed.

---

## 10. View Layer

### EditorView Responsibilities

- Render the document model to a contenteditable DOM element
- Observe DOM mutations via MutationObserver
- Map DOM changes back to document model transactions
- Synchronize browser selection with model selection
- Handle keyboard, mouse, clipboard, drag-and-drop, and composition events
- Apply decorations to the rendered output
- Manage NodeViews for custom rendering

### Rendering Pipeline

```
Document (Model)
  -> toDOM (per NodeSpec/MarkSpec)
    -> contenteditable div (Browser)
      -> MutationObserver watches for changes
        -> Reconciliation: DOM changes -> model transactions
```

### DOM Event Flow

1. User types or pastes
2. `beforeinput` event fires (modern approach) or `keydown`/`compositionstart`
3. Browser modifies contenteditable DOM
4. `MutationObserver` detects changes
5. View reads DOM state, computes diff against model
6. View creates and dispatches a transaction
7. New state is applied, DOM is re-rendered to match model
8. Selection is synchronized

### IME / Composition Handling

The hardest part of contenteditable. During composition:
- Let the browser handle it natively (do NOT intercept)
- After `compositionend`, read the resulting DOM state
- Reconcile with the document model
- Crucial: the IME holds a reference to a specific text node -- if re-rendering destroys that node, composition breaks

### Selection Synchronization

Bidirectional:
- Model -> DOM: after each state update, set the browser selection to match model selection
- DOM -> Model: when the user clicks or arrow-keys, read the browser selection and update model

Must handle DOM positions that fall between nodes, inside non-editable content, or at mark boundaries.

---

## 11. NodeViews (Custom Rendering)

For nodes that need interactive or non-standard rendering (images with resize handles, embedded spreadsheets, code blocks with syntax highlighting).

### NodeView Interface

```rust
trait NodeView {
    fn dom(&self) -> &Element;                    // Root DOM element
    fn content_dom(&self) -> Option<&Element>;    // Where child content goes (None = no editable content)
    fn update(&mut self, node: &Node, decorations: &[Decoration]) -> bool;  // Update DOM for new node
    fn select_node(&mut self);                    // Called when node is selected
    fn deselect_node(&mut self);                  // Called when deselected
    fn stop_event(&self, event: &Event) -> bool;  // Prevent event from reaching editor
    fn ignore_mutation(&self, record: &MutationRecord) -> bool;  // Ignore DOM mutation
    fn destroy(&mut self);                        // Cleanup
}
```

**With content**: `content_dom` returns an element where ProseMirror manages child content normally.

**Black box**: `content_dom` returns None. The node is opaque to ProseMirror -- the NodeView handles all internal rendering.

---

## 12. TipTap Extension System

TipTap wraps ProseMirror with a declarative extension API. Three base types:

### Extension (Functionality Only)

For features that don't add schema types (history, placeholder, focus, etc.).

### Node Extension

Extends Extension with schema node definition and rendering.

### Mark Extension

Extends Extension with schema mark definition and rendering.

### Common Extension Methods

| Method | Description |
|--------|-------------|
| `addOptions()` | Define configurable options |
| `addStorage()` | Persistent state |
| `addCommands()` | Editor commands |
| `addKeyboardShortcuts()` | Key bindings |
| `addInputRules()` | Text pattern triggers |
| `addPasteRules()` | Paste pattern triggers |
| `addProseMirrorPlugins()` | Raw ProseMirror plugins |
| `addGlobalAttributes()` | Attributes across multiple types |
| `addExtensions()` | Bundle extensions |
| `parseHTML()` | HTML -> model parsing rules |
| `renderHTML()` | Model -> HTML rendering |
| `addAttributes()` | Node/mark attributes with defaults |
| `addNodeView()` | Custom interactive rendering |
| `configure()` | Set options externally |
| `extend()` | Create modified version |

### Lifecycle Hooks

| Hook | When |
|------|------|
| `onBeforeCreate` | Before editor initialization |
| `onCreate` | Editor ready |
| `onUpdate` | Content changed |
| `onSelectionUpdate` | Selection changed |
| `onTransaction` | Any state change |
| `onFocus` | Editor focused |
| `onBlur` | Editor blurred |
| `onDestroy` | Editor destroyed |

### Priority

Default 100. Higher values load first, affecting plugin order and schema precedence.

---

## 13. Node Extensions Inventory (28 total)

### StarterKit Nodes

| Node | Element | Content | Group | Key Features |
|------|---------|---------|-------|-------------|
| **Document** | (root) | `block+` | top | Required root node |
| **Text** | (text node) | leaf | inline | Required; carries marks |
| **Paragraph** | `<p>` | `inline*` | block | `Ctrl+Alt+0` to set |
| **Heading** | `<h1>`-`<h6>` | `inline*` | block | Levels 1-6; `Ctrl+Alt+1-6`; `# ` input rule |
| **Blockquote** | `<blockquote>` | `block+` | block | `Ctrl+Shift+B`; `> ` input rule |
| **BulletList** | `<ul>` | `listItem+` | block | `Ctrl+Shift+8`; `* ` input rule |
| **OrderedList** | `<ol>` | `listItem+` | block | `Ctrl+Shift+7`; `1. ` input rule |
| **ListItem** | `<li>` | `paragraph block*` | - | defining; Tab/Shift+Tab for indent |
| **CodeBlock** | `<pre><code>` | `text*` | block | `Ctrl+Alt+C`; ` ``` ` input rule; `language` attr |
| **HardBreak** | `<br>` | leaf | inline | `Shift+Enter` |
| **HorizontalRule** | `<hr>` | leaf | block | `---` input rule |

### Additional Nodes

| Node | Element | Content | Key Features |
|------|---------|---------|-------------|
| **TaskList** | `<ul data-type="taskList">` | `taskItem+` | `Ctrl+Shift+9`; `[ ] ` input rule |
| **TaskItem** | `<li data-type="taskItem">` | `paragraph block*` | `checked` attr; checkbox toggle |
| **Image** | `<img>` | leaf, atom | `src`, `alt`, `title` attrs; optional resize handles |
| **Table** | `<table>` | `tableRow+` | Isolating; resizable columns |
| **TableRow** | `<tr>` | `(tableCell\|tableHeader)*` | Row container |
| **TableCell** | `<td>` | `block+` | `colspan`, `rowspan`, `colwidth` attrs |
| **TableHeader** | `<th>` | `block+` | Same attrs as cell; header role |
| **Mention** | `<span>` | leaf, atom, inline | `id`, `label` attrs; suggestion system |
| **CodeBlockLowlight** | `<pre><code>` | `text*` | Extends CodeBlock with syntax highlighting |
| **Details** | `<details>` | `detailsSummary detailsContent` | Collapsible content |
| **DetailsSummary** | `<summary>` | `inline*` | Summary text |
| **DetailsContent** | `<div>` | `block+` | Collapsible body |
| **Emoji** | inline atom | leaf | Unicode emoji; suggestion integration |
| **Mathematics** | inline/block | leaf | KaTeX rendering; `$...$` input rule |
| **YouTube** | `<iframe>` | leaf | YouTube embed with video options |
| **Audio** | `<audio>` | leaf | Audio player with controls |

---

## 14. Mark Extensions Inventory (10 total)

| Mark | Element | Shortcut | Input Rule | Exclusions |
|------|---------|----------|------------|------------|
| **Bold** | `<strong>` | `Ctrl+B` | `**text**` | - |
| **Italic** | `<em>` | `Ctrl+I` | `*text*` | - |
| **Strike** | `<s>` | `Ctrl+Shift+S` | `~~text~~` | - |
| **Code** | `<code>` | `Ctrl+E` | `` `text` `` | Excludes all other marks |
| **Underline** | `<u>` | `Ctrl+U` | - | - |
| **Link** | `<a>` | - | Autolink on type/paste | - |
| **Highlight** | `<mark>` | `Ctrl+Shift+H` | `==text==` | - |
| **Subscript** | `<sub>` | `Ctrl+,` | - | - |
| **Superscript** | `<sup>` | `Ctrl+.` | - | - |
| **TextStyle** | `<span>` | - | - | Base for Color, FontFamily, FontSize |

### Link Extension Details

- Attributes: `href`, `target` (default `_blank`), `rel` (default `noopener noreferrer nofollow`)
- **Autolink**: detects URLs while typing; configurable protocols, `isAllowedUri` validation
- **linkOnPaste**: auto-wraps pasted URLs
- Commands: `setLink({ href })`, `toggleLink({ href })`, `unsetLink()`

### TextStyle Extension Details

Does nothing alone. Provides a `<span>` container for styling sub-extensions:
- **Color** -- `setColor(value)`, `unsetColor()` -> `<span style="color: ...">`
- **BackgroundColor** -- `setBackgroundColor(value)`, `unsetBackgroundColor()`
- **FontFamily** -- `setFontFamily(name)`, `unsetFontFamily()`
- **FontSize** -- `setFontSize(size)`, `unsetFontSize()`
- **LineHeight** -- `setLineHeight(value)`, `unsetLineHeight()`

---

## 15. Functionality Extensions Inventory

### Core Functionality

| Extension | Description | Config |
|-----------|-------------|--------|
| **History** | Undo/redo stack | `depth` (100), `newGroupDelay` (500ms) |
| **Dropcursor** | Visual indicator when dragging content | `color`, `width`, `class` |
| **Gapcursor** | Cursor in positions between non-editable nodes | CSS-rendered |
| **Placeholder** | Hint text on empty nodes | `placeholder` (string or fn), CSS `::before` |
| **TrailingNode** | Auto-add node after last block | `node` type, `notAfter` types |
| **ListKeymap** | Improved backspace/delete in lists | `listTypes` config |
| **Focus** | CSS class on focused node | `className`, `mode` |
| **Selection** | Preserve visual selection on blur | `className` |

### Menus

| Extension | Trigger | Positioning |
|-----------|---------|-------------|
| **BubbleMenu** | Text selected | Floating UI, above selection |
| **FloatingMenu** | Empty line/paragraph | Floating UI, configurable placement |

Both accept `shouldShow` callback and developer-defined content.

### Text Processing

| Extension | Description |
|-----------|-------------|
| **Typography** | Auto-replace: `--` -> em dash, `...` -> ellipsis, smart quotes, `(c)` -> copyright, fractions, arrows, etc. (~20 rules, individually configurable) |
| **CharacterCount** | Count chars/words; optional limit enforcement |
| **TextAlign** | Alignment commands: left/center/right/justify on configured node types |

### Tables

| Extension | Description |
|-----------|-------------|
| **Table** | Full table support with column resizing |
| **TableRow** | Row container |
| **TableCell** | Cell with colspan/rowspan/colwidth |
| **TableHeader** | Header cell |
| **TableKit** | Bundle of all table extensions |

#### Table Commands

| Command | Description |
|---------|-------------|
| `insertTable({ rows, cols, withHeaderRow })` | Create table |
| `deleteTable()` | Remove table |
| `addRowBefore()` / `addRowAfter()` | Insert row |
| `deleteRow()` | Remove row |
| `addColumnBefore()` / `addColumnAfter()` | Insert column |
| `deleteColumn()` | Remove column |
| `mergeCells()` / `splitCell()` / `mergeOrSplit()` | Cell merge/split |
| `toggleHeaderRow()` / `toggleHeaderColumn()` / `toggleHeaderCell()` | Header toggle |
| `setCellAttribute(name, value)` | Set cell attribute |
| `goToNextCell()` / `goToPreviousCell()` | Cell navigation |
| `fixTables()` | Repair malformed tables |

### Collaboration

| Extension | Description |
|-----------|-------------|
| **Collaboration** | Yjs binding via y-prosemirror; replaces History extension |
| **CollaborationCursor** | Remote cursor/selection rendering via Yjs Awareness |

### Suggestion System

Powers @mentions, slash commands, emoji pickers:

| Config | Description |
|--------|-------------|
| `char` | Trigger character (default `@`) |
| `items` | Return filtered suggestions (supports async) |
| `command` | Execute on selection |
| `render` | Lifecycle: `onStart`, `onUpdate`, `onKeyDown`, `onExit` |
| `allowSpaces` | Allow spaces in query |
| `startOfLine` | Only trigger at line start |

### Additional

| Extension | Description |
|-----------|-------------|
| **DragHandle** | Draggable handle for block nodes |
| **FileHandler** | Handle file drag-and-drop and paste |
| **InvisibleCharacters** | Show spaces, hard breaks, paragraph marks |
| **UniqueID** | Assign/maintain unique IDs on nodes |
| **TableOfContents** | Track headings for ToC generation |
| **Comments** | Inline and block commenting with threads |

---

## 16. Collaboration Architecture

### ProseMirror Native (OT-based)

Central authority model:
- `sendableSteps(state)` -- get uncommitted steps
- `receiveTransaction(state, steps, clientIDs)` -- apply remote steps
- `getVersion(state)` -- current version number
- Step rebasing for concurrent edits

### Yjs/CRDT Integration (y-prosemirror)

Three plugins:
- **ySyncPlugin** -- synchronizes EditorState with Y.XmlFragment
- **yCursorPlugin** -- renders remote cursors via Awareness protocol
- **yUndoPlugin** -- per-client undo/redo (won't undo others' changes)

Uses `Y.XmlFragment` to map ProseMirror's document tree. Block nodes become XmlElements, inline content uses XmlText with formatting attributes as marks.

### Awareness Protocol

Non-persistent state for user presence:
- Cursor position and selection range
- User name, color, avatar
- Broadcast to all connected clients
- Each provider (y-websocket, etc.) implements awareness

---

## 17. Rust/WASM Implementation Considerations

### Existing Rust Resources

| Resource | Status | Usable For |
|----------|--------|------------|
| **prosemirror-rs** (Xiphoseer) | Minimal, pre-release | Model/transform reference only |
| **Matrix Rich Text Editor** | Archived | Architecture reference |
| **yrs** | Mature, production | Collaboration engine |
| **Loro** | Active, newer | Alternative CRDT with better rich text semantics (Peritext) |

### What Must Be Built From Scratch

1. **Document model + schema** (~3000 lines in ProseMirror) -- Node, Fragment, Slice, Schema, ContentMatch DFA
2. **Transform/Step system** -- all 8 step types with position mapping, inversion, composition
3. **View layer** (~6000+ lines) -- contenteditable bridge, DOM mutation observation, input handling, selection sync, IME composition
4. **Plugin/extension system** -- decoration set, plugin state, event handling
5. **yrs-to-model bridge** -- bidirectional mapping between yrs XML types and custom schema

### Critical Implementation Notes

| Concern | Detail |
|---------|--------|
| **Immutability** | ProseMirror nodes are immutable. Use structural sharing (Rc/Arc) for efficiency. |
| **UTF-16 vs UTF-8** | Browser DOM uses UTF-16 positions; Rust strings are UTF-8. Position mapping must account for multi-byte characters. |
| **Content DFA** | Content expressions compile to a deterministic finite automaton. Implement ContentMatch as a state machine. |
| **Mark ordering** | Marks have a schema-defined canonical order. Mark arrays must be kept sorted. |
| **Text normalization** | Adjacent text nodes with identical mark sets must always be merged. |
| **WASM-DOM boundary** | Every DOM operation crosses the WASM-JS boundary. Batch operations to minimize interop cost. |
| **Composition events** | During IME composition, let the browser handle it natively. Read DOM state after compositionend. |

### Relevant Rust Crates

| Category | Crates |
|----------|--------|
| **CRDT** | `yrs`, `loro`, `automerge` |
| **Rope** | `ropey`, `crop`, `jumprope`, `xi-rope` |
| **Arena trees** | `indextree`, `generational-indextree`, `ego-tree` |
| **HTML parsing** | `html5ever`, `ammonia` (sanitizer), `scraper`, `lol_html` |
| **Markdown** | `pulldown-cmark`, `comrak` |
| **WASM interop** | `wasm-bindgen`, `web-sys`, `js-sys`, `gloo` |
| **Framework** | `leptos` (recommended), `dioxus`, `yew` |
| **Regex** | `regex` (for input rules) |
| **Serialization** | `serde`, `serde_json` |

### Recommended Architecture

The view layer (contenteditable interaction) is the hardest part and benefits most from staying close to the browser. Two viable approaches:

**A. Hybrid (Recommended)**: Rust/WASM for document model, schema, transforms, collaboration engine. Thin JavaScript layer for contenteditable interaction, DOM mutation observation, and selection management. Communicate via `wasm-bindgen`. This is the approach used by the Matrix Rich Text Editor and leptos-tiptap.

**B. Full Rust**: Build the entire view layer in Rust via `web-sys`. All DOM APIs exist in web-sys (Selection, MutationObserver, ClipboardEvent, CompositionEvent, InputEvent). The challenge is WASM-JS boundary cost for frequent DOM operations. Batch DOM reads/writes aggressively. No production editor has shipped this way.

### Framework Integration

For Leptos (the recommended Rust frontend framework):
- Use `NodeRef<Div>` to get a reference to the editor container
- Initialize the editor in an `Effect::new()` callback (runs after DOM mount)
- The editor component is a "black box" -- Leptos provides the container but does not reconcile its children
- Application chrome (toolbar, sidebar, file browser) uses normal Leptos components
