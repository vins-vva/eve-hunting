//! Eve Hunting — desktop client (Rust + Rune), drawing its own titlebar.
//!
//! A left nav with one page per wormhole hub (Thera, Turnur), tracking the connections
//! published by EVE-Scout.
//! A background thread polls the API every minute; new connections show up in the table,
//! marked "NEW". For every k-space exit, the page lists the systems within 10 jumps that had
//! NPC kills in the last hour (ESI hourly stats, the same ones Dotlan shows).
//!
//! The window reopens where it was, on the same page, with the same filters, sorts, splitter
//! and nav (see `settings.rs`).
//!
//! `cargo run` opens the window.
//! `cargo run -- --snap thera.png --scene demo` renders the page headless from the data in
//! `fixtures/` (no network, default settings).

mod api;
mod hubs;
mod settings;
mod universe;

use std::sync::atomic::{AtomicBool, Ordering};

use rune::{AppSpec, WindowOptions};

/// True for headless renders (`--snap`, `--tree`, …): no network thread, no settings file.
pub static HEADLESS: AtomicBool = AtomicBool::new(false);

fn main() {
    let headless = std::env::args().any(|a| {
        matches!(
            a.as_str(),
            "--snap" | "--trace" | "--tree" | "--frames" | "--strip" | "--first-pixels" | "--list-scenes" | "--list-windows"
        )
    });
    HEADLESS.store(headless, Ordering::Relaxed);
    rune::prewarm_gpu();

    let saved = if headless { settings::Settings::default() } else { settings::load() };
    let mut options = WindowOptions::default();
    let mut maximized = false;
    if let Some(place) = saved.window.filter(|p| p.is_usable()) {
        options.position = Some((place.x, place.y));
        options.physical_size = Some((place.width, place.height));
        maximized = place.maximized;
    }
    let spec = AppSpec {
        min_size: Some((820.0, 480.0)),
        scenes: hubs::SCENES,
        setup: Some(hubs::setup),
        poll: Some(hubs::poll),
        on_key: Some(hubs::on_key),
        ..AppSpec::new(hubs::APP_NAME, (1360.0, 880.0), hubs::init, hubs::view)
    };
    rune::App::new(spec)
        .titlebar(rune::Titlebar::new(hubs::TITLEBAR_HEIGHT))
        .init(move || hubs::init_with(&saved))
        .window_options(options)
        .maximized(maximized)
        .on_window(hubs::remember_window)
        .on_close(hubs::on_close)
        .run();
}
