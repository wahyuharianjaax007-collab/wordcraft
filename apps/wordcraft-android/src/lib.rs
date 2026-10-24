//! WordCraft on Android.
//!
//! Runs the same [`wordcraft_ui_egui::WordApp`] as the desktop and web apps, through eframe's
//! NativeActivity runner (wgpu). Differences from desktop:
//!
//! - No system file dialog: the host answers every file request with an error, which makes the app
//!   show its own built-in file browser. Android 11+ hides documents (.docx and the like) from an
//!   app until it has "All files access", so the app opens that settings screen when it lacks it.
//! - No printing, no control server, no preferences file (settings reset on restart).
//! - The on-screen keyboard appears when the page or a text box is tapped, and goes away when
//!   anything else is tapped.
//! - Back button: the first press sends Escape (closes dialogs, menus, the File screen); a second
//!   press within a moment quits (asking about unsaved changes first).
//! - The interface is never drawn larger than what fits a dialog or the File pages on the screen,
//!   and a new or opened document is shown at "Page Width" zoom.
//! - Full screen mode is enabled (system status and navigation bars are hidden).
#![cfg(target_os = "android")]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use winit::platform::android::activity::AndroidApp;
use wordcraft_engine::Session;
use wordcraft_ui_egui::theme::InterfaceSize;
use wordcraft_ui_egui::{FileDialogAnswer, FileDialogRequest, Services, WordApp};

/// Two Back presses within this time quit the app.
const BACK_EXIT_WINDOW: Duration = Duration::from_millis(1800);
/// A touch that moves less than this many points is a tap, not a scroll.
const TAP_SLOP_POINTS: f32 = 12.0;

/// Where the built-in file browser starts, and what `~` means.
fn start_dir(app: &AndroidApp) -> Option<PathBuf> {
    let shared = PathBuf::from("/storage/emulated/0/Documents");
    if shared.is_dir() {
        return Some(shared);
    }
    app.external_data_path().or_else(|| app.internal_data_path())
}

/// Android 11+ shows an app no documents (.docx, .pdf, ...) outside its own folder until the user
/// allows "All files access". Opens that settings page for this app when it isn't allowed yet.
fn ask_for_all_files_access(app: &AndroidApp) {
    use jni::objects::{JObject, JValue};
    // SAFETY: the pointers come from android-activity and stay valid while the app runs.
    let Ok(vm) = (unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) }) else { return };
    let Ok(mut env) = vm.attach_current_thread() else { return };
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
    let result = (|| -> jni::errors::Result<()> {
        // The setting exists from Android 11 (API 30).
        let sdk = env.get_static_field("android/os/Build$VERSION", "SDK_INT", "I")?.i()?;
        if sdk < 30 {
            return Ok(());
        }
        if env.call_static_method("android/os/Environment", "isExternalStorageManager", "()Z", &[])?.z()? {
            return Ok(());
        }
        let action = env.new_string("android.settings.MANAGE_APP_ALL_FILES_ACCESS_PERMISSION")?;
        let package = env.call_method(&activity, "getPackageName", "()Ljava/lang/String;", &[])?.l()?;
        let prefix = env.new_string("package:")?;
        let uri_text = env
            .call_method(&prefix, "concat", "(Ljava/lang/String;)Ljava/lang/String;", &[JValue::Object(&package)])?
            .l()?;
        let uri = env
            .call_static_method("android/net/Uri", "parse", "(Ljava/lang/String;)Landroid/net/Uri;", &[JValue::Object(&uri_text)])?
            .l()?;
        let intent = env.new_object(
            "android/content/Intent",
            "(Ljava/lang/String;Landroid/net/Uri;)V",
            &[JValue::Object(&action), JValue::Object(&uri)],
        )?;
        env.call_method(&activity, "startActivity", "(Landroid/content/Intent;)V", &[JValue::Object(&intent)])?;
        Ok(())
    })();
    if result.is_err() {
        let _ = env.exception_clear();
    }
}

/// Menghilangkan status bar dan navigation bar (Full Screen / Immersive Mode).
fn hide_system_bars(app: &AndroidApp) {
    use jni::objects::{JObject, JValue};
    // SAFETY: the pointers come from android-activity and stay valid while the app runs.
    let Ok(vm) = (unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) }) else { return };
    let Ok(mut env) = vm.attach_current_thread() else { return };
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };

    let result = (|| -> jni::errors::Result<()> {
        let window = env.call_method(&activity, "getWindow", "()Landroid/view/Window;", &[])?.l()?;
        let decor_view = env.call_method(&window, "getDecorView", "()Landroid/view/View;", &[])?.l()?;
        let sdk = env.get_static_field("android/os/Build$VERSION", "SDK_INT", "I")?.i()?;

        if sdk >= 30 {
            // Android 11+ (API 30+)
            let controller = env.call_method(&window, "getInsetsController", "()Landroid/view/WindowInsetsController;", &[])?.l()?;
            if !controller.is_null() {
                let hide_mask = 1 | 2; // Type.statusBars() | Type.navigationBars()
                env.call_method(&controller, "hide", "(I)V", &[JValue::Int(hide_mask)])?;
                let behavior = 2; // BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
                env.call_method(&controller, "setSystemBarsBehavior", "(I)V", &[JValue::Int(behavior)])?;
            }
        } else {
            // Android 10 dan ke bawah (Legacy)
            let flags = 0x00000400 | 0x00000002 | 0x00001000; // FULLSCREEN | HIDE_NAVIGATION | IMMERSIVE_STICKY
            env.call_method(&decor_view, "setSystemUiVisibility", "(I)V", &[JValue::Int(flags)])?;
        }
        Ok(())
    })();
    if result.is_err() { let _ = env.exception_clear(); }
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
    /// Whether the on-screen keyboard is up.
    keyboard_shown: bool,
    /// The last tap was on the page (or a text box over it): the keyboard may show while it is edited.
    keyboard_wanted: bool,
    /// Where the pointer went down, to tell a tap from a scroll when it comes up.
    press_at: Option<egui::Pos2>,
    /// When Back was last pressed (for the press-twice-to-quit rule).
    last_back: Option<Instant>,
    /// A second Back arrived: quit on the next frame.
    exit_requested: bool,
    /// The File screen was open last frame.
    was_backstage: bool,
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
        self.keyboard_wanted = false;
        for pressed in [true, false] {
            raw.events.push(egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed, repeat: false, modifiers: Default::default() });
        }
        self.app.status("Press Back again to exit");
    }

    /// A tap on the page asks for the keyboard; a press anywhere else takes the request back.
    fn track_taps(&mut self, raw: &egui::RawInput) {
        let page = self.app.canvas.canvas_rect;
        for event in &raw.events {
            if let egui::Event::PointerButton { pos, pressed, .. } = event {
                let on_page = page.is_some_and(|r| r.contains(*pos));
                if *pressed {
                    self.press_at = Some(*pos);
                    if !on_page {
                        self.keyboard_wanted = false;
                    }
                } else if let Some(start) = self.press_at.take() {
                    let dist = start.distance(*pos);
                    // Jika jarak sangat kecil (Tap), izinkan keyboard. Jika besar (Swipe/Scroll), matikan.
                    if on_page && dist < TAP_SLOP_POINTS {
                        self.keyboard_wanted = true;
                    } else if dist >= TAP_SLOP_POINTS {
                        self.keyboard_wanted = false;
                    }
                }
            }
        }
    }

    /// Never draw the interface so large that a dialog, the ribbon or the File pages don't fit the
    /// screen. Only ever makes it smaller: a smaller size chosen in File > Options is kept.
    fn cap_interface_size(&mut self, ctx: &egui::Context) {
        let ppp = ctx.pixels_per_point();
        let rect = ctx.content_rect();
        let (w_px, h_px) = (rect.width() * ppp, rect.height() * ppp);
        if w_px <= 0.0 || h_px <= 0.0 {
            return;
        }

        // Deteksi orientasi layar
        let is_portrait = h_px > w_px;

        // Atur batas minimum secara dinamis berdasarkan orientasi
        let min_width = if self.app.ui.backstage {
            if is_portrait { 800.0 } else { 1100.0 }
        } else {
            if is_portrait { 500.0 } else { 760.0 } // Portrait dilebarkan agar UI membesar
        };
        let min_height = if is_portrait { 800.0 } else { 520.0 };

        let cap = (w_px / min_width).min(h_px / min_height);
        let native = ctx.native_pixels_per_point().unwrap_or(1.0);
        let current = self.app.ui.interface_size.percent().map_or(native, |p| f32::from(p) / 100.0);
        
        if current > cap + 0.005 {
            if let Some(size) = InterfaceSize::from_percent((f64::from(cap) * 100.0).floor()) {
                self.app.ui.interface_size = size;
            }
        }
    }

    /// A document that was just made or opened is shown at Page Width, not wider than the screen.
    fn fit_page_after_file_screen(&mut self) {
        let open = self.app.ui.backstage;
        let just_closed = self.was_backstage && !open;
        self.was_backstage = open;
        let view = &self.app.session.view;
        if just_closed && view.fit.is_empty() && (view.zoom - 1.0).abs() < 0.001 {
            let _ = self.app.run("view.pageWidth", serde_json::json!({}));
        }
    }
}

impl eframe::App for AndroidShell {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.cap_interface_size(ctx);
        self.app.logic(ctx);
        self.fit_page_after_file_screen();
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
        self.track_taps(raw);
        self.app.raw_input_hook(raw);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Full screen mode: UI langsung menutupi seluruh layar tanpa terpotong status bar.
        ui.scope_builder(egui::UiBuilder::new().max_rect(ui.max_rect()), |ui| self.app.ui(ui));

        // The page and text boxes report where their caret is (IME output) while they take typing.
        let caret = ui.ctx().output(|o| o.ime.as_ref().map(|ime| ime.cursor_rect));
        let wants_keyboard = if let Some(caret) = caret {
            match self.app.canvas.canvas_rect {
                Some(page) if page.contains(caret.center()) => self.keyboard_wanted,
                _ => true,
            }
        } else {
            false
        };
        
        // Pastikan egui benar-benar sedang meminta input teks sebelum keyboard muncul.
        let wants_keyboard = wants_keyboard && ui.ctx().wants_keyboard_input();

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
    
    ask_for_all_files_access(&app);
    hide_system_bars(&app); // Memanggil fungsi Full Screen

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
            Ok(Box::new(AndroidShell {
                app: word,
                android,
                keyboard_shown: false,
                keyboard_wanted: false,
                press_at: None,
                last_back: None,
                exit_requested: false,
                was_backstage: false,
            }))
        }),
    );
    if let Err(e) = result {
        log::error!("WordCraft failed to start: {e}");
    }
}
