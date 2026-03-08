use eframe::egui;
use serde::{Deserialize, Serialize};
use zdiff::lexer::{Lexer, RawToken};
use crate::ui_egui::panes::ZAppPane;

pub struct FileDiffPaneCtx<'a> {
    pub file_1_name: Option<&'a String>,
    pub file_2_name: Option<&'a String>,
    pub file_1: Option<&'a String>,
    pub file_2: Option<&'a String>,
    pub diff_path: Option<&'a Vec<(i32, i32)>>,
    pub tokens_1: Option<&'a Vec<RawToken>>,
    pub tokens_2: Option<&'a Vec<RawToken>>,
}

#[derive(Serialize, Deserialize)]
pub struct FileDiffPane {
    pub title: Option<String>,
}

impl ZAppPane for FileDiffPane {
    fn title(&self) -> String {
        self.title.clone().unwrap_or(format!("Pane"))
    }
}

impl FileDiffPane {
    pub fn new(title: Option<String>) -> Self {
        Self {
            title,
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, ctx: &mut FileDiffPaneCtx) -> egui_tiles::UiResponse {
        let (Some(f1), Some(f2), Some(path)) = (ctx.file_1, ctx.file_2, ctx.diff_path) else {
            ui.centered_and_justified(|ui| {
                ui.label("Load two files to see the diff.");
            });
            return egui_tiles::UiResponse::None;
        };

        let lex1 = Lexer::new(f1);
        let lex2 = Lexer::new(f2);
        let tokens_1: Vec<_> = Lexer::new(f1).collect();
        let tokens_2: Vec<_> = Lexer::new(f2).collect();

        ui.vertical(|ui| {
            ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for window in path.windows(2) {
                        let (x1, y1) = window[0];
                        let (x2, y2) = window[1];

                        let (prefix, text, color, bg) = if x2 > x1 && y2 > y1 {
                            (" ", lex1.token_value(&tokens_1[x1 as usize]), ui.visuals().text_color(), egui::Color32::TRANSPARENT)
                        } else if x2 > x1 {
                            ("-", lex1.token_value(&tokens_1[x1 as usize]), egui::Color32::from_rgb(255, 100, 100), egui::Color32::from_rgba_unmultiplied(255, 0, 0, 15))
                        } else {
                            ("+", lex2.token_value(&tokens_2[y1 as usize]), egui::Color32::from_rgb(100, 255, 100), egui::Color32::from_rgba_unmultiplied(0, 255, 0, 15))
                        };

                        // Render the line with background highlighting
                        let width = ui.available_width();
                        egui::Frame::new().fill(bg).show(ui, |ui| {
                            ui.set_width(width);
                            ui.label(egui::RichText::new(format!("{} {}", prefix, text)).color(color));
                        });
                    }
                });
        });

        egui_tiles::UiResponse::None
    }
}
