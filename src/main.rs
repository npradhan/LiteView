#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod decode;
mod source;
#[cfg(feature = "video")]
mod video;
#[cfg(not(feature = "video"))]
#[path = "video_stub.rs"]
mod video;
mod worker;

use std::path::PathBuf;

/// The text of a panic payload.
pub fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown error".to_owned()
    }
}

/// The release build has no console window, so an error that ends the program would
/// otherwise vanish without a trace. Show it in a message box instead. Errors on
/// background threads are caught where they happen and shown in the viewer.
fn install_crash_dialog() {
    let main_thread = std::thread::current().id();
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_hook(info);
        if std::thread::current().id() == main_thread {
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title("LiteView has stopped")
                .set_description(format!(
                    "LiteView ran into an error and has to close.\n\n{info}"
                ))
                .show();
        }
    }));
}

fn main() -> eframe::Result<()> {
    install_crash_dialog();
    // A folder (or a single archive/image) may be given on the command line; then
    // open it full screen straight away. Otherwise start with a small window that
    // has a Load Folder button.
    let path = std::env::args_os().nth(1).map(PathBuf::from);
    let viewport = egui::ViewportBuilder::default()
        .with_title("LiteView")
        .with_min_inner_size([320.0, 200.0]);
    let viewport = if path.is_some() {
        viewport.with_fullscreen(true)
    } else {
        viewport.with_inner_size([420.0, 260.0])
    };
    let options = eframe::NativeOptions {
        viewport,
        centered: true,
        ..Default::default()
    };
    eframe::run_native(
        "LiteView",
        options,
        Box::new(move |cc| Ok(Box::new(app::LiteView::new(cc, path)))),
    )
}
