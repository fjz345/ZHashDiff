use eframe::egui::{self, Layout, PointerButton};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

use crate::{
    logger::LogCollector,
    ui_egui::panes::{FileExplorerPane, LogPane, Pane, TreeBehavior},
};
use eframe::{
    CreationContext,
    epaint::{Pos2, Vec2},
};
use egui_tiles::Tile;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
enum AppState {
    #[default]
    Startup,
    Idle,
    Exit,
}

#[derive(Serialize, Deserialize)]
pub struct ZApp {
    monitor_size: Vec2,
    scale_factor: f32,
    native_pixel_per_point: f32,
    state: AppState,
    tree: egui_tiles::Tree<Pane>,
    #[serde(skip)]
    log_buffer: Arc<Mutex<Vec<String>>>,
}

const HARDCODED_MONITOR_SIZE: Vec2 = Vec2::new(2560.0, 1440.0);
impl ZApp {
    // stupid work around since persistance storage does not work??
    pub fn request_init(&mut self) {
        self.state = AppState::Startup;
    }

    pub fn new(cc: &CreationContext<'_>, log_buffer: Arc<Mutex<Vec<String>>>) -> Self {
        // Can not get window screen size from CreationContext
        let monitor_size = HARDCODED_MONITOR_SIZE;
        const RESOLUTION_REF: f32 = 1080.0;
        let scale_factor: f32 = monitor_size.x.min(monitor_size.y) / RESOLUTION_REF;

        let native_pixel_per_point = cc.egui_ctx.native_pixels_per_point().unwrap_or(1.0);

        Self {
            monitor_size: monitor_size,
            scale_factor: scale_factor,
            native_pixel_per_point: native_pixel_per_point,
            state: AppState::Startup,
            tree: Self::create_tree(log_buffer.clone()),
            log_buffer: log_buffer,
        }
    }

    fn startup(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Fix startup not having correct references
        {
            self.log_buffer = LogCollector::init().expect("Failed to init logger");

            for tile in &mut self.tree.tiles.iter_mut() {
                match tile.1 {
                    Tile::Pane(p) => match p {
                        Pane::Log(log_pane) => log_pane.log_buffer = self.log_buffer.clone(),
                        Pane::FileExplorer(pane) => {
                            pane.update_worker_count(pane.concurrent_hashes)
                        }
                    },
                    _ => {}
                }
            }
        }

        let visuals: egui::Visuals = egui::Visuals::dark();
        ctx.set_visuals(visuals);
        log::info!("pixels_per_point{:?}", ctx.pixels_per_point());
        log::info!("native_pixels_per_point{:?}", ctx.native_pixels_per_point());
        ctx.set_pixels_per_point(self.scale_factor); // Maybe mult native_pixels_per_point?
        // ctx.set_debug_on_hover(true);

        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
    }

    fn create_tree(log_buffer: Arc<Mutex<Vec<String>>>) -> egui_tiles::Tree<Pane> {
        let mut tiles = egui_tiles::Tiles::default();

        let mut tabs = vec![];

        let tile_console = tiles.insert_pane(Pane::Log(LogPane {
            title: Some("Log".to_string()),
            log_buffer: log_buffer.clone(),
            scroll_to_bottom: true,
        }));

        let tile_file_explorer = tiles.insert_pane(Pane::FileExplorer(FileExplorerPane::new(
            Some("File Explorer".to_string()),
        )));

        let master_tile = tiles.insert_horizontal_tile(vec![tile_file_explorer]);
        tabs.push(tiles.insert_vertical_tile(vec![master_tile, tile_console]));

        let root = tiles.insert_tab_tile(tabs);

        egui_tiles::Tree::new("my_tree", root, tiles)
    }

    fn draw_ui_tree(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.with_layout(Layout::left_to_right(egui::Align::Min), |ui| {
                let mut behavior = TreeBehavior {};
                self.tree.ui(&mut behavior, ui);

                for (_tile_id, tile) in self.tree.tiles.iter() {
                    if let Tile::Pane(Pane::FileExplorer(file_explorer)) = tile {
                        let count = file_explorer.count_files_and_folders();

                        let active = file_explorer.count_active_hashes();
                        let total_pending = file_explorer.count_hash_queue();
                        let waiting = total_pending.saturating_sub(active);
                        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                            "ZHashDiff - {} files/folders ({} active, {} queued)",
                            count, active, waiting
                        )));

                        break;
                    }
                }
            });
        });
    }

    fn request_shutdown(&mut self) {
        self.state = AppState::Exit;
    }

    fn process_ctx_inputs(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut user_quit: bool = false;
        {
            let _input_ctx = ctx.input(|r| {
                // Esc
                if r.key_down(egui::Key::Escape) {
                    user_quit = true;
                }

                // DoubleLeftClick
                if r.pointer.button_double_clicked(PointerButton::Primary) {
                    let mouse_pos = r.pointer.interact_pos().unwrap();
                    log::info!("double click @({},{})", mouse_pos.x, mouse_pos.y);
                }

                if r.pointer.button_clicked(PointerButton::Middle) {
                    let mouse_pos: Pos2 = r.pointer.interact_pos().unwrap();

                    log::info!("middle click @({},{})", mouse_pos.x, mouse_pos.y);
                }
            });
        }

        if user_quit {
            self.request_shutdown();
        }
    }
}

impl eframe::App for ZApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        log::info!("SAVING...");

        #[cfg(feature = "serde")]
        if let Ok(json) = serde_json::to_string(self) {
            log::debug!("SAVED with state: {:?}", self.state);
            storage.set_string(eframe::APP_KEY, json);
        }
        log::info!("SAVED!");
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        match self.state {
            AppState::Startup => {
                self.startup(ctx, frame);
                self.state = AppState::Idle;
            }
            AppState::Idle => {
                self.draw_ui_tree(ctx, frame);
                self.process_ctx_inputs(ctx, frame);
            }
            AppState::Exit => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }

        // let screen_size_px = [ctx.used_size().x as u32, ctx.used_size().y as u32];
        // if let Some(event) = &mut self.app_ctx.clipboard_event {
        //     let pixel_read = read_pixels_from_frame(
        //         frame,
        //         screen_size_px,
        //         self.native_pixel_per_point,
        //         self.scale_factor,
        //         event.frame_rect.min.x,
        //         event.frame_rect.max.y,
        //         event.frame_rect.size().x,
        //         event.frame_rect.size().y,
        //     );
        //     if pixel_read.data.len() > 0 {
        //         event.frame_pixels = Some(pixel_read);
        //     } else {
        //         event.frame_pixels = None;
        //     }
        // }
    }
}
