use eframe::egui::{self, Button, Key, Layout, Modifiers};

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

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Keybindings {
    pub open_file_source: Option<Shortcut>,
    pub open_file_target: Option<Shortcut>,
    pub refresh_diff_rows_only: Option<Shortcut>,
    pub refresh_diff: Option<Shortcut>, // Implies refresh_diff_rows_only
    pub open_options_keybindings: Option<Shortcut>,
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
                    shift: true,
                    ..Default::default()
                },
            }),
            refresh_diff_rows_only: Some(Shortcut {
                key: Key::R,
                modifiers: Modifiers {
                    ctrl: true,
                    command: true,
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
        }
    }
}

pub fn ui_keybindings(ui: &mut egui::Ui, keybindings: &mut Keybindings) {
    if ui.button("Reset to defaults").clicked() {
        *keybindings = Keybindings::default();
    }
    ui.separator();

    egui::ScrollArea::vertical().show(ui, |ui| {
        let mut ui_shortcut_row = |label: &str, shortcut: &mut Option<Shortcut>| {
            ui.horizontal(|ui| {
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

                // do not know how to make this work, can not create button with explicit id
                // ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(Button::new(button_text)).clicked() {
                    ui.memory_mut(|mem| mem.request_focus(id));
                }
                // });

                if is_listening {
                    let mut next_shortcut = None;
                    let mut should_surrender = false;

                    ui.input(|i| {
                        if i.key_pressed(Key::Escape) {
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
            });
        };

        ui_shortcut_row("Open Source File", &mut keybindings.open_file_source);
        ui_shortcut_row("Open Target File", &mut keybindings.open_file_target);
        ui_shortcut_row("Refresh Diff", &mut keybindings.refresh_diff);
        ui_shortcut_row(
            "Refresh Diff Rows Only",
            &mut keybindings.refresh_diff_rows_only,
        );
        ui_shortcut_row(
            "Open Keybindings Options",
            &mut keybindings.open_options_keybindings,
        );
    });
}
