use eframe::egui::{self, Key, Modifiers};

#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Shortcut {
    pub key: Key,
    pub modifiers: Modifiers,
}

impl Shortcut {
    pub fn format(&self) -> String {
        let mut s = String::new();
        if self.modifiers.ctrl {
            s.push_str("Ctrl+");
        }
        if self.modifiers.shift {
            s.push_str("Shift+");
        }
        if self.modifiers.alt {
            s.push_str("Alt+");
        }
        if self.modifiers.mac_cmd {
            s.push_str("Cmd+");
        }
        s.push_str(&format!("{:?}", self.key));
        s
    }

    pub fn matches(&self, r: &egui::InputState) -> bool {
        r.key_pressed(self.key) && r.modifiers == self.modifiers
    }
}

type RootAndPath = (String, String);

#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct QuickDiffPaths {
    pub target: RootAndPath,
    pub source: Option<RootAndPath>,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Keybindings {
    pub open_file_source: Option<Shortcut>,
    pub open_file_target: Option<Shortcut>,
    pub refresh_diff_rows_only: Option<Shortcut>,
    pub refresh_diff: Option<Shortcut>, // Implies refresh_diff_rows_only
    pub open_options_keybindings: Option<Shortcut>,
    pub open_universal_path: Option<Shortcut>,
    pub find: Option<Shortcut>,
    pub goto: Option<Shortcut>,
    pub next_conflict: Option<Shortcut>,
    pub prev_conflict: Option<Shortcut>,
    pub next_find: Option<Shortcut>,
    pub prev_find: Option<Shortcut>,
    pub user_quick_diffs: Vec<(Option<Shortcut>, QuickDiffPaths)>,
    pub revision_graph: Option<Shortcut>,
    pub timelapse_view: Option<Shortcut>,
}

impl Default for Keybindings {
    fn default() -> Self {
        Self {
            open_file_source: Some(Shortcut {
                key: Key::F1,
                modifiers: Modifiers {
                    ctrl: true,
                    command: true,
                    ..Default::default()
                },
            }),
            open_file_target: Some(Shortcut {
                key: Key::F2,
                modifiers: Modifiers {
                    ctrl: true,
                    command: true,
                    ..Default::default()
                },
            }),
            refresh_diff: Some(Shortcut {
                key: Key::R,
                modifiers: Modifiers {
                    ctrl: true,
                    command: true,
                    ..Default::default()
                },
            }),
            refresh_diff_rows_only: Some(Shortcut {
                key: Key::R,
                modifiers: Modifiers {
                    ctrl: true,
                    command: true,
                    alt: true,
                    ..Default::default()
                },
            }),
            open_options_keybindings: Some(Shortcut {
                key: Key::O,
                modifiers: Modifiers {
                    ctrl: true,
                    command: true,
                    ..Default::default()
                },
            }),
            open_universal_path: Some(Shortcut {
                key: Key::U,
                modifiers: Modifiers {
                    ctrl: true,
                    command: true,
                    ..Default::default()
                },
            }),
            find: Some(Shortcut {
                key: Key::F,
                modifiers: Modifiers {
                    ctrl: true,
                    command: true,
                    ..Default::default()
                },
            }),
            goto: Some(Shortcut {
                key: Key::G,
                modifiers: Modifiers {
                    ctrl: true,
                    command: true,
                    ..Default::default()
                },
            }),
            next_conflict: Some(Shortcut {
                key: Key::Num2,
                modifiers: Modifiers {
                    ctrl: true,
                    command: true,
                    ..Default::default()
                },
            }),
            prev_conflict: Some(Shortcut {
                key: Key::Num1,
                modifiers: Modifiers {
                    ctrl: true,
                    command: true,
                    shift: true,
                    ..Default::default()
                },
            }),
            next_find: Some(Shortcut {
                key: Key::Enter,
                modifiers: Modifiers {
                    ctrl: false,
                    ..Default::default()
                },
            }),
            prev_find: Some(Shortcut {
                key: Key::Enter,
                modifiers: Modifiers {
                    shift: true,
                    ..Default::default()
                },
            }),
            user_quick_diffs: vec![
                (
                    Some(Shortcut {
                        key: Key::F1,
                        modifiers: Modifiers {
                            ..Default::default()
                        },
                    }),
                    QuickDiffPaths {
                        target: Default::default(),
                        source: None,
                        ..Default::default()
                    },
                ),
                (
                    Some(Shortcut {
                        key: Key::F2,
                        modifiers: Modifiers {
                            ..Default::default()
                        },
                    }),
                    QuickDiffPaths {
                        target: Default::default(),
                        source: None,
                        ..Default::default()
                    },
                ),
                (
                    Some(Shortcut {
                        key: Key::F3,
                        modifiers: Modifiers {
                            ..Default::default()
                        },
                    }),
                    QuickDiffPaths {
                        target: Default::default(),
                        source: None,
                        ..Default::default()
                    },
                ),
                (
                    Some(Shortcut {
                        key: Key::F4,
                        modifiers: Modifiers {
                            ..Default::default()
                        },
                    }),
                    QuickDiffPaths {
                        target: Default::default(),
                        source: None,
                        ..Default::default()
                    },
                ),
            ],
            revision_graph: Some(Shortcut {
                key: Key::R,
                modifiers: Modifiers {
                    ctrl: true,
                    command: true,
                    shift: true,
                    ..Default::default()
                },
            }),
            timelapse_view: Some(Shortcut {
                key: Key::T,
                modifiers: Modifiers {
                    ctrl: true,
                    command: true,
                    shift: true,
                    ..Default::default()
                },
            }),
        }
    }
}

pub fn ui_keybindings(ui: &mut egui::Ui, keybindings: &mut Keybindings) {
    if ui.button("Reset to defaults").clicked() {
        *keybindings = Keybindings::default();
    }
    if ui.button("Add Quick Diff").clicked() {
        keybindings
            .user_quick_diffs
            .push((None, QuickDiffPaths::default()));
    }
    ui.separator();

    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("p4_settings_grid")
            .num_columns(2)
            .spacing([40.0, 8.0])
            .show(ui, |ui| {
                let ui_shortcut_row =
                    |ui: &mut egui::Ui, label: &str, shortcut: &mut Option<Shortcut>| {
                        ui.label(label);

                        let id = ui.next_auto_id();
                        let is_listening = ui.memory(|mem| mem.has_focus(id));

                        let button_text = if is_listening {
                            "Press any key...".to_string()
                        } else {
                            shortcut
                                .as_ref()
                                .map_or_else(|| "None".to_string(), Shortcut::format)
                        };

                        if ui.add(egui::Button::new(button_text)).clicked() {
                            ui.memory_mut(|mem| mem.request_focus(id));
                        }

                        if is_listening {
                            let mut next_shortcut = None;
                            let mut should_surrender = false;

                            ui.input(|i| {
                                if i.key_pressed(egui::Key::Escape) {
                                    should_surrender = true;
                                } else if i.pointer.secondary_clicked() {
                                    next_shortcut = None;
                                    should_surrender = true;
                                } else {
                                    for event in &i.events {
                                        if let egui::Event::Key {
                                            key,
                                            pressed: true,
                                            modifiers,
                                            ..
                                        } = event
                                        {
                                            next_shortcut = Some(Shortcut {
                                                key: *key,
                                                modifiers: *modifiers,
                                            });
                                            should_surrender = true;
                                            break;
                                        }
                                    }
                                }
                            });

                            if should_surrender {
                                log::info!("Setting shortcut for {}: {:?}", label, next_shortcut);
                                *shortcut = next_shortcut;
                                ui.memory_mut(|mem| mem.surrender_focus(id));
                            }
                        }
                        ui.end_row();
                    };

                let ui_quick_diff_path_row =
                    |ui: &mut egui::Ui, label: &str, root: &mut String, path: &mut String| {
                        ui.label(label);

                        ui.horizontal(|ui| {
                            let widget = egui::TextEdit::singleline(root).desired_width(150.0);
                            ui.add(widget);
                            ui.label("/");
                            let widget =
                                egui::TextEdit::singleline(path).desired_width(f32::INFINITY);
                            ui.add(widget);
                        });

                        ui.end_row();
                    };

                for (i, (user_qd, quick_diff_path)) in
                    keybindings.user_quick_diffs.iter_mut().enumerate()
                {
                    ui_shortcut_row(ui, &format!("User Quick Diff {}", i + 1), user_qd);
                    ui_quick_diff_path_row(
                        ui,
                        "Target Path",
                        &mut quick_diff_path.target.0,
                        &mut quick_diff_path.target.1,
                    );

                    let mut action_remove = false;
                    let mut action_add = false;

                    match &mut quick_diff_path.source {
                        Some((root, path)) => {
                            if ui.button("Remove").clicked() {
                                action_remove = true;
                            }
                            ui.horizontal(|ui| {
                                let widget = egui::TextEdit::singleline(root).desired_width(150.0);
                                ui.add(widget);
                                ui.label("/");
                                let widget =
                                    egui::TextEdit::singleline(path).desired_width(f32::INFINITY);
                                ui.add(widget);
                            });
                        }
                        None => {
                            ui.label("Source Path");
                            if ui.button("➕ Add Source").clicked() {
                                action_add = true;
                            }
                        }
                    }
                    ui.end_row();

                    if action_remove {
                        quick_diff_path.source = None;
                    } else if action_add {
                        quick_diff_path.source = Some((String::new(), String::new()));
                    }
                }

                ui.separator();
                ui.end_row();

                ui_shortcut_row(ui, "Open Source File", &mut keybindings.open_file_source);
                ui_shortcut_row(ui, "Open Target File", &mut keybindings.open_file_target);
                ui_shortcut_row(ui, "Refresh Diff", &mut keybindings.refresh_diff);
                ui_shortcut_row(
                    ui,
                    "Refresh Diff Rows Only",
                    &mut keybindings.refresh_diff_rows_only,
                );
                ui_shortcut_row(
                    ui,
                    "Open Keybindings Options",
                    &mut keybindings.open_options_keybindings,
                );
                ui_shortcut_row(ui, "Open P4Config", &mut keybindings.open_universal_path);
                ui_shortcut_row(ui, "Find", &mut keybindings.find);
                ui_shortcut_row(ui, "Goto", &mut keybindings.goto);
                ui_shortcut_row(ui, "Next Conflict", &mut keybindings.next_conflict);
                ui_shortcut_row(ui, "Previous Conflict", &mut keybindings.prev_conflict);
                ui_shortcut_row(ui, "Next Find Result", &mut keybindings.next_find);
                ui_shortcut_row(ui, "Previous Find Result", &mut keybindings.prev_find);
                ui_shortcut_row(ui, "Revision Graph", &mut keybindings.revision_graph);
                ui_shortcut_row(ui, "Timelapse View", &mut keybindings.timelapse_view);
            });
    });
}
