# Quip Spreadsheet Editor

## Reference Documentation

- **Supported Formulas and Functions:** <https://help.salesforce.com/s/articleView?id=sf.quip_supported_formulas_and_functions.htm&type=5>
- **Spreadsheet Basics:** <https://help.salesforce.com/s/articleView?id=xcloud.quip_spreadsheet_basics.htm&type=5>
- **Quip Spreadsheet Functions Document:** <https://quip.com/4PwFADor2VYB>
- **Quip Blog - Spreadsheets:** <https://quip.com/blog/spreadsheets>
- **Quip Blog - Spreadsheet Features:** <https://quip.com/blog/spreadsheets-features>
- **Quip Blog - Data Referencing:** <https://quip.com/blog/Data-Referencing-for-Quip-Spreadsheets>

---

## Layout and Modes

### Embedded Mode (Default)

Spreadsheets embed inline within Quip documents alongside text, images, and task lists. A compact view shows within the document flow. Clicking surfaces the spreadsheet toolbar at the top.

### Maximized Mode

Click the maximize button to expand to a full-screen traditional spreadsheet view. Multiple spreadsheets in a document appear as sheet tabs at the bottom.

### Table Mode

Hides row/column numbers and letters for a cleaner embedded appearance. Access via settings menu > "View in Table Mode". Retains full spreadsheet functionality underneath.

---

## Toolbar and Menu Bar

The spreadsheet toolbar appears when clicking on a spreadsheet. Context-specific controls:

| Control | Description |
|---------|-------------|
| **Format Painter** | Copy formatting between cells |
| **Text Wrap** | Toggle text wrapping in cells |
| **More Formats (123)** | Number, currency, percentage, date/time formats |
| **Conditional Formatting** | Rule-based cell formatting |
| **Text Alignment** | Cell alignment options |
| **Freeze Rows** | Keep header rows visible while scrolling |
| **Table Mode** | Toggle row/column number visibility |
| **Clock icon** | Cell-level edit history |
| **Chart icon** | Insert chart from selected data |
| **Highlighter** | Cell background color |

---

## Cell Styling

### Text Formatting

| Style | Shortcut |
|-------|----------|
| Bold | `Ctrl+B` |
| Italic | `Ctrl+I` |
| Underline | `Ctrl+U` |
| Strikethrough | toolbar |
| Hyperlink | `Ctrl+K` |

### Cell Properties

- **Text color** -- via toolbar color picker
- **Background/highlight color** -- via highlighter icon, multiple bright colors available
- **Text alignment** -- left, center, right
- **Text wrap** -- via toolbar toggle
- **Borders** -- supported, preserved during import/export
- **Merge cells** -- combine adjacent cells

### Font Limitation

Quip does not allow changing the font typeface inside spreadsheets. All data uses the default typeface. Font sizing uses header levels (large, medium, small, normal) rather than arbitrary point sizes.

---

## Number Formats

Accessible via the "123" toolbar button:

- Plain number (configurable decimal places, thousands separator)
- Currency (locale-aware currency symbol)
- Percentage
- Date formats (curated list including ISO 8601)
- Time formats
- Custom date/time format patterns

---

## Cell Content Types

- Text and numbers
- Formulas (starting with `=`)
- Dates and times (stored as serial numbers)
- Boolean (TRUE/FALSE)
- `@mentions` of people and documents (triggers notifications)
- Images (`@image`)
- File attachments (`@file`)
- Checkboxes (toggle with `Ctrl+Enter`)
- Dropdown lists (via data validation)
- Bulleted/numbered lists and checklists (within cells)
- Error values (#REF!, #VALUE!, #DIV/0!, #N/A, #NAME?, #NUM!, #NULL!, #CIRCULAR!)

---

## Cell Editing

- Double-click or start typing to enter edit mode
- `Alt+Enter` inserts a hard return within a cell
- `Ctrl+;` inserts current time
- `Ctrl+:` inserts today's date
- `@` within a cell opens mention menu
- `Ctrl+Shift+.` opens View Cell Content dialog for complex cells
- Formula entry starts with `=`
- Formula autocomplete suggests function names

---

## Selection Model

| Action | Shortcut |
|--------|----------|
| Select cell | Click |
| Select range | Click and drag, or Shift+Click |
| Multi-range select | Ctrl+Click (Windows) / Cmd+Click (Mac) |
| Select entire row | `Shift+Space` |
| Select entire column | `Ctrl+Space` |
| Extend selection to edge | `Ctrl+Shift+Arrow` |
| Mobile range select | Tap-and-hold first cell, then tap final cell |

---

## Row and Column Operations

### Insert

- Right arrow past last column shows "Add Column" button
- Down arrow past last row shows "Add Row" button
- "Add Rows and Columns" button for bulk addition
- `Ctrl+I` to insert rows/columns at cursor

### Delete

- `Ctrl+-` to remove selected rows/columns

### Resize

- Drag column/row border
- Right-click > "Resize" for auto-fit or specific pixel dimensions
- Dimensions preserved in export

### Hide/Unhide

- Hide rows, columns, and entire sheets
- Unhide via Edit menu or context menu

### Freeze

- Freeze header rows to keep visible while scrolling
- Freeze columns supported

### Lock Cells

- Lock a cell, range, or entire sheet to prevent accidental modification
- Access via Format menu > "Lock Edits"
- Anyone with edit access can lock/unlock

---

## Sorting and Filtering

### Single-Column Sort

Click column header or use Data menu. Ascending or descending.

### Multi-Column Sort

Access via Data menu > Custom Sort. Add multiple sort columns, each with independent ascending/descending order.

### Filtering

- Column filters to show/hide rows based on criteria
- "Update Filter" button appears when underlying data changes
- Filters preserved during import/export
- Filters persist at the document level for all viewers

---

## Conditional Formatting

Access via the formatting icon in the spreadsheet menu bar.

### Rule Types

- Equal to a value
- Greater than a value
- Less than a value
- Between two values
- Text contains specific string

### Formatting Applied

Cell background color changes. Multiple rules can be stacked on a range.

### Limitations

No color scales, data bars, icon sets, or gradient formatting.

---

## Data Validation (9 Rule Types)

| Type | Description |
|------|-------------|
| **Attachments** | Cell must contain file attachments |
| **People** | Cell must contain @-mentioned people |
| **Dates** | Cell must contain date values |
| **Numbers** | Cell must contain numeric values |
| **Text** | Cell must contain text |
| **URLs** | Cell must contain valid URLs |
| **Emails** | Cell must contain email addresses |
| **Checkboxes** | Cell renders as a toggleable checkbox |
| **Dropdown** | Configurable from a defined list or existing cell range |

Dropdown shortcut: `Alt+Down` (Windows) / `Option+Down` (Mac)

---

## Charts

### Chart Types

- **Bar** graph
- **Line** graph
- **Pie** chart

### Creation

1. Select data range in spreadsheet
2. Click chart icon in toolbar
3. Select chart type

### Behavior

- Live-updating when data changes
- Customizable colors and labels
- Embeddable in slide presentations
- Supports contextual comments and chat
- Charts in embedded spreadsheets supported

### Limitations

- Charts are excluded from PDF export

---

## Copy and Paste

When pasting, a toolbar menu offers:

| Mode | Description |
|------|-------------|
| **Normal paste** | Pastes values and formulas |
| **Transpose** | Convert rows to columns or vice versa |
| **Values only** | Paste calculation results without formulas |
| **Link cells** | Transfer all copied cell properties to a different table |
| **Data Reference** | Creates a live-syncing reference link between spreadsheets |

Data References can optionally disable format syncing to apply custom styling.

---

## Fill Operations

| Action | Shortcut |
|--------|----------|
| Fill down | `Ctrl+D` |
| Fill right | `Ctrl+R` |
| Fill selection with text | `Ctrl+Enter` |

Traditional drag-to-fill with pattern/series detection (like Excel) is not natively supported.

---

## Formula Bar

- Redesigned menu and formula bar for discoverability
- Supports 400+ functions with autocomplete
- Cell references by typing cell names
- Formulas can reference cells across sheets
- Inline document text can reference spreadsheet cells via `=cellname`
- Mobile: three keyboard modes (standard, numeric keypad, formula-editing with autocomplete)

---

## Context Menu (Right-Click)

Access: right-click, `Alt+Shift+C`, `Shift+F10`, or Menu key.

- Mark Row as Column Headers
- Resize (auto-fit or specific pixel value)
- Hide/Unhide rows/columns
- Lock Edits
- View Cell Content
- Insert/Delete rows/columns
- Cut/Copy/Paste

---

## Cell Reference Syntax

| Syntax | Description |
|--------|-------------|
| `A1` | Relative reference |
| `$A$1` | Absolute reference (locked column and row) |
| `$A1` | Mixed reference (locked column) |
| `A$1` | Mixed reference (locked row) |
| `A1:B10` | Range reference |
| `Sheet1!A1` | Cross-sheet reference within same document |
| `REFERENCERANGE(source)` | Cross-document live reference |
| `REFERENCESHEET(source)` | Cross-document sheet reference |

### Inline Document References

Type `=cellname` in document text to embed a live-updating value from a spreadsheet in the same document.

---

## Operator Precedence

1. Negation (`-`, unary minus)
2. Percentage (`%`)
3. Exponentiation (`^`)
4. Multiplication and Division (`*`, `/`)
5. Addition and Subtraction (`+`, `-`)
6. Concatenation (`&`)
7. Comparison (`=`, `<>`, `<`, `>`, `<=`, `>=`)

---

## Array Formula Support

- Dynamic array functions: `SORT()`, `FILTER()`, `UNIQUE()`, `TRANSPOSE()`
- Results spill into adjacent cells
- `SUMPRODUCT` works as an array-aware function
- `ARRAYFORMULA()` enables array arithmetic for a single formula
- `ARRAY_CONSTRAIN()` truncates an array to given dimensions
- Traditional `Ctrl+Shift+Enter` CSE-style array formulas are not supported; Quip uses the dynamic array approach

---

## Error Types

| Error | Meaning |
|-------|---------|
| `#REF!` | Invalid cell reference (deleted row/column) |
| `#VALUE!` | Wrong argument type (text where number expected) |
| `#DIV/0!` | Division by zero |
| `#N/A` | Value not available (lookup found no match) |
| `#NAME?` | Unrecognized formula name |
| `#NUM!` | Invalid numeric value |
| `#NULL!` | Incorrect range operator |
| `#CIRCULAR!` | Circular reference detected |

Error handling functions: `IFERROR()`, `IFNA()`, `ISERROR()`, `ISERR()`, `ERROR.TYPE()`

---

## Import and Export

### Import Formats

- Excel (.xls, .xlsx) -- preserves hyperlinks, images, merged cells, currency, borders, text color, filters, data validation
- CSV
- OpenOffice
- Drag-and-drop on desktop

### Export Formats

- Excel (.xlsx) -- supports hyperlinks, images, merged cells, column width, row height, text alignment, text color
- PDF (max 40,000 cells, charts excluded)

---

## Keyboard Shortcuts

| Action | Mac | Windows |
|--------|-----|---------|
| Move to edge of data | `Cmd+Arrow` | `Ctrl+Arrow` |
| Extend selection to edge | `Cmd+Shift+Arrow` | `Ctrl+Shift+Arrow` |
| Select entire row | `Shift+Space` | `Shift+Space` |
| Select entire column | `Ctrl+Space` | `Ctrl+Space` |
| Fill down | `Ctrl+D` | `Ctrl+D` |
| Fill right | `Ctrl+R` | `Ctrl+R` |
| Fill selection with text | `Ctrl+Enter` | `Ctrl+Enter` |
| Hard return in cell | `Option+Enter` | `Alt+Enter` |
| Insert current time | `Ctrl+;` | `Ctrl+;` |
| Insert today's date | `Cmd+;` | `Ctrl+:` |
| Insert rows/columns | `Ctrl+I` | `Ctrl+I` |
| Delete rows/columns | `Ctrl+-` | `Ctrl+-` |
| Scroll focused cell into view | `Ctrl+Backspace` | `Ctrl+Backspace` |
| Jump to cell | `Option+F5` | `Alt+F5` |
| Context menu | `Option+Shift+C` | `Alt+Shift+C` |
| View cell content | `Cmd+Shift+.` | `Ctrl+Shift+.` |
| Toggle checkbox / activate link | `Cmd+Enter` | `Ctrl+Enter` |
| Open dropdown | `Option+Down` | `Alt+Down` |
| Navigate to menubar | `Option+F6` | `Alt+F6` |
| Show all shortcuts | `Cmd+/` | `Ctrl+/` |

---

## Spreadsheet Functions (400+)

Quip supports 400+ spreadsheet functions compatible with Excel and Google Sheets conventions. Functions are organized by category below.

### Math and Trigonometry

| Function | Syntax | Description |
|----------|--------|-------------|
| `ABS` | `ABS(number)` | Absolute value |
| `ACOS` | `ACOS(number)` | Arccosine |
| `ACOSH` | `ACOSH(number)` | Inverse hyperbolic cosine |
| `ACOT` | `ACOT(number)` | Inverse cotangent |
| `ACOTH` | `ACOTH(number)` | Inverse hyperbolic cotangent |
| `ADD` | `ADD(number1, number2, ...)` | Sum of values |
| `ARABIC` | `ARABIC(text)` | Roman numeral to Arabic number |
| `ASIN` | `ASIN(number)` | Arcsine |
| `ASINH` | `ASINH(number)` | Inverse hyperbolic sine |
| `ATAN` | `ATAN(number)` | Arctangent |
| `ATAN2` | `ATAN2(x, y)` | Arctangent from x and y coordinates |
| `ATANH` | `ATANH(number)` | Inverse hyperbolic tangent |
| `BASE` | `BASE(number, radix, [min_length])` | Number to text in specified base |
| `CEILING` | `CEILING(number, significance)` | Round up to nearest multiple |
| `CEILING.MATH` | `CEILING.MATH(number, [significance], [mode])` | Round up to nearest multiple |
| `CEILING.PRECISE` | `CEILING.PRECISE(number, [significance])` | Round up to nearest multiple |
| `COMBIN` | `COMBIN(n, k)` | Combinations without repetition |
| `COMBINA` | `COMBINA(n, k)` | Combinations with repetition |
| `COS` | `COS(number)` | Cosine |
| `COSH` | `COSH(number)` | Hyperbolic cosine |
| `COT` | `COT(number)` | Cotangent |
| `COTH` | `COTH(number)` | Hyperbolic cotangent |
| `CSC` | `CSC(number)` | Cosecant |
| `CSCH` | `CSCH(number)` | Hyperbolic cosecant |
| `DECIMAL` | `DECIMAL(text, radix)` | Text in base to decimal |
| `DEGREES` | `DEGREES(radians)` | Radians to degrees |
| `EVEN` | `EVEN(number)` | Round up to nearest even integer |
| `EXP` | `EXP(number)` | e raised to power |
| `FACT` | `FACT(number)` | Factorial |
| `FACTDOUBLE` | `FACTDOUBLE(number)` | Double factorial |
| `FLOOR` | `FLOOR(number, significance)` | Round down to nearest multiple |
| `FLOOR.MATH` | `FLOOR.MATH(number, [significance], [mode])` | Round down to nearest multiple |
| `FLOOR.PRECISE` | `FLOOR.PRECISE(number, [significance])` | Round down to nearest multiple |
| `GCD` | `GCD(number1, number2, ...)` | Greatest common divisor |
| `INT` | `INT(number)` | Round down to nearest integer |
| `ISO.CEILING` | `ISO.CEILING(number, [significance])` | Round up (ISO standard) |
| `LCM` | `LCM(number1, number2, ...)` | Least common multiple |
| `LN` | `LN(number)` | Natural logarithm |
| `LOG` | `LOG(number, [base])` | Logarithm (default base 10) |
| `LOG10` | `LOG10(number)` | Base-10 logarithm |
| `MOD` | `MOD(number, divisor)` | Remainder after division |
| `MROUND` | `MROUND(number, multiple)` | Round to nearest multiple |
| `MULTINOMIAL` | `MULTINOMIAL(number1, number2, ...)` | Multinomial coefficient |
| `MUNIT` | `MUNIT(dimension)` | Unit matrix |
| `ODD` | `ODD(number)` | Round to nearest odd integer |
| `PI` | `PI()` | Value of pi |
| `POWER` | `POWER(number, power)` | Number raised to power |
| `PRODUCT` | `PRODUCT(number1, ...)` | Multiply all arguments |
| `QUOTIENT` | `QUOTIENT(numerator, denominator)` | Integer portion of division |
| `RADIANS` | `RADIANS(degrees)` | Degrees to radians |
| `RAND` | `RAND()` | Random number 0 to 1 |
| `RANDBETWEEN` | `RANDBETWEEN(bottom, top)` | Random integer in range |
| `ROMAN` | `ROMAN(number, [form])` | Arabic to Roman numeral |
| `ROUND` | `ROUND(number, num_digits)` | Round to specified digits |
| `ROUNDDOWN` | `ROUNDDOWN(number, num_digits)` | Round down toward zero |
| `ROUNDUP` | `ROUNDUP(number, num_digits)` | Round up away from zero |
| `SEC` | `SEC(number)` | Secant |
| `SECH` | `SECH(number)` | Hyperbolic secant |
| `SERIESSUM` | `SERIESSUM(x, n, m, coefficients)` | Sum of power series |
| `SIGN` | `SIGN(number)` | Sign of number (1, 0, -1) |
| `SIN` | `SIN(number)` | Sine |
| `SINH` | `SINH(number)` | Hyperbolic sine |
| `SQRT` | `SQRT(number)` | Square root |
| `SQRTPI` | `SQRTPI(number)` | Square root of (number * pi) |
| `SUBTOTAL` | `SUBTOTAL(function_num, ref1, ...)` | Subtotal with specified aggregation (1-11 or 101-111) |
| `SUM` | `SUM(number1, ...)` | Sum of all arguments |
| `SUMIF` | `SUMIF(range, criteria, [sum_range])` | Conditional sum |
| `SUMIFS` | `SUMIFS(sum_range, criteria_range1, criteria1, ...)` | Multi-condition sum |
| `SUMPRODUCT` | `SUMPRODUCT(array1, ...)` | Sum of products of corresponding elements |
| `SUMSQ` | `SUMSQ(number1, ...)` | Sum of squares |
| `SUMX2MY2` | `SUMX2MY2(array_x, array_y)` | Sum of difference of squares |
| `SUMX2PY2` | `SUMX2PY2(array_x, array_y)` | Sum of sum of squares |
| `SUMXMY2` | `SUMXMY2(array_x, array_y)` | Sum of squares of differences |
| `TAN` | `TAN(number)` | Tangent |
| `TANH` | `TANH(number)` | Hyperbolic tangent |
| `TRUNC` | `TRUNC(number, [num_digits])` | Truncate to integer |

### Statistical

| Function | Syntax | Description |
|----------|--------|-------------|
| `AVEDEV` | `AVEDEV(number1, ...)` | Average of absolute deviations from mean |
| `AVERAGE` | `AVERAGE(number1, ...)` | Arithmetic mean |
| `AVERAGEA` | `AVERAGEA(value1, ...)` | Average including text and logical values |
| `AVERAGEIF` | `AVERAGEIF(range, criteria, [avg_range])` | Conditional average |
| `AVERAGEIFS` | `AVERAGEIFS(avg_range, criteria_range1, criteria1, ...)` | Multi-condition average |
| `BETA.DIST` | `BETA.DIST(x, alpha, beta, cumulative, [A], [B])` | Beta distribution |
| `BETA.INV` | `BETA.INV(probability, alpha, beta, [A], [B])` | Inverse beta distribution |
| `BETADIST` | `BETADIST(x, alpha, beta, [A], [B])` | Beta distribution (compatibility) |
| `BETAINV` | `BETAINV(probability, alpha, beta, [A], [B])` | Inverse beta (compatibility) |
| `BINOM.DIST` | `BINOM.DIST(number_s, trials, probability, cumulative)` | Binomial distribution |
| `BINOM.DIST.RANGE` | `BINOM.DIST.RANGE(trials, probability, number_s, [number_s2])` | Binomial distribution range probability |
| `BINOM.INV` | `BINOM.INV(trials, probability, alpha)` | Inverse binomial distribution |
| `BINOMDIST` | `BINOMDIST(number_s, trials, probability, cumulative)` | Binomial distribution (compatibility) |
| `CHIDIST` | `CHIDIST(x, deg_freedom)` | Chi-squared distribution (compatibility) |
| `CHIINV` | `CHIINV(probability, deg_freedom)` | Inverse chi-squared (compatibility) |
| `CHISQ.DIST` | `CHISQ.DIST(x, deg_freedom, cumulative)` | Chi-squared distribution |
| `CHISQ.DIST.RT` | `CHISQ.DIST.RT(x, deg_freedom)` | Right-tailed chi-squared |
| `CHISQ.INV` | `CHISQ.INV(probability, deg_freedom)` | Inverse left-tailed chi-squared |
| `CHISQ.INV.RT` | `CHISQ.INV.RT(probability, deg_freedom)` | Inverse right-tailed chi-squared |
| `CHISQ.TEST` | `CHISQ.TEST(actual_range, expected_range)` | Chi-squared test for independence |
| `CHITEST` | `CHITEST(actual_range, expected_range)` | Chi-squared test (compatibility) |
| `CONFIDENCE` | `CONFIDENCE(alpha, standard_dev, size)` | Confidence interval (compatibility) |
| `CONFIDENCE.NORM` | `CONFIDENCE.NORM(alpha, standard_dev, size)` | Confidence interval (normal) |
| `CONFIDENCE.T` | `CONFIDENCE.T(alpha, standard_dev, size)` | Confidence interval (t-distribution) |
| `CORREL` | `CORREL(array1, array2)` | Correlation coefficient |
| `COUNT` | `COUNT(value1, ...)` | Count numeric values |
| `COUNTA` | `COUNTA(value1, ...)` | Count non-empty values |
| `COUNTBLANK` | `COUNTBLANK(range)` | Count blank cells |
| `COUNTIF` | `COUNTIF(range, criteria)` | Conditional count |
| `COUNTIFS` | `COUNTIFS(criteria_range1, criteria1, ...)` | Multi-condition count |
| `COUNTUNIQUE` | `COUNTUNIQUE(value1, ...)` | Count unique values |
| `COVAR` | `COVAR(array1, array2)` | Covariance (compatibility) |
| `COVARIANCE.P` | `COVARIANCE.P(array1, array2)` | Population covariance |
| `COVARIANCE.S` | `COVARIANCE.S(array1, array2)` | Sample covariance |
| `CRITBINOM` | `CRITBINOM(trials, probability, alpha)` | Inverse binomial (compatibility) |
| `DEVSQ` | `DEVSQ(number1, ...)` | Sum of squares of deviations |
| `EXPON.DIST` | `EXPON.DIST(x, lambda, cumulative)` | Exponential distribution |
| `EXPONDIST` | `EXPONDIST(x, lambda, cumulative)` | Exponential distribution (compatibility) |
| `F.DIST` | `F.DIST(x, deg1, deg2, cumulative)` | F probability distribution |
| `F.DIST.RT` | `F.DIST.RT(x, deg1, deg2)` | Right-tailed F distribution |
| `F.INV` | `F.INV(probability, deg1, deg2)` | Inverse F distribution |
| `F.INV.RT` | `F.INV.RT(probability, deg1, deg2)` | Inverse right-tailed F distribution |
| `F.TEST` | `F.TEST(array1, array2)` | F-test result |
| `FDIST` | `FDIST(x, deg1, deg2)` | F distribution (compatibility) |
| `FINV` | `FINV(probability, deg1, deg2)` | Inverse F (compatibility) |
| `FISHER` | `FISHER(x)` | Fisher transformation |
| `FISHERINV` | `FISHERINV(y)` | Inverse Fisher transformation |
| `FORECAST` | `FORECAST(x, known_ys, known_xs)` | Value along linear trend |
| `FREQUENCY` | `FREQUENCY(data, bins)` | Frequency distribution |
| `GAMMA` | `GAMMA(number)` | Gamma function value |
| `GAMMA.DIST` | `GAMMA.DIST(x, alpha, beta, cumulative)` | Gamma distribution |
| `GAMMA.INV` | `GAMMA.INV(probability, alpha, beta)` | Inverse gamma distribution |
| `GAMMADIST` | `GAMMADIST(x, alpha, beta, cumulative)` | Gamma distribution (compatibility) |
| `GAMMAINV` | `GAMMAINV(probability, alpha, beta)` | Inverse gamma (compatibility) |
| `GAMMALN` | `GAMMALN(x)` | Natural log of gamma function |
| `GAMMALN.PRECISE` | `GAMMALN.PRECISE(x)` | Natural log of gamma (precise) |
| `GAUSS` | `GAUSS(z)` | 0.5 less than standard normal CDF |
| `GEOMEAN` | `GEOMEAN(number1, ...)` | Geometric mean |
| `GROWTH` | `GROWTH(known_ys, [known_xs], [new_xs], [const])` | Values along exponential trend |
| `HARMEAN` | `HARMEAN(number1, ...)` | Harmonic mean |
| `HYPGEOM.DIST` | `HYPGEOM.DIST(sample_s, num_sample, pop_s, num_pop, cumulative)` | Hypergeometric distribution |
| `HYPGEOMDIST` | `HYPGEOMDIST(sample_s, num_sample, pop_s, num_pop)` | Hypergeometric (compatibility) |
| `INTERCEPT` | `INTERCEPT(known_ys, known_xs)` | Linear regression intercept |
| `KURT` | `KURT(number1, ...)` | Kurtosis |
| `LARGE` | `LARGE(array, k)` | K-th largest value |
| `LINEST` | `LINEST(known_ys, [known_xs], [const], [stats])` | Linear trend parameters |
| `LOGEST` | `LOGEST(known_ys, [known_xs], [const], [stats])` | Exponential trend parameters |
| `LOGINV` | `LOGINV(probability, mean, stdev)` | Inverse lognormal (compatibility) |
| `LOGNORM.DIST` | `LOGNORM.DIST(x, mean, stdev, cumulative)` | Lognormal distribution |
| `LOGNORM.INV` | `LOGNORM.INV(probability, mean, stdev)` | Inverse lognormal distribution |
| `LOGNORMDIST` | `LOGNORMDIST(x, mean, stdev)` | Lognormal (compatibility) |
| `MAX` | `MAX(number1, ...)` | Maximum value |
| `MAXA` | `MAXA(value1, ...)` | Maximum including text/logical |
| `MAXIFS` | `MAXIFS(max_range, criteria_range1, criteria1, ...)` | Conditional maximum |
| `MEDIAN` | `MEDIAN(number1, ...)` | Median value |
| `MIN` | `MIN(number1, ...)` | Minimum value |
| `MINA` | `MINA(value1, ...)` | Minimum including text/logical |
| `MINIFS` | `MINIFS(min_range, criteria_range1, criteria1, ...)` | Conditional minimum |
| `MODE` | `MODE(number1, ...)` | Most frequent value (compatibility) |
| `MODE.MULT` | `MODE.MULT(number1, ...)` | Multiple modes |
| `MODE.SNGL` | `MODE.SNGL(number1, ...)` | Single mode |
| `NEGBINOMDIST` | `NEGBINOMDIST(number_f, number_s, probability)` | Negative binomial distribution |
| `NORM.DIST` | `NORM.DIST(x, mean, stdev, cumulative)` | Normal distribution |
| `NORMDIST` | `NORMDIST(x, mean, stdev, cumulative)` | Normal distribution (compatibility) |
| `NORM.INV` | `NORM.INV(probability, mean, stdev)` | Inverse normal distribution |
| `NORMINV` | `NORMINV(probability, mean, stdev)` | Inverse normal (compatibility) |
| `NORM.S.DIST` | `NORM.S.DIST(z, cumulative)` | Standard normal distribution |
| `NORMSDIST` | `NORMSDIST(z)` | Standard normal (compatibility) |
| `NORM.S.INV` | `NORM.S.INV(probability)` | Inverse standard normal |
| `NORMSINV` | `NORMSINV(probability)` | Inverse standard normal (compatibility) |
| `PEARSON` | `PEARSON(array1, array2)` | Pearson correlation coefficient |
| `PERCENTILE` | `PERCENTILE(array, k)` | K-th percentile |
| `PERCENTILE.EXC` | `PERCENTILE.EXC(array, k)` | K-th percentile (exclusive) |
| `PERCENTILE.INC` | `PERCENTILE.INC(array, k)` | K-th percentile (inclusive) |
| `PERCENTRANK` | `PERCENTRANK(array, x, [significance])` | Percentile rank |
| `PERCENTRANK.EXC` | `PERCENTRANK.EXC(array, x, [significance])` | Percentile rank (exclusive) |
| `PERCENTRANK.INC` | `PERCENTRANK.INC(array, x, [significance])` | Percentile rank (inclusive) |
| `PERMUT` | `PERMUT(n, k)` | Permutations |
| `PERMUTATIONA` | `PERMUTATIONA(n, k)` | Permutations with repetition |
| `POISSON` | `POISSON(x, mean, cumulative)` | Poisson distribution (compatibility) |
| `POISSON.DIST` | `POISSON.DIST(x, mean, cumulative)` | Poisson distribution |
| `PROB` | `PROB(x_range, prob_range, lower, [upper])` | Probability between limits |
| `QUARTILE` | `QUARTILE(array, quart)` | Quartile value |
| `QUARTILE.EXC` | `QUARTILE.EXC(array, quart)` | Quartile (exclusive) |
| `QUARTILE.INC` | `QUARTILE.INC(array, quart)` | Quartile (inclusive) |
| `RANK` | `RANK(number, ref, [order])` | Rank in list |
| `RANK.AVG` | `RANK.AVG(number, ref, [order])` | Rank with average for ties |
| `RANK.EQ` | `RANK.EQ(number, ref, [order])` | Rank with top rank for ties |
| `RSQ` | `RSQ(known_ys, known_xs)` | R-squared value |
| `SKEW` | `SKEW(number1, ...)` | Skewness |
| `SKEW.P` | `SKEW.P(number1, ...)` | Population skewness |
| `SLOPE` | `SLOPE(known_ys, known_xs)` | Linear regression slope |
| `SMALL` | `SMALL(array, k)` | K-th smallest value |
| `STANDARDIZE` | `STANDARDIZE(x, mean, stdev)` | Normalized value (z-score) |
| `STDEV` | `STDEV(number1, ...)` | Sample standard deviation |
| `STDEV.P` | `STDEV.P(number1, ...)` | Population standard deviation |
| `STDEV.S` | `STDEV.S(number1, ...)` | Sample standard deviation |
| `STDEVA` | `STDEVA(value1, ...)` | Sample stdev including text/logical |
| `STDEVP` | `STDEVP(number1, ...)` | Population stdev (compatibility) |
| `STDEVPA` | `STDEVPA(value1, ...)` | Population stdev including text/logical |
| `STEYX` | `STEYX(known_ys, known_xs)` | Standard error of predicted y |
| `T.DIST` | `T.DIST(x, deg_freedom, cumulative)` | Student's t-distribution |
| `T.DIST.2T` | `T.DIST.2T(x, deg_freedom)` | Two-tailed t-distribution |
| `T.DIST.RT` | `T.DIST.RT(x, deg_freedom)` | Right-tailed t-distribution |
| `TDIST` | `TDIST(x, deg_freedom, tails)` | t-distribution (compatibility) |
| `T.INV` | `T.INV(probability, deg_freedom)` | Inverse left-tailed t-distribution |
| `T.INV.2T` | `T.INV.2T(probability, deg_freedom)` | Inverse two-tailed t-distribution |
| `TINV` | `TINV(probability, deg_freedom)` | Inverse t (compatibility) |
| `TREND` | `TREND(known_ys, [known_xs], [new_xs], [const])` | Values along linear trend |
| `TRIMMEAN` | `TRIMMEAN(array, percent)` | Mean excluding outliers |
| `T.TEST` | `T.TEST(array1, array2, tails, type)` | Student's t-test |
| `TTEST` | `TTEST(array1, array2, tails, type)` | t-test (compatibility) |
| `VAR` | `VAR(number1, ...)` | Sample variance |
| `VAR.P` | `VAR.P(number1, ...)` | Population variance |
| `VAR.S` | `VAR.S(number1, ...)` | Sample variance |
| `VARA` | `VARA(value1, ...)` | Sample variance including text/logical |
| `VARP` | `VARP(number1, ...)` | Population variance (compatibility) |
| `VARPA` | `VARPA(value1, ...)` | Population variance including text/logical |
| `WEIBULL` | `WEIBULL(x, alpha, beta, cumulative)` | Weibull distribution (compatibility) |
| `WEIBULL.DIST` | `WEIBULL.DIST(x, alpha, beta, cumulative)` | Weibull distribution |
| `Z.TEST` | `Z.TEST(array, x, [sigma])` | One-tailed z-test p-value |
| `ZTEST` | `ZTEST(array, x, [sigma])` | z-test (compatibility) |

### Text

| Function | Syntax | Description |
|----------|--------|-------------|
| `CHAR` | `CHAR(number)` | Character from code number |
| `CLEAN` | `CLEAN(text)` | Remove nonprintable characters |
| `CODE` | `CODE(text)` | Code for first character |
| `CONCAT` | `CONCAT(text1, text2, ...)` | Combine text from multiple values |
| `CONCATENATE` | `CONCATENATE(text1, text2, ...)` | Join text items |
| `DOLLAR` | `DOLLAR(number, [decimals])` | Number to currency text |
| `EXACT` | `EXACT(text1, text2)` | Case-sensitive text comparison |
| `FIND` | `FIND(find_text, within_text, [start])` | Find text position (case-sensitive) |
| `FIXED` | `FIXED(number, [decimals], [no_commas])` | Format number with fixed decimals |
| `LEFT` | `LEFT(text, [num_chars])` | Leftmost characters |
| `LEN` | `LEN(text)` | Character count |
| `LOWER` | `LOWER(text)` | Convert to lowercase |
| `MID` | `MID(text, start, num_chars)` | Characters from middle |
| `PROPER` | `PROPER(text)` | Capitalize first letter of each word |
| `REPLACE` | `REPLACE(old_text, start, num_chars, new_text)` | Replace by position |
| `REPLACEB` | `REPLACEB(old_text, start, num_bytes, new_text)` | Replace by byte position |
| `REPT` | `REPT(text, times)` | Repeat text |
| `RIGHT` | `RIGHT(text, [num_chars])` | Rightmost characters |
| `RIGHTB` | `RIGHTB(text, [num_bytes])` | Rightmost bytes |
| `SEARCH` | `SEARCH(find_text, within_text, [start])` | Find text position (case-insensitive, wildcards) |
| `SEARCHB` | `SEARCHB(find_text, within_text, [start])` | Search by bytes |
| `SUBSTITUTE` | `SUBSTITUTE(text, old_text, new_text, [instance])` | Replace text by matching |
| `T` | `T(value)` | Return text if value is text, else empty |
| `TEXT` | `TEXT(value, format_text)` | Format number as text |
| `TEXTJOIN` | `TEXTJOIN(delimiter, ignore_empty, text1, ...)` | Join text with delimiter |
| `TRIM` | `TRIM(text)` | Remove extra spaces |
| `UNICHAR` | `UNICHAR(number)` | Unicode character from code point |
| `UNICODE` | `UNICODE(text)` | Unicode code point of first character |
| `UPPER` | `UPPER(text)` | Convert to uppercase |
| `VALUE` | `VALUE(text)` | Convert text to number |

### Logical

| Function | Syntax | Description |
|----------|--------|-------------|
| `AND` | `AND(logical1, ...)` | TRUE if all arguments TRUE |
| `FALSE` | `FALSE()` | Logical FALSE |
| `IF` | `IF(test, value_if_true, value_if_false)` | Conditional evaluation |
| `IFERROR` | `IFERROR(value, value_if_error)` | Handle errors |
| `IFNA` | `IFNA(value, value_if_na)` | Handle #N/A errors |
| `IFS` | `IFS(condition1, value1, ...)` | Multiple conditions |
| `NOT` | `NOT(logical)` | Reverse logical value |
| `OR` | `OR(logical1, ...)` | TRUE if any argument TRUE |
| `SWITCH` | `SWITCH(expression, value1, result1, ..., [default])` | Match against list of values |
| `TRUE` | `TRUE()` | Logical TRUE |
| `XOR` | `XOR(logical1, ...)` | TRUE if odd number of arguments TRUE |

### Lookup and Reference

| Function | Syntax | Description |
|----------|--------|-------------|
| `ADDRESS` | `ADDRESS(row, col, [abs], [a1], [sheet])` | Cell reference as text |
| `CHOOSE` | `CHOOSE(index, value1, value2, ...)` | Value from list by index |
| `COLUMN` | `COLUMN([reference])` | Column number |
| `COLUMNS` | `COLUMNS(array)` | Number of columns |
| `FORMULATEXT` | `FORMULATEXT(reference)` | Formula as text string |
| `HLOOKUP` | `HLOOKUP(lookup_value, table, row_index, [range_lookup])` | Horizontal lookup |
| `INDEX` | `INDEX(array, row, [col])` | Value by row and column index |
| `INDIRECT` | `INDIRECT(ref_text, [a1])` | Reference from text string |
| `LOOKUP` | `LOOKUP(lookup_value, lookup_vector, [result_vector])` | Look up value in range |
| `MATCH` | `MATCH(lookup_value, lookup_array, [match_type])` | Position of value in array |
| `OFFSET` | `OFFSET(reference, rows, cols, [height], [width])` | Reference offset from starting point |
| `ROW` | `ROW([reference])` | Row number |
| `ROWS` | `ROWS(array)` | Number of rows |
| `VLOOKUP` | `VLOOKUP(lookup_value, table, col_index, [range_lookup])` | Vertical lookup |

### Date and Time

| Function | Syntax | Description |
|----------|--------|-------------|
| `DATE` | `DATE(year, month, day)` | Create date serial number |
| `DATEDIF` | `DATEDIF(start, end, unit)` | Difference between dates ("Y", "M", "D", "MD", "YM", "YD") |
| `DATEVALUE` | `DATEVALUE(date_text)` | Text to date serial number |
| `DAY` | `DAY(serial_number)` | Day of month (1-31) |
| `DAYS` | `DAYS(end_date, start_date)` | Days between dates |
| `DAYS360` | `DAYS360(start, end, [method])` | Days between dates (360-day year) |
| `EDATE` | `EDATE(start, months)` | Date offset by months |
| `EOMONTH` | `EOMONTH(start, months)` | Last day of month offset |
| `HOUR` | `HOUR(serial_number)` | Hour component (0-23) |
| `ISOWEEKNUM` | `ISOWEEKNUM(date)` | ISO week number |
| `MINUTE` | `MINUTE(serial_number)` | Minute component (0-59) |
| `MONTH` | `MONTH(serial_number)` | Month (1-12) |
| `NETWORKDAYS` | `NETWORKDAYS(start, end, [holidays])` | Working days between dates |
| `NETWORKDAYS.INTL` | `NETWORKDAYS.INTL(start, end, [weekend], [holidays])` | Working days with custom weekends |
| `NOW` | `NOW()` | Current date and time |
| `SECOND` | `SECOND(serial_number)` | Seconds component (0-59) |
| `TIME` | `TIME(hour, minute, second)` | Create time serial number |
| `TIMEVALUE` | `TIMEVALUE(time_text)` | Text to time serial number |
| `TODAY` | `TODAY()` | Current date |
| `WEEKDAY` | `WEEKDAY(serial_number, [return_type])` | Day of week (1-7) |
| `WEEKNUM` | `WEEKNUM(serial_number, [return_type])` | Week number |
| `WORKDAY` | `WORKDAY(start, days, [holidays])` | Date offset by working days |
| `WORKDAY.INTL` | `WORKDAY.INTL(start, days, [weekend], [holidays])` | Working day offset with custom weekends |
| `YEAR` | `YEAR(serial_number)` | Year from date |
| `YEARFRAC` | `YEARFRAC(start, end, [basis])` | Fraction of year between dates |

### Financial

| Function | Syntax | Description |
|----------|--------|-------------|
| `ACCRINT` | `ACCRINT(issue, first_interest, settlement, rate, par, frequency, [basis])` | Accrued interest for periodic-interest security |
| `CUMIPMT` | `CUMIPMT(rate, nper, pv, start, end, type)` | Cumulative interest between periods |
| `CUMPRINC` | `CUMPRINC(rate, nper, pv, start, end, type)` | Cumulative principal between periods |
| `DB` | `DB(cost, salvage, life, period, [month])` | Fixed-declining balance depreciation |
| `DDB` | `DDB(cost, salvage, life, period, [factor])` | Double-declining balance depreciation |
| `DISC` | `DISC(settlement, maturity, pr, redemption, [basis])` | Discount rate |
| `DOLLARDE` | `DOLLARDE(fractional, fraction)` | Fractional dollar to decimal |
| `DOLLARFR` | `DOLLARFR(decimal, fraction)` | Decimal dollar to fractional |
| `EFFECT` | `EFFECT(nominal_rate, npery)` | Effective annual interest rate |
| `FV` | `FV(rate, nper, pmt, [pv], [type])` | Future value |
| `FVSCHEDULE` | `FVSCHEDULE(principal, schedule)` | Future value with variable rates |
| `INTRATE` | `INTRATE(settlement, maturity, investment, redemption, [basis])` | Interest rate for fully invested security |
| `IPMT` | `IPMT(rate, per, nper, pv, [fv], [type])` | Interest payment for period |
| `IRR` | `IRR(values, [guess])` | Internal rate of return |
| `ISPMT` | `ISPMT(rate, per, nper, pv)` | Interest paid during period |
| `MDURATION` | `MDURATION(settlement, maturity, coupon, yld, frequency, [basis])` | Modified Macauley duration |
| `MIRR` | `MIRR(values, finance_rate, reinvest_rate)` | Modified internal rate of return |
| `NOMINAL` | `NOMINAL(effect_rate, npery)` | Nominal annual interest rate |
| `NPER` | `NPER(rate, pmt, pv, [fv], [type])` | Number of periods |
| `NPV` | `NPV(rate, value1, ...)` | Net present value |
| `PMT` | `PMT(rate, nper, pv, [fv], [type])` | Loan payment |
| `PPMT` | `PPMT(rate, per, nper, pv, [fv], [type])` | Principal payment for period |
| `PRICE` | `PRICE(settlement, maturity, rate, yld, redemption, frequency, [basis])` | Security price |
| `PRICEDISC` | `PRICEDISC(settlement, maturity, discount, redemption, [basis])` | Discounted security price |
| `PRICEMAT` | `PRICEMAT(settlement, maturity, issue, rate, yld, [basis])` | Security price (interest at maturity) |
| `PV` | `PV(rate, nper, pmt, [fv], [type])` | Present value |
| `RATE` | `RATE(nper, pmt, pv, [fv], [type], [guess])` | Interest rate per period |
| `RECEIVED` | `RECEIVED(settlement, maturity, investment, discount, [basis])` | Amount received at maturity |
| `SLN` | `SLN(cost, salvage, life)` | Straight-line depreciation |
| `SYD` | `SYD(cost, salvage, life, per)` | Sum-of-years-digits depreciation |
| `TBILLEQ` | `TBILLEQ(settlement, maturity, discount)` | Treasury bill bond-equivalent yield |
| `TBILLPRICE` | `TBILLPRICE(settlement, maturity, discount)` | Treasury bill price |
| `TBILLYIELD` | `TBILLYIELD(settlement, maturity, pr)` | Treasury bill yield |
| `VDB` | `VDB(cost, salvage, life, start, end, [factor], [no_switch])` | Variable declining balance depreciation |
| `XIRR` | `XIRR(values, dates, [guess])` | IRR for irregular cash flows |
| `XNPV` | `XNPV(rate, values, dates)` | NPV for irregular cash flows |
| `YIELD` | `YIELD(settlement, maturity, rate, pr, redemption, frequency, [basis])` | Security yield |
| `YIELDDISC` | `YIELDDISC(settlement, maturity, pr, redemption, [basis])` | Discounted security yield |
| `YIELDMAT` | `YIELDMAT(settlement, maturity, issue, rate, pr, [basis])` | Security yield (interest at maturity) |

### Engineering

| Function | Syntax | Description |
|----------|--------|-------------|
| `BESSELI` | `BESSELI(x, n)` | Modified Bessel function In(x) |
| `BESSELJ` | `BESSELJ(x, n)` | Bessel function Jn(x) |
| `BESSELK` | `BESSELK(x, n)` | Modified Bessel function Kn(x) |
| `BESSELY` | `BESSELY(x, n)` | Bessel function Yn(x) |
| `BIN2DEC` | `BIN2DEC(number)` | Binary to decimal |
| `BIN2HEX` | `BIN2HEX(number, [places])` | Binary to hexadecimal |
| `BIN2OCT` | `BIN2OCT(number, [places])` | Binary to octal |
| `BITAND` | `BITAND(number1, number2)` | Bitwise AND |
| `BITLSHIFT` | `BITLSHIFT(number, shift)` | Bitwise left shift |
| `BITOR` | `BITOR(number1, number2)` | Bitwise OR |
| `BITRSHIFT` | `BITRSHIFT(number, shift)` | Bitwise right shift |
| `BITXOR` | `BITXOR(number1, number2)` | Bitwise XOR |
| `COMPLEX` | `COMPLEX(real, imaginary, [suffix])` | Create complex number |
| `CONVERT` | `CONVERT(number, from_unit, to_unit)` | Unit conversion |
| `DEC2BIN` | `DEC2BIN(number, [places])` | Decimal to binary |
| `DEC2HEX` | `DEC2HEX(number, [places])` | Decimal to hexadecimal |
| `DEC2OCT` | `DEC2OCT(number, [places])` | Decimal to octal |
| `DELTA` | `DELTA(number1, [number2])` | Test equality (returns 1 or 0) |
| `ERF` | `ERF(lower, [upper])` | Error function |
| `ERFC` | `ERFC(x)` | Complementary error function |
| `GESTEP` | `GESTEP(number, [step])` | Test >= step (returns 1 or 0) |
| `HEX2BIN` | `HEX2BIN(number, [places])` | Hexadecimal to binary |
| `HEX2DEC` | `HEX2DEC(number)` | Hexadecimal to decimal |
| `HEX2OCT` | `HEX2OCT(number, [places])` | Hexadecimal to octal |
| `IMABS` | `IMABS(inumber)` | Complex number absolute value |
| `IMAGINARY` | `IMAGINARY(inumber)` | Imaginary coefficient |
| `IMARGUMENT` | `IMARGUMENT(inumber)` | Argument (angle) of complex number |
| `IMCONJUGATE` | `IMCONJUGATE(inumber)` | Complex conjugate |
| `IMCOS` | `IMCOS(inumber)` | Complex cosine |
| `IMCOSH` | `IMCOSH(inumber)` | Complex hyperbolic cosine |
| `IMCOT` | `IMCOT(inumber)` | Complex cotangent |
| `IMCSC` | `IMCSC(inumber)` | Complex cosecant |
| `IMCSCH` | `IMCSCH(inumber)` | Complex hyperbolic cosecant |
| `IMDIV` | `IMDIV(inumber1, inumber2)` | Complex division |
| `IMEXP` | `IMEXP(inumber)` | Complex exponential |
| `IMLN` | `IMLN(inumber)` | Complex natural log |
| `IMLOG10` | `IMLOG10(inumber)` | Complex base-10 log |
| `IMLOG2` | `IMLOG2(inumber)` | Complex base-2 log |
| `IMPOWER` | `IMPOWER(inumber, power)` | Complex number raised to power |
| `IMPRODUCT` | `IMPRODUCT(inumber1, ...)` | Complex product |
| `IMREAL` | `IMREAL(inumber)` | Real coefficient |
| `IMSEC` | `IMSEC(inumber)` | Complex secant |
| `IMSECH` | `IMSECH(inumber)` | Complex hyperbolic secant |
| `IMSIN` | `IMSIN(inumber)` | Complex sine |
| `IMSINH` | `IMSINH(inumber)` | Complex hyperbolic sine |
| `IMSQRT` | `IMSQRT(inumber)` | Complex square root |
| `IMSUB` | `IMSUB(inumber1, inumber2)` | Complex subtraction |
| `IMSUM` | `IMSUM(inumber1, ...)` | Complex sum |
| `IMTAN` | `IMTAN(inumber)` | Complex tangent |
| `OCT2BIN` | `OCT2BIN(number, [places])` | Octal to binary |
| `OCT2DEC` | `OCT2DEC(number)` | Octal to decimal |
| `OCT2HEX` | `OCT2HEX(number, [places])` | Octal to hexadecimal |

### Information

| Function | Syntax | Description |
|----------|--------|-------------|
| `ERROR.TYPE` | `ERROR.TYPE(error_val)` | Number for error type |
| `ISBLANK` | `ISBLANK(value)` | TRUE if blank |
| `ISERR` | `ISERR(value)` | TRUE if error (except #N/A) |
| `ISERROR` | `ISERROR(value)` | TRUE if any error |
| `ISEVEN` | `ISEVEN(number)` | TRUE if even |
| `ISFORMULA` | `ISFORMULA(reference)` | TRUE if formula |
| `ISLOGICAL` | `ISLOGICAL(value)` | TRUE if logical |
| `ISNA` | `ISNA(value)` | TRUE if #N/A |
| `ISNONTEXT` | `ISNONTEXT(value)` | TRUE if not text |
| `ISNUMBER` | `ISNUMBER(value)` | TRUE if number |
| `ISODD` | `ISODD(number)` | TRUE if odd |
| `ISREF` | `ISREF(value)` | TRUE if reference |
| `ISTEXT` | `ISTEXT(value)` | TRUE if text |
| `N` | `N(value)` | Convert value to number |
| `NA` | `NA()` | Return #N/A error |
| `TYPE` | `TYPE(value)` | Type of value (1=number, 2=text, 4=logical, 16=error, 64=array) |

### Database

| Function | Syntax | Description |
|----------|--------|-------------|
| `DAVERAGE` | `DAVERAGE(database, field, criteria)` | Average of matching entries |
| `DCOUNT` | `DCOUNT(database, field, criteria)` | Count numeric matching entries |
| `DCOUNTA` | `DCOUNTA(database, field, criteria)` | Count nonblank matching entries |
| `DGET` | `DGET(database, field, criteria)` | Single value from matching entry |
| `DMAX` | `DMAX(database, field, criteria)` | Maximum of matching entries |
| `DMIN` | `DMIN(database, field, criteria)` | Minimum of matching entries |
| `DPRODUCT` | `DPRODUCT(database, field, criteria)` | Product of matching entries |
| `DSTDEV` | `DSTDEV(database, field, criteria)` | Sample standard deviation of matches |
| `DSTDEVP` | `DSTDEVP(database, field, criteria)` | Population standard deviation of matches |
| `DSUM` | `DSUM(database, field, criteria)` | Sum of matching entries |
| `DVAR` | `DVAR(database, field, criteria)` | Sample variance of matches |
| `DVARP` | `DVARP(database, field, criteria)` | Population variance of matches |

### Array and Dynamic

| Function | Syntax | Description |
|----------|--------|-------------|
| `ARRAYFORMULA` | `ARRAYFORMULA(formula)` | Enable array arithmetic for formula |
| `ARRAY_CONSTRAIN` | `ARRAY_CONSTRAIN(array, height, width)` | Truncate array to dimensions |
| `FILTER` | `FILTER(source, bool_array1, ...)` | Filter array by conditions |
| `SORT` | `SORT(range, [sort_index], [sort_order], [by_col])` | Return sorted array |
| `TRANSPOSE` | `TRANSPOSE(array)` | Swap rows and columns |
| `UNIQUE` | `UNIQUE(range, [by_col], [exactly_once])` | Return unique values |

### Matrix

| Function | Syntax | Description |
|----------|--------|-------------|
| `MDETERM` | `MDETERM(array)` | Matrix determinant |
| `MINVERSE` | `MINVERSE(array)` | Matrix inverse |
| `MMULT` | `MMULT(array1, array2)` | Matrix product |
| `MUNIT` | `MUNIT(dimension)` | Unit matrix |

### Quip-Specific

| Function | Syntax | Description |
|----------|--------|-------------|
| `REFERENCERANGE` | `REFERENCERANGE(source_range)` | Live-synced reference to range in another spreadsheet |
| `REFERENCESHEET` | `REFERENCESHEET(source_sheet)` | Live-synced reference to entire sheet in another spreadsheet |

---

## Quip-Specific Differences from Excel/Google Sheets

1. **Embedded model** -- Spreadsheets live inside documents, not as standalone files
2. **Data Referencing** -- `REFERENCERANGE` and `REFERENCESHEET` for live cross-document links (no file-path references)
3. **Inline cell references** -- Type `=cellname` in document text for live-updating values
4. **@-mentions in cells** -- Mention people/documents in cells, triggering notifications
5. **No VBA/Apps Script** -- Automation via Quip API or Salesforce Flow only
6. **Simplified conditional formatting** -- Basic rules only (no color scales, data bars, icon sets)
7. **No drag-to-fill** -- Use `Ctrl+D`/`Ctrl+R` instead
8. **Fixed typeface** -- Cannot change font in spreadsheets
9. **Circular reference handling** -- Refined detection that ignores non-circular reference chains through VLOOKUP, HLOOKUP, IF, IFERROR, IFNA, SUMIF, SUMIFS, COUNTIF, COUNTIFS, AVERAGEIF, AVERAGEIFS
10. **Live Paste** -- Copy-paste between documents can maintain live links
11. **PDF export limit** -- Maximum 40,000 cells; charts excluded from PDF
