// #![windows_subsystem = "windows"]
// #![allow(dead_code)]
// #![allow(unreachable_patterns)]

use std::env;

use eframe::egui::{self};
use zcommon::logger::LogCollector;

use crate::ui_egui::app::ZApp;

mod ui_egui;

fn main() -> eframe::Result {
    unsafe { env::set_var("RUST_LOG", "debug") }; // or "info" or "debug"
    #[cfg(feature = "pretty-backtrace")]
    {
        color_backtrace::install();
    }

    let log_buffer = LogCollector::init().expect("Failed to init logger");

    // Test build with zdiff
    {
        let zdiff_path = env!("ZDIFF_BIN_PATH");
        println!("The zdiff binary is located at: {}", zdiff_path);
    }

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([2560.0, 1440.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };

    eframe::run_native(
        "ZHashDiff",
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

            let mut app = ZApp::new(cc, log_buffer.clone());
            app.request_init();
            Ok(Box::<ZApp>::new(app))
        }),
    )
}
