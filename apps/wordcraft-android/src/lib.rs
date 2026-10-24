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
//! - Back button: the first press sends Escape (closes dialogs, menus, the File screen); a second
//!   press within a moment quits (asking about unsaved changes first).
//! - The interface is drawn smaller than the display's own scale (portrait and landscape), so
//!   dialogs fit on a phone screen, and the page is shown at "Page Width" zoom.
//! - The app is drawn below the status bar (its height is read from Android).
#![cfg(target_os = "android")]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use winit::platform::android::activity::AndroidApp;
use wordcraft_engine::Session;
use wordcraft_ui_egui::theme::InterfaceSize;
use wordcraft_ui_egui::{FileDialogAnswer, FileDialogRequest, Services, WordApp};

/// Two Back presses within this time quit the app.
const BACK_EXIT_WINDOW: Duration = Duration::from_millis(1800);
/// The interface is drawn at this share of the display's own scale, in portrait and in landscape.
const PORTRAIT_SCALE: f64 = 0.75;
const LANDSCAPE_SCALE: f64 = 0.8;
/// A dialog is about this many points tall: the short side of the screen must hold one (landscape).
const MIN_SHORT_SIDE_POINTS: f32 = 520.0;

/// Where the built-in file browser starts, and what `~` means.
fn start_dir(app: &AndroidApp) -> Option<PathBuf> {
    let shared = PathBuf::from("/storage/emulated/0/Documents");
    if std::fs::read_dir(&shared).is_ok() {
        return Some(shared);
    }
    app.external_data_path().or_else(|| app.internal_data_path())
}

/// Height in pixels of the Android status bar, or 0 when it can't be read.
fn status_bar_height_px(app: &AndroidApp) -> f32 {
    use jni::objects::{JObject, JValue};
    let read = || -> jni::errors::Result<i32> {
        // SAFETY: the pointers come from android-activity and stay valid while the app runs.
        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast())? };
        let mut env = vm.attach_current_thread()?;
        let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
        let res = env.call_method(&activity, "getResources", "()Landroid/content/res/Resources;", &[])?.l()?;
        let name = env.new_string("status_bar_height")?;
        let kind = env.new_string("dimen")?;
        let pkg = env.new_string("android")?;
        let id = env
            .call_method(
                &res,
                "getIdentifier",
                "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)I",
                &[JValue::Object(&name), JValue::Object(&kind), JValue::Object(&pkg)],
            )?
            .i()?;
        if id == 0 {
            return Ok(0);
        }
        env.call_method(&res, "getDimensionPixelSize", "(I)I", &[JValue::Int(id)])?.i()
    };
    read().map(|px| px as f32).unwrap_or(0.0)
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
    /// When Back was last pressed (for the press-twice-to-quit rule).
    last_back: Option<Instant>,
    /// A second Back arrived: quit on the next frame.
    exit_requested: bool,
    /// The orientation the interface size was last matched to.
    portrait: Option<bool>,
    /// Height of the status bar in screen pixels: the app is kept below it.
    status_bar_px: f32,
}

impl AndroidShell {
    /// Back pressed: the first press is Escape; a quick second one quits.
    fn on_back(&mut self, raw: &mut egui::RawInput) {
        let now = Instant::now();
        if self.last_back.is_some_and(|t| now.duration_since(t) < BACK_EXIT_WINDOW) {
            self.last_back = None;
            self.exit_requested = true;
            return;
        }
        self.last_back = Some(now);
        for pressed in [true, false] {
            raw.events.push(egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed, repeat: false, modifiers: Default::default() });
        }
        self.app.status("Press Back again to exit");
    }

    /// Picks the interface size when the phone is turned (and at start): smaller than the display's
    /// own scale so a dialog fits the short side of the screen, then fits the page to the width.
    /// Only changes on a turn, so a size chosen in File > Options stays until the next turn.
    fn adapt_to_orientation(&mut self, ctx: &egui::Context) {
        let ppp = ctx.pixels_per_point();
        let rect = ctx.content_rect();
        let (w_px, h_px) = (rect.width() * ppp, rect.height() * ppp);
        if w_px <= 0.0 || h_px <= 0.0 {
            return;
        }
        let portrait = h_px > w_px;
        if self.portrait == Some(portrait) {
            return;
        }
        self.portrait = Some(portrait);
        let native = ctx.native_pixels_per_point().unwrap_or(1.0);
        let by_density = native * if portrait { PORTRAIT_SCALE as f32 } else { LANDSCAPE_SCALE as f32 };
        let by_short_side = w_px.min(h_px) / MIN_SHORT_SIDE_POINTS;
        let scale = by_density.min(by_short_side);
        self.app.ui.interface_size = InterfaceSize::from_percent(f64::from(scale) * 100.0).unwrap_or(InterfaceSize::AUTO);
        let _ = self.app.run("view.pageWidth", serde_json::json!({}));
    }
}

impl eframe::App for AndroidShell {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.adapt_to_orientation(ctx);
        self.app.logic(ctx);
        if std::mem::take(&mut self.exit_requested) && self.app.close_requested() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        // Set once an unsaved-changes prompt (opened by the quit above) has been answered.
        if self.app.quit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        // winit reports Android's Back button as a BrowserBack key press.
        let mut back = false;
        raw.events.retain(|e| match e {
            egui::Event::Key { key: egui::Key::BrowserBack, pressed, .. } => {
                back |= *pressed;
                false
            }
            _ => true,
        });
        if back {
            self.on_back(raw);
        }
        self.app.raw_input_hook(raw);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Keep the app below the status bar instead of drawing under the clock and icons.
        let full = ui.max_rect();
        let top = (self.status_bar_px / ui.ctx().pixels_per_point()).min(full.height() / 4.0);
        let mut inner = full;
        inner.min.y += top;
        if top > 0.0 {
            let strip = egui::Rect::from_min_max(full.min, egui::pos2(full.max.x, inner.min.y));
            ui.painter().rect_filled(strip, 0.0, ui.visuals().panel_fill);
        }
        ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| self.app.ui(ui));
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

    let status_bar_px = status_bar_height_px(&app);
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
            Ok(Box::new(AndroidShell { app: word, android, keyboard_shown: false, last_back: None, exit_requested: false, portrait: None, status_bar_px }))
        }),
    );
    if let Err(e) = result {
        log::error!("WordCraft failed to start: {e}");
    }
}
