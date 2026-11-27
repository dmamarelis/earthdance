//! EarthDance native app.
//!
//!     cargo run --release -p ed-app            (real data, processed live from the acquisitions)
//!     cargo run --release -p ed-app -- --demo  (the synthetic demo set)

mod app;
mod charts;
mod map;
mod panels;
mod report;
mod sheets;
mod shot;
mod sound;
mod status_ui;
mod theme;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default().with_title("EarthDance · Ο Χορός της Γης").with_inner_size([1440.0, 900.0]).with_min_inner_size([1000.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native("EarthDance", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
