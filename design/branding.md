# OgreNotes Branding

## Name

**OgreNotes** -- a collaborative document, spreadsheet, and messaging platform.

The name evokes strength and layers ("ogres have layers"), reflecting a product with depth beneath a straightforward surface.

---

## Tagline Options

- "Documents with teeth."
- "Layered collaboration."
- "Work together. No nonsense."
- "When other providers abandon you go Ogre."

---

## Voice and Tone

- **Direct** -- no corporate fluff, no marketing speak
- **Confident** -- the tool does what it says
- **Approachable** -- powerful but not intimidating
- **Dry humor welcome** -- personality without forced whimsy

Examples:
- Good: "Your document is saved." Not: "Your document has been safely whisked away to the cloud!"
- Good: "3 people editing." Not: "You and 2 collaborators are co-creating together in real-time!"

---

## Logo Concept

- An ogre silhouette or face integrated with a document/page motif
- Simple enough to work as a favicon (16x16), app icon, and full logo
- Must be recognizable in monochrome (no color dependency)

### Icon Variants

| Context | Format |
|---------|--------|
| Favicon | 16x16, 32x32 simplified glyph |
| App icon | 512x512 with detail |
| Toolbar / sidebar | 24x24 monochrome |
| Full logo | Wordmark + icon, horizontal layout |
| Document watermark | Light, unobtrusive |

---

## Color Palette

### Primary

| Name | Hex | Usage |
|------|-----|-------|
| **Swamp Green** | `#2D5F2D` | Primary brand color, buttons, links |
| **Ogre Brown** | `#5C3D2E` | Secondary accents, headers |
| **Parchment** | `#F5F0E8` | Background, document canvas |

### Functional

| Name | Hex | Usage |
|------|-----|-------|
| **Ink** | `#1A1A1A` | Body text |
| **Stone** | `#6B6B6B` | Secondary text, borders |
| **Mist** | `#E8E4DC` | Subtle backgrounds, hover states |
| **Bone** | `#FFFFFF` | Cards, modals, content areas |

### Accent / Status

| Name | Hex | Usage |
|------|-----|-------|
| **Moss** | `#4A7C4A` | Success, online indicators |
| **Amber** | `#D4920B` | Warnings, notifications |
| **Rust** | `#C0392B` | Errors, destructive actions |
| **River** | `#2980B9` | Links, info, selection highlight |
| **Violet** | `#7B5EA7` | Collaboration cursors (1 of N) |

### Dark Mode

| Name | Hex | Usage |
|------|-----|-------|
| **Deep Bog** | `#1A1F1A` | Dark background |
| **Charcoal** | `#2A2A2A` | Card/surface background |
| **Ash** | `#B0B0B0` | Secondary text |
| **Pale Green** | `#7DC87D` | Primary accent (dark mode variant) |

---

## Typography

### Application UI

| Role | Font | Fallback |
|------|------|----------|
| Headings | Inter | system-ui, sans-serif |
| Body | Inter | system-ui, sans-serif |
| Monospace | JetBrains Mono | ui-monospace, monospace |

### Document Themes (User-Selectable)

Following the Quip model of document-level typography themes:

| Theme | Headings | Body | Character |
|-------|----------|------|-----------|
| **Default** | Inter | Inter | Clean, modern |
| **Editorial** | Playfair Display | Source Serif 4 | Newspaper/magazine feel |
| **Handwritten** | Caveat | Nunito | Casual, friendly |
| **Technical** | JetBrains Mono | JetBrains Mono | Code-centric, monospaced |
| **Classic** | Merriweather | Merriweather | Traditional serif |

All fonts should be self-hosted or use open-source alternatives to avoid third-party dependencies. OpenDyslexic available as an accessibility option.

---

## Iconography

- **Style**: Outlined, 1.5px stroke, rounded caps/joins
- **Size grid**: 16, 20, 24px
- **Source**: Use an open-source icon set (Lucide, Phosphor, or Tabler Icons) for consistency
- **Custom icons**: Only where the icon set lacks a match (e.g., ogre-specific branding elements)

---

## Spacing and Layout

- **Base unit**: 4px grid
- **Content width**: Max 720px for document canvas (comfortable reading width)
- **Sidebar width**: 240px expanded, 48px collapsed
- **Border radius**: 6px for cards/buttons, 4px for inputs, 2px for tags/badges

---

## UI Component Naming

Use straightforward names that match the product's no-nonsense tone:

| Quip Term | OgreNotes Term |
|-----------|----------------|
| Thread | Document / Chat |
| Desktop (folder) | Home |
| Conversation Pane | Comments |
| Blue Tab / Section Menu | Block Menu |
| Command Library | Command Palette |
| Live Apps | Embeds |
| Star / Favorite | Pin |

---

## File and URL Conventions

- Document URLs: `/d/{id}/{slug}` (e.g., `/d/abc123/q3-planning`)
- Folder URLs: `/f/{id}/{slug}`
- Chat URLs: `/c/{id}/{slug}`
- User profile URLs: `/u/{username}`
- API base: `/api/v1/`

---

## Loading and Empty States

- **Loading**: Subtle skeleton screens matching document layout. No spinners.
- **Empty document**: "Start typing, or press `/` for commands." in placeholder text.
- **Empty folder**: "Nothing here yet. Create a document or drag one in."
- **No search results**: "No matches. Try different keywords."

Keep empty states helpful and brief. No illustrations of sad animals.

---

## Error Messages

Follow the same direct tone:

- "Couldn't save. Check your connection and try again."
- "This document was deleted."
- "You don't have access. Ask the owner to share it with you."
- "Something went wrong. If it keeps happening, let us know."

Never blame the user. State what happened and what to do next.
