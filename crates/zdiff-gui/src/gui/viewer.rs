//! Which viewer shows a diff. One module per viewer kind; Text is the existing diff pane.
//!
//! Selection order: the per-diff override, then the extension map, then sniffing.

use std::collections::BTreeMap;

use eframe::egui;
use zdiff::universal_path::UniversalPath;

pub mod hex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ViewerKind {
    Text,
    Hex,
}

impl ViewerKind {
    pub const ALL: [ViewerKind; 2] = [ViewerKind::Text, ViewerKind::Hex];

    pub fn name(self) -> &'static str {
        match self {
            ViewerKind::Text => "Text",
            ViewerKind::Hex => "Hex",
        }
    }
}

/// Interim rule until the text-encoding-eol decoder lands: a NUL byte or invalid UTF-8 means
/// Hex. An empty file is Text.
pub fn sniff_viewer_kind(bytes: &[u8]) -> ViewerKind {
    if bytes.contains(&0) || std::str::from_utf8(bytes).is_err() {
        ViewerKind::Hex
    } else {
        ViewerKind::Text
    }
}

/// Case-insensitive, leading dot ignored: `PNG`, `.png` and `png` are one entry.
pub fn normalize_extension(extension: &str) -> String {
    extension.trim().trim_start_matches('.').to_lowercase()
}

/// The extension of the path's file name, normalized. Depot paths carry the revision outside
/// the string, so `//depot/a.png#3` still gives `png`.
pub fn path_extension(path: &UniversalPath) -> Option<String> {
    let name = match path {
        UniversalPath::Local(path) => path.file_name()?.to_str()?,
        UniversalPath::Depot(path, _) => path.rsplit('/').next()?,
    };
    let (stem, extension) = name.rsplit_once('.')?;
    (!stem.is_empty() && !extension.is_empty()).then(|| normalize_extension(extension))
}

/// Persisted extension-to-viewer table. Keys are always normalized, also when loaded from a
/// hand-edited save.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(
        from = "BTreeMap<String, ViewerKind>",
        into = "BTreeMap<String, ViewerKind>"
    )
)]
pub struct ExtensionMap {
    entries: BTreeMap<String, ViewerKind>,
}

impl Default for ExtensionMap {
    fn default() -> Self {
        Self::defaults()
    }
}

impl From<BTreeMap<String, ViewerKind>> for ExtensionMap {
    fn from(entries: BTreeMap<String, ViewerKind>) -> Self {
        let mut map = Self {
            entries: BTreeMap::new(),
        };
        for (extension, kind) in entries {
            map.insert(&extension, kind);
        }
        map
    }
}

impl From<ExtensionMap> for BTreeMap<String, ViewerKind> {
    fn from(map: ExtensionMap) -> Self {
        map.entries
    }
}

impl ExtensionMap {
    /// Unknown extensions are sniffed, so the defaults only hold viewers sniffing can't pick.
    pub fn defaults() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    pub fn reset(&mut self) {
        *self = Self::defaults();
    }

    pub fn get(&self, extension: &str) -> Option<ViewerKind> {
        self.entries.get(&normalize_extension(extension)).copied()
    }

    /// Returns false, and changes nothing, for an extension that is empty once normalized.
    pub fn insert(&mut self, extension: &str, kind: ViewerKind) -> bool {
        let extension = normalize_extension(extension);
        if extension.is_empty() {
            return false;
        }
        self.entries.insert(extension, kind);
        true
    }

    pub fn remove(&mut self, extension: &str) {
        self.entries.remove(&normalize_extension(extension));
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, ViewerKind)> {
        self.entries.iter().map(|(ext, kind)| (ext.as_str(), *kind))
    }
}

/// The user's viewer choice for the shown pair; `None` is Auto.
#[derive(Debug, Default)]
pub struct ViewerOverride {
    pub kind: Option<ViewerKind>,
    pair: Option<(UniversalPath, UniversalPath)>,
}

impl ViewerOverride {
    /// Call before resolving. Any pair other than the one the override was made for resets it to
    /// Auto; the same pair swapped or reloaded keeps it.
    pub fn observe_pair(&mut self, path_1: &UniversalPath, path_2: &UniversalPath) {
        let same_pair = self
            .pair
            .as_ref()
            .is_some_and(|(a, b)| (a == path_1 && b == path_2) || (a == path_2 && b == path_1));
        if !same_pair {
            self.kind = None;
            self.pair = Some((path_1.clone(), path_2.clone()));
        }
    }
}

/// One loaded side: its display path (the extension comes from it, never from a temp copy) and
/// what its content sniffed as. Hex means binary, which has no text load.
#[derive(Debug, Clone, Copy)]
pub struct LoadedSide<'a> {
    pub path: &'a UniversalPath,
    pub sniffed: ViewerKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewerResolution {
    pub kind: ViewerKind,
    /// Why the pair isn't shown in the viewer it asked for; logged once per change.
    pub fallback: Option<String>,
}

/// The viewer for a pair, `None` while neither side is loaded.
pub fn resolve_viewer(
    override_kind: Option<ViewerKind>,
    map: &ExtensionMap,
    side_1: Option<LoadedSide>,
    side_2: Option<LoadedSide>,
) -> Option<ViewerResolution> {
    let resolve_side = |side: LoadedSide| {
        let wanted = override_kind
            .or_else(|| path_extension(side.path).and_then(|ext| map.get(&ext)))
            .unwrap_or(side.sniffed);
        // A binary load has no text to show.
        if wanted == ViewerKind::Text && side.sniffed == ViewerKind::Hex {
            let reason = format!("{} is binary and can't be shown as Text", side.path);
            ViewerResolution {
                kind: ViewerKind::Hex,
                fallback: Some(reason),
            }
        } else {
            ViewerResolution {
                kind: wanted,
                fallback: None,
            }
        }
    };

    match (side_1.map(resolve_side), side_2.map(resolve_side)) {
        (None, None) => None,
        (Some(only), None) | (None, Some(only)) => Some(only),
        (Some(first), Some(second)) if first.kind == second.kind => {
            let fallback = match (first.fallback, second.fallback) {
                (Some(a), Some(b)) => Some(format!("{}; {}", a, b)),
                (a, b) => a.or(b),
            };
            Some(ViewerResolution {
                kind: first.kind,
                fallback,
            })
        }
        (Some(first), Some(second)) => {
            let mismatch = format!(
                "The sides resolve to different viewers ({}, {}); showing Hex",
                first.kind.name(),
                second.kind.name()
            );
            let fallback = match first.fallback.or(second.fallback) {
                Some(reason) => format!("{}; {}", reason, mismatch),
                None => mismatch,
            };
            Some(ViewerResolution {
                kind: ViewerKind::Hex,
                fallback: Some(fallback),
            })
        }
    }
}

/// Settings editor for the extension map. The extension being added lives in egui's temp memory.
pub fn ui_extension_map(ui: &mut egui::Ui, map: &mut ExtensionMap) {
    if ui.button("Reset to defaults").clicked() {
        map.reset();
    }
    ui.label("Unmapped extensions are sniffed: text opens as Text, binary as Hex.");
    ui.separator();

    let new_id = ui.id().with("new_extension");
    let mut new_extension = ui.data_mut(|d| d.get_temp::<String>(new_id).unwrap_or_default());
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut new_extension)
                .desired_width(80.0)
                .hint_text("zbin"),
        );
        for kind in ViewerKind::ALL {
            if ui.button(format!("Add as {}", kind.name())).clicked()
                && map.insert(&new_extension, kind)
            {
                new_extension.clear();
            }
        }
    });
    ui.data_mut(|d| d.insert_temp(new_id, new_extension));
    ui.separator();

    let mut edits = Vec::new();
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("extension_map_grid")
            .num_columns(3)
            .spacing([20.0, 8.0])
            .show(ui, |ui| {
                for (extension, kind) in map.iter() {
                    ui.label(format!(".{}", extension));
                    let mut new_kind = Some(kind);
                    egui::ComboBox::from_id_salt(("extension_map_kind", extension))
                        .selected_text(kind.name())
                        .show_ui(ui, |ui| {
                            for option in ViewerKind::ALL {
                                ui.selectable_value(&mut new_kind, Some(option), option.name());
                            }
                        });
                    if ui.button("Remove").clicked() {
                        new_kind = None;
                    }
                    if new_kind != Some(kind) {
                        edits.push((extension.to_owned(), new_kind));
                    }
                    ui.end_row();
                }
            });
    });
    for (extension, kind) in edits {
        match kind {
            Some(kind) => {
                map.insert(&extension, kind);
            }
            None => map.remove(&extension),
        }
    }
}

/// Stops for the shared conflict cursor: the shown viewer's. The other viewer's state is stale.
pub fn conflict_count(kind: Option<ViewerKind>, text: usize, hex: usize) -> usize {
    match kind {
        Some(ViewerKind::Hex) => hex,
        Some(ViewerKind::Text) | None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_navigation_counts_the_shown_viewers_stops() {
        // The conflict shortcuts drive one shared cursor; its max must come from the viewer on
        // screen, because the other viewer's state is stale.
        assert_eq!(conflict_count(Some(ViewerKind::Text), 7, 3), 7);
        assert_eq!(conflict_count(Some(ViewerKind::Hex), 7, 3), 3);
        assert_eq!(conflict_count(None, 7, 3), 7);
    }

    fn side(path: &UniversalPath, sniffed: ViewerKind) -> Option<LoadedSide<'_>> {
        Some(LoadedSide { path, sniffed })
    }

    fn kind(resolution: Option<ViewerResolution>) -> Option<ViewerKind> {
        resolution.map(|r| r.kind)
    }

    fn zbin_hex_map() -> ExtensionMap {
        let mut map = ExtensionMap::defaults();
        assert!(map.insert("zbin", ViewerKind::Hex));
        map
    }

    #[test]
    fn override_beats_map_beats_sniff() {
        let a = UniversalPath::from("a.zbin");
        let b = UniversalPath::from("b.zbin");
        let (text, hex) = (ViewerKind::Text, ViewerKind::Hex);
        let map = zbin_hex_map();
        let none = ExtensionMap::defaults();

        // Sniff alone.
        let sniffed = |s1, s2| kind(resolve_viewer(None, &none, side(&a, s1), side(&b, s2)));
        assert_eq!(sniffed(text, text), Some(text));
        assert_eq!(sniffed(hex, hex), Some(hex));

        // The map beats the sniff: text content mapped to Hex opens in Hex.
        assert_eq!(
            kind(resolve_viewer(None, &map, side(&a, text), side(&b, text))),
            Some(hex)
        );

        // The override beats the map, in both directions.
        assert_eq!(
            kind(resolve_viewer(
                Some(text),
                &map,
                side(&a, text),
                side(&b, text)
            )),
            Some(text)
        );
        assert_eq!(
            kind(resolve_viewer(
                Some(hex),
                &none,
                side(&a, text),
                side(&b, text)
            )),
            Some(hex)
        );

        // Nothing asked for differs from what was shown: no fallback message.
        assert_eq!(
            resolve_viewer(Some(text), &map, side(&a, text), side(&b, text))
                .unwrap()
                .fallback,
            None
        );
    }

    #[test]
    fn nothing_loaded_resolves_to_no_viewer_and_one_side_resolves_alone() {
        let a = UniversalPath::from("a.zbin");
        let map = zbin_hex_map();
        assert_eq!(resolve_viewer(None, &map, None, None), None);
        assert_eq!(
            kind(resolve_viewer(None, &map, None, side(&a, ViewerKind::Text))),
            Some(ViewerKind::Hex)
        );
        let txt = UniversalPath::from("a.txt");
        assert_eq!(
            kind(resolve_viewer(
                None,
                &map,
                side(&txt, ViewerKind::Text),
                None
            )),
            Some(ViewerKind::Text)
        );
    }

    #[test]
    fn extensions_normalize_case_and_leading_dot() {
        for spelling in ["PNG", ".png", "png", ".PnG"] {
            assert_eq!(normalize_extension(spelling), "png", "{spelling}");
        }

        let mut map = ExtensionMap::defaults();
        assert!(map.insert("PNG", ViewerKind::Hex));
        assert!(map.insert(".png", ViewerKind::Hex));
        assert_eq!(map.iter().count(), 1, "one entry for all spellings");
        for spelling in ["PNG", ".png", "png"] {
            assert_eq!(map.get(spelling), Some(ViewerKind::Hex), "{spelling}");
        }
        map.remove(".Png");
        assert_eq!(map.get("png"), None);

        assert!(!map.insert(".", ViewerKind::Hex), "empty extension");
        assert_eq!(map.iter().count(), 0);
    }

    #[test]
    fn path_extension_comes_from_the_file_name_of_local_and_depot_paths() {
        assert_eq!(
            path_extension(&UniversalPath::from(r"C:\dir.d\Image.PNG")),
            Some("png".into())
        );
        assert_eq!(
            path_extension(&UniversalPath::Depot(
                "//depot/a.b/Image.Png".into(),
                Some(5)
            )),
            Some("png".into())
        );
        assert_eq!(
            path_extension(&UniversalPath::from(r"C:\dir.d\Makefile")),
            None
        );
        assert_eq!(
            path_extension(&UniversalPath::from(r"C:\dir\.gitignore")),
            None
        );
        assert_eq!(
            path_extension(&UniversalPath::Depot("//depot/dir.d/Makefile".into(), None)),
            None
        );
    }

    #[test]
    fn map_lookup_uses_the_paths_extension_in_any_case() {
        let map = zbin_hex_map();
        let upper = UniversalPath::from(r"C:\x\DATA.ZBIN");
        let depot = UniversalPath::Depot("//depot/x/data.Zbin".into(), Some(3));
        assert_eq!(
            kind(resolve_viewer(
                None,
                &map,
                side(&upper, ViewerKind::Text),
                side(&depot, ViewerKind::Text)
            )),
            Some(ViewerKind::Hex)
        );
    }

    #[test]
    fn sides_resolving_to_different_viewers_fall_back_to_hex_with_a_reason() {
        let a = UniversalPath::from("a.zbin");
        let b = UniversalPath::from("b.txt");
        let resolution = resolve_viewer(
            None,
            &zbin_hex_map(),
            side(&a, ViewerKind::Text),
            side(&b, ViewerKind::Text),
        )
        .unwrap();
        assert_eq!(resolution.kind, ViewerKind::Hex);
        assert!(resolution.fallback.is_some());

        // Sniffed text against sniffed binary is a mismatch too.
        let resolution = resolve_viewer(
            None,
            &ExtensionMap::defaults(),
            side(&b, ViewerKind::Text),
            side(&a, ViewerKind::Hex),
        )
        .unwrap();
        assert_eq!(resolution.kind, ViewerKind::Hex);
        assert!(resolution.fallback.is_some());
    }

    #[test]
    fn forced_text_on_binary_content_falls_back_to_hex_with_a_reason() {
        // NUL bytes without a BOM: the loader's binary verdict.
        assert_eq!(sniff_viewer_kind(b"ab\0cd"), ViewerKind::Hex);

        let a = UniversalPath::from("a.dat");
        let b = UniversalPath::from("b.dat");
        let (text, hex) = (ViewerKind::Text, ViewerKind::Hex);

        // By the override.
        let resolution = resolve_viewer(
            Some(text),
            &ExtensionMap::defaults(),
            side(&a, hex),
            side(&b, hex),
        )
        .unwrap();
        assert_eq!(resolution.kind, hex);
        assert!(resolution.fallback.is_some());

        // By the map, with only one side binary.
        let mut map = ExtensionMap::defaults();
        map.insert("dat", text);
        let resolution = resolve_viewer(None, &map, side(&a, text), side(&b, hex)).unwrap();
        assert_eq!(resolution.kind, hex);
        assert!(resolution.fallback.is_some());

        // A one-sided diff falls back too.
        let resolution = resolve_viewer(Some(text), &map, None, side(&b, hex)).unwrap();
        assert_eq!(resolution.kind, hex);
        assert!(resolution.fallback.is_some());
    }

    #[test]
    fn reset_restores_the_defaults() {
        let mut map = ExtensionMap::defaults();
        map.insert("zbin", ViewerKind::Hex);
        map.insert("txt", ViewerKind::Text);
        assert_ne!(map, ExtensionMap::defaults());
        map.reset();
        assert_eq!(map, ExtensionMap::defaults());
        assert_eq!(ExtensionMap::default(), ExtensionMap::defaults());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn map_survives_a_save_and_loads_normalized() {
        let map = zbin_hex_map();
        let json = serde_json::to_string(&map).unwrap();
        assert_eq!(serde_json::from_str::<ExtensionMap>(&json).unwrap(), map);

        // A hand-edited save still finds its entries.
        let edited: ExtensionMap = serde_json::from_str(r#"{".ZBIN":"Hex"}"#).unwrap();
        assert_eq!(edited, map);
    }

    #[test]
    fn override_resets_when_a_new_pair_is_opened() {
        let a = UniversalPath::from("a.txt");
        let b = UniversalPath::from("b.txt");
        let c = UniversalPath::from("c.txt");

        let mut viewer_override = ViewerOverride::default();
        viewer_override.observe_pair(&a, &b);
        viewer_override.kind = Some(ViewerKind::Hex);

        // The same pair, every frame or after a reload, keeps it; swapping sides too.
        viewer_override.observe_pair(&a, &b);
        assert_eq!(viewer_override.kind, Some(ViewerKind::Hex));
        viewer_override.observe_pair(&b, &a);
        assert_eq!(viewer_override.kind, Some(ViewerKind::Hex));

        // Another file on either side is a new pair.
        viewer_override.observe_pair(&a, &c);
        assert_eq!(viewer_override.kind, None);

        // And the new pair keeps its own override.
        viewer_override.kind = Some(ViewerKind::Text);
        viewer_override.observe_pair(&a, &c);
        assert_eq!(viewer_override.kind, Some(ViewerKind::Text));
    }
}
