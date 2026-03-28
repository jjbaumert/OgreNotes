use leptos::prelude::*;
use leptos_router::components::*;
use leptos_router::path;

use crate::pages;

#[component]
pub fn App() -> impl IntoView {
    view! {
        <Router>
            <Routes fallback=|| view! { <p>"Page not found"</p> }>
                <Route path=path!("/login") view=pages::login::LoginPage />
                <Route path=path!("/auth/complete") view=pages::auth_complete::AuthCompletePage />
                <Route path=path!("/") view=pages::home::HomePage />
                <Route path=path!("/d/:id/:slug") view=pages::document::DocumentPage />
                <Route path=path!("/d/:id") view=pages::document::DocumentPage />
            </Routes>
        </Router>
    }
}
