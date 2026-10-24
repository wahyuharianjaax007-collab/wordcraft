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
//! - Landscape mode is full screen; Portrait mode shows the status bar and avoids the notch.
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

/// Toggles full screen mode based on the argument.
fn set_fullscreen(app: &AndroidApp, fullscreen: bool) {
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
                let mask = 1 | 2; // Type.statusBars() | Type.navigationBars()
                if fullscreen {
                    env.call_method(&controller, "hide", "(I)V", &[JValue::Int(mask)])?;
                    let behavior = 2; // BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
                    env.call_method(&controller, "setSystemBarsBehavior", "(I)V", &[JValue::Int(behavior)])?;
                } else {
                    env.call_method(&controller, "show", "(I)V", &[JValue::Int(mask)])?;
                }
            }
        } else {
            // Android 10 dan ke bawah (Legacy)
            let flags = if fullscreen {
                0x00000400 | 0x00000002 | 0x00001000 // FULLSCREEN | HIDE_NAVIGATION | IMMERSIVE_STICKY
            } else {
                0 // VISIBLE
            };
            env.call_method(&decor_view, "setSystemUiVisibility", "(I)V", &[JValue::Int(flags)])?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = env.exception_clear();
    }
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
    /// Height of the status bar in screen pixels: the app is kept below it in portrait.
    status_bar_px: f32,
    /// Tracks the orientation to toggle fullscreen correctly.
    is_portrait: bool,
    /// Tracks if fullscreen is currently active.
    fullscreen_active: bool,
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
            raw.events.push(egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: Default::default(),
            });
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

    /// Atur ukuran antarmuka secara dinamis berdasarkan orientasi layar dengan batas aman.
    fn cap_interface_size(&mut self, ctx: &egui::Context) {
        let ppp = ctx.pixels_per_point();
        let rect = ctx.content_rect();
        let (w_px, h_px) = (rect.width() * ppp, rect.height() * ppp);
        if w_px <= 0.0 || h_px <= 0.0 {
            return;
        }

        // Target skala: Landscape 200% (2.0), Portrait 240% (2.4)
        let target_scale = if self.is_portrait { 2.4 } else { 2.0 };

        let min_width = if self.is_portrait { 360.0 } else { 760.0 };
        let min_height = if self.is_portrait { 360.0 } else { 520.0 };

        let max_fit_scale = (w_px / min_width).min(h_px / min_height);
        let cap = target_scale.min(max_fit_scale);

        let native = ctx.native_pixels_per_point().unwrap_or(1.0);
        let current = self.app.ui.interface_size.percent().map_or(native, |p| f32::from(p) / 100.0);

        // Jika masih Auto, ATAU jika masih nyangkut di batas lama (160%), paksa ubah ke target baru.
        if self.app.ui.interface_size.percent().is_none() || (current <= 1.6 + 0.005 && target_scale > 1.6) {
            if let Some(size) = InterfaceSize::from_percent((f64::from(cap) * 100.0).floor()) {
                self.app.ui.interface_size = size;
            }
        } else if current > cap + 0.005 {
            // Jika user milih manual tapi kepotong layar, kecilin
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
        let rect = ctx.content_rect();
        let is_portrait = rect.height() > rect.width();
        
        // Deteksi perubahan orientasi
        if self.is_portrait != is_portrait {
            self.is_portrait = is_portrait;
            
            // Landscape = full screen (true), Portrait = tidak full screen (false)
            let fullscreen = !is_portrait;
            set_fullscreen(&self.android, fullscreen);
            self.fullscreen_active = fullscreen;
            
            // Reset ke Auto agar skala dihitung ulang saat rotasi
            self.app.ui.interface_size = InterfaceSize::AUTO;
        }

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
        // Keep the app below the status bar in portrait, but use full height in landscape.
        let full = ui.max_rect();
        let top = if self.is_portrait {
            (self.status_bar_px / ui.ctx().pixels_per_point()).min(full.height() / 4.0)
        } else {
            0.0
        };
        
        let mut inner = full;
        inner.min.y += top;
        if top > 0.0 {
            let strip = egui::Rect::from_min_max(full.min, egui::pos2(full.max.x, inner.min.y));
            ui.painter().rect_filled(strip, 0.0, ui.visuals().panel_fill);
        }
        ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| self.app.ui(ui));

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
        let wants_keyboard = wants_keyboard && ui.ctx().egui_wants_keyboard_input();

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
            Ok(Box::new(AndroidShell {
                app: word,
                android,
                keyboard_shown: false,
                keyboard_wanted: false,
                press_at: None,
                last_back: None,
                exit_requested: false,
                was_backstage: false,
                status_bar_px,
                is_portrait: false, // Force first frame logic to set the correct state
                fullscreen_active: false,
            }))
        }),
    );
    if let Err(e) = result {
        log::error!("WordCraft failed to start: {e}");
    }
}
