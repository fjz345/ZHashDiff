use std::env;

use eframe::egui;
use zdiff::{diff_ir::{DiffOp, generate_ir}, lexer::{Lexer,  TokenKind, visualize_diff_grid, visualize_diff_grid_with_path}, myers::{backtrack, myers_diff, myers_diff_trace}};

use crate::app::ZApp;

mod app;
mod ui_egui;

// fn main() {
//     unsafe { env::set_var("RUST_LOG", "debug") }; // or "info" or "debug"
//     color_backtrace::install();

//     let rust_file_1 = r#"
//         fn calculate(a: i32, b: i32) -> i32 {
//             let res = a + b;
//             return res;
//         }
//     "#;

//     let rust_file_2 = r#"
//         fn calculate(a: i32, b: i32) -> i32 {
//             if a == 0 { return b; }
//             let total = a + b;
//             return total;
//         }
//     "#;

//     let lexer_1 = Lexer::new(rust_file_1);
//     let lexer_2 = Lexer::new(rust_file_2);
    
//     // Filter out whitespace so we only see the "logic" diff
//     let tokens_1 = lexer_1.clone()
//         .filter(|f| f.kind != TokenKind::Whitespace)
//         .collect::<Vec<_>>();
//     let tokens_2 = lexer_2.clone()
//         .filter(|f| f.kind != TokenKind::Whitespace)
//         .collect::<Vec<_>>();

//     let trace = myers_diff_trace(&tokens_1, &tokens_2, |t1, t2| {
//         t1.kind == t2.kind && lexer_1.token_value(t1) == lexer_2.token_value(t2)
//     });

//     let path = backtrack(trace, tokens_1.len() as i32, tokens_2.len() as i32);

//     let ir = generate_ir(&tokens_1, &tokens_2, &path);

//     for entry in &ir.entries {
//         let prefix = match entry.operation {
//             DiffOp::Equal => " ",
//             DiffOp::Delete => "-",
//             DiffOp::Insert => "+",
//         };
//         let token_value = match entry.operation {
//             DiffOp::Equal | DiffOp::Delete => lexer_1.token_value(entry.token),
//             DiffOp::Insert => lexer_2.token_value(entry.token),
//         };
//         println!("{} {}", prefix, token_value);
//     }

//     println!("\nTotal Distance: {}", ir.distance);

//     // visualize_diff_grid_with_path(&lexer_1, &tokens_1, &lexer_2, &tokens_2, &path, |t1, t2| {
//     //     t1.kind == t2.kind && lexer_1.token_value(t1) == lexer_2.token_value(t2)
//     // });

//     // let distance = path.windows(2)
//     //     .filter(|w| {
//     //         let (x1, y1) = w[0];
//     //         let (x2, y2) = w[1];
//     //         (x1 == x2 && y1 != y2) || (x1 != x2 && y1 == y2)
//     //     })
//     //     .count();

//     // println!("\nToken edit distance: {}", distance);
// }



fn main() -> eframe::Result {
    unsafe { env::set_var("RUST_LOG", "debug") }; // or "info" or "debug"
    color_backtrace::install();

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