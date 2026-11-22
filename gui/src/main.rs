// Prevent console window in addition to Slint window in Windows release builds when, e.g., starting the app via file manager. Ignored on other platforms.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::error::Error;

#[cfg(debug_assertions)]
mod sample;

slint::include_modules!();

/// The first character of a name, uppercased, for text cards.
fn initial(name: &str) -> String {
    name.chars()
        .find(|c| !c.is_whitespace())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default()
}

fn main() -> Result<(), Box<dyn Error>> {
    let ui = AppWindow::new()?;
    ui.global::<TextUtil>()
        .on_initial(|name| initial(name.as_str()).into());

    // Sample data to see the layout; removed when the window is wired to the engine.
    #[cfg(debug_assertions)]
    sample::fill(&ui);

    ui.run()?;

    Ok(())
}
