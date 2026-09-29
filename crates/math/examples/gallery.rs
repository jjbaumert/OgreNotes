//! Writes an HTML page of sample equations (block and inline) to stdout,
//! for eyeballing the MathML output in a browser:
//!
//!   cargo run -p ogrenotes-math --example gallery > /tmp/math.html

use ogrenotes_math::{to_mathml, Display};

const SAMPLES: &[&str] = &[
    "E = mc^2",
    "x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}",
    "\\int_{-\\infty}^{\\infty} e^{-x^2}\\,dx = \\sqrt{\\pi}",
    "\\sum_{n=1}^{\\infty} \\frac{1}{n^2} = \\frac{\\pi^2}{6}",
    "\\nabla \\times \\mathbf{E} = -\\frac{\\partial \\mathbf{B}}{\\partial t}",
    "\\lim_{h \\to 0} \\frac{f(x+h) - f(x)}{h} = f'(x)",
    "\\begin{vmatrix} a & b \\\\ c & d \\end{vmatrix} = ad - bc",
    "A = \\begin{pmatrix} 1 & 0 & 0 \\\\ 0 & \\cos\\theta & -\\sin\\theta \\\\ 0 & \\sin\\theta & \\cos\\theta \\end{pmatrix}",
    "|x| = \\begin{cases} x & x \\geq 0 \\\\ -x & \\text{otherwise} \\end{cases}",
    "\\begin{aligned} (a+b)^2 &= a^2 + 2ab + b^2 \\\\ &\\geq 4ab \\end{aligned}",
    "\\left( \\sum_{i=1}^n a_i b_i \\right)^2 \\leq \\left( \\sum_{i=1}^n a_i^2 \\right) \\left( \\sum_{i=1}^n b_i^2 \\right)",
    "\\hat{\\theta} = \\operatorname*{arg\\,max}_\\theta \\; \\mathcal{L}(\\theta \\mid x)",
    "\\mathbb{E}[X] = \\sum_{x \\in \\mathbb{R}} x \\Pr(X = x)",
    "\\binom{n}{k} = \\frac{n!}{k!\\,(n-k)!}",
    "\\underbrace{1 + 1 + \\cdots + 1}_{n \\text{ times}} = n",
    "\\forall \\epsilon > 0 \\; \\exists \\delta > 0 : |x - a| < \\delta \\implies |f(x) - f(a)| < \\epsilon",
    "\\sqrt[3]{x} \\not= \\vec{v} \\cdot \\overline{w} \\pmod{7}",
    "{\\color{red} a} + \\textcolor{#0a0}{b}",
];

fn main() {
    println!("<!doctype html><meta charset=utf-8><title>ogrenotes-math gallery</title>");
    println!("<style>body{{font:16px sans-serif;max-width:900px;margin:2em auto}}math{{font-family:math}}code{{color:#666}}</style>");
    for src in SAMPLES {
        let block = to_mathml(src, Display::Block).unwrap_or_else(|e| format!("<b>error: {e}</b>"));
        let inline = to_mathml(src, Display::Inline).unwrap_or_else(|e| format!("<b>error: {e}</b>"));
        println!("<p><code>{}</code></p>{block}<p>Inline: {inline} and more text.</p><hr>", src.replace('<', "&lt;"));
    }
}
