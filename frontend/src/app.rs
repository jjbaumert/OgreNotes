// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use leptos::prelude::*;
use leptos_router::components::*;
use leptos_router::path;

use crate::components;
use crate::pages;

#[component]
pub fn App() -> impl IntoView {
    view! {
        <Router>
            // M-P8 piece B: WCAG 2.4.1 skip-link. Must be the first
            // focusable element on the page; CSS hides it until
            // :focus. Each top-level page anchors `#main-content`
            // on its main wrapper.
            <a class="skip-to-content" href="#main-content">
                {crate::t!("a11y-skip-to-content")}
            </a>
            <Routes fallback=|| view! { <p>{crate::t!("app-not-found")}</p> }>
                // Sidebar-free routes stay flat (login, auth, admin,
                // workspace, the /profile redirect).
                <Route path=path!("/login") view=pages::login::LoginPage />
                <Route path=path!("/auth/complete") view=pages::auth_complete::AuthCompletePage />
                // Legacy /profile entry (sidebar) → unified settings.
                <Route path=path!("/profile") view=pages::settings::ProfileRedirect />
                <Route path=path!("/admin/users") view=pages::admin::users::AdminUsersPage />
                <Route path=path!("/admin/metrics") view=pages::admin::metrics::AdminMetricsPage />
                <Route path=path!("/admin/audit") view=pages::admin::audit::AdminAuditPage />
                <Route path=path!("/auth/mfa-enroll") view=pages::mfa_enroll::MfaEnrollPage />
                <Route path=path!("/auth/mfa-challenge") view=pages::mfa_challenge::MfaChallengePage />
                <Route path=path!("/workspaces/:id/saml") view=pages::workspace_saml::WorkspaceSamlPage />
                <Route path=path!("/workspaces/:id/scim") view=pages::workspace_scim::WorkspaceScimPage />

                // #152: the sidebar pages render inside a persistent
                // `AppShell` (sidebar + `.app-layout` wrapper + `<Outlet/>`).
                // The shell stays mounted across these routes, so navigating
                // between them swaps only the outlet content — the sidebar
                // never remounts, so it never flashes. Child paths carry NO
                // leading slash (they compose under the empty parent path).
                <ParentRoute path=path!("") view=components::app_shell::AppShell>
                    <Route path=path!("") view=pages::home::HomePage />
                    // Trash view — reuses HomePage, which detects the /trash
                    // path and opens the user's Trash folder (#104).
                    <Route path=path!("trash") view=pages::home::HomePage />
                    <Route path=path!("settings") view=pages::settings::SettingsPage />
                    <Route path=path!("d/:id/:slug") view=pages::document::DocumentPage />
                    <Route path=path!("d/:id") view=pages::document::DocumentPage />
                </ParentRoute>
            </Routes>
        </Router>
    }
}
