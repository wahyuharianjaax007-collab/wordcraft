//! WordCraft on Android.

#![cfg(target_os = "android")]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use winit::platform::android::activity::AndroidApp;
use wordcraft_engine::Session;
use wordcraft_ui_egui::theme::InterfaceSize;
use wordcraft_ui_egui::{FileDialogAnswer, FileDialogRequest, Services, WordApp};

const BACK_EXIT_WINDOW: Duration = Duration::from_millis(1800);

fn start_dir(app: &AndroidApp) -> Option<PathBuf> {
    let shared = PathBuf::from("/storage/emulated/0/Documents");
    if shared.is_dir() {
        return Some(shared);
    }
    app.external_data_path().or_else(|| app.internal_data_path())
}

fn status_bar_height_px(app: &AndroidApp) -> f32 {
    use jni::objects::{JObject, JValue};
    let read = || -> jni::errors::Result<i32> {
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

fn ask_for_all_files_access(app: &AndroidApp) {
    use jni::objects::{JObject, JValue};
    let Ok(vm) = (unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) }) else { return };
    let Ok(mut env) = vm.attach_current_thread() else { return };
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
    let result = (|| -> jni::errors::Result<()> {
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

fn set_fullscreen(app: &AndroidApp, fullscreen: bool) {
    use jni::objects::{JObject, JValue};
    let Ok(vm) = (unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) }) else { return };
    let Ok(mut env) = vm.attach_current_thread() else { return };
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };

    let result = (|| -> jni::errors::Result<()> {
        let window = env.call_method(&activity, "getWindow", "()Landroid/view/Window;", &[])?.l()?;
        let decor_view = env.call_method(&window, "getDecorView", "()Landroid/view/View;", &[])?.l()?;
        let sdk = env.get_static_field("android/os/Build$VERSION", "SDK_INT", "I")?.i()?;

        if sdk >= 30 {
            let controller = env.call_method(&window, "getInsetsController", "()Landroid/view/WindowInsetsController;", &[])?.l()?;
            if !controller.is_null() {
                let mask = 1 | 2;
                if fullscreen {
                    env.call_method(&controller, "hide", "(I)V", &[JValue::Int(mask)])?;
                    let behavior = 2; 
                    env.call_method(&controller, "setSystemBarsBehavior", "(I)V", &[JValue::Int(behavior)])?;
                } else {
                    env.call_method(&controller, "show", "(I)V", &[JValue::Int(mask)])?;
                }
            }
        } else {
            let flags = if fullscreen {
                0x00000400 | 0x00000002 | 0x00001000
            } else {
                0 
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
    last_back: Option<Instant>,
    exit_requested: bool,
    was_backstage: bool,
    status_bar_px: f32,
    is_portrait: bool,
    fullscreen_active: bool,
}

impl AndroidShell {
    fn on_back(&mut self, raw: &mut egui::RawInput) {
        let now = Instant::now();
        if self.last_back.is_some_and(|t| now.duration_since(t) < BACK_EXIT_WINDOW) {
            self.last_back = None;
            self.exit_requested = true;
            return;
        }
        self.last_back = Some(now);
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

    fn cap_interface_size(&mut self, ctx: &egui::Context) {
        let ppp = ctx.pixels_per_point();
        let rect = ctx.content_rect();
        let (w_px, h_px) = (rect.width() * ppp, rect.height() * ppp);
        if w_px <= 0.0 || h_px <= 0.0 {
            return;
        }

        let target_scale: f32 = if self.is_portrait { 2.4 } else { 2.0 };
        let min_width = if self.is_portrait { 360.0 } else { 760.0 };
        let min_height = if self.is_portrait { 360.0 } else { 520.0 };

        let max_fit_scale = (w_px / min_width).min(h_px / min_height);
        let cap = target_scale.min(max_fit_scale);

        let native = ctx.native_pixels_per_point().unwrap_or(1.0);
        let current = self.app.ui.interface_size.percent().map_or(native, |p| f32::from(p) / 100.0);

        if self.app.ui.interface_size.percent().is_none() || (current <= 1.6 + 0.005 && target_scale > 1.6) {
            if let Some(size) = InterfaceSize::from_percent((f64::from(cap) * 100.0).floor()) {
                self.app.ui.interface_size = size;
            }
        } else if current > cap + 0.005 {
            if let Some(size) = InterfaceSize::from_percent((f64::from(cap) * 100.0).floor()) {
                self.app.ui.interface_size = size;
            }
        }
    }

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
        
        if self.is_portrait != is_portrait {
            self.is_portrait = is_portrait;
            let fullscreen = !is_portrait;
            set_fullscreen(&self.android, fullscreen);
            self.fullscreen_active = fullscreen;
            self.app.ui.interface_size = InterfaceSize::AUTO;
        }

        // BUG 1 FIX: Kalau landscape, paksa re-hide system bars terus-terusan
        // biar kalau user swipe turun, dia langsung sembunyi lagi.
        if !self.is_portrait {
            let frame = ctx.cumulative_frame_nr();
            if frame % 30 == 0 {
                set_fullscreen(&self.android, true);
            }
        }

        self.cap_interface_size(ctx);
        self.app.logic(ctx);
        self.fit_page_after_file_screen();
        
        if std::mem::take(&mut self.exit_requested) && self.app.close_requested() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if self.app.quit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
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

        // BUG 2 FIX: Hanya munculkan keyboard jika widget benar-benar minta teks
        let wants_keyboard = ui.ctx().egui_wants_keyboard_input();

        if wants_keyboard != self.keyboard_shown {
            self.keyboard_shown = wants_keyboard;
            if wants_keyboard {
                self.android.show_soft_input(false);
            } else {
                self.android.hide_soft_input(false);
            }
        } else if !wants_keyboard && self.keyboard_shown {
            // Force hide if out of sync
            self.android.hide_soft_input(false);
            self.keyboard_shown = false;
        }
    }
}

#[unsafe(no_mangle)]
pub fn android_main(app: AndroidApp) {
    android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Info).with_tag("wordcraft"));

    if let Some(data) = app.internal_data_path() {
        let tmp = data.join("tmp");
        let _ = std::fs::create_dir_all(&tmp);
        unsafe { std::env::set_var("TMPDIR", &tmp) };
    }
    if let Some(dir) = start_dir(&app) {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::env::set_current_dir(&dir);
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
            word.start_screen();
            Ok(Box::new(AndroidShell {
                app: word,
                android,
                keyboard_shown: false,
                last_back: None,
                exit_requested: false,
                was_backstage: false,
                status_bar_px,
                is_portrait: false,
                fullscreen_active: false,
            }))
        }),
    );
    if let Err(e) = result {
        log::error!("WordCraft failed to start: {e}");
    }
}
