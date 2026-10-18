use eframe::egui;

struct LauncherApp {
    query: String,
}

impl Default for LauncherApp {
    fn default() -> Self {
        Self {
            query: String::new(),
        }
    }
}

impl eframe::App for LauncherApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("egui Launcher");
            ui.text_edit_singleline(&mut self.query);
        });
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([640.0, 360.0])
            .with_title("egui Launcher"),
        ..Default::default()
    };

    eframe::run_native(
        "egui Launcher",
        options,
        Box::new(|_cc| Ok(Box::new(LauncherApp::default()))),
    )
}