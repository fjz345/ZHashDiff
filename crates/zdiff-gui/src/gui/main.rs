use std::env;

use dotenvy::dotenv;
use eframe::egui;

use crate::app::ZApp;

mod app;
mod clamped_cursor;
mod file;
mod keybindings;
mod p4;
mod quick_diff;
pub mod ui_egui;

#[cfg(feature = "debug_alloc")]
use stats_alloc::INSTRUMENTED_SYSTEM;
#[cfg(feature = "debug_alloc")]
use stats_alloc::StatsAlloc;
#[cfg(feature = "debug_alloc")]
use std::alloc::System;
#[cfg(feature = "debug_alloc")]
#[global_allocator]
pub static STATS_ALLOC: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

fn main() -> eframe::Result {
    dotenv().ok();

    let p4port = env::var("P4PORT").expect("P4PORT must be set");
    let p4user = env::var("P4USER").expect("P4USER must be set");
    let p4client = env::var("P4CLIENT").expect("P4CLIENT must be set");
    let p4password = env::var("P4PASSWORD").expect("P4PASSWORD must be set");

    println!("Connecting to {} as {}", p4port, p4user);

    unsafe { env::set_var("RUST_LOG", "debug") }; // or "info" or "debug"
    #[cfg(feature = "pretty-backtrace")]
    {
        color_backtrace::install();
    }
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu=warn,naga=warn"),
    )
    .init();

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([2560.0, 1440.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };

    eframe::run_native(
        "ZDiff",
        native_options,
        Box::new(move |cc: &eframe::CreationContext<'_>| {
            egui_extras::install_image_loaders(&cc.egui_ctx);

            #[cfg(feature = "serde")]
            {
                // Try to load saved state from storage
                if let Some(storage) = cc.storage {
                    if let Some(json) = storage.get_string(eframe::APP_KEY) {
                        if let Ok(mut app) = serde_json::from_str::<ZApp>(&json) {
                            log::info!("Found previous app storage");
                            app.request_init();
                            return Ok(Box::new(app));
                        }
                    }
                }
            }

            let mut app = ZApp::new(cc);
            app.request_init();
            Ok(Box::<ZApp>::new(app))
        }),
    )
}
