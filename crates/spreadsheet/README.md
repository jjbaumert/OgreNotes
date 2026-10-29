# Shared spreadsheet engine

The browser's standalone spreadsheets and embedded document grids use this
crate. The original frontend module paths re-export the same evaluator, formula
parser, functions, pivots, statistics, and formula translation.

`embedded` decodes version-1 inline workbook sheet attributes. Sparse
`cell:ROW:COL` attributes contain raw values or formulas. Independent
`format:ROW:COL` attributes contain number-format names or patterns. Sheet
identities are stable `blockId` values; names identify cross-sheet formulas.
Active sheet, selection, scroll, and drafts are local to the viewer.

Inline workbooks allow up to 32 sheets, 1,000 rows and 256 columns per sheet,
and 10,000 stored values and formats in total. Cell sources and computed text
are limited to 4,096 UTF-8 bytes, number formats to 128 bytes. Numeric patterns
include `0`, `0.00` (up to 12 decimal places), `#,##0.00`, and a trailing `%`.
Unsupported patterns retain their source and show ordinary numeric text. Canonical sparse
coordinates must fit their sheet. Empty sparse values represent editor clears.

Inline evaluation uses the existing evaluator with a sparse typed workbook
snapshot and dependency ordering, so cross-sheet arithmetic uses computed
values independently of number formatting. Cycles display `#CIRCULAR!`.
Duplicate sheet names (ignoring ASCII case) or identities invalidate the
workbook instead of silently hiding data. Renaming a sheet does not rewrite
existing formulas; update formulas that refer to the old name.

Server exports evaluate persisted, untrusted formulas. Inline evaluation enables
bounded arithmetic, aggregate, logical, lookup, text, and date functions listed
in `embedded::bounded_function`. Unsupported functions, including allocation
functions such as `REPT` and dynamic arrays such as `SEQUENCE`, preserve their
source and display `#NAME?`. Oversized expression/reference work displays
`#NUM!`; references outside the grid display `#REF!`. No foreign-document fetch
is authorized by embedding a formula. Standalone spreadsheet behavior is
unchanged by these inline limits.

Native Yrs snapshots preserve source formulas and formats. HTML, Markdown,
DOCX and PDF exports show best-effort computed tables. Browser date functions
use JavaScript dates; native export uses UTC with ISO/RFC3339 parsing. Volatile
functions such as `NOW`, `TODAY`, and `RAND` recalculate at export time.
