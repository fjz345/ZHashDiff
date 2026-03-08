use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::ui_egui::panes::ZAppPane;

pub struct FileDiffPaneCtx<'a> {
    pub file_1_name: Option<&'a String>,
    pub file_2_name: Option<&'a String>,
    pub file_1: Option<&'a String>,
    pub file_2: Option<&'a String>,
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
        ui.label("HELLO THERE!"); 

        egui_tiles::UiResponse::None
    }
}
