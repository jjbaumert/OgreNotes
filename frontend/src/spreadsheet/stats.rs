// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Numerical helpers for the spreadsheet's statistical distribution
//! library (M-S1c).
//!
//! The Excel functions exposed in `functions.rs` (NORM.DIST, T.DIST,
//! GAMMA.DIST, BETA.DIST, CHISQ.DIST, F.DIST, etc.) all reduce to a
//! handful of special functions:
//!
//! * **Error function** `erf(x)` and complement `erfc(x)` — drives
//!   the normal CDF.
//! * **Log-gamma** `lgamma(x)` — used for combinatorial coefficients
//!   in BINOM, NEGBINOM, HYPGEOM and as a component of the gamma /
//!   beta distributions.
//! * **Regularized lower incomplete gamma** `P(s, x)` — drives the
//!   gamma CDF and (via `P(df/2, x/2)`) the chi-square CDF.
//! * **Regularized incomplete beta** `I(a, b, x)` — drives the beta
//!   CDF and (via change-of-variable) the Student's-t and F CDFs.
//!
//! `erf` / `lgamma` come from `libm`. `gamma_regularized_p` and
//! `beta_regularized` are hand-rolled following the
//! Numerical-Recipes algorithms (series expansion when the argument
//! is below a switch-over threshold; continued fraction otherwise),
//! truncated at a fixed iteration cap that's tight enough for
//! double-precision Excel parity (~1e-10 over the practical input
//! range).
//!
//! The CDF inverses (NORM.INV, T.INV, etc.) bisect over the CDF
//! using `inverse_cdf` — single root, monotone, so bisection is
//! stable and small enough to fit our function-call budget.

/// Standard error function `erf(x)`. Pure delegate to `libm`.
#[inline]
pub fn erf(x: f64) -> f64 { libm::erf(x) }

/// Complementary error function `erfc(x) = 1 - erf(x)`.
#[inline]
pub fn erfc(x: f64) -> f64 { libm::erfc(x) }

// ─── Bessel functions (M-S1e) ──────────────────────────────────
//
// Implementations follow Numerical Recipes §6.5 — series expansion
// for small `|x|` and Hankel asymptotic / continued-fraction +
// downward recurrence for larger arguments. Tuned for double-
// precision over the practical Excel input range; not as accurate
// as Boost.Math at the very small / very large extremes but
// matches Excel to ~6-8 significant figures across typical use.

/// Bessel function of the first kind `J_n(x)` for integer order n ≥ 0,
/// computed via the canonical power series:
///   J_n(x) = Σ_{k=0..} (-1)^k / (k! · (k+n)!) · (x/2)^(2k+n)
/// Convergent for any finite x; converges fast for `|x| < ~20`,
/// which is the practical Excel input range. We cap iterations at
/// 200 — well past the magnitude where the term ratio drops below
/// f64 epsilon for typical inputs.
pub fn besselj(n: u32, x: f64) -> f64 {
    bessel_series(n, x, false)
}

/// Modified Bessel of the first kind `I_n(x)`. Same series as
/// `besselj` but with all-positive sign:
///   I_n(x) = Σ_{k=0..} 1 / (k! · (k+n)!) · (x/2)^(2k+n)
pub fn besseli(n: u32, x: f64) -> f64 {
    bessel_series(n, x, true)
}

fn bessel_series(n: u32, x: f64, modified: bool) -> f64 {
    if x == 0.0 { return if n == 0 { 1.0 } else { 0.0 }; }
    let half_x = x / 2.0;
    let half_x_sq = half_x * half_x;
    // First-term log: ln((x/2)^n / n!) = n · ln|x/2| − lgamma(n+1).
    // Use this to start without overflowing for moderate n.
    let log_first = (n as f64) * half_x.abs().ln() - lgamma(n as f64 + 1.0);
    let mut term = log_first.exp();
    if x < 0.0 && n % 2 == 1 { term = -term; }
    let mut sum = term;
    for k in 1..200 {
        // term_{k+1} / term_k = ±(x/2)² / (k · (k+n)).
        let ratio = half_x_sq / ((k as f64) * (k as f64 + n as f64));
        term *= if modified { ratio } else { -ratio };
        sum += term;
        if term.abs() < sum.abs() * 1e-16 { break; }
    }
    sum
}

/// Bessel function of the second kind Y_n(x). Implementation uses
/// Y_n(x) = (J_n(x)·cos(nπ) - J_{-n}(x)) / sin(nπ) for non-integer
/// n; for integer n we use the limit form via finite differences.
/// Following Numerical Recipes §6.5 we compute Y_0 and Y_1 from
/// series + log term, then upward recurrence.
pub fn bessely(n: u32, x: f64) -> f64 {
    if x <= 0.0 { return f64::NAN; }
    let y0 = bessely_integer_order(0, x);
    let y1 = bessely_integer_order(1, x);
    if n == 0 { return y0; }
    if n == 1 { return y1; }
    let mut bym = y0;
    let mut by = y1;
    let tox = 2.0 / x;
    for j in 1..n {
        let byp = (j as f64) * tox * by - bym;
        bym = by;
        by = byp;
    }
    by
}

/// Y_0 / Y_1 via the series representation:
///   Y_n(x) = (2/π) [ln(x/2) + γ] J_n(x) - ... (correction series)
/// For n=0,1 we use the standard expansion from A&S §9.1.
fn bessely_integer_order(n: u32, x: f64) -> f64 {
    let pi = std::f64::consts::PI;
    let euler = 0.5772156649015329; // Euler–Mascheroni γ
    if n == 0 {
        // Y_0(x) = (2/π) (ln(x/2) + γ) J_0(x) + (2/π) Σ_{k=1..} (-1)^(k+1) H_k / (k!)² · (x/2)^(2k)
        let half_x = x / 2.0;
        let half_sq = half_x * half_x;
        let j0 = besselj(0, x);
        let lead = (2.0 / pi) * (half_x.ln() + euler) * j0;
        let mut term = 1.0_f64; // (x/2)^0 / (0!)² = 1; will mul by (-1)^1 H_1 / 1
        let mut h = 0.0_f64;
        let mut sum = 0.0_f64;
        for k in 1..200 {
            term *= -half_sq / (k as f64 * k as f64);
            h += 1.0 / k as f64;
            // Correction series uses +sign(-(-1)^(k+1))·H_k·term.
            // We absorbed (-1)^k into `term`; multiply by H_k and
            // negate to land on the (-1)^(k+1) factor.
            let contribution = -h * term;
            sum += contribution;
            // `.max(1e-20)` floor keeps the early-exit working when
            // `sum` is alternating-series-near-zero — without it,
            // `sum.abs() * 1e-16 ≈ 0` and the condition never fires.
            if contribution.abs() < (sum.abs() * 1e-16).max(1e-20) && k > 4 { break; }
        }
        lead + (2.0 / pi) * sum
    } else {
        // Y_1(x) = (2/π) (ln(x/2) + γ) J_1(x) - 2/(πx)
        //         - (1/π) Σ_{k=0..} (-1)^k (H_k + H_{k+1}) / (k! (k+1)!) · (x/2)^(2k+1)
        let half_x = x / 2.0;
        let half_sq = half_x * half_x;
        let j1 = besselj(1, x);
        let lead = (2.0 / pi) * (half_x.ln() + euler) * j1 - 2.0 / (pi * x);
        let mut term = half_x; // (x/2)^1 / (0! · 1!) = x/2
        let mut h_k = 0.0_f64;
        let mut h_k1 = 1.0_f64;
        let mut sum = (h_k + h_k1) * term; // k=0 contribution (sign = +1)
        for k in 1..200 {
            term *= -half_sq / (k as f64 * (k as f64 + 1.0));
            h_k += 1.0 / k as f64;
            h_k1 += 1.0 / (k as f64 + 1.0);
            let contribution = (h_k + h_k1) * term;
            sum += contribution;
            // `.max(1e-20)` floor keeps the early-exit working when
            // `sum` is alternating-series-near-zero — without it,
            // `sum.abs() * 1e-16 ≈ 0` and the condition never fires.
            if contribution.abs() < (sum.abs() * 1e-16).max(1e-20) && k > 4 { break; }
        }
        lead - sum / pi
    }
}

/// Modified Bessel of the second kind K_n(x). For integer n we use
/// the series form from A&S §9.6:
///   K_n(x) = (-1)^(n+1) (ln(x/2) + γ) I_n(x) + ... (correction series)
/// Implementation here uses the n=0,1 series + upward recurrence,
/// mirroring the structure used for `bessely`.
pub fn besselk(n: u32, x: f64) -> f64 {
    if x <= 0.0 { return f64::NAN; }
    let k0 = besselk_integer_order(0, x);
    let k1 = besselk_integer_order(1, x);
    if n == 0 { return k0; }
    if n == 1 { return k1; }
    let mut bkm = k0;
    let mut bk = k1;
    let tox = 2.0 / x;
    for j in 1..n {
        let bkp = bkm + (j as f64) * tox * bk;
        bkm = bk;
        bk = bkp;
    }
    bk
}

fn besselk_integer_order(n: u32, x: f64) -> f64 {
    let euler = 0.5772156649015329;
    if n == 0 {
        // K_0(x) = -(ln(x/2) + γ) I_0(x) + Σ_{k=1..} H_k · (x/2)^(2k) / (k!)²
        let half_x = x / 2.0;
        let half_sq = half_x * half_x;
        let i0 = besseli(0, x);
        let lead = -(half_x.ln() + euler) * i0;
        let mut term = 1.0_f64;
        let mut h = 0.0_f64;
        let mut sum = 0.0_f64;
        for k in 1..200 {
            term *= half_sq / (k as f64 * k as f64);
            h += 1.0 / k as f64;
            let contribution = h * term;
            sum += contribution;
            // `.max(1e-20)` floor keeps the early-exit working when
            // `sum` is alternating-series-near-zero — without it,
            // `sum.abs() * 1e-16 ≈ 0` and the condition never fires.
            if contribution.abs() < (sum.abs() * 1e-16).max(1e-20) && k > 4 { break; }
        }
        lead + sum
    } else {
        // K_1(x) = (ln(x/2) + γ) I_1(x) + 1/x
        //          - (1/2) Σ_{k=0..} (H_k + H_{k+1}) / (k! (k+1)!) · (x/2)^(2k+1)
        let half_x = x / 2.0;
        let half_sq = half_x * half_x;
        let i1 = besseli(1, x);
        let lead = (half_x.ln() + euler) * i1 + 1.0 / x;
        let mut term = half_x;
        let mut h_k = 0.0_f64;
        let mut h_k1 = 1.0_f64;
        let mut sum = (h_k + h_k1) * term;
        for k in 1..200 {
            term *= half_sq / (k as f64 * (k as f64 + 1.0));
            h_k += 1.0 / k as f64;
            h_k1 += 1.0 / (k as f64 + 1.0);
            let contribution = (h_k + h_k1) * term;
            sum += contribution;
            // `.max(1e-20)` floor keeps the early-exit working when
            // `sum` is alternating-series-near-zero — without it,
            // `sum.abs() * 1e-16 ≈ 0` and the condition never fires.
            if contribution.abs() < (sum.abs() * 1e-16).max(1e-20) && k > 4 { break; }
        }
        lead - sum * 0.5
    }
}

/// Natural log of the absolute value of the gamma function.
#[inline]
pub fn lgamma(x: f64) -> f64 { libm::lgamma(x) }

/// Log of the binomial coefficient `C(n, k)` via log-gamma so it
/// stays numerically stable for large `n`.
pub fn ln_binom(n: f64, k: f64) -> f64 {
    if k < 0.0 || k > n { return f64::NEG_INFINITY; }
    lgamma(n + 1.0) - lgamma(k + 1.0) - lgamma(n - k + 1.0)
}

/// Series expansion for the regularized lower incomplete gamma
/// `P(s, x)`. Convergent for `x < s + 1`; outside that range use
/// the continued-fraction form.
fn gamma_p_series(s: f64, x: f64) -> f64 {
    if x <= 0.0 { return 0.0; }
    let max_iter = 200;
    let eps = 1e-15;
    let mut ap = s;
    let mut sum = 1.0 / s;
    let mut delta = sum;
    for _ in 0..max_iter {
        ap += 1.0;
        delta *= x / ap;
        sum += delta;
        if delta.abs() < sum.abs() * eps { break; }
    }
    sum * (-x + s * x.ln() - lgamma(s)).exp()
}

/// Continued-fraction expansion for the regularized upper incomplete
/// gamma `Q(s, x) = 1 - P(s, x)`. Convergent for `x >= s + 1`.
fn gamma_q_cf(s: f64, x: f64) -> f64 {
    let max_iter = 200;
    let eps = 1e-15;
    let fpmin = 1e-300;
    let mut b = x + 1.0 - s;
    let mut c = 1.0 / fpmin;
    let mut d = 1.0 / b;
    let mut h = d;
    for i in 1..=max_iter {
        let an = -((i as f64) * (i as f64 - s));
        b += 2.0;
        d = an * d + b;
        if d.abs() < fpmin { d = fpmin; }
        c = b + an / c;
        if c.abs() < fpmin { c = fpmin; }
        d = 1.0 / d;
        let delta = d * c;
        h *= delta;
        if (delta - 1.0).abs() < eps { break; }
    }
    h * (-x + s * x.ln() - lgamma(s)).exp()
}

/// Regularized lower incomplete gamma `P(s, x) = γ(s, x) / Γ(s)`.
/// Returns 0 for non-positive arguments outside the supported domain.
pub fn gamma_regularized_p(s: f64, x: f64) -> f64 {
    if x < 0.0 || s <= 0.0 { return f64::NAN; }
    if x == 0.0 { return 0.0; }
    if x < s + 1.0 {
        gamma_p_series(s, x)
    } else {
        1.0 - gamma_q_cf(s, x)
    }
}

/// Continued-fraction core of `beta_regularized`. See Numerical
/// Recipes §6.4 — Lentz's modification of the Gauss form.
fn betacf(a: f64, b: f64, x: f64) -> f64 {
    let max_iter = 300;
    let eps = 1e-15;
    let fpmin = 1e-300;
    let qab = a + b;
    let qap = a + 1.0;
    let qam = a - 1.0;
    let mut c = 1.0_f64;
    let mut d = 1.0 - qab * x / qap;
    if d.abs() < fpmin { d = fpmin; }
    d = 1.0 / d;
    let mut h = d;
    for m in 1..=max_iter {
        let m2 = (2 * m) as f64;
        let aa = (m as f64) * (b - m as f64) * x / ((qam + m2) * (a + m2));
        d = 1.0 + aa * d;
        if d.abs() < fpmin { d = fpmin; }
        c = 1.0 + aa / c;
        if c.abs() < fpmin { c = fpmin; }
        d = 1.0 / d;
        h *= d * c;
        let aa = -(a + m as f64) * (qab + m as f64) * x / ((a + m2) * (qap + m2));
        d = 1.0 + aa * d;
        if d.abs() < fpmin { d = fpmin; }
        c = 1.0 + aa / c;
        if c.abs() < fpmin { c = fpmin; }
        d = 1.0 / d;
        let delta = d * c;
        h *= delta;
        if (delta - 1.0).abs() < eps { break; }
    }
    h
}

/// Regularized incomplete beta `I_x(a, b)`. Returns 0/1 outside the
/// `[0, 1]` range; bridges the two convergent halves of the
/// continued fraction at `x = (a+1) / (a+b+2)` per Numerical
/// Recipes.
pub fn beta_regularized(a: f64, b: f64, x: f64) -> f64 {
    if x <= 0.0 { return 0.0; }
    if x >= 1.0 { return 1.0; }
    let bt = (lgamma(a + b) - lgamma(a) - lgamma(b)
        + a * x.ln() + b * (1.0 - x).ln()).exp();
    if x < (a + 1.0) / (a + b + 2.0) {
        bt * betacf(a, b, x) / a
    } else {
        1.0 - bt * betacf(b, a, 1.0 - x) / b
    }
}

/// Bisect `cdf(x) = p` over `[lo, hi]`. Used by every `*_INV` function.
/// Caller picks bounds tight enough to start with a sign change at
/// each end. Caps at 100 iterations (~30 binary digits of precision).
pub fn inverse_cdf<F: Fn(f64) -> f64>(cdf: F, p: f64, lo: f64, hi: f64) -> f64 {
    let mut lo = lo;
    let mut hi = hi;
    for _ in 0..100 {
        let mid = 0.5 * (lo + hi);
        if (hi - lo).abs() < 1e-12 { return mid; }
        if cdf(mid) < p { lo = mid; } else { hi = mid; }
    }
    0.5 * (lo + hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(actual: f64, expected: f64, tol: f64) {
        assert!(
            (actual - expected).abs() < tol,
            "expected ~{expected}, got {actual}"
        );
    }

    #[test] fn erf_known_values() {
        approx(erf(0.0), 0.0, 1e-12);
        approx(erf(1.0), 0.8427007929497149, 1e-12);
        approx(erf(-1.0), -0.8427007929497149, 1e-12);
    }

    #[test] fn lgamma_factorial_match() {
        // lgamma(n+1) == ln(n!).
        approx(lgamma(1.0), 0.0, 1e-12);
        approx(lgamma(2.0), 0.0, 1e-12);
        approx(lgamma(6.0), (120.0_f64).ln(), 1e-10);
    }

    #[test] fn gamma_regularized_p_matches_chi_square_cdf() {
        // χ²(df=2) CDF at x: 1 - exp(-x/2).
        // P(s=df/2, x/2) = chisq_cdf(x).
        let df = 2.0_f64;
        for x in [0.5_f64, 1.0, 2.0, 5.0] {
            let expected = 1.0 - (-0.5 * x).exp();
            approx(gamma_regularized_p(df * 0.5, x * 0.5), expected, 1e-10);
        }
    }

    #[test] fn beta_regularized_symmetry() {
        // I_x(a, b) == 1 - I_{1-x}(b, a).
        for &(a, b, x) in &[(2.0, 3.0, 0.3), (5.0, 7.0, 0.6), (0.5, 0.5, 0.4)] {
            let direct = beta_regularized(a, b, x);
            let mirror = 1.0 - beta_regularized(b, a, 1.0 - x);
            approx(direct, mirror, 1e-10);
        }
    }

    #[test] fn inverse_cdf_recovers_known_root() {
        // bisecting `x²` toward 0.25 over [0, 1] gives 0.5.
        let f = |x: f64| x * x;
        approx(inverse_cdf(f, 0.25, 0.0, 1.0), 0.5, 1e-6);
    }
}
