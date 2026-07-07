// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! `/auth/mfa-enroll` — the post-login enrollment page (Phase 4
//! M-E3 piece E).
//!
//! Two states:
//!
//! 1. **Pre-enroll** — show the QR code + manual-entry secret +
//!    recovery codes, then ask the user to type the first TOTP from
//!    their authenticator to confirm.
//! 2. **Verified** — show a success banner and navigate home.
//!
//! Reached via either:
//!   - the post-login redirect when `mfaEnrollmentRequired: true`
//!     comes back on the login response (workspace requires MFA),
//!   - direct navigation by a user who voluntarily opts in.
//!
//! The QR encodes the `otpauth://` provisioning URI the server
//! returned from `/auth/mfa/enroll`. Rendered as inline SVG by the
//! `qrcode` crate — no canvas, no PNG, scales cleanly.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;
use qrcode::render::svg;
use qrcode::QrCode;

use crate::api::{client, mfa};

#[component]
pub fn MfaEnrollPage() -> impl IntoView {
    let (data, set_data) = signal::<Option<mfa::EnrollResponse>>(None);
    let (code_input, set_code_input) = signal::<String>(String::new());
    let (error, set_error) = signal::<Option<String>>(None);
    let (verified, set_verified) = signal(false);
    let (busy, set_busy) = signal(false);
    let navigate = use_navigate();

    if !client::is_authenticated() {
        let nav = navigate.clone();
        nav("/login", Default::default());
        return view! { <div>{crate::t!("common-redirecting-login")}</div> }.into_any();
    }

    // Kick off the enrollment fetch ONCE per reactive owner.
    // Wrapping in `Effect::new` with a `has_run` sentinel guards
    // against a Leptos re-mount cycle (hot-reload, router revisit,
    // future strict-mode double-invoke) firing two `enroll()` calls
    // that race — the loser would overwrite the secret on the
    // server while the user is staring at the QR from the winner.
    Effect::new(move |has_run: Option<bool>| {
        if has_run == Some(true) {
            return true;
        }
        spawn_local(async move {
            match mfa::enroll().await {
                Ok(d) => set_data.set(Some(d)),
                Err(e) => set_error.set(Some(crate::t!("mfa-enroll-error-failed", err = format!("{e:?}")))),
            }
        });
        true
    });

    let on_verify = {
        let navigate = navigate.clone();
        move |_| {
            let code = code_input.get();
            if code.trim().len() < 6 {
                set_error.set(Some(crate::t!("mfa-enter-totp")));
                return;
            }
            set_busy.set(true);
            set_error.set(None);
            let navigate = navigate.clone();
            spawn_local(async move {
                match mfa::verify(code.trim()).await {
                    Ok(()) => {
                        set_verified.set(true);
                        // Brief pause so the user sees the success
                        // banner before we navigate.
                        gloo_timers::future::TimeoutFuture::new(800).await;
                        navigate("/", Default::default());
                    }
                    Err(e) => {
                        set_error.set(Some(crate::t!("mfa-enroll-error-verify-failed", err = format!("{e:?}"))));
                        set_busy.set(false);
                    }
                }
            });
        }
    };

    view! {
        <main id="main-content" tabindex="-1" class="mfa-page">
            <div class="mfa-card">
                <h1 class="mfa-title">{crate::t!("mfa-enroll-title")}</h1>
                <p class="mfa-subtitle">
                    {crate::t!("mfa-enroll-subtitle")}
                </p>

                {move || error.get().map(|e| view! {
                    <div class="mfa-error" role="alert">{e}</div>
                })}

                {move || if verified.get() {
                    view! {
                        <div class="mfa-success">
                            {crate::t!("mfa-enroll-success")}
                        </div>
                    }.into_any()
                } else {
                    view! { <div></div> }.into_any()
                }}

                {move || data.get().map(|d| {
                    let svg = qr_svg(&d.provisioning_uri).unwrap_or_else(|err|
                        format!("<p>QR render failed: {err}</p>"));
                    let secret = d.secret.clone();
                    let codes = d.recovery_codes.clone();
                    view! {
                        // SAFETY on `inner_html`: the SVG comes from
                        // `qrcode 0.14`'s renderer, which encodes the
                        // provisioning URI into QR matrix coordinates
                        // and emits them as `<path d="M.. h.. v..">`
                        // path commands. The URI bytes never reach an
                        // SVG text node or an unescaped attribute
                        // value, so a hypothetical malicious URI
                        // can't escape via this path. The only user-
                        // controlled strings in the output are the
                        // two hardcoded color literals "#000"/"#fff"
                        // passed below.
                        <div class="mfa-qr" inner_html=svg></div>

                        <div class="mfa-secret">
                            <div class="mfa-secret-label">{crate::t!("mfa-enroll-manual-entry")}</div>
                            <code class="mfa-secret-value">{secret}</code>
                        </div>

                        <details class="mfa-recovery">
                            <summary>{crate::t!("mfa-enroll-recovery-codes-summary")}</summary>
                            <p class="mfa-recovery-warning">
                                {crate::t!("mfa-enroll-recovery-warning")}
                            </p>
                            <ul class="mfa-recovery-list">
                                {codes
                                    .into_iter()
                                    .map(|c| view! { <li><code>{c}</code></li> })
                                    .collect_view()}
                            </ul>
                        </details>

                        <div class="mfa-verify">
                            <label for="mfa-code-input">{crate::t!("mfa-enroll-code-label")}</label>
                            <input
                                id="mfa-code-input"
                                type="text"
                                inputmode="numeric"
                                pattern="[0-9]*"
                                maxlength="6"
                                placeholder="123456"
                                prop:value=move || code_input.get()
                                on:input=move |ev| set_code_input.set(event_target_value(&ev))
                            />
                            <button
                                class="mfa-verify-btn"
                                disabled=move || busy.get() || verified.get()
                                on:click=on_verify.clone()
                            >
                                {move || if busy.get() { crate::t!("mfa-verifying") } else { crate::t!("mfa-enroll-confirm") }}
                            </button>
                        </div>
                    }
                })}
            </div>
        </main>
    }
    .into_any()
}

/// Render the provisioning URI as an inline SVG QR code. Scaled to
/// 240px square; the SVG embeds its own viewBox so it scales further
/// without rasterizing.
fn qr_svg(uri: &str) -> Result<String, String> {
    let code = QrCode::new(uri.as_bytes()).map_err(|e| e.to_string())?;
    Ok(code
        .render::<svg::Color<'_>>()
        .min_dimensions(240, 240)
        .quiet_zone(true)
        .dark_color(svg::Color("#000"))
        .light_color(svg::Color("#fff"))
        .build())
}
