use eframe::egui::{self};
use serde::{Deserialize, Serialize};

use crate::ui_egui::diff_pane::{FileDiffPane, FileDiffPaneCtx};

pub struct TreeBehavior<'a> {
    pub ctx_file_diff: FileDiffPaneCtx<'a>,
}

impl egui_tiles::Behavior<Pane> for TreeBehavior<'_> {
    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        pane.title().into()
    }

    fn pane_ui(
        &mut self,
        ui: &mut egui::Ui,
        _tile_id: egui_tiles::TileId,
        pane: &mut Pane,
    ) -> egui_tiles::UiResponse {
        let response = match pane {
            Pane::FileDiff(file_diff_pane) => {
                let mut ctx = &mut self.ctx_file_diff;
                let response = file_diff_pane.ui(ui, &mut ctx);
                egui_tiles::UiResponse::from(response)
            }
        };

        response
    }
}

#[derive(Serialize, Deserialize)]
pub enum Pane {
    FileDiff(FileDiffPane),
}

impl Pane {
    pub fn title(&self) -> String {
        match self {
            Pane::FileDiff(p) => p.title(),
        }
    }
}

pub trait ZAppPane {
    fn title(&self) -> String {
        "Pane".to_string()
    }
}
