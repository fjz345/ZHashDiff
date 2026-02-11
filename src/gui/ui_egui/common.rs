use eframe::egui;

pub fn hash_to_color(hash: &str) -> egui::Color32 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hash;

    let hue_digit = hash
        .chars()
        .next()
        .and_then(|c| c.to_digit(16))
        .unwrap_or(0) as f32;
    let shade_digit = hash
        .chars()
        .nth(1)
        .and_then(|c| c.to_digit(16))
        .unwrap_or(0) as f32;

    let mut hasher = DefaultHasher::new();
    hash.hash(&mut hasher);

    let hue = hue_digit / 16.0;

    let s_base = 0.4 + (shade_digit / 16.0) * 0.4;
    let v_base = 0.6 + (1.0 - (shade_digit / 16.0)) * 0.3;

    let saturation = (s_base).clamp(0.3, 0.95);
    let value = (v_base).clamp(0.4, 0.95);

    egui::Color32::from(egui::ecolor::Hsva::new(hue, saturation, value, 1.0))
}
