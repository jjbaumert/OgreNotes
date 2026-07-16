# Bundled third-party fonts

All fonts in this directory are self-hosted to avoid third-party runtime
dependencies (no Google Fonts / CDN calls at page load) and are licensed
under the **SIL Open Font License, Version 1.1** (see `OFL.txt` for the
license text). Latin subset, Regular (400) + Bold (700) weights, sourced
from the [Fontsource](https://fontsource.org) redistributions of the
upstream families.

Used by the document typography themes (#59 T-12; see
`design/branding.md` §Typography and `style/fonts.css`).

| File(s) | Family | Copyright / upstream |
|---------|--------|----------------------|
| `OpenDyslexic-*.woff2`, `OpenDyslexicMono-Regular.woff2` | OpenDyslexic | © Abbie Gonzalez, OpenDyslexic project |
| `playfair-display-{400,700}.woff2` | Playfair Display | © Claus Eggers Sørensen |
| `source-serif-4-{400,700}.woff2` | Source Serif 4 | © Adobe (Frank Grießhammer) |
| `caveat-{400,700}.woff2` | Caveat | © Impallari Type (Pablo Impallari) |
| `nunito-{400,700}.woff2` | Nunito | © Vernon Adams, Cyreal, Jacques Le Bailly |
| `merriweather-{400,700}.woff2` | Merriweather | © Sorkin Type Co |
| `jetbrains-mono-{400,700}.woff2` | JetBrains Mono | © JetBrains s.r.o. |

To refresh a family, re-fetch the corresponding
`@fontsource/<family>@5/files/<family>-latin-<weight>-normal.woff2` and
keep this table in sync.
