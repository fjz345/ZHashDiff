use std::env;

use eframe::egui;

use crate::app::ZApp;

mod app;
mod clamped_cursor;
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
    unsafe { env::set_var("RUST_LOG", "debug") }; // or "info" or "debug"
    #[cfg(feature = "pretty-backtrace")]
    {
        color_backtrace::install();
    }
    env_logger::init();

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
