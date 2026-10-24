//! WordCraft on Android (arm64-v8a): the real engine and egui shell, running on winit's
//! NativeActivity backend.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use wordcraft_engine::Session;
use wordcraft_ui_egui::{FileDialogRequest, Services, UiState, WordApp};

mod file_picker;
mod modifiers;
mod prefs_watch;
mod touch_scroll;

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

fn prefs_file() -> Option<PathBuf> {
    DATA_DIR.get().map(|d| d.join("ui.json"))
}

fn load_prefs(app: &mut WordApp) {
    if let Some(p) = prefs_file()
        && let Ok(bytes) = std::fs::read(&p)
        && let Ok(ui) = serde_json::from_slice::<UiState>(&bytes)
    {
        app.apply_prefs(ui);
    }
    app.ui.window = None;
}

const PREFS_EVERY: Duration = Duration::from_secs(2);

fn prefs_bytes(app: &WordApp) -> Vec<u8> {
    serde_json::to_vec_pretty(&app.prefs()).unwrap_or_default()
}

fn open_extensions(purpose: &str) -> Option<Vec<String>> {
    match purpose {
        "picture" => file_picker::extensions(&[&["png", "jpg", "jpeg", "gif", "webp", "bmp"]]),
        "recipients" => file_picker::extensions(&[&["csv", "tsv", "txt", "xlsx", "xlsm", "ods"]]),
        _ => file_picker::extensions(&[&["docx", "docm", "dotx", "dotm", "doc", "dot", "odt", "rtf", "txt", "md", "html", "htm", "tex", "json"]]),
    }
}

fn file_dialog(req: FileDialogRequest) -> Receiver<Option<String>> {
    let (kind, title, extensions, suggested) = match req {
        FileDialogRequest::Open { purpose } => (file_picker::Kind::Open, "Open \u{2014} Android", open_extensions(&purpose), None),
        FileDialogRequest::Save { name } => (file_picker::Kind::Save, "Save As \u{2014} Android", None, Some(name)),
    };
    let paths = file_picker::request(kind, title, extensions, suggested);
    let (answer, rx) = std::sync::mpsc::channel();
    let waiting = std::thread::Builder::new().name("file dialog".into()).spawn(move || {
        let picked = paths.recv().ok().and_then(|v| v.into_iter().next()).map(|p| p.to_string_lossy().into_owned());
        let _ = answer.send(picked);
    });
    if let Err(e) = waiting {
        log::error!("couldn't start the file-dialog thread: {e}");
    }
    rx
}

#[cfg(target_os = "android")]
thread_local! {
    static APP: std::cell::RefCell<Option<winit::platform::android::activity::AndroidApp>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(target_os = "android")]
fn new_window() {
    let Some(app) = APP.with(|slot| slot.borrow().clone()) else {
        log::error!("no AndroidApp in this window: cannot open another");
        return;
    };
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    let opened = vm.attach_current_thread(|env| -> jni::errors::Result<()> {
        use jni::objects::{JObject, JValue};
        use jni::signature::{RuntimeFieldSignature, RuntimeMethodSignature};
        use jni::strings::JNIStr;

        let name = |n: &'static std::ffi::CStr| JNIStr::from_cstr(n).expect("a literal name");
        let method = RuntimeMethodSignature::from_str;
        let get_component = method("()Landroid/content/ComponentName;")?;
        let set_component = method("(Landroid/content/ComponentName;)Landroid/content/Intent;")?;
        let add_flags = method("(I)Landroid/content/Intent;")?;
        let start_activity = method("(Landroid/content/Intent;)V")?;
        let no_args = method("()V")?;
        let int_field = RuntimeFieldSignature::from_str("I")?;

        let activity = unsafe { JObject::from_raw(env, app.activity_as_ptr() as jni::sys::jobject) };
        let component = env.call_method(&activity, name(c"getComponentName"), get_component.method_signature(), &[])?.l()?;
        let intent_class = name(c"android/content/Intent");
        let intent = env.new_object(intent_class, no_args.method_signature(), &[])?;
        env.call_method(&intent, name(c"setComponent"), set_component.method_signature(), &[JValue::Object(&component)])?;
        let flags = env.get_static_field(intent_class, name(c"FLAG_ACTIVITY_NEW_DOCUMENT"), int_field.field_signature())?.i()?
            | env.get_static_field(intent_class, name(c"FLAG_ACTIVITY_MULTIPLE_TASK"), int_field.field_signature())?.i()?;
        env.call_method(&intent, name(c"addFlags"), add_flags.method_signature(), &[JValue::Int(flags)])?;
        env.call_method(&activity, name(c"startActivity"), start_activity.method_signature(), &[JValue::Object(&intent)])?;
        Ok(())
    });
    match opened {
        Ok(()) => log::info!("asked the system for a second window"),
        Err(e) => log::error!("couldn't ask the system for a second window: {e}"),
    }
}

/// Fix for Android 14 blank status bar icons
#[cfg(target_os = "android")]
fn set_status_bar_appearance(app: &winit::platform::android::activity::AndroidApp, light_background: bool) {
    use jni::objects::{JObject, JValue};
    let Ok(vm) = (unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) }) else { return };
    let Ok(mut env) = vm.attach_current_thread() else { return };
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };

    let result = (|| -> jni::errors::Result<()> {
        let window = env.call_method(&activity, "getWindow", "()Landroid/view/Window;", &[])?.l()?;
        let sdk = env.get_static_field("android/os/Build$VERSION", "SDK_INT", "I")?.i()?;
        if sdk < 30 {
            return Ok(());
        }
        let controller = env.call_method(&window, "getInsetsController", "()Landroid/view/WindowInsetsController;", &[])?.l()?;
        if !controller.is_null() {
            // APPEARANCE_LIGHT_STATUS_BARS = 8 (This means dark icons on a light background)
            let mask = 8;
            if light_background {
                env.call_method(&controller, "setSystemBarsAppearance", "(II)V", &[JValue::Int(mask), JValue::Int(mask)])?;
            } else {
                env.call_method(&controller, "setSystemBarsAppearance", "(II)V", &[JValue::Int(0), JValue::Int(mask)])?;
            }
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = env.exception_clear();
    }
}

#[cfg(target_os = "android")]
fn android_services() -> Services {
    Services {
        file_dialog: Some(Box::new(file_dialog)),
        new_window: Some(Box::new(new_window)),
        ..Default::default()
    }
}

#[cfg(not(target_os = "android"))]
fn android_services() -> Services {
    Services { file_dialog: Some(Box::new(file_dialog)), ..Default::default() }
}

struct FileLogger {
    sink: Mutex<Option<std::fs::File>>,
}

impl log::Log for FileLogger {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, record: &log::Record) {
        if let Ok(mut guard) = self.sink.lock() {
            if let Some(f) = guard.as_mut() {
                use std::io::Write;
                let _ = writeln!(f, "[{}] {} {}", record.level(), record.target(), record.args());
            }
        }
    }
    fn flush(&self) {}
}

static LOGGER: OnceLock<FileLogger> = OnceLock::new();

fn init_logging(dir: &Path) {
    let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("wordcraft.log")) else {
        return;
    };
    let logger = LOGGER.get_or_init(|| FileLogger { sink: Mutex::new(None) });
    if let Ok(mut guard) = logger.sink.lock() {
        *guard = Some(file);
    }
    if log::set_logger(logger).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub fn android_main(app: winit::platform::android::activity::AndroidApp) {
    let _window = prefs_watch::counted_window();
    APP.with(|slot| *slot.borrow_mut() = Some(app.clone()));
    let data_dir = app.internal_data_path().unwrap_or_else(|| PathBuf::from("."));
    let _ = std::fs::create_dir_all(&data_dir);
    let _ = DATA_DIR.set(data_dir.clone());
    init_logging(&data_dir);
    log::info!("WordCraft {} starting on Android; data dir {}", env!("CARGO_PKG_VERSION"), data_dir.display());
    
    if let Some(ext) = app.external_data_path() {
        let _ = std::fs::create_dir_all(&ext);
        let _ = file_picker::EXT_DIR.set(ext);
    }
    file_picker::register_fonts();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("WordCraft").with_app_id("ai.storyteller.wordcraft"),
        android_app: Some(app),
        ..Default::default()
    };

    let result = eframe::run_native(
        "WordCraft",
        options,
        Box::new(|cc| {
            let mut app = WordApp::new(Session::new(wordcraft_doc::Document::new()), android_services());
            load_prefs(&mut app);
            app.integrated_titlebar = false;
            if let Some(rs) = &cc.wgpu_render_state {
                let info = rs.adapter.get_info();
                log::info!("wgpu adapter: {info:?}");
            } else {
                log::warn!("no wgpu render state");
            }
            Ok(Box::new(Shell {
                inner: app,
                modifiers: modifiers::Tracked::default(),
                touch: touch_scroll::TouchScroll::default(),
                prefs: prefs_watch::Writer::new(prefs_file(), PREFS_EVERY),
                press_at: None,
                keyboard_wanted: false,
                last_light_mode: false, // Default assumption
            }))
        }),
    );

    match result {
        Ok(()) => log::info!("WordCraft exited normally"),
        Err(e) => log::error!("WordCraft stopped: {e}"),
    }
    APP.with(|slot| *slot.borrow_mut() = None);
}

struct Shell {
    inner: WordApp,
    modifiers: modifiers::Tracked,
    touch: touch_scroll::TouchScroll,
    prefs: prefs_watch::Writer,
    press_at: Option<egui::Pos2>,
    keyboard_wanted: bool,
    last_light_mode: bool, // Track theme to avoid JNI calls every frame
}

impl Shell {
    fn quit_if_asked(&mut self, ctx: &egui::Context) {
        if !self.inner.quit_requested {
            return;
        }
        let bytes = prefs_bytes(&self.inner);
        self.prefs.store_now(&bytes);
        log::info!("quitting");
        log::logger().flush();
        if prefs_watch::windows() > 1 {
            self.inner.quit_requested = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else {
            std::process::exit(0);
        }
    }
}

impl eframe::App for Shell {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.inner.logic(ctx);
        if self.prefs.due() {
            let bytes = prefs_bytes(&self.inner);
            self.prefs.store(&bytes);
        }
        self.quit_if_asked(ctx);

        // --- ANDROID 14 STATUS BAR FIX ---
        // If the app is in light mode, force dark icons on the status bar. Else, force light icons.
        let light_mode = ctx.style().visuals.light_mode;
        if light_mode != self.last_light_mode {
            self.last_light_mode = light_mode;
            if let Some(app) = APP.with(|slot| slot.borrow().clone()) {
                set_status_bar_appearance(&app, light_mode);
            }
        }
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        let modifiers = self.modifiers.apply(raw);
        self.touch.apply(raw, modifiers);
        self.inner.raw_input_hook(raw);

        let page = self.inner.canvas.canvas_rect;
        for event in &raw.events {
            if let egui::Event::PointerButton { pos, pressed, button, .. } = event {
                if *button == egui::PointerButton::Primary {
                    let on_page = page.is_some_and(|r| r.contains(*pos));
                    if *pressed {
                        self.press_at = Some(*pos);
                        if !on_page {
                            self.keyboard_wanted = false;
                        }
                    } else if let Some(start) = self.press_at.take() {
                        let dist = start.distance(*pos);
                        if on_page && dist < 12.0 {
                            self.keyboard_wanted = true;
                        } else {
                            self.keyboard_wanted = false;
                        }
                    }
                }
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // We pad the top by the status bar height dynamically in portrait mode.
        // Wait, the previous lib.rs code had padding here. Let's re-add it.
        // Actually, in the user's previous code (which they provided), there was no padding in the Shell. 
        // The padding was in the previous lib.rs I gave them. Let's check their original code.
        // Their original code: `fn ui(&mut self, ui: &mut egui::Ui...) { self.inner.ui(ui); file_picker::draw(...); ... }`
        // Ah, they removed the padding! That means the top bar was drawn OVER the app.
        // Let's add the padding back safely.
        let full = ui.max_rect();
        let ppp = ui.ctx().pixels_per_point();
        let app = APP.with(|slot| slot.borrow().clone());
        let mut inner_rect = full;
        
        if let Some(app) = app {
            // Read status bar height
            let sbh_px = {
                use jni::objects::{JObject, JValue};
                let mut h = 0.0;
                if let Ok(vm) = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) } {
                    if let Ok(mut env) = vm.attach_current_thread() {
                        let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
                        if let Ok(res) = env.call_method(&activity, "getResources", "()Landroid/content/res/Resources;", &[]).and_then(|v| v.l()) {
                            if let Ok(name) = env.new_string("status_bar_height") {
                                if let Ok(kind) = env.new_string("dimen") {
                                    if let Ok(pkg) = env.new_string("android") {
                                        if let Ok(id) = env.call_method(&res, "getIdentifier", "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)I", &[JValue::Object(&name), JValue::Object(&kind), JValue::Object(&pkg)]).and_then(|v| v.i()) {
                                            if id != 0 {
                                                if let Ok(px) = env.call_method(&res, "getDimensionPixelSize", "(I)I", &[JValue::Int(id)]).and_then(|v| v.i()) {
                                                    h = px as f32;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                h
            };
            
            // Only pad if we are in portrait mode (not full screen)
            let is_portrait = full.height() > full.width();
            if is_portrait && sbh_px > 0.0 {
                let top = (sbh_px / ppp).min(full.height() / 4.0);
                inner_rect.min.y += top;
                let strip = egui::Rect::from_min_max(full.min, egui::pos2(full.max.x, inner_rect.min.y));
                ui.painter().rect_filled(strip, 0.0, ui.visuals().panel_fill);
            }
        }

        ui.scope_builder(egui::UiBuilder::new().max_rect(inner_rect), |ui| {
            self.inner.ui(ui);
            file_picker::draw(ui.ctx());
        });
        
        self.quit_if_asked(ui.ctx());

        let wants_keyboard = if self.inner.ui.backstage || self.inner.dialog.is_some() {
            ui.ctx().egui_wants_keyboard_input()
        } else {
            self.keyboard_wanted
        };

        if !wants_keyboard {
            ui.ctx().output_mut(|o| o.ime = None);
        }
    }

    fn on_exit(&mut self) {
        let bytes = prefs_bytes(&self.inner);
        self.prefs.store_now(&bytes);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn winit_carries_the_android_scroll_and_hover_patch() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let lock = std::fs::read_to_string(root.join("Cargo.lock")).unwrap();
        let winit = lock.split("[[package]]").find(|p| p.contains("\nname = \"winit\"\n")).expect("winit is in Cargo.lock");
        assert!(!winit.contains("\nsource = "), "winit must resolve to vendor/winit, not:{winit}");

        let android = std::fs::read_to_string(root.join("vendor/winit/src/platform_impl/android/mod.rs")).unwrap();
        let scroll = "MotionAction::Scroll => {";
        let hover = "MotionAction::HoverMove => {";
        assert!(android.contains(scroll), "vendor/winit lost its {scroll} patch");
        assert!(android.contains(hover), "vendor/winit lost its {hover} patch");
        assert!(android.contains("event::WindowEvent::MouseWheel"), "the scroll patch must emit a wheel event");
        assert!(android.contains("event::WindowEvent::CursorMoved { device_id, position }"), "and must set the pointer position, or an app that only scrolls when hovered ignores it");
        assert!(android.contains("event::WindowEvent::CursorMoved"), "the hover patch must move the pointer");
    }
}
