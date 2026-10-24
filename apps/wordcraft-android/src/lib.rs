//! WordCraft on Android.
//!
//! Runs the same [`wordcraft_ui_egui::WordApp`] as the desktop and web apps, through eframe's
//! NativeActivity runner (wgpu). Differences from desktop:
//!
//! - No system file dialog: the host answers every file request with an error, which makes the app
//!   show its own built-in file browser (the same fallback used on Linux desktops without a file
//!   chooser). The browser starts in `Documents` when the app may read it, else in the app's own
//!   folder.
//! - No printing, no control server, no preferences file (settings reset on restart).
//! - The on-screen keyboard is shown while the editor asks for text input.
#![cfg(target_os = "android")]

use std::path::PathBuf;

use winit::platform::android::activity::AndroidApp;
use wordcraft_engine::Session;
use wordcraft_ui_egui::{FileDialogAnswer, FileDialogRequest, Services, WordApp};

/// Where the built-in file browser starts, and what `~` means.
fn start_dir(app: &AndroidApp) -> Option<PathBuf> {
    let shared = PathBuf::from("/storage/emulated/0/Documents");
    if std::fs::read_dir(&shared).is_ok() {
        return Some(shared);
    }
    app.external_data_path().or_else(|| app.internal_data_path())
}

fn services() -> Services {
    Services {
        // Answering with an error makes the app open its built-in file browser instead.
        file_dialog: Some(Box::new(|_req: FileDialogRequest| {
            let (tx, rx) = std::sync::mpsc::channel::<FileDialogAnswer>();
            let _ = tx.send(Err("no system file dialog on Android".to_string()));
            rx
        })),
        ..Default::default()
    }
}

struct AndroidShell {
    app: WordApp,
    android: AndroidApp,
    keyboard_shown: bool,
}

impl eframe::App for AndroidShell {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.app.logic(ctx);
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.app.raw_input_hook(raw);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.app.ui(ui);
        // The editor reports where its caret is (IME output) while it wants typing: show the
        // on-screen keyboard then, hide it otherwise.
        let wants_keyboard = ui.ctx().output(|o| o.ime.is_some());
        if wants_keyboard != self.keyboard_shown {
            self.keyboard_shown = wants_keyboard;
            if wants_keyboard {
                self.android.show_soft_input(false);
            } else {
                self.android.hide_soft_input(false);
            }
        }
    }
}

#[unsafe(no_mangle)]
pub fn android_main(app: AndroidApp) {
    android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Info).with_tag("wordcraft"));

    // Android's default temp dir (/data/local/tmp) isn't writable by apps, and there is no HOME.
    if let Some(data) = app.internal_data_path() {
        let tmp = data.join("tmp");
        let _ = std::fs::create_dir_all(&tmp);
        // SAFETY: start-up, before this app creates any thread of its own.
        unsafe { std::env::set_var("TMPDIR", &tmp) };
    }
    if let Some(dir) = start_dir(&app) {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::env::set_current_dir(&dir);
        // SAFETY: as above.
        unsafe { std::env::set_var("HOME", &dir) };
    }

    let options = eframe::NativeOptions { android_app: Some(app.clone()), ..Default::default() };
    let android = app;
    let result = eframe::run_native(
        "WordCraft",
        options,
        Box::new(move |_cc| {
            let region = wordcraft_ui_egui::i18n::system_region();
            let doc = wordcraft_engine::blank_document(&region);
            let mut session = Session::new(doc);
            session.system_region = region;
            let mut word = WordApp::new(session, services());
            // No document to show: start on File > Home, as the desktop app does.
            word.start_screen();
            Ok(Box::new(AndroidShell { app: word, android, keyboard_shown: false }))
        }),
    );
    if let Err(e) = result {
        log::error!("WordCraft failed to start: {e}");
    }
                 }
