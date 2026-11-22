//! Development tool: renders the sample home screen to PNG files, headless.
//!
//! SLINT_BACKEND=headless cargo run -p linux-store --example snapshot --features slint/mcp -- OUT_DIR
//!
//! Writes OUT_DIR/{wide,narrow,tiny,search}.png.

use slint::Model;

#[path = "../src/sample.rs"]
mod sample;

slint::include_modules!();

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
    sample::fill(&ui);
    ui.show()?;
    for (name, w, h) in [
        ("wide", 1400, 850),
        ("narrow", 800, 700),
        ("tiny", 500, 500),
    ] {
        ui.window().set_size(slint::PhysicalSize::new(w, h));
        save(&ui, &format!("{dir}/{name}.png"))?;
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
