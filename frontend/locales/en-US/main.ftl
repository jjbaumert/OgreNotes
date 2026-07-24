# OgreNotes — en-US strings.
#
# This is the SOURCE-OF-TRUTH catalog. Every user-visible string
# in the frontend should resolve from here. Other locale files
# (e.g. ar/main.ftl) are translations of these keys; any key
# present here but missing in another locale falls back to en-US.
#
# Key naming convention: `<surface>.<noun-or-action>`. Surfaces
# follow the Leptos component / page boundary (sidebar, home,
# editor, etc.). Use hyphens, not snake_case, per Fluent syntax.
#
# When adding a key:
#   1. Add it here first.
#   2. Add the translation to ar/main.ftl (or leave it absent —
#      the fallback chain handles missing keys).
#   3. Replace the raw string in the Leptos view with t!("key").
#   4. The i18n-audit script will flag any remaining raw English
#      strings in view! macros (M-P2 piece 2+).
#
# See design/i18n.md for the full architecture; Fluent reference
# at https://projectfluent.org/.

# ─── Common (shared across surfaces) ────────────────────────────

common-loading = Loading…
common-send = Send
common-close = Close
# Document Details panel (#141)
document-details-title = Document details
document-details-name = Name
document-details-type = Type
document-details-created = Created
document-details-modified = Last modified
document-details-words = Words
document-details-characters = Characters
doc-type-document = Document
doc-type-spreadsheet = Spreadsheet
# Editor gutter (#139)
editor-page-break = Page { $n }
# Find & Replace bar (#147)
find-placeholder = Find
find-replace-placeholder = Replace with
find-no-results = No results
find-prev = Previous match
find-next = Next match
find-replace = Replace
find-replace-all = Replace all
common-delete = Delete
common-cancel = Cancel
common-untitled = Untitled
common-redirecting-login = Redirecting to login…
common-redirecting = Redirecting…
common-open-navigation = Open navigation
common-restore-here = Restore here

# ─── Sidebar ────────────────────────────────────────────────────

sidebar-section-navigation = Navigation
# Favorites (#144)
document-favorite = Add to favorites
# Expand / full screen (#145)
document-expand-enter = Expand to full screen
document-expand-exit = Exit full screen
document-unfavorite = Remove from favorites
# Favorite star dropdown + collections (#144)
document-favorite-menu = Favorites
favorite-menu-added = Added to Favorites
favorite-menu-add = Add to Favorites
favorite-menu-remove = Remove from Favorites
favorite-menu-collections = Collections
favorite-menu-new-collection = New Collection…
collection-new-prompt = Name this collection
sidebar-section-favorites = Favorites
sidebar-empty-favorites = No favorites yet
sidebar-empty-collection = Empty collection
sidebar-doc-open-new-tab = Open in New Tab
sidebar-doc-actions-aria = Document actions
sidebar-new-aria = Create new
sidebar-new-document = New Document
sidebar-new-spreadsheet = New Spreadsheet
menubar-help = Help
sidebar-home = Home
sidebar-search = Search
sidebar-templates = Templates
sidebar-sign-out = Sign out
sidebar-aria-main-nav = Main navigation
sidebar-aria-collapse = Collapse sidebar
sidebar-aria-expand = Expand sidebar

# ─── Menu bar (document chrome) ─────────────────────────────────

menubar-document = Document
menubar-edit = Edit
menubar-view = View
menubar-insert = Insert
menubar-format = Format

# #142: Document menu — Mark / Unmark template (label flips on state).
menubar-mark-template = Mark as Template…
menubar-unmark-template = Unmark Template

# #142: Document header — one-click copy from an open template.
document-use-template = Use Template

# #142: Template picker modal (shell-mounted; opens from sidebar, the
# Document menu, and the home page).
template-picker-title = Pick a template
template-picker-loading = Loading templates…
template-picker-empty = No templates yet — mark any document via Document → Mark as Template…
template-picker-use = Use
template-picker-using = Copying…
# Phase 2 — mail merge. Templates with [[placeholders]] show "Fill…" on
# the row instead of "Use", and a second step collects values.
template-picker-fill = Fill…
template-picker-fill-title = Fill in template values
template-picker-fill-hint = Leave a field blank to keep the placeholder as-is.
template-picker-create = Create
template-picker-cancel = Cancel
template-picker-back = Back to template list
# Phase 3 — sample templates gallery. The picker groups rows by tag.
template-picker-section-mine = Your templates
template-picker-section-shared = Shared with you
template-picker-section-sample = Samples
# Fallback bucket for rows whose `gallery` tag is absent (server drift).
# Not translated with the same care as the others — the bucket should
# be empty on any healthy deploy.
template-picker-section-untagged = Templates
modal-close = Close

# ─── Notification panel ─────────────────────────────────────────

notifications-title = Notifications
notifications-mark-all-read = Mark all read
notifications-clear-all = Clear all
notifications-dismiss = Dismiss
notifications-empty = No notifications

# ─── Chat panel (sidebar) ───────────────────────────────────────

chat-section-title = Chats
chat-empty = No chats yet
chat-back = ← Back to chats
chat-message-placeholder = Type a message…

# ─── Document outline pane ──────────────────────────────────────

outline-title = Outline
outline-empty = No headings
outline-aria-close = Close outline

# ─── Editor toolbar ─────────────────────────────────────────────
#
# Tooltips on the formatting toolbar. Keyboard shortcuts are
# embedded in the source string so the translator can position
# the shortcut idiomatically (e.g. before/after the action name,
# or with locale-appropriate delimiters).

toolbar-undo = Undo (Ctrl+Z)
toolbar-redo = Redo (Ctrl+Shift+Z)
toolbar-bold = Bold (Ctrl+B)
toolbar-italic = Italic (Ctrl+I)
toolbar-underline = Underline (Ctrl+U)
toolbar-strikethrough = Strikethrough
# Subscript / Superscript (#143)
toolbar-subscript = Subscript
toolbar-superscript = Superscript
toolbar-code = Code (Ctrl+E)
# Toolbar alignment controls (#134)
toolbar-align-left = Align left
toolbar-align-center = Align center
toolbar-align-right = Align right
toolbar-text-color = Text Color
toolbar-remove-color = Remove color
toolbar-highlight = Highlight
toolbar-remove-highlight = Remove highlight
toolbar-image = Image
toolbar-link = Link (Ctrl+K)
toolbar-horizontal-rule = Horizontal Rule
menubar-insert-link = Link…
menubar-insert-table = Table
toolbar-insert-table = Insert Table
toolbar-insert-label = Insert:
toolbar-comment = Comment (Ctrl+Alt+C)
toolbar-comment-label = Comment
toolbar-more = More
toolbar-aria-more = More toolbar options
toolbar-prompt-url = Enter URL:

# Block-type dropdown (formatting menu)
toolbar-block-paragraph = Paragraph
toolbar-block-heading-1 = Heading 1
toolbar-block-heading-2 = Heading 2
toolbar-block-heading-3 = Heading 3
toolbar-block-heading-4 = Heading 4
toolbar-block-bulleted-list = Bulleted List
toolbar-block-numbered-list = Numbered List
toolbar-block-checklist = Checklist
toolbar-block-blockquote = Blockquote
toolbar-block-code-block = Code Block
toolbar-block-format = Format

# Number formats (spreadsheet block-type menu)
toolbar-num-general = General
toolbar-num-integer = Integer
toolbar-num-decimal-1 = Decimal (1)
toolbar-num-decimal-2 = Decimal (2)
toolbar-num-thousands = Thousands
toolbar-num-currency-usd = Currency (USD)
toolbar-num-currency-eur = Currency (EUR)
toolbar-num-percent = Percent

# ─── Comment popup (inline floating panel) ──────────────────────

comment-new-title = New Comment
comment-thread-title = Comment Thread
comment-aria-prev = Previous comment
comment-aria-next = Next comment
comment-placeholder-new = Add a comment about this section
comment-placeholder-reply = Type a message…
comment-edited = edited
comment-edit = Edit
comment-save = Save

# ─── Conversation pane (side panel) ─────────────────────────────

conversation-thread = Thread
conversation-comment-on-block = Comment on block
conversation-comments = Comments
conversation-empty = No comments yet. Start a conversation!
conversation-placeholder-block = Comment on this block…
conversation-placeholder-add = Add a comment…
conversation-placeholder-reply = Reply…
conversation-aria-prev = Previous comment
conversation-aria-next = Next comment
conversation-back = ← Back
conversation-status-open = Open
conversation-status-resolved = Resolved
conversation-resolve = Resolve
conversation-reopen = Reopen
conversation-typing-1 = { $name } is typing…
conversation-typing-2 = { $a } and { $b } are typing…
conversation-typing-many = Several people are typing…

# ─── History viewer (version panel) ─────────────────────────────

history-title = Edit History
history-empty = No version history yet
history-no-prior = No earlier version to diff against — this is the first snapshot.
history-changes-in-v = Changes in v{ $version }
history-restore-version = Restore version
history-aria-close = Close
history-jump-to-block-title = Jump to block in the live document
history-jump-to-block-label = Jump to block ↗
history-restoring = Restoring…
history-restore-to-this-version = Restore to this version
history-restore-confirm-message = Replace the current document with this version? Any unsaved local changes will be lost.
history-restore-confirm-label = Restore
history-deleted-badge = (deleted)

# Node-type labels for diff cards
node-paragraph = Paragraph
node-heading = Heading
node-bullet-list = Bullet list
node-ordered-list = Numbered list
node-list-item = List item
node-task-list = Task list
node-task-item = Task
node-blockquote = Quote
node-code-block = Code block
node-horizontal-rule = Divider
node-image = Image
node-table = Table
node-table-row = Table row
node-table-cell = Table cell
node-table-header = Table header
node-block = Block

# ─── Login page ─────────────────────────────────────────────────

login-tagline = Documents with teeth.
login-error-name-email-required = Name and email are required
login-placeholder-name = Display name
login-placeholder-email = Email
login-signing-in = Signing in…
login-dev-button = Dev Login (custom)
login-github = Sign in with GitHub
login-google = Sign in with Google

# ─── Share dialog ───────────────────────────────────────────────

share-title = Share
share-placeholder-email = Enter email address
share-button = Share
share-members-heading = Current members
share-role-owner = Owner
share-role-edit = Can Edit
share-role-comment = Can Comment
share-role-view = Can View
share-error-no-folder = Document has no folder — cannot share
share-error-enter-email = Enter an email address
share-status-searching = Searching…
share-error-search-failed = Search failed: { $err }
share-error-no-user = No user found with email '{ $email }'
share-status-shared-with = Shared with { $name }
share-error-failed = Failed to share: { $err }

# Link sharing (doc-scoped section of the share dialog)
share-link-heading = Link sharing
share-link-mode-off = Off
share-link-mode-view = Can view
share-link-mode-edit = Can edit
share-link-note = Anyone in your workspace with the link can open this.
share-link-off = Link sharing is off.
share-link-opt-comments = Allow comments
share-link-opt-history = Show edit history
share-link-opt-conversation = Show conversation
share-link-opt-request = Allow requests to edit
share-link-copy = Copy link
share-link-copied = Link copied
share-link-saved = Saved
share-link-error = Couldn't save: { $err }
# Viewer-facing request-edit-access affordance (#110)
share-link-request-banner = You're viewing this document through a shared link.
share-link-request-button = Request edit access
share-link-request-sending = Sending…
share-link-request-sent = Request sent
share-link-request-retry = Couldn't send — try again

# ─── Document page ──────────────────────────────────────────────

document-loading = Loading document…
document-trash-banner = This document is in Trash — Restore to edit.
document-trash-restore = Restore
document-trash-delete-forever = Delete forever
document-locked-banner = This document is locked. Editing is disabled until it is unlocked.
document-locked-unlock = Unlock
document-share-tooltip = Share
# Document menu: rename + move (#146)
document-rename-prompt = Rename document
document-move-folder-title = Move to folder
document-move-here = Move here
# Multi-folder membership (#149)
document-folder-add = + Add to folder
document-folder-add-title = Add to folder
document-folder-add-confirm = Add here
document-folder-remove = Remove from this folder
# Duplicate dialog (#146)
duplicate-dialog-title = Duplicate document
duplicate-name-label = Name
duplicate-destination-label = Destination folder
duplicate-confirm = Duplicate
duplicate-share-warning = This folder is shared — { $count } other people have access, so they'll be able to see the copy.
# Focus/expand toggle (#134)
document-focus-enter = Focus mode
document-focus-exit = Exit focus mode
document-trash-dialog-title = Move to Trash
document-trash-dialog-message = This document will be moved to Trash. You can restore it later.
document-trash-dialog-confirm = Move to Trash
document-purge-dialog-title = Delete forever?
document-purge-dialog-message = This permanently deletes the document and all of its content. This cannot be undone.
document-purge-dialog-confirm = Delete forever
document-restore-folder-title = Restore to folder

# ─── Home page ──────────────────────────────────────────────────

home-new-document = + New Document
home-new-from-template = + New from Template
home-new-spreadsheet = + New Spreadsheet
home-new-folder = + New Folder

# ─── Folder management (#150) ───────────────────────────────────
folder-action-rename = Rename
folder-action-delete = Delete
folder-rename-prompt = Rename folder
folder-delete-dialog-title = Delete folder?
folder-delete-dialog-message = This permanently deletes this empty folder. This cannot be undone.
folder-delete-dialog-confirm = Delete folder
folder-delete-not-empty = This folder isn't empty. Move or remove its contents first, then delete the folder.

# ─── MFA (enroll + challenge) ───────────────────────────────────

mfa-verifying = Verifying…
mfa-enter-totp = Enter the 6-digit code from your authenticator
mfa-enter-recovery = Enter your recovery code

mfa-enroll-title = Set up two-factor authentication
mfa-enroll-subtitle = Scan the QR code with your authenticator app, then enter the 6-digit code to confirm.
mfa-enroll-success = Enrollment confirmed. Redirecting…
mfa-enroll-error-failed = Enroll failed: { $err }
mfa-enroll-error-verify-failed = Verify failed: { $err }
mfa-enroll-manual-entry = Manual entry
mfa-enroll-recovery-codes-summary = Recovery codes (save these now!)
mfa-enroll-recovery-warning = Each code can be used once if you lose access to your authenticator. We won't show them again.
mfa-enroll-code-label = Authenticator code
mfa-enroll-confirm = Confirm

mfa-challenge-title = Two-factor authentication
mfa-challenge-subtitle-totp = Open your authenticator app and enter the 6-digit code.
mfa-challenge-subtitle-recovery = Enter one of your single-use recovery codes.
mfa-challenge-verify = Verify
mfa-challenge-missing-handle = Missing MFA handle, redirecting to login…
mfa-challenge-error-invalid-totp = Invalid code — check your authenticator and try again
mfa-challenge-error-invalid-recovery = Invalid recovery code — each code can only be used once
mfa-challenge-use-totp = Use authenticator code instead
mfa-challenge-use-recovery = Lost your authenticator? Use a recovery code

# ─── Admin console (platform-admin pages) ───────────────────────

admin-loading = Loading admin console…
admin-redirecting = Redirecting…
admin-status-active = active
admin-status-disabled = disabled
admin-status-never = never
admin-role-admin = admin
admin-role-user = user
admin-retry = Retry

# Admin sub-nav
admin-nav-users = Users
admin-nav-metrics = Metrics
admin-nav-audit = Audit
admin-nav-back = Back to app

# Admin · Users
admin-users-title = Admin · Users
admin-users-search-placeholder = Filter by email prefix
admin-users-th-email = Email
admin-users-th-name = Name
admin-users-th-role = Role
admin-users-th-state = State
admin-users-th-last-active = Last active
admin-users-th-actions = Actions
admin-users-enable = Enable
admin-users-disable = Disable
admin-users-promote = Promote
admin-users-demote = Demote
admin-users-prev = Prev
admin-users-next = Next
admin-users-error-list-failed = List failed: { $err }
admin-users-error-action-failed = { $action } failed: { $err }

# Admin · Audit
admin-audit-title = Admin · Audit log
admin-audit-label-target = Target user_id
admin-audit-label-actor = Actor user_id
admin-audit-label-kind = Kind
admin-audit-placeholder-kind = e.g. disable, loginFailure
admin-audit-label-from = From (ISO)
admin-audit-label-to = To (ISO)
admin-audit-search = Search
admin-audit-error-target-required = Target user_id is required
admin-audit-error-load-failed = Load failed: { $err }
admin-audit-th-when = When
admin-audit-th-source = Source
admin-audit-th-kind = Kind
admin-audit-th-actor = Actor
admin-audit-th-target = Target
admin-audit-th-detail = Detail

# Admin · Metrics
admin-metrics-title = Admin · Metrics
admin-metrics-refresh = Refresh
admin-metrics-error-fetch-failed = Fetch failed: { $err }
admin-metrics-counters = Counters
admin-metrics-gauges = Gauges
admin-metrics-histograms = Histograms
admin-metrics-th-key = Key
admin-metrics-th-value = Value
admin-metrics-th-count = Count
admin-metrics-th-sum = Sum
admin-metrics-th-min = Min
admin-metrics-th-max = Max

# ─── Workspace SCIM tokens ──────────────────────────────────────

scim-title = Workspace SCIM Tokens
scim-subtitle = Mint a bearer token for your IdP's SCIM provisioning connector. The plaintext is shown once at creation — copy it immediately.
scim-base-url-heading = SCIM Base URL
scim-base-url-help = Paste this into your IdP's SCIM connector configuration.
scim-fresh-heading = New token: { $name }
scim-fresh-warning = Copy this token NOW — it will not be shown again.
scim-fresh-copy = Copy
scim-create-heading = Create a new token
scim-create-placeholder = Label (e.g., Okta connector)
scim-create-button = Create
scim-existing-heading = Existing tokens
scim-empty = No tokens yet. Create one above.
scim-th-name = Name
scim-th-token-id = Token ID
scim-th-created = Created
scim-th-last-used = Last used
scim-th-status = Status
scim-status-active = active
scim-status-revoked = revoked
scim-revoke = Revoke
scim-error-name-required = Token name is required
scim-error-load-failed = Load failed: { $err }
scim-error-create-failed = Create failed: { $err }
scim-error-revoke-failed = Revoke failed: { $err }

# ─── Workspace SAML SSO ─────────────────────────────────────────

saml-title = Workspace SAML SSO
saml-subtitle-prefix = Configure a SAML 2.0 IdP for this workspace. Members will be able to sign in via the IdP at
saml-subtitle-suffix = once you save.
saml-status-saved = SAML config saved.
saml-status-removed = SAML config removed.
saml-status-copied = SP metadata URL copied to clipboard.
saml-sp-heading = SP Metadata
saml-sp-help = Copy this URL into your IdP's "add service provider" flow. Or fetch the URL once and paste the response XML into your IdP.
saml-copy = Copy
saml-idp-heading = IdP Configuration
saml-idp-help = Paste the metadata XML your IdP gives you. The full XML — including the <EntityDescriptor> root element — is required.
saml-label-entity-id = IdP Entity ID
saml-placeholder-entity-id = https://idp.example.com/metadata
saml-label-metadata-xml = IdP Metadata XML
saml-label-email-attr = Email attribute name
saml-label-name-attr = Name attribute name
saml-save = Save
saml-update = Update
saml-remove = Remove
saml-error-entity-id-required = IdP entity ID is required
saml-error-metadata-required = IdP metadata XML is required
saml-error-load-failed = Load failed: { $err }
saml-error-save-failed = Save failed: { $err }
saml-error-delete-failed = Delete failed: { $err }
saml-meta-first-configured = First configured
saml-meta-last-updated = ; last updated

# ─── Spreadsheet view (chrome) ──────────────────────────────────

ss-empty = No data
ss-format-painter-title = Format painter — click to copy formatting from the active cell, then click a target. Shift-click for sticky mode.
ss-format-painter-status = Painter — click a cell to apply, Esc to cancel
ss-format-painter-status-sticky = Painter (sticky) — click cells to apply, Esc to stop
ss-sort-tooltip = Sort the spreadsheet…

# Status bar (per-selection aggregates)
ss-status-count = Count: { $value }
ss-status-sum = Sum: { $value }
ss-status-avg = Avg: { $value }
ss-status-min = Min: { $value }
ss-status-max = Max: { $value }

# Sheet tabs
ss-rename-sheet-prompt = Rename sheet:
ss-touch-menu-aria = Cell actions
ss-ctx-rename = Rename
ss-ctx-delete = Delete

# Find / replace bar
ss-find-placeholder = Find…
ss-replace-placeholder = Replace…
ss-find-next = Next
ss-find-replace = Replace
ss-find-replace-all = Replace All
ss-find-no-results = 0 results

# Filter dropdown
ss-filter-header = Filter: { $col }
ss-filter-show-all = Show All
ss-filter-custom-prompt = Custom filter (e.g. >100, <0, =Done, contains:err, empty, notempty):
ss-filter-custom-button = Custom filter…
ss-filter-empty-value = (empty)

# Sort dialog
ss-sort-title = Sort
ss-sort-range-label = Range:
ss-sort-has-headers = First row contains headers (skip during sort)
ss-sort-by-label = Sort by
ss-sort-then-by-label = Then by
ss-sort-asc = Ascending
ss-sort-desc = Descending
ss-sort-remove-level-title = Remove this sort level
ss-sort-add-level = + Add sort level
ss-sort-cancel = Cancel
ss-sort-apply = Apply
ss-sort-err-parse-range = Couldn't parse range. Use A1-style notation, e.g. A1:G41.
ss-sort-err-no-keys = Add at least one sort key.

# Foreign-document consent dialog
ss-foreign-title = This document fetches data from other workbooks
ss-foreign-hint = Allowing fetches uses your account's read access to those documents. Approval lasts for this session only.
ss-foreign-deny = Deny
ss-foreign-allow = Allow

# ─── Spreadsheet context menu (cell right-click) ────────────────

ss-ctx-menu-insert = Insert
ss-ctx-menu-delete = Delete
ss-ctx-menu-sort = Sort
ss-ctx-menu-format = Format
ss-ctx-menu-comment = Comment
ss-ctx-menu-hide = Hide / Unhide
ss-ctx-menu-data = Data
ss-ctx-menu-cond-fmt = Conditional Formatting
ss-ctx-menu-validation = Data Validation
ss-ctx-insert-row-above = Insert row above
ss-ctx-insert-row-below = Insert row below
ss-ctx-insert-col-left = Insert column left
ss-ctx-insert-col-right = Insert column right
ss-ctx-sort-dialog = Sort…
ss-ctx-delete-row = Delete row
ss-ctx-delete-rows = Delete { $count } rows
ss-ctx-delete-col = Delete column
ss-ctx-delete-cols = Delete { $count } columns
ss-ctx-clear-contents = Clear contents
ss-ctx-copy-markdown = Copy as markdown
ss-ctx-sort-a-z = Sort A → Z
ss-ctx-sort-z-a = Sort Z → A
ss-ctx-freeze-rows = Freeze rows above
ss-ctx-unfreeze-rows = Unfreeze rows
ss-ctx-freeze-cols = Freeze columns left
ss-ctx-unfreeze-cols = Unfreeze columns

# Cell validation
ss-ctx-set-checkbox = Set as Checkbox
ss-ctx-set-dropdown = Set as Dropdown…
ss-ctx-remove-validation = Remove Validation
ss-ctx-dropdown-prompt = Enter dropdown options (comma-separated):

# Conditional formatting
ss-ctx-cond-fmt = Conditional Formatting…
ss-ctx-cond-fmt-prompt = Conditional format (e.g., >100, <0, =Done, contains:error, empty, notempty):
ss-ctx-cond-fmt-color-prompt = Background color (e.g., #ff0000, red, #ffd):
ss-ctx-color-scale = Color Scale…
ss-ctx-color-scale-prompt = Color scale: low,high or low,mid,high (e.g. #ff0000,#ffff00,#00ff00):
ss-ctx-data-bar = Data Bar…
ss-ctx-data-bar-prompt = Data bar color:
ss-ctx-icon-set = Icon Set…
ss-ctx-icon-set-prompt = Icon set: arrows or traffic

# Charts + pivots
ss-ctx-insert-chart = Insert Chart…
ss-ctx-chart-type-prompt = Chart type (bar, line, pie):
ss-ctx-chart-title-prompt = Chart title:
ss-ctx-chart-unknown-type = Unknown chart type. Use one of: bar, line, pie.
ss-ctx-insert-pivot = Insert Pivot Table…
ss-ctx-pivot-needs-multi = Pivot needs a multi-row, multi-column source selection. Select your data (with the header row in row 1) and try again.
ss-multi-region-copy = Copy and cut work on a single rectangular selection. Clear the extra regions (press Esc) and try again.
ss-multi-region-op = This action won't work on a multiple selection. Clear the extra regions (press Esc) and try again.

# CSV import + merge
ss-ctx-import-csv = Import CSV…
ss-ctx-merge-cells = Merge Cells
ss-ctx-unmerge-cells = Unmerge Cells

# Hide / unhide
ss-ctx-hide-row = Hide Row
ss-ctx-unhide-all-rows = Unhide All Rows
ss-ctx-hide-col = Hide Column
ss-ctx-unhide-all-cols = Unhide All Columns

# Cell lock + comments + named ranges
ss-ctx-lock-cell = Lock Cell
ss-ctx-unlock-cell = Unlock Cell
ss-ctx-add-comment = Add Comment…
ss-ctx-edit-comment = Edit Comment…
ss-ctx-open-comment = Open Comment Thread…
ss-ctx-comment-prompt = Comment:
ss-ctx-remove-comment = Remove Comment
ss-comment-preview-empty = No messages yet
ss-comment-replies-none = No replies
ss-comment-replies-one = 1 reply
ss-comment-replies-many = { $count } replies
ss-ctx-define-name = Define Name…
ss-ctx-name-prompt = Name for this range:
ss-ctx-remove-name = Remove Name…
ss-ctx-no-named-ranges = No named ranges defined.
ss-ctx-remove-name-prompt = Remove which name? Defined: { $names }

# ─── Pivot table editor ─────────────────────────────────────────

ss-pivot-title = Pivot table editor
ss-pivot-foreign-source-label = Foreign source:
ss-pivot-foreign-hint = Foreign-source editing isn't supported yet. Edit the pivot config via the JSON attribute or remove and re-create as a local pivot.
ss-pivot-layout = Layout
ss-pivot-layout-compact = Compact
ss-pivot-layout-outline = Outline
ss-pivot-layout-tabular = Tabular
ss-pivot-totals = Totals
ss-pivot-totals-none = None
ss-pivot-totals-rows = Rows
ss-pivot-totals-cols = Cols
ss-pivot-totals-both = Both
ss-pivot-edit-filter-tooltip = Edit filter
ss-pivot-axis-row = Row
ss-pivot-axis-col = Column
ss-pivot-labels-header = { $axis } labels — { $col }
ss-pivot-close-tooltip = Close
ss-pivot-close-editor-tooltip = Close editor (pivot keeps rendering)
ss-pivot-filter-all = All
ss-pivot-filter-none = None
ss-pivot-filter-prefix = Filter { $col }
ss-pivot-source-label = Source:
ss-pivot-delete = Delete pivot
ss-pivot-section-fields = Fields
ss-pivot-section-rows = Rows
ss-pivot-section-cols = Columns
ss-pivot-section-values = Values
ss-pivot-section-filters = Filters
ss-pivot-search-placeholder = Search fields…
ss-pivot-bin-width-tooltip = Bin width

# Date granularity options (pivot Row/Column chip)
ss-pivot-date-year = Year
ss-pivot-date-quarter = Quarter
ss-pivot-date-month = Month
ss-pivot-date-day = Day
ss-pivot-date-hour = Hour

# NOT translated: SUM / COUNT / COUNTA / AVG / MIN / MAX / MEDIAN /
# PRODUCT / STDDEV / STDDEVP / VAR / VARP aggregation labels — they
# echo the formula language's function names (which are not
# localized — see scripts/i18n-audit.sh exclusion of
# spreadsheet/{eval,parser,functions}.rs).
#
# Also not translated: filter_cond_label output strings (mix of
# operators "<", ">", "=" with English words "empty", "contains").
# Treat as v1 limitation; a future commit can split into Fluent
# templates if a translator complains.

# ─── App-level / router ─────────────────────────────────────────

app-not-found = Page not found

# ─── Accessibility (Phase 5 M-P8) ───────────────────────────────

a11y-skip-to-content = Skip to main content
a11y-toolbar-label = Document formatting toolbar
a11y-toolbar-group-undo = Undo and redo
a11y-toolbar-group-block-type = Block type
a11y-toolbar-group-inline = Inline formatting
a11y-toolbar-group-align = Alignment
a11y-toolbar-group-block = Block formatting
a11y-toolbar-group-insert = Insert
a11y-file-table-label = Documents and folders
a11y-breadcrumb-label = Folder breadcrumb

# ─── @-menu (mention picker) ────────────────────────────────────

at-menu-empty = Type to search people and documents…

# #148 section headers + insertable entries.
at-menu-section-people = People
at-menu-section-documents = Documents
at-menu-section-insert = Insert
at-menu-section-ai = AI
at-menu-ask-ai-hint = Ask the assistant…

# #148 v2 AI wrappers — fixed-prompt shortcuts over @-ask. Each
# composes a system prompt against the current selection or the
# whole document; see `at_menu.rs::compose_directive_prompt` for
# the exact wording.
at-menu-ai-summarize = Summarize this
at-menu-ai-translate = Translate this
at-menu-ai-rewrite = Rewrite this
at-menu-ai-brainstorm = Brainstorm ideas

# @date / /date insertable — one entry per format style.
at-menu-insert-date-medium = Today's date (May 19, 2026)
at-menu-insert-date-short = Today's date (5/19/26)
at-menu-insert-date-long = Today's date + time
at-menu-insert-date-iso = Today's date (ISO 8601)

# Insert catalog entries surfaced by /-menu, toolbar, and @-menu.
# Convention: insert-<id>-label + insert-<id>-description.
insert-table-label = Table
insert-table-description = Insert a 3×3 table
insert-image-label = Image
insert-image-description = Upload and insert an image
insert-horizontal-rule-label = Divider
insert-horizontal-rule-description = Insert a horizontal rule
insert-code-block-label = Code block
insert-code-block-description = Turn this block into a code block

# @-ask insertion button (visible only in the @-ask flow).
ask-insert-into-document = Insert into document

# ─── File browser (home page table) ─────────────────────────────

file-browser-empty = Nothing here yet. Create a document or folder.
file-browser-th-title = Title
file-browser-th-added = Added
file-type-folder = Folder
file-type-document = Document
file-type-spreadsheet = Spreadsheet
file-type-chat = Chat

# ─── Folder picker (restore-to-folder dialog) ───────────────────

folder-picker-not-available =  (not available)

# ─── Formula keyboard (on-screen, mobile spreadsheet) ───────────

formula-key-backspace = Backspace
formula-key-cancel = Cancel (Esc)
formula-key-commit = Commit (Enter)
# Mode-switcher tabs (Phase 5 M-P3 piece C). "Standard" defers to
# the device's built-in keyboard; "Numeric" shows a number pad;
# "Formula" shows operators + function chips. Forced to "Formula"
# when the cell value starts with `=`.
kbd-mode-standard = Aa
kbd-mode-numeric = 123
kbd-mode-formula = ƒx
kbd-standard-hint = Use your device keyboard

# ─── Search dialog (Ctrl+K) ─────────────────────────────────────

search-placeholder = Search documents…
search-searching = Searching…
search-no-results = No results found
search-dialog-label = Search documents or run commands

# ─── Ask (Phase 6 RAG agent) ────────────────────────────────────

ask-dialog-title = Ask the assistant
ask-badge = AI
ask-placeholder = Ask a question about your documents…
ask-empty-hint = The assistant searches your documents and cites what it found. Ask something concrete — "what does our auth design say about session expiry?"
ask-sources-heading = Sources
ask-error-rate-limit = Too many requests. Wait a moment and try again.
ask-error-disabled = The assistant has been disabled for your workspace by an administrator.
ask-error-unavailable = The assistant is temporarily unavailable.
sidebar-ask = Ask

# ─── Relationships (Phase 6 M-6.2 piece D) ─────────────────────

relationship-heading = Related
relationship-empty = No related documents yet.
relationship-add-aria = Add a related document
relationship-remove-aria = Remove this relationship
relationship-picker-placeholder = Search documents to link…
relationship-picker-aria = Search documents to link
relationship-picker-confirm = Link
relationship-type-aria = Relationship type
relationship-error-self = A document cannot link to itself.
relation-type-implements = Implements
relation-type-derived-from = Derived from
relation-type-depends-on = Depends on
relation-type-references = References
relation-type-supersedes = Supersedes

# ─── Theme selector ─────────────────────────────────────────────

theme-aria-label = Theme
theme-system = Follow system theme
theme-light = Light theme
theme-dark = Dark theme
# Short labels for the segmented theme picker in Settings → Appearance.
theme-label-system = System
theme-label-light = Light
theme-label-dark = Dark

# ─── Locale selector ────────────────────────────────────────────

locale-aria-label = Language

# ─── Inline selection / comment-bubble affordances ──────────────

selection-toolbar-comment = Comment on selection
comment-highlights-add = Add comment

# ─── Auth-callback page ─────────────────────────────────────────

auth-complete-status = Completing sign in…

# ─── Sync indicator (Phase 5 M-P3 piece B) ──────────────────────
# Pill in the document header showing whether the user's edits are
# landing on the server. `{$count}` is the number of un-sent local
# updates queued in the CollabClient while offline.

sync-saved = Saved
sync-saving = Saving…
sync-offline = Offline
sync-offline-pending = Offline — {$count} pending
sync-saved-tooltip = Your changes are saved.
sync-saving-tooltip = Sending your latest changes to the server…
sync-offline-tooltip = You're disconnected. Reconnect to keep collaborating.
sync-offline-pending-tooltip = You're disconnected. {$count} change(s) haven't reached the server yet.

# Editor width toggle (S/M/L)
editor-width-group = Editor width
editor-width-narrow = Narrow width
editor-width-medium = Medium width
editor-width-wide = Wide width

# ─── Command palette (Phase 5 M-P4 piece A) ─────────────────────
# Reached via `>` prefix in the Ctrl+K search dialog. v1 ships
# Global-scope commands only; piece B adds Editor / Spreadsheet
# / Home scopes.

palette-no-actions = No commands match.
cmd-go-home = Go to home
cmd-toggle-dark-mode = Toggle dark mode
cmd-open-trash = Open trash
cmd-sign-out = Sign out
cmd-ask = Ask the assistant
cmd-about-palette = Command palette: about
# Editor-scoped commands (M-P4 piece B).
cmd-bold = Bold
cmd-italic = Italic
cmd-underline = Underline
cmd-strike = Strikethrough
cmd-code = Inline code
cmd-heading-1 = Heading 1
cmd-heading-2 = Heading 2
cmd-heading-3 = Heading 3
cmd-paragraph = Paragraph
cmd-bullet-list = Bulleted list
cmd-ordered-list = Numbered list
cmd-task-list = Task list
cmd-blockquote = Blockquote
cmd-code-block = Code block
cmd-divider = Insert divider
cmd-insert-table = Insert table
cmd-undo = Undo
cmd-redo = Redo

# ─── Home-page drop-to-import (Phase 5 M-P5 piece D) ────────────

home-drop-title = Drop to import
home-drop-hint = Markdown (.md) or HTML (.html) — up to 1 MB
home-import-default-title = Imported

# ─── Toolbar — embed insert (Phase 5 M-P6 piece B) ──────────────

toolbar-embed = Embed media (URL)

# ─── Live-app blocks (#136) — see design/live-app-blocks.md ─────

insert-calendar-label = Calendar
insert-calendar-description = Inline calendar with month, week, and day views
calendar-view-month = Month
calendar-view-week = Week
calendar-view-day = Day
calendar-nav-prev = Previous
calendar-nav-next = Next
calendar-nav-today = Today
calendar-empty-day = Click to add event
calendar-event-untitled = (no title)
calendar-modal-add-title = New event
calendar-modal-edit-title = Edit event
calendar-modal-content-label = Title
calendar-modal-color-label = Color
calendar-modal-allday-label = All day
calendar-all-day-strip = all-day
calendar-modal-start-label = Starts
calendar-modal-end-label = Ends
calendar-modal-save = Save
calendar-modal-delete = Delete
insert-kanban-label = Kanban Board
insert-kanban-description = Columns of cards for tracking work in progress
kanban-add-column = Add column
kanban-add-card = Add card
kanban-delete-column = Delete column
kanban-untitled-column = Untitled column
kanban-untitled-card = (untitled)
kanban-modal-add-title = New card
kanban-modal-edit-title = Edit card
kanban-modal-title-label = Title
kanban-modal-content-label = Description
kanban-modal-due-label = Due date
kanban-modal-labels-label = Labels (name|color; …)
kanban-modal-assignee-label = Assignee
kanban-column-rename-prompt = Rename column
insert-mermaid-label = Mermaid diagram
insert-mermaid-description = Insert a diagram rendered from Mermaid text
kanban-column-delete-confirm = Delete this column and all its cards?
kanban-column-wip-limit-prompt = WIP limit (empty to clear)
mermaid-modal-title = Edit Diagram
mermaid-modal-save = Save
mermaid-modal-error-empty = Diagram source cannot be empty.
mermaid-modal-error-too-long = Diagram source is too long ({ $max } character limit).

# ─── File-browser bulk selection (Phase 5 M-P7 piece C) ─────────

file-browser-th-select = Select
bulk-selection-count = {$count} selected
bulk-selection-cancel = Cancel
bulk-selection-delete = Delete
bulk-delete-dialog-title = Move selected documents to trash?
bulk-delete-dialog-message = The selected documents will move to your trash. You can restore them within 30 days.
bulk-delete-dialog-confirm = Move to trash

# ─── Account settings page (design/account-menu.md, step 1) ─────

settings-title = Settings
settings-aria-tabs = Settings sections
settings-tab-profile = Profile
settings-tab-appearance = Appearance
settings-tab-notifications = Notifications
settings-tab-accessibility = Accessibility
settings-tab-help = Help & Support
settings-coming-soon = This section is coming soon.
settings-appearance-theme = Theme
settings-appearance-language = Language
# Document typography themes (#59 T-12)
settings-appearance-doc-theme = Document typography
settings-doc-theme-aria = Document typography theme
settings-doc-theme-default = Default (Inter)
settings-doc-theme-editorial = Editorial (Playfair Display / Source Serif)
settings-doc-theme-handwritten = Handwritten (Caveat / Nunito)
settings-doc-theme-technical = Technical (JetBrains Mono)
settings-doc-theme-classic = Classic (Merriweather)
# BYOK — bring-your-own Anthropic key (#29)
settings-byok-label = AI assistant — use your own Anthropic key
settings-byok-hint = Stored only in this browser and sent with your AI requests; never saved on our servers. Leave blank to use the workspace key.
settings-byok-active = Using your key
settings-byok-none = Using the workspace key.
settings-byok-save = Save key
settings-byok-clear = Remove key
settings-a11y-dyslexic-label = Dyslexia-friendly font
settings-a11y-dyslexic-hint = Use a more legible typeface for document text.
settings-a11y-reduce-motion-label = Reduce motion
settings-a11y-reduce-motion-hint = Minimize animations and transitions across the app.

# ─── Account menu (account-menu step 3) ─────────────────────────

account-menu-aria = Account menu
account-menu-profile = Profile & Status
account-menu-settings = Settings
account-menu-shortcuts = Keyboard shortcuts

# ─── Profile settings form (account-menu step 4) ────────────────

settings-profile-name = Display name
settings-profile-avatar = Avatar URL
settings-profile-email = Email
settings-profile-email-hint = Your email is managed by your sign-in provider and can't be changed here.
settings-save = Save changes
settings-saving = Saving…
settings-saved = Saved
settings-profile-error = Couldn't save your changes. Please try again.
settings-profile-load-error = Couldn't load your profile. Reload the page to try again.
settings-profile-name-required = Display name can't be empty.
settings-profile-avatar-invalid = Avatar URL must start with http:// or https://.

# ─── Status editor (account-menu step 5) ────────────────────────

settings-status-heading = Status
settings-status-emoji = Status emoji
settings-status-text = What's your status?
settings-status-expiry = Clear after
settings-status-expiry-never = Don't clear
settings-status-expiry-30m = 30 minutes
settings-status-expiry-1h = 1 hour
settings-status-expiry-4h = 4 hours
settings-status-set = Set status
settings-status-clear = Clear status

# ─── Notification settings (account-menu step 6) ────────────────

settings-notif-email-heading = Email notifications
settings-notif-all = All activity
settings-notif-mentions = Mentions only
settings-notif-off = Off
settings-notif-hint = Controls which activity emails you. In-app notifications are unaffected.

# ─── Help & Support (account-menu step 6) ───────────────────────

settings-help-shortcuts = Keyboard shortcuts
settings-help-shortcut-palette = Open command palette / search
settings-help-shortcut-actions = Command palette (actions)
settings-help-version = Version

# ─── Collab error toast (Phase 2a LiveApp gate) ─────────────────

collab-liveapp-rejected-toast = Your last change couldn't be saved. Refresh to see the current state.

# --- menu bar + editor context menu (i18n backfill) ---
menu-cut = Cut
menu-copy = Copy
menu-paste = Paste
menu-copy-block-link = Copy Link to Block
menu-bold = Bold
menu-italic = Italic
menu-underline = Underline
menu-strikethrough = Strikethrough
menu-code = Code
menu-comment = Comment
menu-alignment = Alignment
menu-align-left = Left
menu-align-center = Center
menu-align-right = Right
menubar-doc-new = New
menubar-doc-share = Share…
menubar-doc-copy-link = Copy Link
menubar-doc-move-folder = Move to Folder…
menubar-doc-duplicate = Duplicate…
menubar-doc-new-template = New from Template…
menubar-doc-export = Export
menubar-doc-export-html = HTML
menubar-doc-export-markdown = Markdown (copy)
menubar-doc-export-csv = CSV
menubar-doc-export-excel = Excel (.xlsx)
menubar-doc-print = Print…
menubar-doc-history = Document History…
menubar-doc-details = Document Details…
menubar-doc-rename = Rename Document…
menubar-doc-delete = Delete Document…
menubar-edit-undo = Undo
menubar-edit-redo = Redo
menubar-edit-find = Find and Replace
menubar-view-comments = Show Comments
menubar-view-conversation = Show Conversation
menubar-view-cursors = Show Cursors
menubar-view-focus = Focus Mode
menubar-view-line-numbers = Show Line Numbers
menubar-view-page-breaks = Show Page Breaks
menubar-view-outline = Show Outline
menubar-format-subscript = Subscript
menubar-format-superscript = Superscript
menubar-format-paragraph-style = Paragraph Style
menubar-format-list = List
menubar-format-clear = Clear Formatting
menubar-format-lock = Lock Edits
editorctx-paragraph-style = Paragraph style
editorctx-insert-link = Insert link…
