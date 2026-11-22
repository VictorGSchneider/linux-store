//! Pure conversions from the logic crates' types to what the UI shows. Plain Rust structs come
//! first (testable without a window); the Slint structs are built at the edge.
use store_core::{App, Row, SourceId};

use crate::state::Place;
use crate::{AppPageData, CardData, ChipData, EmptyKind, RowData, SourceButtonData};

/// What a card shows, before it becomes a Slint struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardView {
    pub key: String,
    pub title: String,
    pub subtitle: String,
    /// The source label: the row's source in a source row, else the first variant's (the
    /// host-ordered primary).
    pub source: String,
    pub installed: bool,
    /// Already darkened for white text, see [`readable_accent`].
    pub accent: (u8, u8, u8),
    pub icon_path: Option<String>,
}

/// Longest card title, in characters.
pub const MAX_CARD_TITLE_CHARS: usize = 200;
/// Longest card subtitle (the description on a text card), in characters.
pub const MAX_CARD_SUBTITLE_CHARS: usize = 300;

/// A card for `app` labelled with its primary source (search results, Installeds, Updates).
pub fn card_view(app: &App) -> CardView {
    card_view_labelled(app, None)
}

/// A card for `app` in the row of `row_source`: the label is the row's source, which the app
/// has a variant in, even when another source comes first in the host order.
pub fn card_view_in_row(app: &App, row_source: &SourceId) -> CardView {
    card_view_labelled(app, Some(row_source))
}

fn card_view_labelled(app: &App, label: Option<&SourceId>) -> CardView {
    CardView {
        key: app.key.clone(),
        title: sanitize_text(&app.title, MAX_CARD_TITLE_CHARS),
        subtitle: sanitize_text(
            app.description().unwrap_or_default(),
            MAX_CARD_SUBTITLE_CHARS,
        ),
        source: label
            .or_else(|| app.variants.first().map(|p| &p.source))
            .map(SourceId::to_string)
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
    /// `count` with thousands separators, for the plural-aware text in `chip.slint`.
    pub count_text: String,
    pub selected: bool,
    /// The source is being refreshed right now.
    pub updating: bool,
}

/// One chip per source, in the engine's order. `updating` tells which sources are refreshing.
pub fn chip_views(
    chips: &[(SourceId, usize)],
    selected: Option<&SourceId>,
    updating: &dyn Fn(&SourceId) -> bool,
) -> Vec<ChipView> {
    chips
        .iter()
        .map(|(source, count)| ChipView {
            source: source.to_string(),
            count: *count,
            count_text: group_thousands(*count),
            selected: selected == Some(source),
            updating: updating(source),
        })
        .collect()
}

pub fn chip_data(views: Vec<ChipView>) -> Vec<ChipData> {
    views
        .into_iter()
        .map(|v| ChipData {
            source: v.source.into(),
            count: v.count.min(i32::MAX as usize) as i32,
            count_text: v.count_text.into(),
            selected: v.selected,
            updating: v.updating,
        })
        .collect()
}

/// `n` with a comma every three digits ("125,813"). The interface is English for now.
pub fn group_thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
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
    let cards: Vec<CardData> = row
        .apps
        .iter()
        .map(|app| {
            let view = card_view_in_row(app, &row.source);
            let icon = view.icon_path.as_deref().and_then(icon_of);
            card_data(&view, icon)
        })
        .collect();
    RowData {
        title: row.source.to_string().into(),
        cards: slint::ModelRc::new(slint::VecModel::from(cards)),
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

/// Longest message from the helper or catalog that is shown, in characters.
pub const MAX_MESSAGE_CHARS: usize = 300;
/// Longest app description shown on the page, in characters.
pub const MAX_DESCRIPTION_CHARS: usize = 4000;
/// Longest title shown on the page, in characters.
pub const MAX_TITLE_CHARS: usize = 200;
/// Longest homepage URL offered as a link, in characters. A longer one is not offered at all:
/// cutting it would open a different address.
pub const MAX_URL_CHARS: usize = 2000;
/// Why a source's button is disabled.
pub const NO_INSTALLER_REASON: &str = "no installer for this source yet";

/// Untrusted text for display: control characters become spaces (so they cannot break lines or
/// move the cursor), the result is trimmed and cut at `max` characters with an ellipsis. The UI
/// shows it as plain text, never as markup.
pub fn sanitize_text(text: &str, max: usize) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.chars().count() <= max {
        return cleaned.to_string();
    }
    let mut cut: String = cleaned.chars().take(max.saturating_sub(1)).collect();
    cut.truncate(cut.trim_end().len());
    cut.push('…');
    cut
}

/// A byte count for people: `—` for zero (unknown), else KiB/MiB/GiB with one decimal.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    if bytes == 0 {
        return "—".into();
    }
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// Whether `url` may be handed to `xdg-open`: an allow-list of `http://` and `https://` with a
/// non-empty host and no whitespace or control characters anywhere. Everything else (other
/// schemes, `file:`, `javascript:`, malformed text) is refused.
pub fn is_openable_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let Some(rest) = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
    else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    !host.is_empty()
        && !host.starts_with(['@', ':'])
        && !url.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// What one source button shows, before it becomes a Slint struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceButton {
    pub source: String,
    /// The package the button acts on (the variant's name), sent with the click so the engine
    /// can refuse when the index has changed since.
    pub package: String,
    /// The installed version when installed, else the newest one.
    pub version: String,
    pub installed: bool,
    pub enabled: bool,
    pub reason: String,
}

/// One button per variant, in the app's (host) order. A source the installer cannot act on is
/// disabled with a reason.
pub fn source_buttons(app: &App, installable: &[SourceId]) -> Vec<SourceButton> {
    app.variants
        .iter()
        .map(|p| {
            let enabled = installable.contains(&p.source);
            SourceButton {
                source: p.source.to_string(),
                package: p.name.clone(),
                version: sanitize_text(
                    p.installed_version.as_deref().unwrap_or(&p.version),
                    MAX_TITLE_CHARS,
                ),
                installed: p.is_installed(),
                enabled,
                reason: if enabled {
                    String::new()
                } else {
                    NO_INSTALLER_REASON.to_string()
                },
            }
        })
        .collect()
}

/// What the app page shows, before it becomes a Slint struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageView {
    pub key: String,
    pub title: String,
    pub description: String,
    pub size: String,
    /// The first `url` among the variants that may be opened (see [`homepage_of`]), else empty.
    pub homepage: String,
    pub accent: (u8, u8, u8),
    pub icon_path: Option<String>,
    pub sources: Vec<SourceButton>,
}

pub fn page_view(view: &store_app::AppView) -> PageView {
    let app = &view.app;
    let primary = app.variants.first();
    let bytes = primary
        .map(|p| {
            if p.installed_size > 0 {
                p.installed_size
            } else {
                p.download_size
            }
        })
        .unwrap_or(0);
    PageView {
        key: app.key.clone(),
        title: sanitize_text(&app.title, MAX_TITLE_CHARS),
        description: sanitize_text(app.description().unwrap_or_default(), MAX_DESCRIPTION_CHARS),
        size: human_size(bytes),
        homepage: homepage_of(app),
        accent: readable_accent(store_app::accent(&app.title)),
        icon_path: app.icon().map(str::to_string),
        sources: source_buttons(app, &view.installable),
    }
}

/// The link the page offers: the first variant `url` that is openable as it is and at most
/// [`MAX_URL_CHARS`] long. It is never cut or rewritten, so the link opens what it shows.
pub fn homepage_of(app: &App) -> String {
    app.variants
        .iter()
        .filter_map(|p| p.url.as_deref())
        .find(|u| u.chars().count() <= MAX_URL_CHARS && is_openable_url(u))
        .unwrap_or_default()
        .to_string()
}

pub fn page_data(view: &PageView, icon: Option<slint::Image>) -> AppPageData {
    let (r, g, b) = view.accent;
    AppPageData {
        title: view.title.as_str().into(),
        description: view.description.as_str().into(),
        has_icon: icon.is_some(),
        icon: icon.unwrap_or_default(),
        accent: slint::Color::from_rgb_u8(r, g, b),
        size: view.size.as_str().into(),
        homepage: view.homepage.as_str().into(),
        sources: slint::ModelRc::new(slint::VecModel::from(
            view.sources
                .iter()
                .map(|s| SourceButtonData {
                    source: s.source.as_str().into(),
                    version: s.version.as_str().into(),
                    installed: s.installed,
                    enabled: s.enabled,
                    // `app-page.slint` translates the known reason with `@tr`.
                    reason: s.reason.as_str().into(),
                })
                .collect::<Vec<_>>(),
        )),
    }
}

/// The item count next to a list page's title: "(12)", "(1,234)". The title itself is
/// translated in the UI.
pub fn count_text(count: usize) -> String {
    format!("({})", group_thousands(count))
}

/// Which explanation a list page shows instead of its (empty) grid.
pub fn empty_kind(place: Place, count: usize) -> EmptyKind {
    match (place, count) {
        (_, n) if n > 0 => EmptyKind::None,
        (Place::Installed, _) => EmptyKind::NothingInstalled,
        (Place::Updates, _) => EmptyKind::UpToDate,
        (Place::Home, _) => EmptyKind::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use store_core::Package;

    #[test]
    fn count_text_formats_zero_one_and_many() {
        assert_eq!(count_text(0), "(0)");
        assert_eq!(count_text(1), "(1)");
        assert_eq!(count_text(1234), "(1,234)");
    }

    #[test]
    fn thousands_are_grouped() {
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(1), "1");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(1000), "1,000");
        assert_eq!(group_thousands(16609), "16,609");
        assert_eq!(group_thousands(125_813), "125,813");
        assert_eq!(group_thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn chips_carry_grouped_counts_and_the_updating_state() {
        let chips = vec![(SourceId::new("pacman"), 16609), (SourceId::new("aur"), 1)];
        let v = chip_views(&chips, None, &|s| s.as_str() == "aur");
        assert_eq!(v[0].count_text, "16,609");
        assert!(!v[0].updating);
        assert_eq!(v[1].count_text, "1");
        assert!(v[1].updating);
    }

    #[test]
    fn card_text_is_sanitised_and_capped() {
        let mut p = pkg("pacman", false);
        p.description = Some(format!("evil\u{1b}[2J\n{}", "d".repeat(1000)));
        let a = app(&format!("T\u{7}itle{}", "t".repeat(500)), vec![p]);
        let v = card_view(&a);
        assert!(!v.title.chars().any(char::is_control));
        assert!(!v.subtitle.chars().any(char::is_control));
        assert_eq!(v.title.chars().count(), MAX_CARD_TITLE_CHARS);
        assert_eq!(v.subtitle.chars().count(), MAX_CARD_SUBTITLE_CHARS);
        assert!(v.title.starts_with("T itle"));
    }

    #[test]
    fn a_card_in_a_source_row_is_labelled_with_the_row_source() {
        // Host order puts pacman first, but the card sits in the flatpak row.
        let a = app("Weather", vec![pkg("pacman", false), pkg("flatpak", false)]);
        assert_eq!(
            card_view_in_row(&a, &SourceId::new("flatpak")).source,
            "flatpak"
        );
        assert_eq!(
            card_view_in_row(&a, &SourceId::new("pacman")).source,
            "pacman"
        );
        // Elsewhere (search, lists) the primary source labels it.
        assert_eq!(card_view(&a).source, "pacman");
    }

    #[test]
    fn source_buttons_carry_the_package_they_act_on() {
        let mut flat = pkg("flatpak", false);
        flat.name = "org.mozilla.firefox".into();
        let a = app("Firefox", vec![pkg("pacman", false), flat]);
        let b = source_buttons(&a, &[SourceId::new("pacman")]);
        assert_eq!(b[0].package, "firefox");
        assert_eq!(b[1].package, "org.mozilla.firefox");
    }

    #[test]
    fn an_over_long_or_unopenable_homepage_is_not_offered() {
        let long = format!("https://example.org/{}", "a".repeat(MAX_URL_CHARS));
        let mut a = pkg("pacman", false);
        a.url = Some(long);
        let mut b = pkg("flatpak", false);
        b.url = Some("https://example.org/ok".into());
        // The long one is skipped, not cut: the next openable one is offered.
        let v = page_view(&view(app("X", vec![a.clone(), b]), &[]));
        assert_eq!(v.homepage, "https://example.org/ok");
        let only_long = page_view(&view(app("X", vec![a]), &[]));
        assert_eq!(only_long.homepage, "");
        let mut c = pkg("pacman", false);
        c.url = Some("javascript:alert(1)".into());
        assert_eq!(page_view(&view(app("X", vec![c]), &[])).homepage, "");
        // Exactly at the limit is still offered, unchanged.
        let exact = format!("https://e.org/{}", "b".repeat(MAX_URL_CHARS - 14));
        assert_eq!(exact.chars().count(), MAX_URL_CHARS);
        let mut d = pkg("pacman", false);
        d.url = Some(exact.clone());
        assert_eq!(page_view(&view(app("X", vec![d]), &[])).homepage, exact);
    }

    #[test]
    fn empty_state_depends_on_the_page_and_the_count() {
        assert_eq!(empty_kind(Place::Installed, 0), EmptyKind::NothingInstalled);
        assert_eq!(empty_kind(Place::Updates, 0), EmptyKind::UpToDate);
        assert_eq!(empty_kind(Place::Installed, 1), EmptyKind::None);
        assert_eq!(empty_kind(Place::Updates, 7), EmptyKind::None);
        assert_eq!(empty_kind(Place::Home, 0), EmptyKind::None);
    }

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
        let v = chip_views(&chips, Some(&SourceId::new("pacman")), &|_| false);
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

    fn view(app: App, installable: &[&str]) -> store_app::AppView {
        store_app::AppView {
            app,
            installable: installable.iter().map(|s| SourceId::new(*s)).collect(),
        }
    }

    #[test]
    fn source_buttons_follow_variant_order() {
        let a = app(
            "Firefox",
            vec![
                pkg("pacman", false),
                pkg("flatpak", false),
                pkg("aur", false),
            ],
        );
        let buttons = source_buttons(&a, &[SourceId::new("pacman")]);
        let order: Vec<_> = buttons.iter().map(|b| b.source.as_str()).collect();
        assert_eq!(order, ["pacman", "flatpak", "aur"]);
        assert_eq!(buttons[0].version, "1");
    }

    #[test]
    fn non_installable_source_is_disabled_with_a_reason() {
        let a = app("Firefox", vec![pkg("pacman", false), pkg("flatpak", false)]);
        let b = source_buttons(&a, &[SourceId::new("pacman")]);
        assert!(b[0].enabled);
        assert_eq!(b[0].reason, "");
        assert!(!b[1].enabled);
        assert_eq!(b[1].reason, "no installer for this source yet");
    }

    #[test]
    fn installed_variant_offers_remove() {
        let mut p = pkg("pacman", true);
        p.version = "2".into();
        p.installed_version = Some("1".into());
        let a = app("Firefox", vec![p, pkg("flatpak", false)]);
        let b = source_buttons(&a, &[SourceId::new("pacman"), SourceId::new("flatpak")]);
        assert!(b[0].installed);
        assert_eq!(b[0].version, "1", "shows the installed version");
        assert!(!b[1].installed);
        assert_eq!(b[1].version, "1");
    }

    #[test]
    fn size_is_human_readable() {
        assert_eq!(human_size(0), "—");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KiB");
        assert_eq!(human_size(5 * 1024 * 1024 + 512 * 1024), "5.5 MiB");
        assert_eq!(human_size(3 * 1024 * 1024 * 1024), "3.0 GiB");
        assert_eq!(human_size(u64::MAX), "17179869184.0 GiB");
    }

    #[test]
    fn page_view_uses_first_url_and_installed_size() {
        let mut a = pkg("pacman", false);
        a.installed_size = 1536;
        let mut b = pkg("flatpak", false);
        b.url = Some("https://example.org".into());
        let v = page_view(&view(app("X", vec![a, b]), &["pacman"]));
        assert_eq!(v.size, "1.5 KiB");
        assert_eq!(v.homepage, "https://example.org");
        let empty = page_view(&view(app("X", vec![pkg("pacman", false)]), &[]));
        assert_eq!(empty.size, "—");
        assert_eq!(empty.homepage, "");
        assert_eq!(empty.description, "");
    }

    #[test]
    fn only_http_urls_open() {
        assert!(is_openable_url("https://example.org/a?b=c#d"));
        assert!(is_openable_url("http://example.org"));
        assert!(is_openable_url("HTTPS://Example.org"));
        for bad in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "ftp://example.org",
            "",
            "https://",
            "http:///path",
            "https://exa mple.org",
            "https://example.org/a b",
            "https://example.org/\nrm -rf",
            "https://example.org/\u{7}",
            " https://example.org",
            "https://@",
            "example.org",
            "mailto:a@b.c",
        ] {
            assert!(!is_openable_url(bad), "{bad:?} must be refused");
        }
    }

    #[test]
    fn sanitize_strips_control_characters_and_caps_length() {
        assert_eq!(sanitize_text("a\u{1b}[31mb\nc\td\0", 300), "a [31mb c d");
        assert_eq!(sanitize_text("  hi  ", 300), "hi");
        let long = "x".repeat(1000);
        let s = sanitize_text(&long, MAX_MESSAGE_CHARS);
        assert_eq!(s.chars().count(), MAX_MESSAGE_CHARS);
        assert!(s.ends_with('…'));
        // multi-byte characters are cut on character boundaries
        let wide = "é".repeat(500);
        assert_eq!(sanitize_text(&wide, 10).chars().count(), 10);
        assert_eq!(sanitize_text(&"y".repeat(300), 300).chars().count(), 300);
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
