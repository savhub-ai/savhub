mod api;
mod app;
mod contexts;
mod i18n;
pub mod icons;
mod location;
mod pages;
mod storage;
mod urls;
mod ws;

#[cfg(target_arch = "wasm32")]
fn main() {
    dioxus::launch(app::App);
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    println!("Run the frontend with `dx serve --platform web` from the frontend crate.");
}
