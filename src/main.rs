#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod decode;
mod source;
mod worker;

use std::path::PathBuf;

fn main() -> eframe::Result<()> {
    // A folder (or a single archive/image) may be given on the command line;
    // otherwise ask the user to pick a folder.
    let path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .or_else(|| {
            rfd::FileDialog::new()
                .set_title("LiteView — choose a folder of images")
                .pick_folder()
        });
    let Some(path) = path else {
        return Ok(());
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("LiteView")
            .with_fullscreen(true)
            .with_min_inner_size([480.0, 320.0]),
        ..Default::default()
    };
    eframe::run_native(
        "LiteView",
        options,
        Box::new(move |cc| Ok(Box::new(app::LiteView::new(cc, path)))),
    )
}
