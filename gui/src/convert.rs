//! Pure conversions from the logic crates' types to what the UI shows. Plain Rust structs come
//! first (testable without a window); the Slint structs are built at the edge.
use store_core::{App, Row, SourceId};

use crate::{CardData, ChipData, RowData};

/// What a card shows, before it becomes a Slint struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardView {
    pub key: String,
    pub title: String,
    pub subtitle: String,
    /// The source of the first variant (the host-ordered primary).
    pub source: String,
    pub installed: bool,
    /// Already darkened for white text, see [`readable_accent`].
    pub accent: (u8, u8, u8),
    pub icon_path: Option<String>,
}

pub fn card_view(app: &App) -> CardView {
    CardView {
        key: app.key.clone(),
        title: app.title.clone(),
        subtitle: app.description().unwrap_or_default().to_string(),
        source: app
            .variants
            .first()
            .map(|p| p.source.to_string())
            .unwrap_or_default(),
        installed: app.is_installed(),
        accent: readable_accent(store_app::accent(&app.title)),
        icon_path: app.icon().map(str::to_string),
    }
}

/// Highest relative luminance (WCAG, sRGB) a card accent may have: white text on it then has a
/// contrast of (1.0 + 0.05) / (0.30 + 0.05) = 3.0, the WCAG minimum for large text.
pub const MAX_ACCENT_LUMINANCE: f64 = 0.30;

fn to_linear(c: u8) -> f64 {
    let c = f64::from(c) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn from_linear(l: f64) -> u8 {
    let c = if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    (c * 255.0).round().clamp(0.0, 255.0) as u8
}

/// WCAG relative luminance of an sRGB color.
pub fn luminance((r, g, b): (u8, u8, u8)) -> f64 {
    0.2126 * to_linear(r) + 0.7152 * to_linear(g) + 0.0722 * to_linear(b)
}

/// Scales the color in linear light (keeping its hue) until its luminance is at most
/// [`MAX_ACCENT_LUMINANCE`]. Colors that are dark enough are unchanged.
pub fn readable_accent(color: (u8, u8, u8)) -> (u8, u8, u8) {
    let l = luminance(color);
    if l <= MAX_ACCENT_LUMINANCE {
        return color;
    }
    // Aim slightly under the limit so that rounding to 8 bits cannot cross it.
    let mut k = (MAX_ACCENT_LUMINANCE - 0.005) / l;
    loop {
        let scaled = (
            from_linear(to_linear(color.0) * k),
            from_linear(to_linear(color.1) * k),
            from_linear(to_linear(color.2) * k),
        );
        if luminance(scaled) <= MAX_ACCENT_LUMINANCE {
            return scaled;
        }
        k *= 0.97;
    }
}

/// The initial shown on a text card: the first letter or digit of `name` (else its first
/// non-blank character), uppercased; empty for an empty name.
pub fn initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .or_else(|| name.chars().find(|c| !c.is_whitespace()))
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default()
}

/// The 1-based day of the year for `days` since 1970-01-01 (UTC).
pub fn day_of_year(days: u64) -> u64 {
    // Civil-from-days (Howard Hinnant); `doy` counts from March 1st.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let after_feb = doy < 306;
    // The calendar year this day belongs to.
    let year = yoe + era * 400 + i64::from(!after_feb);
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let day = if after_feb {
        doy + 59 + i64::from(leap)
    } else {
        doy - 306
    };
    day as u64 + 1
}

pub struct ChipView {
    pub source: String,
    pub count: usize,
    pub selected: bool,
}

/// One chip per source, in the engine's order.
pub fn chip_views(chips: &[(SourceId, usize)], selected: Option<&SourceId>) -> Vec<ChipView> {
    chips
        .iter()
        .map(|(source, count)| ChipView {
            source: source.to_string(),
            count: *count,
            selected: selected == Some(source),
        })
        .collect()
}

pub fn chip_data(views: Vec<ChipView>) -> Vec<ChipData> {
    views
        .into_iter()
        .map(|v| ChipData {
            source: v.source.into(),
            count: v.count.min(i32::MAX as usize) as i32,
            selected: v.selected,
        })
        .collect()
}

pub fn card_data(view: &CardView, icon: Option<slint::Image>) -> CardData {
    let (r, g, b) = view.accent;
    CardData {
        key: view.key.as_str().into(),
        title: view.title.as_str().into(),
        subtitle: view.subtitle.as_str().into(),
        source: view.source.as_str().into(),
        has_icon: icon.is_some(),
        icon: icon.unwrap_or_default(),
        accent: slint::Color::from_rgb_u8(r, g, b),
        installed: view.installed,
    }
}

pub fn cards(apps: &[App], icon_of: &dyn Fn(&str) -> Option<slint::Image>) -> Vec<CardData> {
    apps.iter()
        .map(|app| {
            let view = card_view(app);
            let icon = view.icon_path.as_deref().and_then(icon_of);
            card_data(&view, icon)
        })
        .collect()
}

pub fn row_data(row: &Row, icon_of: &dyn Fn(&str) -> Option<slint::Image>) -> RowData {
    RowData {
        title: row.source.to_string().into(),
        cards: slint::ModelRc::new(slint::VecModel::from(cards(&row.apps, icon_of))),
    }
}

/// Straight-alpha RGBA8 to a Slint image. Call on the UI thread.
pub fn image_from_rgba(rgba: &store_app::Rgba) -> slint::Image {
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        &rgba.pixels,
        rgba.width,
        rgba.height,
    );
    slint::Image::from_rgba8(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use store_core::Package;

    fn pkg(source: &str, installed: bool) -> Package {
        Package {
            source: SourceId::new(source),
            name: "firefox".into(),
            version: "1".into(),
            installed_version: installed.then(|| "1".to_string()),
            title: None,
            description: None,
            repo: None,
            url: None,
            download_size: 0,
            installed_size: 0,
            app_id: None,
            categories: vec![],
            icon: None,
            popularity: None,
            upgradable: false,
        }
    }

    fn app(title: &str, variants: Vec<Package>) -> App {
        App {
            key: format!("k-{title}"),
            title: title.into(),
            variants,
        }
    }

    #[test]
    fn card_for_app_with_title_and_source_label() {
        let a = app("Firefox", vec![pkg("flatpak", false), pkg("pacman", false)]);
        let v = card_view(&a);
        assert_eq!(v.title, "Firefox");
        assert_eq!(v.source, "flatpak");
        assert_eq!(v.key, "k-Firefox");
    }

    #[test]
    fn card_without_icon_uses_accent_and_description() {
        let mut p = pkg("pacman", false);
        p.description = Some("Fast browser".into());
        let v = card_view(&app("Firefox", vec![p]));
        assert_eq!(v.icon_path, None);
        assert_eq!(v.subtitle, "Fast browser");
        assert_eq!(v.accent, readable_accent(store_app::accent("Firefox")));
        let d = card_data(&v, None);
        assert!(!d.has_icon);
        assert_eq!(d.subtitle, "Fast browser");
        let none = card_view(&app("X", vec![pkg("pacman", false)]));
        assert_eq!(none.subtitle, "");
    }

    #[test]
    fn card_marks_installed() {
        assert!(card_view(&app("A", vec![pkg("pacman", true)])).installed);
        assert!(!card_view(&app("A", vec![pkg("pacman", false)])).installed);
    }

    #[test]
    fn chip_labels_keep_engine_order() {
        let chips = vec![
            (SourceId::new("flatpak"), 3),
            (SourceId::new("pacman"), 20),
            (SourceId::new("aur"), 7),
        ];
        let v = chip_views(&chips, Some(&SourceId::new("pacman")));
        let labels: Vec<_> = v.iter().map(|c| c.source.as_str()).collect();
        assert_eq!(labels, ["flatpak", "pacman", "aur"]);
        let selected: Vec<_> = v.iter().map(|c| c.selected).collect();
        assert_eq!(selected, [false, true, false]);
        assert_eq!(v[1].count, 20);
    }

    #[test]
    fn accent_is_readable_with_white_text() {
        let mut names: Vec<String> = (0..2000).map(|n| format!("app-{n}")).collect();
        names.extend(["Firefox", "GIMP", "", "VLC", "Blender"].map(String::from));
        for name in &names {
            let a = readable_accent(store_app::accent(name));
            assert!(luminance(a) <= MAX_ACCENT_LUMINANCE, "{name}: {a:?}");
        }
        // Extreme inputs; dark colors stay unchanged.
        assert!(luminance(readable_accent((255, 255, 255))) <= MAX_ACCENT_LUMINANCE);
        assert!(luminance(readable_accent((200, 200, 200))) <= MAX_ACCENT_LUMINANCE);
        assert_eq!(readable_accent((60, 70, 80)), (60, 70, 80));
    }

    #[test]
    fn accent_keeps_the_hue_order() {
        let (r, g, b) = readable_accent((200, 120, 60));
        assert!(r > g && g > b);
    }

    #[test]
    fn initial_of_names() {
        assert_eq!(initial("firefox"), "F");
        assert_eq!(initial("  gimp"), "G");
        assert_eq!(initial("(Z) tool"), "Z");
        assert_eq!(initial("élan"), "É");
        assert_eq!(initial("!!"), "!");
        assert_eq!(initial(""), "");
        assert_eq!(initial("   "), "");
    }

    #[test]
    fn day_of_year_values() {
        assert_eq!(day_of_year(0), 1); // 1970-01-01
        assert_eq!(day_of_year(58), 59); // 1970-02-28
        assert_eq!(day_of_year(364), 365); // 1970-12-31
        assert_eq!(day_of_year(365), 1); // 1971-01-01
        assert_eq!(day_of_year(19_783), 61); // 2024-03-01 (leap year)
        assert_eq!(day_of_year(20_088), 366); // 2024-12-31
        assert_eq!(day_of_year(20_413), 325); // 2025-11-21
    }
}
