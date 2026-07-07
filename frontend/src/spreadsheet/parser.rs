// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Formula tokenizer and parser.
//!
//! Parses spreadsheet formulas (e.g., `=SUM(A1:B5) + 10`) into an AST.
//! Operator precedence (highest to lowest):
//!   1. Unary negation (-)
//!   2. Percentage (%)
//!   3. Exponentiation (^)
//!   4. Multiplication / Division (* /)
//!   5. Addition / Subtraction (+ -)
//!   6. Concatenation (&)
//!   7. Comparison (= <> < > <= >=)

use std::fmt;

// ─── Cell References ───────────────────────────────────────────

/// A cell reference like A1, $A$1, B$3, etc.
#[derive(Debug, Clone, PartialEq)]
pub struct CellRef {
    pub col: usize,    // 0-based column index
    pub row: usize,    // 0-based row index
    pub abs_col: bool, // $ prefix on column
    pub abs_row: bool, // $ prefix on row
}

impl CellRef {
    pub fn new(col: usize, row: usize) -> Self {
        Self { col, row, abs_col: false, abs_row: false }
    }

    pub fn label(&self) -> String {
        let col_str = col_to_letters(self.col);
        format!(
            "{}{}{}{}",
            if self.abs_col { "$" } else { "" },
            col_str,
            if self.abs_row { "$" } else { "" },
            self.row + 1,
        )
    }
}

/// A range reference like A1:B5.
#[derive(Debug, Clone, PartialEq)]
pub struct RangeRef {
    pub start: CellRef,
    pub end: CellRef,
}

/// Convert 0-based column index to letters: 0→A, 25→Z, 26→AA.
pub fn col_to_letters(mut col: usize) -> String {
    let mut result = String::new();
    loop {
        result.insert(0, (b'A' + (col % 26) as u8) as char);
        if col < 26 { break; }
        col = col / 26 - 1;
    }
    result
}

/// Parse column letters to 0-based index: A→0, Z→25, AA→26.
pub fn letters_to_col(s: &str) -> usize {
    let mut result = 0usize;
    for b in s.bytes() {
        result = result * 26 + (b - b'A') as usize + 1;
    }
    result - 1
}

// ─── AST ───────────────────────────────────────────────────────

/// Binary operators.
#[derive(Debug, Clone, PartialEq)]
pub enum BinOp {
    Add, Sub, Mul, Div, Pow, Concat,
    Eq, Neq, Lt, Gt, Lte, Gte,
}

/// Spreadsheet formula expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Number(f64),
    Text(String),
    Bool(bool),
    Error(SpreadsheetError),
    CellRef(CellRef),
    Range(RangeRef),
    /// Sheet-qualified cell reference (`Sheet2!B2`). The sheet name
    /// is the unquoted identifier preceding `!`; the cell reference
    /// is resolved against that sheet's snapshot at evaluation time
    /// (see `SpreadsheetEngine::set_local_sheets_snapshot`).
    /// Unknown sheet names yield `#REF!`.
    SheetCellRef { sheet: String, cell: CellRef },
    /// Sheet-qualified range (`Sheet2!A1:B5`). Used as a function
    /// argument like `SUM(Sheet2!A1:B5)`.
    SheetRange { sheet: String, range: RangeRef },
    BinOp { op: BinOp, left: Box<Expr>, right: Box<Expr> },
    UnaryNeg(Box<Expr>),
    Percent(Box<Expr>),
    FuncCall { name: String, args: Vec<Expr> },
    /// Named-range reference. The string is the user-defined alias.
    /// Resolved at evaluation time against the engine's named_ranges
    /// map (case-insensitive). An unresolved name evaluates to
    /// `#NAME?`. Names whose target is a multi-cell range cannot
    /// stand alone — same as `Expr::Range`, they're only useful as
    /// function arguments.
    Name(String),
}

/// Spreadsheet error values.
#[derive(Debug, Clone, PartialEq)]
pub enum SpreadsheetError {
    Ref,     // #REF!
    Value,   // #VALUE!
    Div0,    // #DIV/0!
    Na,      // #N/A
    Name,    // #NAME?
    Num,     // #NUM!
    Null,    // #NULL!
    Circular,// #CIRCULAR!
    /// `#SPILL!` — a dynamic-array formula's spill region overlaps a
    /// non-empty cell or another spill block.
    Spill,
    /// `#LOADING!` — a transient placeholder for formulas waiting on
    /// async data (currently: cross-document REFERENCERANGE /
    /// REFERENCESHEET while the foreign-doc fetch is in flight). Not
    /// an error in the user-facing sense; the cell will recompute
    /// when the data arrives. XLSX export translates this to `#N/A`
    /// since Excel has no equivalent.
    Loading,
}

impl fmt::Display for SpreadsheetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpreadsheetError::Ref => write!(f, "#REF!"),
            SpreadsheetError::Value => write!(f, "#VALUE!"),
            SpreadsheetError::Div0 => write!(f, "#DIV/0!"),
            SpreadsheetError::Na => write!(f, "#N/A"),
            SpreadsheetError::Name => write!(f, "#NAME?"),
            SpreadsheetError::Num => write!(f, "#NUM!"),
            SpreadsheetError::Null => write!(f, "#NULL!"),
            SpreadsheetError::Circular => write!(f, "#CIRCULAR!"),
            SpreadsheetError::Spill => write!(f, "#SPILL!"),
            SpreadsheetError::Loading => write!(f, "#LOADING!"),
        }
    }
}

// ─── Tokenizer ─────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    String(String),
    Bool(bool),
    CellRef(CellRef),
    Ident(String),   // function name
    Plus, Minus, Star, Slash, Caret, Percent, Ampersand,
    Eq, Neq, Lt, Gt, Lte, Gte,
    LParen, RParen, Comma, Colon, Bang,
    Error(SpreadsheetError),
}

fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        let ch = chars[i];

        // Whitespace
        if ch.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        // String literal
        if ch == '"' {
            i += 1;
            let mut s = String::new();
            while i < len && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < len {
                    i += 1;
                    s.push(chars[i]);
                } else {
                    s.push(chars[i]);
                }
                i += 1;
            }
            if i < len { i += 1; } // skip closing "
            tokens.push(Token::String(s));
            continue;
        }

        // Number literal
        if ch.is_ascii_digit() || (ch == '.' && i + 1 < len && chars[i + 1].is_ascii_digit()) {
            let start = i;
            while i < len && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            // Scientific notation
            if i < len && (chars[i] == 'e' || chars[i] == 'E') {
                i += 1;
                if i < len && (chars[i] == '+' || chars[i] == '-') { i += 1; }
                while i < len && chars[i].is_ascii_digit() { i += 1; }
            }
            let num_str: String = chars[start..i].iter().collect();
            let num: f64 = num_str.parse().map_err(|_| format!("Invalid number: {num_str}"))?;
            tokens.push(Token::Number(num));
            continue;
        }

        // Error literals (#REF!, etc.)
        if ch == '#' {
            let start = i;
            i += 1;
            while i < len && (chars[i].is_ascii_alphabetic() || chars[i] == '/' || chars[i] == '!' || chars[i] == '?') {
                i += 1;
            }
            let err_str: String = chars[start..i].iter().collect();
            let err = match err_str.to_uppercase().as_str() {
                "#REF!" => SpreadsheetError::Ref,
                "#VALUE!" => SpreadsheetError::Value,
                "#DIV/0!" => SpreadsheetError::Div0,
                "#N/A" => SpreadsheetError::Na,
                "#NAME?" => SpreadsheetError::Name,
                "#NUM!" => SpreadsheetError::Num,
                "#NULL!" => SpreadsheetError::Null,
                "#SPILL!" => SpreadsheetError::Spill,
                "#CIRCULAR!" => SpreadsheetError::Circular,
                "#LOADING!" => SpreadsheetError::Loading,
                _ => return Err(format!("Unknown error: {err_str}")),
            };
            tokens.push(Token::Error(err));
            continue;
        }

        // Cell reference or identifier (function name / TRUE / FALSE)
        // Cell ref pattern: optional $ + letters + optional $ + digits
        if ch == '$' || ch.is_ascii_alphabetic() {
            let start = i;
            let mut abs_col = false;
            let mut abs_row = false;

            if chars[i] == '$' {
                abs_col = true;
                i += 1;
            }
            let col_start = i;
            while i < len && chars[i].is_ascii_alphabetic() {
                i += 1;
            }
            let letters: String = chars[col_start..i].iter().collect();

            if i < len && chars[i] == '$' {
                abs_row = true;
                i += 1;
            }

            let row_start = i;
            while i < len && chars[i].is_ascii_digit() {
                i += 1;
            }

            if row_start < i && !letters.is_empty() {
                // Distinguish a cell reference from a function name with a
                // trailing digit. Two heuristics, in order:
                //
                //   (a) more alphabetic chars follow → it's an ident like
                //       BIN2DEC or HEX2BIN.
                //   (b) the next char is '(' → it's a function call like
                //       SUMXMY2(…) or LOG10(…). Cell references are
                //       *never* immediately followed by '(' in spreadsheet
                //       grammar, so this is unambiguous.
                let more_alpha = i < len && chars[i].is_ascii_alphabetic();
                let function_call = i < len && chars[i] == '(';
                if !more_alpha && !function_call {
                    // Genuine cell reference: letters + digits with no trailing alpha or '('.
                    let row_str: String = chars[row_start..i].iter().collect();
                    let row: usize = row_str.parse::<usize>().unwrap_or(1).saturating_sub(1);
                    let col = letters_to_col(&letters.to_uppercase());
                    tokens.push(Token::CellRef(CellRef { col, row, abs_col, abs_row }));
                } else {
                    // Function name with digits (e.g., BIN2DEC) — rescan as identifier
                    i = start;
                    while i < len && (chars[i].is_ascii_alphanumeric() || chars[i] == '_' || chars[i] == '.') {
                        i += 1;
                    }
                    let ident: String = chars[start..i].iter().collect();
                    let upper = ident.to_uppercase();
                    match upper.as_str() {
                        "TRUE" => tokens.push(Token::Bool(true)),
                        "FALSE" => tokens.push(Token::Bool(false)),
                        _ => tokens.push(Token::Ident(upper)),
                    }
                }
            } else if !letters.is_empty() {
                // It's an identifier (function name, TRUE, FALSE)
                // Reset abs_col since $ wasn't part of a cell ref
                i = start;
                while i < len && (chars[i].is_ascii_alphanumeric() || chars[i] == '_' || chars[i] == '.') {
                    i += 1;
                }
                let ident: String = chars[start..i].iter().collect();
                let upper = ident.to_uppercase();
                match upper.as_str() {
                    "TRUE" => tokens.push(Token::Bool(true)),
                    "FALSE" => tokens.push(Token::Bool(false)),
                    _ => tokens.push(Token::Ident(upper)),
                }
            } else {
                return Err(format!("Unexpected character at position {start}"));
            }
            continue;
        }

        // Operators and punctuation
        match ch {
            '+' => { tokens.push(Token::Plus); i += 1; }
            '-' => { tokens.push(Token::Minus); i += 1; }
            '*' => { tokens.push(Token::Star); i += 1; }
            '/' => { tokens.push(Token::Slash); i += 1; }
            '^' => { tokens.push(Token::Caret); i += 1; }
            '%' => { tokens.push(Token::Percent); i += 1; }
            '&' => { tokens.push(Token::Ampersand); i += 1; }
            '(' => { tokens.push(Token::LParen); i += 1; }
            ')' => { tokens.push(Token::RParen); i += 1; }
            ',' => { tokens.push(Token::Comma); i += 1; }
            ':' => { tokens.push(Token::Colon); i += 1; }
            '!' => { tokens.push(Token::Bang); i += 1; }
            '=' => { tokens.push(Token::Eq); i += 1; }
            '<' => {
                if i + 1 < len && chars[i + 1] == '>' {
                    tokens.push(Token::Neq); i += 2;
                } else if i + 1 < len && chars[i + 1] == '=' {
                    tokens.push(Token::Lte); i += 2;
                } else {
                    tokens.push(Token::Lt); i += 1;
                }
            }
            '>' => {
                if i + 1 < len && chars[i + 1] == '=' {
                    tokens.push(Token::Gte); i += 2;
                } else {
                    tokens.push(Token::Gt); i += 1;
                }
            }
            _ => return Err(format!("Unexpected character: '{ch}'")),
        }
    }

    Ok(tokens)
}

// ─── Parser (recursive descent) ────────────────────────────────

/// Hard cap on the recursive-descent parser's nesting depth.
/// `parse_expr` increments `Parser::depth` on entry and bails
/// before recursing past this limit. Excel's own depth limit is
/// 64 in some operation contexts and ~7 in others; 128 here is
/// comfortably above anything a human writes and well below the
/// WASM default stack frame budget.
const MAX_PARSE_DEPTH: usize = 128;

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    /// Depth counter for `parse_expr` — bumped on each recursive
    /// call to enforce `MAX_PARSE_DEPTH`.
    depth: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0, depth: 0 }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn advance(&mut self) -> Option<Token> {
        let tok = self.tokens.get(self.pos)?.clone();
        self.pos += 1;
        Some(tok)
    }

    fn expect(&mut self, expected: &Token) -> Result<(), String> {
        match self.advance() {
            Some(ref t) if t == expected => Ok(()),
            Some(t) => Err(format!("Expected {expected:?}, got {t:?}")),
            None => Err(format!("Expected {expected:?}, got end of input")),
        }
    }

    /// Parse a complete expression. Bounds recursion via
    /// `self.depth` so a pathological formula (e.g.
    /// `((((((...))))))` thousands of levels deep, or
    /// `SUM(SUM(SUM(...)))`) is rejected with a depth-exceeded
    /// error before it can overflow the WASM stack.
    fn parse_expr(&mut self) -> Result<Expr, String> {
        if self.depth >= MAX_PARSE_DEPTH {
            return Err("Expression too deeply nested".into());
        }
        self.depth += 1;
        let result = self.parse_comparison();
        self.depth -= 1;
        result
    }

    /// Level 7: comparison (= <> < > <= >=)
    fn parse_comparison(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_concat()?;
        loop {
            let op = match self.peek() {
                Some(Token::Eq) => BinOp::Eq,
                Some(Token::Neq) => BinOp::Neq,
                Some(Token::Lt) => BinOp::Lt,
                Some(Token::Gt) => BinOp::Gt,
                Some(Token::Lte) => BinOp::Lte,
                Some(Token::Gte) => BinOp::Gte,
                _ => break,
            };
            self.advance();
            let right = self.parse_concat()?;
            left = Expr::BinOp { op, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    /// Level 6: concatenation (&)
    fn parse_concat(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_add()?;
        while matches!(self.peek(), Some(Token::Ampersand)) {
            self.advance();
            let right = self.parse_add()?;
            left = Expr::BinOp { op: BinOp::Concat, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    /// Level 5: addition / subtraction (+ -)
    fn parse_add(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_mul()?;
        loop {
            let op = match self.peek() {
                Some(Token::Plus) => BinOp::Add,
                Some(Token::Minus) => BinOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.parse_mul()?;
            left = Expr::BinOp { op, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    /// Level 4: multiplication / division (* /)
    fn parse_mul(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_pow()?;
        loop {
            let op = match self.peek() {
                Some(Token::Star) => BinOp::Mul,
                Some(Token::Slash) => BinOp::Div,
                _ => break,
            };
            self.advance();
            let right = self.parse_pow()?;
            left = Expr::BinOp { op, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    /// Level 3: exponentiation (^) — right-associative
    fn parse_pow(&mut self) -> Result<Expr, String> {
        let base = self.parse_percent()?;
        if matches!(self.peek(), Some(Token::Caret)) {
            self.advance();
            let exp = self.parse_pow()?; // right-associative
            Ok(Expr::BinOp { op: BinOp::Pow, left: Box::new(base), right: Box::new(exp) })
        } else {
            Ok(base)
        }
    }

    /// Level 2: percentage (postfix %)
    fn parse_percent(&mut self) -> Result<Expr, String> {
        let mut expr = self.parse_unary()?;
        while matches!(self.peek(), Some(Token::Percent)) {
            self.advance();
            expr = Expr::Percent(Box::new(expr));
        }
        Ok(expr)
    }

    /// Level 1: unary negation (-)
    fn parse_unary(&mut self) -> Result<Expr, String> {
        if matches!(self.peek(), Some(Token::Minus)) {
            self.advance();
            let expr = self.parse_unary()?;
            Ok(Expr::UnaryNeg(Box::new(expr)))
        } else if matches!(self.peek(), Some(Token::Plus)) {
            self.advance();
            self.parse_unary() // unary + is a no-op
        } else {
            self.parse_primary()
        }
    }

    /// Primary: numbers, strings, bools, errors, cell refs, ranges, function calls, parens.
    fn parse_primary(&mut self) -> Result<Expr, String> {
        match self.advance() {
            Some(Token::Number(n)) => Ok(Expr::Number(n)),
            Some(Token::String(s)) => Ok(Expr::Text(s)),
            Some(Token::Bool(b)) => Ok(Expr::Bool(b)),
            Some(Token::Error(e)) => Ok(Expr::Error(e)),
            Some(Token::CellRef(cell)) => {
                // A "CellRef" followed by `!` is actually a
                // sheet-qualified reference where the lexer mistook
                // a letters-then-digits sheet name (`Sheet2`,
                // `Q1`, `H1`, ...) for a cell coordinate. Recover
                // by reconstructing the sheet name from the cell's
                // label and forwarding to the sheet-ref path.
                if matches!(self.peek(), Some(Token::Bang)) {
                    let sheet = cell.label();
                    self.advance(); // consume Bang
                    return match self.advance() {
                        Some(Token::CellRef(target)) => {
                            if matches!(self.peek(), Some(Token::Colon)) {
                                self.advance();
                                match self.advance() {
                                    Some(Token::CellRef(end)) => Ok(Expr::SheetRange {
                                        sheet,
                                        range: RangeRef { start: target, end },
                                    }),
                                    _ => Err("Expected cell reference after ':' in sheet range".into()),
                                }
                            } else {
                                Ok(Expr::SheetCellRef { sheet, cell: target })
                            }
                        }
                        _ => Err(format!("Expected cell reference after '{sheet}!'")),
                    };
                }
                // Check for range (A1:B5)
                if matches!(self.peek(), Some(Token::Colon)) {
                    self.advance();
                    match self.advance() {
                        Some(Token::CellRef(end)) => {
                            Ok(Expr::Range(RangeRef { start: cell, end }))
                        }
                        _ => Err("Expected cell reference after ':'".to_string()),
                    }
                } else {
                    Ok(Expr::CellRef(cell))
                }
            }
            Some(Token::Ident(name)) => {
                if matches!(self.peek(), Some(Token::LParen)) {
                    // Function call: NAME(args...)
                    self.advance();
                    let mut args = Vec::new();
                    if !matches!(self.peek(), Some(Token::RParen)) {
                        args.push(self.parse_expr()?);
                        while matches!(self.peek(), Some(Token::Comma)) {
                            self.advance();
                            args.push(self.parse_expr()?);
                        }
                    }
                    self.expect(&Token::RParen)?;
                    Ok(Expr::FuncCall { name, args })
                } else if matches!(self.peek(), Some(Token::Bang)) {
                    // Sheet-qualified cell or range reference:
                    // `SheetName!A1` or `SheetName!A1:B5`. The
                    // identifier upstream is uppercase-folded (see
                    // tokenizer); sheet-name matching at eval time is
                    // case-insensitive to compensate.
                    self.advance(); // consume Bang
                    match self.advance() {
                        Some(Token::CellRef(cell)) => {
                            if matches!(self.peek(), Some(Token::Colon)) {
                                self.advance();
                                match self.advance() {
                                    Some(Token::CellRef(end)) => Ok(Expr::SheetRange {
                                        sheet: name,
                                        range: RangeRef { start: cell, end },
                                    }),
                                    _ => Err("Expected cell reference after ':' in sheet range".into()),
                                }
                            } else {
                                Ok(Expr::SheetCellRef { sheet: name, cell })
                            }
                        }
                        _ => Err(format!("Expected cell reference after '{name}!'")),
                    }
                } else {
                    // Bare identifier: a named-range reference. The
                    // engine resolves it at eval-time; an unknown name
                    // evaluates to `#NAME?`.
                    Ok(Expr::Name(name))
                }
            }
            Some(Token::LParen) => {
                let expr = self.parse_expr()?;
                self.expect(&Token::RParen)?;
                Ok(expr)
            }
            Some(tok) => Err(format!("Unexpected token: {tok:?}")),
            None => Err("Unexpected end of formula".to_string()),
        }
    }
}

// ─── Public API ────────────────────────────────────────────────

/// Validate a candidate named-range identifier. The constraint is
/// shaped by both Excel's name rules and what this parser's tokenizer
/// can actually emit as `Token::Ident` — anything outside this grammar
/// would silently never resolve at evaluation time.
///
/// Allowed: ASCII letters, digits, `_`, and `.` (the latter only
/// after the first character, matching the tokenizer which requires
/// an alphabetic or `_` lead). Must NOT match the
/// `<letters><digits>` shape of a cell reference (e.g. `A1`, `AA10`)
/// so a name can't shadow a real address.
pub fn is_valid_named_range_name(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() { return false; }
    let first = bytes[0];
    if !(first.is_ascii_alphabetic() || first == b'_') { return false; }
    if !bytes.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'.') {
        return false;
    }
    // Reject `LETTERS+DIGITS+` like `A1`, `AA10`.
    let split = bytes.iter().position(|b| b.is_ascii_digit());
    if let Some(idx) = split {
        if idx > 0 && bytes[..idx].iter().all(|b| b.is_ascii_alphabetic())
            && bytes[idx..].iter().all(|b| b.is_ascii_digit())
        {
            return false;
        }
    }
    true
}

/// Parse a formula string (without leading `=`) into an AST.
pub fn parse_formula(input: &str) -> Result<Expr, String> {
    let tokens = tokenize(input)?;
    if tokens.is_empty() {
        return Ok(Expr::Text(String::new()));
    }
    let mut parser = Parser::new(tokens);
    let expr = parser.parse_expr()?;
    if parser.pos < parser.tokens.len() {
        return Err(format!("Unexpected token after expression: {:?}", parser.tokens[parser.pos]));
    }
    Ok(expr)
}

// ─── Tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_number() {
        assert_eq!(parse_formula("42").unwrap(), Expr::Number(42.0));
        assert_eq!(parse_formula("3.14").unwrap(), Expr::Number(3.14));
    }

    #[test]
    fn parse_string() {
        assert_eq!(parse_formula("\"hello\"").unwrap(), Expr::Text("hello".to_string()));
    }

    #[test]
    fn parse_bool() {
        assert_eq!(parse_formula("TRUE").unwrap(), Expr::Bool(true));
        assert_eq!(parse_formula("FALSE").unwrap(), Expr::Bool(false));
    }

    #[test]
    fn parse_cell_ref() {
        assert_eq!(
            parse_formula("A1").unwrap(),
            Expr::CellRef(CellRef { col: 0, row: 0, abs_col: false, abs_row: false })
        );
        assert_eq!(
            parse_formula("$B$3").unwrap(),
            Expr::CellRef(CellRef { col: 1, row: 2, abs_col: true, abs_row: true })
        );
    }

    #[test]
    fn parse_range() {
        let expr = parse_formula("A1:B5").unwrap();
        assert!(matches!(expr, Expr::Range(RangeRef { .. })));
    }

    #[test]
    fn parse_sheet_cell_ref() {
        // `Sheet2!B2` round-trips through the parser as a
        // sheet-qualified cell reference.
        let expr = parse_formula("Sheet2!B2").unwrap();
        match expr {
            Expr::SheetCellRef { sheet, cell } => {
                // Tokenizer uppercases bare identifiers.
                assert_eq!(sheet, "SHEET2");
                assert_eq!(cell.col, 1);
                assert_eq!(cell.row, 1);
            }
            other => panic!("expected SheetCellRef, got {other:?}"),
        }
    }

    #[test]
    fn parse_sheet_range_ref() {
        let expr = parse_formula("Sheet2!A1:B5").unwrap();
        match expr {
            Expr::SheetRange { sheet, range } => {
                assert_eq!(sheet, "SHEET2");
                assert_eq!(range.start.col, 0);
                assert_eq!(range.end.col, 1);
            }
            other => panic!("expected SheetRange, got {other:?}"),
        }
    }

    #[test]
    fn parse_arithmetic() {
        let expr = parse_formula("1 + 2 * 3").unwrap();
        // Should be 1 + (2 * 3) due to precedence
        assert!(matches!(expr, Expr::BinOp { op: BinOp::Add, .. }));
    }

    #[test]
    fn parse_function_call() {
        let expr = parse_formula("SUM(A1:B5, 10)").unwrap();
        match expr {
            Expr::FuncCall { name, args } => {
                assert_eq!(name, "SUM");
                assert_eq!(args.len(), 2);
            }
            _ => panic!("Expected FuncCall"),
        }
    }

    #[test]
    fn parse_nested() {
        let expr = parse_formula("IF(A1>0, A1*2, 0)").unwrap();
        assert!(matches!(expr, Expr::FuncCall { .. }));
    }

    #[test]
    fn parse_unary_neg() {
        let expr = parse_formula("-5").unwrap();
        assert!(matches!(expr, Expr::UnaryNeg(_)));
    }

    #[test]
    fn parse_percentage() {
        let expr = parse_formula("50%").unwrap();
        assert!(matches!(expr, Expr::Percent(_)));
    }

    #[test]
    fn parse_concat() {
        let expr = parse_formula("\"hello\" & \" world\"").unwrap();
        assert!(matches!(expr, Expr::BinOp { op: BinOp::Concat, .. }));
    }

    #[test]
    fn valid_named_range_accepts_excel_legal_names() {
        // Plain alpha, alphanumeric, underscore, dot mid-string — all
        // valid per Excel's name rules and accepted by the tokenizer.
        assert!(is_valid_named_range_name("PROFIT"));
        assert!(is_valid_named_range_name("_HIDDEN"));
        assert!(is_valid_named_range_name("Q1_Sales"));
        assert!(is_valid_named_range_name("Jan.Sales"));
        assert!(is_valid_named_range_name("PROFIT_2026"));
    }

    #[test]
    fn valid_named_range_rejects_cell_ref_shapes_and_bad_chars() {
        // Cell-ref shaped — would shadow real addresses.
        assert!(!is_valid_named_range_name("A1"));
        assert!(!is_valid_named_range_name("AA10"));
        // Empty / leading digit / non-ASCII / whitespace.
        assert!(!is_valid_named_range_name(""));
        assert!(!is_valid_named_range_name("1bad"));
        assert!(!is_valid_named_range_name("has space"));
        assert!(!is_valid_named_range_name(".leading_dot"));
    }

    #[test]
    fn col_letters_roundtrip() {
        assert_eq!(letters_to_col("A"), 0);
        assert_eq!(letters_to_col("Z"), 25);
        assert_eq!(letters_to_col("AA"), 26);
        assert_eq!(col_to_letters(0), "A");
        assert_eq!(col_to_letters(25), "Z");
        assert_eq!(col_to_letters(26), "AA");
    }

    #[test]
    fn parse_rejects_deeply_nested_expressions() {
        // 200 layers of parentheses is well past MAX_PARSE_DEPTH
        // (128) — the parser must error, not blow the WASM stack.
        let mut formula = String::new();
        for _ in 0..200 { formula.push('('); }
        formula.push('1');
        for _ in 0..200 { formula.push(')'); }
        let result = parse_formula(&formula);
        assert!(result.is_err(), "expected depth-exceeded error");
        let err = result.unwrap_err();
        assert!(
            err.contains("deeply nested"),
            "expected 'deeply nested' error, got: {err}",
        );
    }

    #[test]
    fn parse_accepts_modestly_nested_expressions() {
        // 20 layers stays comfortably inside the bound.
        let mut formula = String::new();
        for _ in 0..20 { formula.push('('); }
        formula.push('1');
        for _ in 0..20 { formula.push(')'); }
        assert!(parse_formula(&formula).is_ok());
    }
}
