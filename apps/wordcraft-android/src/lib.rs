use eframe::egui;
use wordcraft_core::WordCraftApp;

#[no_mangle]
pub fn android_main(app: winit::platform::android::activity::AndroidApp) {
    std::env::set_var("RUST_BACKTRACE", "1");

    let mut options = eframe::NativeOptions::default();
    options.android_app = Some(app);

    let _ = eframe::run_native(
        "WordCraft",
        options,
        Box::new(|cc| {
            // Adjust UI scaling for mobile screens
            configure_mobile_ui(&cc.egui_ctx);
            
            Box::new(WordCraftAndroidApp::new(cc))
        }),
    );
}

/// Sets up scaling and spacing parameters for optimal mobile viewing
fn configure_mobile_ui(ctx: &egui::Context) {
    // 1. Force a higher pixels_per_point factor so desktop UI elements are legible on high-DPI phone screens
    let native_ppp = ctx.pixels_per_point();
    if native_ppp < 1.8 {
        ctx.set_pixels_per_point(2.0);
    }

    // 2. Adjust global style spacing and margins to prevent dialog popups from clipping offscreen
    let mut style = (*ctx.style()).clone();
    
    // Ensure window margins fit within mobile viewports
    style.spacing.window_margin = egui::Margin::same(12.0);
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(10.0, 8.0);
    style.spacing.interact_size.y = style.spacing.interact_size.y.max(32.0); // Make touch targets larger
    
    ctx.set_style(style);
}

pub struct WordCraftAndroidApp {
    inner: WordCraftApp,
}

impl WordCraftAndroidApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            inner: WordCraftApp::new(cc),
        }
    }
}

impl eframe::App for WordCraftAndroidApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Enforce scaling configuration continuously during frame renders
        configure_mobile_ui(ctx);

        // Render core WordCraft application UI
        self.inner.update(ctx, frame);
    }
}
