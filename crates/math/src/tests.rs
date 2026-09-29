// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use super::*;

/// The presentation markup only: what sits between `<semantics><mrow>`
/// and the annotation.
fn body(src: &str) -> String {
    let out = to_mathml(src, Display::Block).unwrap_or_else(|e| panic!("{src:?}: {e}"));
    let start = out.find("<semantics>").unwrap() + "<semantics>".len();
    let end = out.find("<annotation").unwrap();
    out[start..end].to_string()
}

fn err(src: &str) -> MathError {
    to_mathml(src, Display::Block).expect_err(src)
}

fn well_formed(xml: &str) {
    let mut reader = quick_xml::Reader::from_str(xml);
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Eof) => break,
            Ok(_) => {}
            Err(e) => panic!("malformed XML ({e}): {xml}"),
        }
    }
}

#[test]
fn wraps_in_math_with_display_and_annotation() {
    let out = to_mathml("x", Display::Block).unwrap();
    assert!(out.starts_with(r#"<math xmlns="http://www.w3.org/1998/Math/MathML" display="block"><semantics><mrow><mi>x</mi></mrow>"#), "{out}");
    assert!(out.ends_with(r#"<annotation encoding="application/x-tex">x</annotation></semantics></math>"#));
    assert!(to_mathml("x", Display::Inline).unwrap().contains(r#"display="inline""#));
}

#[test]
fn letters_numbers_and_operators() {
    assert_eq!(
        body("x+12.5=y"),
        "<mrow><mi>x</mi><mo>+</mo><mn>12.5</mn><mo>=</mo><mi>y</mi></mrow>"
    );
    assert_eq!(body("a-b"), "<mrow><mi>a</mi><mo>−</mo><mi>b</mi></mrow>");
    assert_eq!(body(".5"), "<mrow><mn>.5</mn></mrow>");
    assert_eq!(body("1."), "<mrow><mn>1</mn><mo>.</mo></mrow>");
    // Whitespace and comments are ignored.
    assert_eq!(body(" a  % comment\n b"), "<mrow><mi>a</mi><mi>b</mi></mrow>");
}

#[test]
fn scripts_and_primes() {
    assert_eq!(body("x^2"), "<mrow><msup><mi>x</mi><mn>2</mn></msup></mrow>");
    assert_eq!(body("x_i^2"), "<mrow><msubsup><mi>x</mi><mi>i</mi><mn>2</mn></msubsup></mrow>");
    assert_eq!(body("x^2_i"), "<mrow><msubsup><mi>x</mi><mi>i</mi><mn>2</mn></msubsup></mrow>");
    assert_eq!(
        body("e^{i\\pi}"),
        "<mrow><msup><mi>e</mi><mrow><mi>i</mi><mi>π</mi></mrow></msup></mrow>"
    );
    // Only one character is the argument without braces.
    assert_eq!(body("x^23"), "<mrow><msup><mi>x</mi><mn>2</mn></msup><mn>3</mn></mrow>");
    assert_eq!(body("f'"), "<mrow><msup><mi>f</mi><mo>′</mo></msup></mrow>");
    assert_eq!(
        body("f''^2"),
        "<mrow><msup><mi>f</mi><mrow><mo>″</mo><mn>2</mn></mrow></msup></mrow>"
    );
    assert_eq!(body("10^{-3}"), "<mrow><msup><mn>10</mn><mrow><mo>−</mo><mn>3</mn></mrow></msup></mrow>");
    // A script with no base attaches to an empty one.
    assert_eq!(body("^{14}C"), "<mrow><msup><mrow/><mn>14</mn></msup><mi>C</mi></mrow>");
}

#[test]
fn fractions_roots_and_binomials() {
    assert_eq!(body("\\frac{a}{b}"), "<mrow><mfrac><mi>a</mi><mi>b</mi></mfrac></mrow>");
    assert_eq!(body("\\frac12"), "<mrow><mfrac><mn>1</mn><mn>2</mn></mfrac></mrow>");
    assert!(body("\\dfrac{a}{b}").contains(r#"<mstyle displaystyle="true" scriptlevel="0"><mfrac>"#));
    assert_eq!(body("\\sqrt{x}"), "<mrow><msqrt><mi>x</mi></msqrt></mrow>");
    assert_eq!(body("\\sqrt[3]{x}"), "<mrow><mroot><mi>x</mi><mn>3</mn></mroot></mrow>");
    let binom = body("\\binom{n}{k}");
    assert!(binom.contains(r#"<mfrac linethickness="0"><mi>n</mi><mi>k</mi></mfrac>"#), "{binom}");
    assert!(binom.contains(r#"<mo fence="true" form="prefix" stretchy="true">(</mo>"#), "{binom}");
}

#[test]
fn symbols_greek_and_functions() {
    assert_eq!(body("\\alpha"), "<mrow><mi>α</mi></mrow>");
    assert_eq!(body("\\Gamma"), r#"<mrow><mi mathvariant="normal">Γ</mi></mrow>"#);
    assert_eq!(body("a\\leq b"), "<mrow><mi>a</mi><mo>≤</mo><mi>b</mi></mrow>");
    assert_eq!(body("\\infty"), "<mrow><mi>∞</mi></mrow>");
    assert_eq!(
        body("\\sin x"),
        r#"<mrow><mi mathvariant="normal">sin</mi><mo>⁡</mo><mi>x</mi></mrow>"#
    );
    assert_eq!(
        body("\\sin^2 x"),
        r#"<mrow><msup><mi mathvariant="normal">sin</mi><mn>2</mn></msup><mo>⁡</mo><mi>x</mi></mrow>"#
    );
    assert!(body("\\operatorname{sgn} x").contains(r#"<mi mathvariant="normal">sgn</mi><mo>⁡</mo>"#));
}

#[test]
fn big_operators_and_limits() {
    assert_eq!(
        body("\\sum_{i=1}^n i"),
        r#"<mrow><munderover><mo movablelimits="true">∑</mo><mrow><mi>i</mi><mo>=</mo><mn>1</mn></mrow><mi>n</mi></munderover><mi>i</mi></mrow>"#
    );
    // Integrals keep side scripts unless \limits.
    assert_eq!(body("\\int_0^1"), "<mrow><msubsup><mo>∫</mo><mn>0</mn><mn>1</mn></msubsup></mrow>");
    assert_eq!(body("\\int\\limits_0^1"), "<mrow><munderover><mo movablelimits=\"false\">∫</mo><mn>0</mn><mn>1</mn></munderover></mrow>");
    assert_eq!(
        body("\\sum\\nolimits_i"),
        r#"<mrow><msub><mo movablelimits="true">∑</mo><mi>i</mi></msub></mrow>"#
    );
    assert!(body("\\lim_{x\\to 0}").starts_with(r#"<mrow><munder><mo movablelimits="true" form="prefix" lspace="0" rspace="0.1667em">lim</mo>"#));
    assert!(body("\\operatorname*{argmax}_x").contains("<munder><mo movablelimits=\"true\""));
    assert!(err("x\\limits_0").message.contains("must follow an operator"));
}

#[test]
fn fonts_map_to_math_alphanumerics() {
    assert_eq!(body("\\mathbb{R}"), "<mrow><mi>ℝ</mi></mrow>");
    assert_eq!(body("\\mathbf{x}"), "<mrow><mi>𝐱</mi></mrow>");
    assert_eq!(body("\\mathcal{L}"), "<mrow><mi>ℒ</mi></mrow>");
    assert_eq!(body("\\mathrm{d}x"), r#"<mrow><mi mathvariant="normal">d</mi><mi>x</mi></mrow>"#);
    assert_eq!(body("\\mathbf{12}"), "<mrow><mn>𝟏𝟐</mn></mrow>");
    // Switches run to the end of the group.
    assert_eq!(body("{\\bf a b} c"), "<mrow><mrow><mi>𝐚</mi><mi>𝐛</mi></mrow><mi>c</mi></mrow>");
    // The font doesn't leak out of its argument.
    assert_eq!(body("\\mathbf{a}b"), "<mrow><mi>𝐚</mi><mi>b</mi></mrow>");
}

#[test]
fn text_and_spacing() {
    assert_eq!(body("\\text{if } x"), "<mrow><mtext>if\u{a0}</mtext><mi>x</mi></mrow>");
    assert_eq!(body("\\text{ a b }"), "<mrow><mtext>\u{a0}a b\u{a0}</mtext></mrow>");
    assert_eq!(body("\\text{a {b} \\$}"), "<mrow><mtext>a b $</mtext></mrow>");
    assert_eq!(body("\\textbf{A}"), "<mrow><mtext>𝐀</mtext></mrow>");
    assert_eq!(body("a\\,b"), r#"<mrow><mi>a</mi><mspace width="0.1667em"/><mi>b</mi></mrow>"#);
    assert_eq!(body("a\\quad b"), r#"<mrow><mi>a</mi><mspace width="1em"/><mi>b</mi></mrow>"#);
    assert!(err("\\text{\\alpha}").message.contains("inside `\\text"));
    assert!(err("\\text{$x$}").message.contains("math inside"));
    assert!(err("\\text x").message.contains("braces"));
}

#[test]
fn delimiters() {
    assert_eq!(
        body("\\left( \\frac{a}{b} \\right)"),
        r#"<mrow><mo fence="true" form="prefix" stretchy="true">(</mo><mfrac><mi>a</mi><mi>b</mi></mfrac><mo fence="true" form="postfix" stretchy="true">)</mo></mrow>"#
    );
    assert!(body("\\left\\{ x \\right.").contains(r#"stretchy="true">{</mo><mi>x</mi></mrow>"#));
    assert_eq!(
        body("|x|"),
        r#"<mrow><mo stretchy="false" lspace="0" rspace="0">|</mo><mi>x</mi><mo stretchy="false" lspace="0" rspace="0">|</mo></mrow>"#
    );
    assert!(body("\\left\\langle x \\middle| y \\right\\rangle").contains(r#"<mo fence="true" stretchy="true">|</mo>"#));
    // Plain parentheses don't stretch.
    assert_eq!(body("(x)"), r#"<mrow><mo stretchy="false">(</mo><mi>x</mi><mo stretchy="false">)</mo></mrow>"#);
    assert!(body("\\big( x \\Big)").contains(r#"minsize="1.2em" maxsize="1.2em">(</mo>"#));
    assert!(body("\\bigl( x \\bigr)").contains(r#"maxsize="1.2em">)</mo>"#));
    assert!(err("\\left( x").message.contains("matching `\\right`"));
    assert!(err("x \\right)").message.contains("matching `\\left`"));
    assert!(err("\\left x").message.contains("isn't a delimiter"));
}

#[test]
fn accents_and_braces() {
    assert_eq!(body("\\hat{x}"), r#"<mrow><mover accent="true"><mi>x</mi><mo stretchy="false">^</mo></mover></mrow>"#);
    assert!(body("\\overline{AB}").contains(r#"<mo stretchy="true">‾</mo>"#));
    assert!(body("\\underline{x}").contains(r#"<munder accentunder="true">"#));
    assert!(body("\\vec v").contains("→"));
    assert_eq!(
        body("\\overbrace{a+b}^{n}"),
        r#"<mrow><mover><mover><mrow><mi>a</mi><mo>+</mo><mi>b</mi></mrow><mo stretchy="true">⏞</mo></mover><mi>n</mi></mover></mrow>"#
    );
}

#[test]
fn environments() {
    let m = body("\\begin{pmatrix} a & b \\\\ c & d \\end{pmatrix}");
    assert!(m.contains("<mtable><mtr><mtd><mi>a</mi></mtd><mtd><mi>b</mi></mtd></mtr><mtr><mtd><mi>c</mi></mtd><mtd><mi>d</mi></mtd></mtr></mtable>"), "{m}");
    assert!(m.starts_with(r#"<mrow><mo fence="true" form="prefix" stretchy="true">(</mo><mtable>"#), "{m}");
    // A trailing \\ doesn't add an empty row.
    assert_eq!(
        body("\\begin{matrix} 1 \\\\ 2 \\\\ \\end{matrix}").matches("<mtr>").count(),
        2
    );
    let cases = body("f(x) = \\begin{cases} 0 & x < 0 \\\\ 1 & \\text{otherwise} \\end{cases}");
    assert!(cases.contains(r#"<mtd style="text-align:left"><mn>0</mn></mtd>"#), "{cases}");
    let aligned = body("\\begin{aligned} a &= b \\\\ &= c \\end{aligned}");
    assert!(aligned.contains(r#"<mtable displaystyle="true">"#), "{aligned}");
    assert!(aligned.contains(r#"<mtd style="text-align:left;padding-left:0"><mrow><mo form="infix">=</mo><mi>b</mi></mrow></mtd>"#), "{aligned}");
    let array = body("\\begin{array}{l|r} a & b \\end{array}");
    assert!(array.contains(r#"<mtd style="text-align:left"><mi>a</mi></mtd><mtd style="text-align:right"><mi>b</mi></mtd>"#), "{array}");
    assert!(body("\\begin{equation} x \\end{equation}").contains("<mi>x</mi>"));
    assert!(err("\\begin{matrix} a \\end{pmatrix}").message.contains("ended by"));
    assert!(err("\\begin{tikzpicture} \\end{tikzpicture}").message.contains("unknown environment"));
    assert!(err("\\begin{matrix} a").message.contains("matching `\\end`"));
    assert!(err("a & b").message.contains("inside an environment"));
    assert!(err("a \\\\ b").message.contains("inside an environment"));
}

#[test]
fn colors_are_validated() {
    assert!(body("\\textcolor{red}{x}").contains(r#"<mstyle mathcolor="red"><mi>x</mi></mstyle>"#));
    assert!(body("{\\color{#00f} x} y").contains(r##"<mstyle mathcolor="#00f"><mi>x</mi></mstyle>"##));
    for bad in ["\\textcolor{red\" onclick=\"x}{a}", "\\color{url(x)} a", "\\color{#12} a", "\\color{} a"] {
        assert!(err(bad).message.contains("invalid color"), "{bad}");
    }
}

#[test]
fn not_negates_relations() {
    assert_eq!(body("a \\not= b"), "<mrow><mi>a</mi><mo>≠</mo><mi>b</mi></mrow>");
    assert!(body("x \\not\\in A").contains("<mo>∉</mo>"));
    assert!(body("x \\not\\prec y").contains("<mo>≺\u{338}</mo>"));
    assert!(err("\\not{ab}").message.contains("needs a relation"));
}

#[test]
fn errors_name_the_problem_and_position() {
    let e = err("x + \\foo");
    assert_eq!(e.message, "unknown command `\\foo`");
    assert_eq!(e.offset, 4);
    assert_eq!(e.to_string(), "unknown command `\\foo` (at character 5)");
    assert!(err("{x").message.contains("missing `}`"));
    assert!(err("x}").message.contains("unexpected `}`"));
    assert!(err("x^").message.contains("missing argument"));
    assert!(err("x^2^3").message.contains("double superscript"));
    assert!(err("x_1_2").message.contains("double subscript"));
    assert!(err("$x$").message.contains("`$`"));
    assert!(err("\\frac{a}").message.contains("missing argument"));
    assert!(err("x \\").message.contains("end of the equation"));
    assert!(err("\\tag{1} x").message.contains("aren't supported"));
    assert_eq!(err("   ").message, "equation is empty");
}

#[test]
fn source_length_is_capped() {
    let long = "x".repeat(MAX_SOURCE_LEN + 1);
    assert!(err(&long).message.contains("too long"));
    assert!(to_mathml(&"x".repeat(MAX_SOURCE_LEN), Display::Block).is_ok());
}

#[test]
fn deep_nesting_is_an_error_not_a_stack_overflow() {
    for (open, close) in [("{", "}"), ("\\frac{", "}{x}"), ("\\sqrt{", "}")] {
        let src = format!("{}x{}", open.repeat(900), close.repeat(900));
        assert!(src.chars().count() <= MAX_SOURCE_LEN);
        assert!(err(&src).message.contains("nested too deeply"), "{open}");
    }
    // Unbraced chains recurse too.
    let src = format!("{}x", "\\hat".repeat(2000));
    assert!(err(&src).message.contains("nested too deeply"));
    // Realistic nesting is fine: 12 nested fractions, 20 nested groups.
    let fracs = format!("{}x{}", "\\frac{".repeat(12), "}{y}".repeat(12));
    assert!(to_mathml(&fracs, Display::Block).is_ok());
    assert!(to_mathml(&format!("{}x{}", "{".repeat(20), "}".repeat(20)), Display::Block).is_ok());
}

/// The browser runs the parser on a 1 MB (WASM default) stack. Hitting
/// the depth limit with the most stack-hungry constructs must still fit,
/// including in unoptimized builds.
#[test]
fn depth_limit_fits_in_a_one_megabyte_stack() {
    let worst = [
        format!("{}x", "\\hat".repeat(200)),
        format!("{}x{}", "\\left(".repeat(200), "\\right)".repeat(200)),
        format!("{}x{}", "\\begin{matrix}".repeat(200), "\\end{matrix}".repeat(200)),
        format!("{}x{}", "x^{".repeat(200), "}".repeat(200)),
        format!("{}x{}", "\\sqrt[".repeat(200), "]{y}".repeat(200)),
    ];
    std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(move || {
            for src in worst {
                assert!(err(&src).message.contains("nested too deeply"), "{src}");
            }
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn source_text_is_escaped_everywhere() {
    let out = to_mathml("a < b \\text{<script>&\"}", Display::Block).unwrap();
    assert!(!out.contains("<script>"), "{out}");
    assert!(out.contains("<mo>&lt;</mo>"));
    assert!(out.contains("<mtext>&lt;script&gt;&amp;&quot;</mtext>"));
    // The annotation carries the escaped source.
    assert!(out.contains("<annotation encoding=\"application/x-tex\">a &lt; b \\text{&lt;script&gt;&amp;&quot;}</annotation>"));
    well_formed(&out);
}

#[test]
fn realistic_equations_render_well_formed() {
    let samples = [
        "E = mc^2",
        "x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}",
        "\\int_{-\\infty}^{\\infty} e^{-x^2}\\,dx = \\sqrt{\\pi}",
        "\\sum_{n=1}^{\\infty} \\frac{1}{n^2} = \\frac{\\pi^2}{6}",
        "\\nabla \\times \\mathbf{E} = -\\frac{\\partial \\mathbf{B}}{\\partial t}",
        "\\lim_{h \\to 0} \\frac{f(x+h) - f(x)}{h}",
        "\\det\\begin{vmatrix} a & b \\\\ c & d \\end{vmatrix} = ad - bc",
        "P(A \\mid B) = \\frac{P(B \\mid A)\\,P(A)}{P(B)}",
        "\\left\\lfloor \\frac{n}{2} \\right\\rfloor + \\binom{n}{k}",
        "\\forall \\epsilon > 0\\; \\exists \\delta > 0 : |x - a| < \\delta \\implies |f(x) - f(a)| < \\epsilon",
        "\\begin{aligned} (a+b)^2 &= a^2 + 2ab + b^2 \\\\ &\\geq 0 \\end{aligned}",
        "\\hat{\\theta} = \\operatorname*{arg\\,max}_\\theta \\; \\mathcal{L}(\\theta)",
        "\\mathbb{E}[X] = \\sum_{x} x \\, \\Pr(X = x)",
        "\\oint_C \\vec{F} \\cdot d\\vec{r}",
        "\\underbrace{1 + 1 + \\cdots + 1}_{n \\text{ times}} = n",
    ];
    for src in samples {
        for display in [Display::Block, Display::Inline] {
            let out = to_mathml(src, display).unwrap_or_else(|e| panic!("{src}: {e}"));
            well_formed(&out);
        }
    }
}

mod fuzz {
    use super::*;
    use proptest::prelude::*;

    /// Fragments that exercise every parser path, including broken ones.
    const PIECES: &[&str] = &[
        "x", "2", "+", "=", "{", "}", "^", "_", "'", "&", "\\\\", "[", "]", "(", ")", "|", ".",
        "\\frac", "\\sqrt", "\\left", "\\right", "\\middle", "\\begin{matrix}", "\\end{matrix}",
        "\\begin{aligned}", "\\end{aligned}", "\\text{", "\\mathbf", "\\bf", "\\color{red}",
        "\\sum", "\\int", "\\limits", "\\lim", "\\hat", "\\overbrace", "\\not", "\\big", "\\alpha",
        "\\", "%", "$", "#", "~", "<", ">", "\"", "é", "𝐱", "\u{0}", " ", "\n",
    ];

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(3000))]

        #[test]
        fn never_panics_and_output_is_well_formed(parts in proptest::collection::vec(0..PIECES.len(), 0..40)) {
            let src: String = parts.iter().map(|&i| PIECES[i]).collect();
            if let Ok(out) = to_mathml(&src, Display::Block) {
                well_formed(&out);
            }
        }

        #[test]
        fn arbitrary_strings_never_panic(src in ".{0,200}") {
            let _ = to_mathml(&src, Display::Inline);
        }
    }
}
