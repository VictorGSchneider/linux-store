//! Static sample data for the home screen (development only).

use slint::{Color, Image, ModelRc, Rgb8Pixel, SharedPixelBuffer, VecModel};

use crate::{AppWindow, CardData, ChipData, RowData};

fn icon(r: u8, g: u8, b: u8) -> Image {
    let mut buf = SharedPixelBuffer::<Rgb8Pixel>::new(64, 64);
    for (i, px) in buf.make_mut_slice().iter_mut().enumerate() {
        let (x, y) = ((i % 64) as u8, (i / 64) as u8);
        *px = Rgb8Pixel {
            r: r.saturating_add(y),
            g: g.saturating_add(x / 2),
            b,
        };
    }
    Image::from_rgb8(buf)
}

type Sample = (&'static str, &'static str, Option<(u8, u8, u8)>, bool);

fn card(sample: &Sample, source: &str, accent: Color) -> CardData {
    let (title, subtitle, tint, installed) = *sample;
    CardData {
        key: format!("{source}:{title}").into(),
        title: title.into(),
        subtitle: subtitle.into(),
        source: source.into(),
        icon: tint.map(|(r, g, b)| icon(r, g, b)).unwrap_or_default(),
        has_icon: tint.is_some(),
        accent,
        installed,
    }
}

const SAMPLES: [Sample; 12] = [
    (
        "Firefox",
        "Fast, private web browser",
        Some((200, 90, 30)),
        true,
    ),
    ("GIMP", "Image editor", None, false),
    (
        "A Very Long Application Name That Must Elide",
        "Does a lot of things, with a long description that wraps over many lines",
        None,
        false,
    ),
    ("Blender", "3D creation suite", Some((230, 120, 20)), false),
    ("VLC", "Media player", Some((240, 150, 0)), true),
    ("Inkscape", "Vector graphics", Some((40, 60, 80)), false),
    ("Krita", "Digital painting", None, false),
    ("Kdenlive", "Video editor", Some((30, 100, 160)), false),
    (
        "OBS Studio",
        "Streaming and recording",
        Some((60, 60, 70)),
        true,
    ),
    ("Audacity", "Audio editor", None, true),
    ("LibreOffice", "Office suite", Some((20, 140, 80)), false),
    ("Thunderbird", "Mail client", Some((40, 90, 180)), false),
];

fn row(title: &str, accent: Color) -> RowData {
    let cards: Vec<CardData> = SAMPLES.iter().map(|s| card(s, title, accent)).collect();
    RowData {
        title: title.into(),
        cards: ModelRc::new(VecModel::from(cards)),
    }
}

pub fn fill(ui: &AppWindow) {
    let chips = vec![
        ChipData {
            source: "pacman".into(),
            count: 14210,
            selected: true,
        },
        ChipData {
            source: "flatpak".into(),
            count: 2310,
            selected: false,
        },
        ChipData {
            source: "aur".into(),
            count: 90123,
            selected: false,
        },
    ];
    ui.set_chips(ModelRc::new(VecModel::from(chips)));
    let rows = vec![
        row("flatpak", Color::from_rgb_u8(0x3d, 0x6a, 0x8c)),
        row("pacman", Color::from_rgb_u8(0x7a, 0x4a, 0x8c)),
        row("aur", Color::from_rgb_u8(0x8c, 0x5a, 0x3d)),
    ];
    ui.set_rows(ModelRc::new(VecModel::from(rows)));
}
