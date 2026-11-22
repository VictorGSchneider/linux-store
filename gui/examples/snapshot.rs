//! Development tool: renders the sample home screen to PNG files, headless.
//!
//! SLINT_BACKEND=headless cargo run -p linux-store --example snapshot --features slint/mcp -- OUT_DIR
//!
//! Writes OUT_DIR/{wide,narrow,tiny,search}.png.

use slint::Model;

slint::include_modules!();

fn card(title: &str, subtitle: &str, source: &str, tint: Option<u8>, installed: bool) -> CardData {
    let icon = tint.map(|t| {
        let mut buf = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(64, 64);
        for (i, px) in buf.make_mut_slice().iter_mut().enumerate() {
            *px = slint::Rgb8Pixel {
                r: t.saturating_add((i / 64) as u8),
                g: 90,
                b: 255 - t,
            };
        }
        slint::Image::from_rgb8(buf)
    });
    CardData {
        key: format!("{source}:{title}").into(),
        title: title.into(),
        subtitle: subtitle.into(),
        source: source.into(),
        has_icon: icon.is_some(),
        icon: icon.unwrap_or_default(),
        accent: slint::Color::from_rgb_u8(0x3d, 0x6a, 0x8c),
        installed,
    }
}

fn fill(ui: &AppWindow) {
    let samples = [
        ("Firefox", "Fast web browser", Some(200), true),
        ("GIMP", "Image editor", None, false),
        (
            "A Very Long Application Name That Must Elide",
            "Does a lot of things, with a long description that wraps over many lines",
            None,
            false,
        ),
        ("Blender", "3D creation suite", Some(120), false),
        ("VLC", "Media player", Some(150), true),
        ("Inkscape", "Vector graphics", Some(40), false),
        ("Krita", "Digital painting", None, false),
        ("Kdenlive", "Video editor", Some(30), false),
        ("OBS Studio", "Streaming", Some(60), true),
        ("Audacity", "Audio editor", None, true),
    ];
    let chips: Vec<ChipData> = [
        ("pacman", 14210, true, false),
        ("flatpak", 1, false, false),
        ("aur", 90123, false, true),
    ]
    .into_iter()
    .map(|(source, count, selected, updating)| ChipData {
        source: source.into(),
        count,
        count_text: linux_store::convert::group_thousands(count as usize).into(),
        selected,
        updating,
    })
    .collect();
    ui.set_chips(slint::ModelRc::new(slint::VecModel::from(chips)));
    let rows: Vec<RowData> = ["flatpak", "pacman", "aur"]
        .into_iter()
        .map(|source| RowData {
            title: source.into(),
            cards: slint::ModelRc::new(slint::VecModel::from(
                samples
                    .iter()
                    .map(|(t, s, tint, inst)| card(t, s, source, *tint, *inst))
                    .collect::<Vec<_>>(),
            )),
        })
        .collect();
    ui.set_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::args().nth(1).expect("output directory");
    let ui = AppWindow::new()?;
    ui.global::<TextUtil>().on_initial(|name| {
        name.chars()
            .find(|c| !c.is_whitespace())
            .map(|c| c.to_uppercase().collect::<String>())
            .unwrap_or_default()
            .into()
    });
    fill(&ui);
    ui.show()?;
    for (name, w, h) in [
        ("wide", 1400, 850),
        ("narrow", 800, 700),
        ("tiny", 500, 500),
    ] {
        ui.window().set_size(slint::PhysicalSize::new(w, h));
        save(&ui, &format!("{dir}/{name}.png"))?;
    }
    // Tab through the focusable elements and snapshot some of the stops.
    ui.window().set_size(slint::PhysicalSize::new(1000, 700));
    for n in 1..=9 {
        let tab: slint::SharedString = slint::platform::Key::Tab.into();
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::KeyPressed { text: tab.clone() });
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::KeyReleased { text: tab });
        if [1, 4, 5, 8, 9].contains(&n) {
            save(&ui, &format!("{dir}/focus{n}.png"))?;
        }
    }
    ui.set_query("fir".into());
    ui.set_results(ui.get_rows().row_data(0).unwrap().cards);
    ui.window().set_size(slint::PhysicalSize::new(1000, 700));
    save(&ui, &format!("{dir}/search.png"))?;
    Ok(())
}

fn save(ui: &AppWindow, path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let img = ui.window().take_snapshot()?;
    let mut enc = png::Encoder::new(std::fs::File::create(path)?, img.width(), img.height());
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(img.as_bytes())?;
    Ok(())
}
