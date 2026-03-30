mod api;
mod app;
mod collab;
mod components;
pub mod editor;
mod pages;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(app::App);
}
