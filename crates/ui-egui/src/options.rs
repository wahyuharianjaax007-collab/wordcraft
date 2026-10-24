//! File › Options (#485): a list of panes on the left (General, Display, Proofing, Save,
//! Language, Accessibility, Advanced, Agents), the chosen pane's settings on the right. Only
//! settings WordCraft carries out are shown. Each pane is one function below, so a new setting
//! goes into its pane's function. Settings agents can change too go through `file.options`; the
//! rest live in [`crate::UiState`] (saved in `ui.json`).

use std::cell::Cell;

use egui::{RichText, Ui, vec2};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use wordcraft_engine::cmd::options::{MAX_WORD_CHARS, PANES};
use wordcraft_geom::Unit;

use crate::WordApp;
use crate::theme::{Tokens, semibold};

thread_local! {
    static UNIT: Cell<Unit> = const { Cell::new(Unit::Inches) };
    static DECIMAL: Cell<char> = const { Cell::new('.') };
}

pub fn unit() -> Unit {
    UNIT.get()
}

pub fn decimal() -> char {
    DECIMAL.get()
}

pub fn length(pt: f32) -> String {
    length_in(unit(), pt)
}

pub fn length_in(unit: Unit, pt: f32) -> String {
    format!("{}{}", unit.number(pt, decimal()), unit_suffix(unit))
}

pub fn unit_suffix(unit: Unit) -> String {
    match unit {
        Unit::Inches => unit.suffix().to_string(),
        Unit::Centimeters => format!(" {}", tl!("cm")),
        Unit::Millimeters => format!(" {}", tl!("mm")),
        Unit::Points => format!(" {}", tl!("pt")),
        Unit::Picas => format!(" {}", tl!("pi")),
    }
}

pub fn parse_length(text: &str, unit: Unit) -> Option<f64> {
    let mut text: String = text.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_lowercase();
    for (english, translated) in [("cm", tl!("cm")), ("mm", tl!("mm")), ("pt", tl!("pt")), ("pi", tl!("pi"))] {
        let translated = translated.to_lowercase();
        if translated != english
            && let Some(number) = text.strip_suffix(translated.as_str())
        {
            text = format!("{number}{english}");
            break;
        }
    }
    wordcraft_geom::parse_measure(&text, unit).map(|pt| f64::from(pt / unit.pt_per_unit()))
}

pub(crate) fn sync(app: &WordApp) {
    UNIT.set(app.session.units());
    DECIMAL.set(app.session.region().decimal());
}

pub trait MeasureExt {
    fn measure(self, unit: Unit) -> Self;
    fn measure_blank(self, unit: Unit) -> Self;
}

impl MeasureExt for egui::DragValue<'_> {
    fn measure(self, unit: Unit) -> Self {
        let sep = decimal();
        self.suffix(unit_suffix(unit))
            .custom_formatter(move |n, decimals| {
                let s = egui::emath::format_with_decimals_in_range(n, decimals);
                if sep == '.' { s } else { s.replace('.', &sep.to_string()) }
            })
            .custom_parser(move |text| parse_length(text, unit))
    }
    fn measure_blank(self, unit: Unit) -> Self {
        self.custom_formatter(|_, _| String::new()).custom_parser(move |text| parse_length(text, unit))
    }
}

const REGIONS: [&str; 16] =
    ["en-US", "en-CA", "en-GB", "en-AU", "en-IN", "es-MX", "es-ES", "pt-BR", "fr-FR", "de-DE", "it-IT", "pl-PL", "ru-RU", "ja-JP", "zh-CN", "ko-KR"];

pub const UNITS: [(Unit, &str); 5] = [
    (Unit::Inches, "Inches"),
    (Unit::Centimeters, "Centimeters"),
    (Unit::Millimeters, "Millimeters"),
    (Unit::Points, "Points"),
    (Unit::Picas, "Picas"),
];

fn pane_label(id: &str) -> &'static str {
    match id {
        "display" => "Display",
        "proofing" => "Proofing",
        "save" => "Save",
        "language" => "Language",
        "accessibility" => "Accessibility",
        "advanced" => "Advanced",
        "agents" => "Agents",
        _ => "General",
    }
}

/// The Options page.
pub fn page(app: &mut WordApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    ui.label(RichText::new(tl!("Options")).font(semibold(26.0)));
    ui.add_space(16.0);
    let current = if PANES.contains(&app.ui.options_pane.as_str()) { app.ui.options_pane.clone() } else { "general".to_string() };
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(170.0);
            for id in PANES {
                let label = tl!(pane_label(id));
                let active = current == id;
                let (r, resp) = ui.allocate_exact_size(vec2(170.0, 30.0), egui::Sense::click());
                resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, active, label));
                if active {
                    ui.painter().rect_filled(r, 4.0, t.checked);
                } else if resp.hovered() {
                    ui.painter().rect_filled(r, 4.0, t.hover);
                }
                let font = if active { semibold(14.0) } else { crate::theme::regular(14.0) };
                ui.painter().with_clip_rect(r).text(egui::pos2(r.min.x + 12.0, r.center().y), egui::Align2::LEFT_CENTER, label, font, t.text);
                if resp.clicked() {
                    app.ui.options_pane = id.to_string();
                }
            }
        });
        ui.add_space(12.0);
        let (line, _) = ui.allocate_exact_size(vec2(1.0, 420.0), egui::Sense::hover());
        ui.painter().rect_filled(line, 0.0, t.border);
        ui.add_space(20.0);
        
        egui::ScrollArea::horizontal().id_salt("options_scroll_right").show(ui, |ui| {
            ui.vertical(|ui| {
                ui.set_max_width(640.0);
                match current.as_str() {
                    "display" => display(app, ui),
                    "proofing" => proofing(app, ui),
                    "save" => save(app, ui),
                    "language" => language(app, ui),
                    "accessibility" => accessibility(app, ui),
                    "advanced" => advanced(app, ui),
                    "agents" => agents(ui),
                    _ => general(app, ui),
                }
            });
        });
    });
}

fn section(ui: &mut Ui, title: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(tl!(title)).font(semibold(15.0)));
    ui.add_space(2.0);
}

fn note(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(tl!(text)).small().weak());
}

fn set(app: &mut WordApp, params: Value) {
    if let Err(e) = app.run("file.options", params) {
        app.status(e);
    }
}

fn flag(app: &mut WordApp, ui: &mut Ui, label: &str, key: &str, value: bool) {
    let mut v = value;
    if ui.checkbox(&mut v, tl!(label)).changed() {
        set(app, json!({ key: v }));
    }
}

fn general(app: &mut WordApp, ui: &mut Ui) {
    section(ui, "Interface");
    theme_picker(app, ui);
    interface_size_picker(app, ui);
    ui.checkbox(&mut app.ui.mini_toolbar, tl!("Show the mini toolbar when text is selected"));
    let mut dark_page = app.session.view.dark_mode;
    if ui
        .checkbox(&mut dark_page, tl!("Dark page (white text on black)"))
        .on_hover_text(tl!("Show documents with their colours inverted, like View › Switch Modes. Saving, printing and PDFs are unchanged."))
        .changed()
    {
        let _ = app.run("view.darkMode", json!({"value": dark_page}));
    }
    ui.checkbox(&mut app.ui.show_discord, tl!("Show the community button in the title bar"));
    default_app_button(app, ui);
    ui.add_space(8.0);
    section(ui, "Start-up");
    let mut start = app.ui.start_screen;
    if ui
        .checkbox(&mut start, tl!("Show the Start screen when WordCraft starts"))
        .on_hover_text(tl!(
            "On: WordCraft opens on File › Home, with a blank document, the templates and your recent documents. Off: it opens a blank document."
        ))
        .changed()
    {
        let _ = app.run("ui.startScreen", json!({"value": start}));
    }
    ui.add_space(8.0);
    section(ui, "Personalize");
    egui::Grid::new("options_user").num_columns(2).spacing(vec2(8.0, 6.0)).show(ui, |ui| {
        ui.label(tl!("User name:"));
        let mut n = app.session.author.clone();
        let protected = wordcraft_engine::cmd::protect::enforced(&app.session.doc);
        if ui.add_enabled(!protected, egui::TextEdit::singleline(&mut n).desired_width(260.0)).changed() && !protected {
            app.session.author = n;
        }
        ui.end_row();
        ui.label(tl!("Initials:"));
        let mut i = app.session.initials.clone();
        let hint = app.session.user_initials();
        if ui
            .add(
                egui::TextEdit::singleline(&mut i).desired_width(80.0).char_limit(wordcraft_engine::cmd::options::MAX_INITIALS_CHARS).hint_text(hint),
            )
            .changed()
        {
            set(app, json!({"initials": i}));
        }
        ui.end_row();
    });
    note(ui, "New comments and tracked changes are recorded under this name and these initials.");
}

fn display(app: &mut WordApp, ui: &mut Ui) {
    section(ui, "Formatting marks");
    note(ui, "These marks show even when Show/Hide ¶ is off.");
    let m = app.session.prefs.marks;
    for (label, key, on) in [
        ("Tab characters", "tabs", m.tabs),
        ("Spaces", "spaces", m.spaces),
        ("Paragraph marks", "paragraphs", m.paragraphs),
        ("Hidden text", "hidden", m.hidden),
    ] {
        let mut v = on;
        if ui.checkbox(&mut v, tl!(label)).changed() {
            set(app, json!({"marks": { key: v }}));
        }
    }
    let mut marks = app.session.view.marks;
    if ui.checkbox(&mut marks, tl!("Show all formatting marks")).changed() {
        let _ = app.run("view.marks", json!({"value": marks}));
    }
    ui.add_space(8.0);
    section(ui, "Page display");
    let mut ruler = app.session.view.ruler;
    if ui.checkbox(&mut ruler, tl!("Show rulers")).changed() {
        let _ = app.run("view.ruler", json!({"value": ruler}));
    }
    numeral_picker(app, ui);
    ui.add_space(8.0);
    section(ui, "Printing");
    ui.checkbox(&mut app.ui.print_backgrounds, tl!("Print the page color"));
    ui.checkbox(&mut app.ui.update_fields_before_print, tl!("Update fields before printing"));
}

fn proofing(app: &mut WordApp, ui: &mut Ui) {
    section(ui, "Spelling");
    let pr = app.session.prefs.clone();
    flag(app, ui, "Ignore words in capitals", "ignoreUppercase", pr.ignore_uppercase);
    flag(app, ui, "Ignore words with numbers", "ignoreNumbers", pr.ignore_numbers);
    flag(app, ui, "Ignore web and file addresses", "ignoreInternet", pr.ignore_internet);
    ui.add_space(8.0);
    section(ui, "As you type");
    let spelling = app.session.view.proofing;
    flag(app, ui, "Check spelling as you type", "checkSpelling", spelling);
    ui.add_enabled_ui(app.session.view.proofing, |ui| flag(app, ui, "Mark grammar errors as you type", "markGrammar", pr.mark_grammar));
    ui.add_space(8.0);
    section(ui, "Custom dictionary");
    custom_dictionary(app, ui);
    ui.add_space(8.0);
    section(ui, "Dictionaries");
    dictionaries(app, ui);
}

fn dictionaries(app: &mut WordApp, ui: &mut Ui) {
    let saved = app.session.prefs.dictionaries_folder.clone().unwrap_or_default();
    let id = egui::Id::new("options_dictionaries_folder");
    let mut folder = ui.data(|d| d.get_temp::<String>(id)).unwrap_or_else(|| saved.clone());
    ui.label(tl!("Dictionaries folder:"));
    ui.horizontal_wrapped(|ui| {
        let r = ui.add(egui::TextEdit::singleline(&mut folder).desired_width(320.0).hint_text(tl!("Not set: the system's usual places")));
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let changed = folder.trim() != saved;
        if ui.add_enabled(changed, egui::Button::new(tl!("Apply"))).clicked() || (enter && changed) {
            let value = if folder.trim().is_empty() { "auto" } else { folder.trim() };
            match app.run("file.options", json!({"dictionariesFolder": value})) {
                Ok(_) => folder = app.session.prefs.dictionaries_folder.clone().unwrap_or_default(),
                Err(e) => app.status(e),
            }
        }
        if ui.add_enabled(!saved.is_empty(), egui::Button::new(tl!("Clear"))).clicked()
            && app.run("file.options", json!({"dictionariesFolder": "auto"})).is_ok()
        {
            folder.clear();
        }
    });
    ui.data_mut(|d| d.insert_temp(id, folder));
    note(ui, "Hunspell .aff and .dic files here (pl_PL, pt_BR, it_IT, hr_HR, tr_TR) are used before the system's.");
    let found_id = egui::Id::new("options_dictionaries_found");
    let generation = wordcraft_engine::proof::dictionary_generation();
    let cached = ui.data(|d| d.get_temp::<(u64, Vec<(String, Option<String>, Option<String>)>)>(found_id)).filter(|(g, _)| *g == generation);
    let found = match cached {
        Some((_, f)) => f,
        None => {
            let f: Vec<(String, Option<String>, Option<String>)> = wordcraft_engine::proof::installed_dictionaries()
                .into_iter()
                .map(|(lang, dic)| {
                    let error = wordcraft_engine::proof::dictionary_load_error(lang);
                    (language_name(lang).to_string(), dic.map(|d| d.to_string_lossy().into_owned()), error)
                })
                .collect();
            ui.data_mut(|d| d.insert_temp(found_id, (generation, f.clone())));
            f
        }
    };
    for (language, file, error) in found {
        let line = match (file, error) {
            (Some(f), Some(e)) => {
                crate::i18n::fmt(tl!("{language}: {file} (unreadable: {reason})"), &[("language", &language), ("file", &f), ("reason", &e)])
            }
            (Some(f), None) => crate::i18n::fmt(tl!("{language}: {file}"), &[("language", &language), ("file", &f)]),
            (None, _) => crate::i18n::fmt(tl!("{language}: no dictionary found"), &[("language", &language)]),
        };
        ui.add(egui::Label::new(RichText::new(line).small()).truncate());
    }
}

fn language_name(lang: wordcraft_engine::proof::ProofLang) -> &'static str {
    use wordcraft_engine::proof::ProofLang;
    match lang {
        ProofLang::Pl => tl!("Polish"),
        ProofLang::Pt => tl!("Portuguese (Brazil)"),
        ProofLang::It => tl!("Italian"),
        ProofLang::Hr => tl!("Croatian"),
        ProofLang::Tr => tl!("Turkish"),
        ProofLang::Ru => tl!("Russian"),
        ProofLang::Cs => tl!("Czech"),
        ProofLang::Id => tl!("Indonesian"),
        ProofLang::En | ProofLang::EnGb | ProofLang::EnCa | ProofLang::Bg | ProofLang::De | ProofLang::Other => "",
    }
}

fn custom_dictionary(app: &mut WordApp, ui: &mut Ui) {
    let words = wordcraft_engine::proof::user_dictionary();
    if words.is_empty() {
        note(ui, "No words yet. Add to Dictionary on a misspelled word puts it here.");
    }
    egui::ScrollArea::vertical().id_salt("options_dictionary").max_height(180.0).show(ui, |ui| {
        for w in &words {
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(vec2(220.0, 20.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.set_min_width(220.0);
                    ui.add(egui::Label::new(w.as_str()).truncate());
                });
                if ui.small_button(tl!("Remove")).clicked() {
                    set(app, json!({"removeWords": [w]}));
                }
            });
        }
    });
    let id = egui::Id::new("options_dictionary_new");
    let mut word = ui.data(|d| d.get_temp::<String>(id)).unwrap_or_default();
    ui.horizontal_wrapped(|ui| {
        let r = ui.add(egui::TextEdit::singleline(&mut word).desired_width(220.0).char_limit(MAX_WORD_CHARS).hint_text(tl!("New word")));
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let w = word.trim().to_string();
        let ok = !w.is_empty() && !w.chars().any(char::is_whitespace);
        if (ui.add_enabled(ok, egui::Button::new(tl!("Add"))).clicked() || (enter && ok)) && app.run("file.options", json!({"addWords": [w]})).is_ok()
        {
            word.clear();
        }
    });
    ui.data_mut(|d| d.insert_temp(id, word));
}

fn save(app: &mut WordApp, ui: &mut Ui) {
    section(ui, "Save documents");
    let browser = app.autosave_block() == Some(crate::AutoSaveBlock::Browser);
    let mut autosave = app.autosave && !browser;
    let r = ui.add_enabled(!browser, egui::Checkbox::new(&mut autosave, tl!("AutoSave documents you have saved in WordCraft")));
    if browser {
        r.on_disabled_hover_text(crate::AutoSaveBlock::Browser.reason());
    } else if r.changed() {
        app.autosave = autosave;
        app.session.autosave = autosave;
    }
    note(ui, "AutoSave writes your changes to the file a moment after you stop typing.");
    ui.add_space(8.0);
    section(ui, "Privacy for this document");
    let settings = &app.session.doc.settings;
    let rows = [
        ("Remove my name from comments, changes and file properties when saving", "removePersonalInfo", settings.remove_personal_info),
        ("Leave out the date and time of comments and tracked changes when saving", "removeDateTime", settings.remove_date_time),
    ];
    for (label, key, value) in rows {
        let mut v = value;
        if ui.checkbox(&mut v, tl!(label)).changed()
            && let Err(e) = app.run("file.privacyOptions", json!({ key: v }))
        {
            app.status(e);
        }
    }
    note(ui, "The document you are editing keeps the names and dates; only the saved file leaves them out.");
}

fn language(app: &mut WordApp, ui: &mut Ui) {
    section(ui, "Interface language");
    language_picker(app, ui);
    ui.add_space(8.0);
    section(ui, "Proofing language");
    let lang = crate::ribbon::caret_language(app);
    let text = if crate::ribbon::caret_no_proof(app) {
        tl!("The text at the insertion point is marked Do not check spelling or grammar.").to_string()
    } else if !crate::ribbon::caret_language_proofed(app) {
        crate::i18n::fmt(tl!("{language}: WordCraft doesn't check spelling or grammar in this language yet."), &[("language", &lang)])
    } else {
        let unreadable = crate::ribbon::caret_unreadable_dictionary(app);
        crate::chrome::dictionary_problem(&lang, &unreadable, crate::ribbon::caret_missing_dictionary(app))
            .unwrap_or_else(|| crate::i18n::fmt(tl!("Spelling and grammar are checked in {language}."), &[("language", &lang)]))
    };
    ui.label(RichText::new(text).small().weak());
}

fn accessibility(app: &mut WordApp, ui: &mut Ui) {
    use wordcraft_engine::speech::{MAX_RATE, MIN_RATE};
    section(ui, "Visual effects");
    animations_picker(app, ui);
    ui.add_space(8.0);
    section(ui, "Read Aloud");
    let mut rate = app.session.read_aloud.rate();
    ui.horizontal_wrapped(|ui| {
        ui.label(tl!("Speed"));
        let r = ui.add(egui::Slider::new(&mut rate, MIN_RATE..=MAX_RATE).step_by(0.05).custom_formatter(|v, _| format!("{v:.2}×")));
        if r.changed() {
            let _ = app.run("readAloud.speed", json!({"value": rate}));
        }
    });
    let mut skip = app.session.read_aloud.skip_citations;
    if ui.checkbox(&mut skip, tl!("Skip citations & bibliography")).changed() {
        let _ = app.run("readAloud.skipCitations", json!({"value": skip}));
    }
}

fn advanced(app: &mut WordApp, ui: &mut Ui) {
    section(ui, "Editing");
    let pr = app.session.prefs.clone();
    flag(app, ui, "Typing replaces selected text", "typingReplacesSelection", pr.typing_replaces_selection);
    flag(app, ui, "Use the Insert key to switch overtype on and off", "insertKeyOvertype", pr.insert_key_overtype);
    flag(app, ui, "Overtype (typing replaces the characters after the caret)", "overtype", pr.overtype);
    let mut guides = app.ui.alignment_guides;
    if ui
        .checkbox(&mut guides, tl!("Show alignment guides when moving pictures and shapes"))
        .on_hover_text(tl!("Moved objects snap to the page's edges and centre, the margins and other objects. Hold Alt to move freely."))
        .changed()
    {
        let _ = app.run("ui.alignmentGuides", json!({"value": guides}));
    }
    ui.add_space(8.0);
    section(ui, "Display");
