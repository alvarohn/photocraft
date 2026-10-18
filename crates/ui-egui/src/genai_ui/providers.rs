//! The providers Generative AI Settings offers and each one's own settings: Google Gemini and
//! OpenAI (an API key each), and a stable-diffusion.cpp server the user runs (an address, no
//! key). The provider, its model and its options are kept in preferences (`dialogs` entry
//! `ui.generativeAiSettings`: `{"provider", "model", "options": {<provider>: {…}}}`); keys only in
//! the credential store.

use photocraft_genai::{gemini, openai, sdcpp};
use serde_json::{Map, Value, json};

use crate::PhotocraftApp;

/// Providers the settings offer: (id, name). Only providers the app can build appear.
pub(super) fn list() -> Vec<(String, &'static str)> {
    vec![
        (String::new(), tl!("None")),
        (gemini::ID.into(), gemini::NAME),
        (openai::ID.into(), openai::NAME),
        (sdcpp::ID.into(), tl!("stable-diffusion.cpp (local)")),
    ]
}

/// Does `provider` need an API key? A local server takes none.
pub fn needs_key(provider: &str) -> bool {
    provider != sdcpp::ID
}

/// `provider`'s models (for the local server: what model it runs), best default first.
fn models(provider: &str) -> &'static [(&'static str, &'static str)] {
    match provider {
        openai::ID => openai::MODELS,
        sdcpp::ID => sdcpp::PRESETS,
        _ => gemini::MODELS,
    }
}

pub(super) fn default_model(provider: &str) -> &'static str {
    match provider {
        openai::ID => openai::DEFAULT_MODEL,
        sdcpp::ID => sdcpp::DEFAULT_PRESET,
        _ => gemini::DEFAULT_MODEL,
    }
}

/// Where a key for `provider` is made.
fn key_page(provider: &str) -> Option<(&'static str, &'static str)> {
    match provider {
        gemini::ID => Some(("https://aistudio.google.com/apikey", tl!("Get a Gemini API key from Google AI Studio"))),
        openai::ID => Some(("https://platform.openai.com/api-keys", tl!("Get an OpenAI API key"))),
        _ => None,
    }
}

/// The saved options of `provider` (an object; empty when none were saved).
pub(super) fn saved_options(app: &PhotocraftApp, provider: &str) -> Value {
    let v = app.session.prefs().dialogs.get(super::PREF).and_then(|v| v.get("options")).and_then(|o| o.get(provider));
    v.filter(|v| v.is_object()).cloned().unwrap_or_else(|| json!({}))
}

/// The model saved for `provider`: the current one if it is the chosen provider, else the one it
/// had when last chosen, else its default.
pub(super) fn saved_model(app: &PhotocraftApp, provider: &str) -> String {
    let (chosen, model) = super::saved(app);
    if chosen == provider && !model.is_empty() {
        return model;
    }
    let kept = saved_options(app, provider).get("model").and_then(Value::as_str).map(str::to_owned);
    kept.unwrap_or_else(|| default_model(provider).to_owned())
}

/// The preference entry after OK: the provider and model chosen, and every provider's options
/// (the chosen one's replaced, with its model, so switching back finds it).
pub(super) fn remembered(app: &PhotocraftApp, provider: &str, model: &str, options: &Value) -> Value {
    let mut all = app.session.prefs().dialogs.get(super::PREF).and_then(|v| v.get("options")).and_then(Value::as_object).cloned().unwrap_or_default();
    if !provider.is_empty() {
        let mut o = options.as_object().cloned().unwrap_or_default();
        o.insert("model".into(), json!(model));
        all.insert(provider.into(), Value::Object(o));
    }
    json!({"provider": provider, "model": model, "options": all})
}

/// Fill the dialog's fields for `provider` (on open, and when another provider is picked).
pub(super) fn load_fields(app: &PhotocraftApp, f: &mut Map<String, Value>, provider: &str) {
    let has_key = needs_key(provider) && app.services.genai.as_ref().and_then(|s| (s.load_key)(provider).ok().flatten()).is_some();
    f.insert("provider".into(), json!(provider));
    f.insert("model".into(), json!(saved_model(app, provider)));
    f.insert("options".into(), saved_options(app, provider));
    f.insert("hasKey".into(), json!(has_key));
}

fn opt<'a>(f: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    f.get("options").and_then(|o| o.get(key))
}

fn set_opt(f: &mut Map<String, Value>, key: &str, value: Value) {
    let mut o = f.get("options").and_then(Value::as_object).cloned().unwrap_or_default();
    o.insert(key.into(), value);
    f.insert("options".into(), Value::Object(o));
}

/// A dropdown row over an option's text values.
fn choice(ui: &mut egui::Ui, f: &mut Map<String, Value>, id: &str, label: &str, key: &str, options: &[(String, &str)], default: &str) {
    ui.label(label);
    let mut v = opt(f, key).and_then(Value::as_str).filter(|v| options.iter().any(|(o, _)| o == v)).unwrap_or(default).to_owned();
    if crate::widgets::dropdown(ui, id, &mut v, options, 220.0) {
        set_opt(f, key, json!(v));
    }
    ui.end_row();
}

fn faint(ui: &mut egui::Ui, text: &str) {
    super::faint(ui, text);
}

/// The provider's rows in the settings grid, after Provider: its model, its key (if it takes one)
/// and its options.
pub(super) fn rows(app: &mut PhotocraftApp, ui: &mut egui::Ui, f: &mut Map<String, Value>, provider: &str) {
    if provider.is_empty() {
        return;
    }
    ui.label(tl!("Model"));
    let mut model = f.get("model").and_then(Value::as_str).unwrap_or(default_model(provider)).to_owned();
    let list: Vec<(String, &str)> = models(provider).iter().map(|(id, name)| ((*id).to_owned(), *name)).collect();
    if crate::widgets::dropdown(ui, "genai-model", &mut model, &list, 220.0) {
        f.insert("model".into(), json!(model));
    }
    ui.end_row();
    if needs_key(provider) {
        key_row(app, ui, f, provider);
    }
    match provider {
        openai::ID => openai_rows(ui, f, &model),
        sdcpp::ID => sdcpp_rows(ui, f, &model),
        _ => {}
    }
}

fn key_row(app: &mut PhotocraftApp, ui: &mut egui::Ui, f: &mut Map<String, Value>, provider: &str) {
    ui.label(tl!("API key"));
    let has_key = f.get("hasKey").and_then(Value::as_bool) == Some(true);
    ui.horizontal(|ui| {
        let hint = match (has_key, provider) {
            (true, _) => tl!("Saved (paste to replace)"),
            (false, openai::ID) => tl!("Paste your OpenAI API key"),
            (false, _) => tl!("Paste your Gemini API key"),
        };
        let r = ui.add(egui::TextEdit::singleline(&mut app.genai.key_input).password(true).hint_text(hint).desired_width(220.0));
        crate::field_tab::register(ui.ctx(), r.id);
        if has_key && crate::widgets::secondary_button(ui, tl!("Remove Key"), 0.0).clicked() {
            let removed = app.services.genai.as_ref().map(|s| (s.delete_key)(provider));
            match removed {
                Some(Ok(())) => {
                    f.insert("hasKey".into(), json!(false));
                    app.session.set_image_provider(None);
                }
                Some(Err(e)) => app.ui.status = e,
                None => {}
            }
        }
    });
    ui.end_row();
}

fn openai_rows(ui: &mut egui::Ui, f: &mut Map<String, Value>, model: &str) {
    let o = openai::Options::from_json(f.get("options").unwrap_or(&Value::Null));
    let qualities: Vec<(String, &str)> = openai::qualities(model)
        .iter()
        .map(|q| {
            let label = match *q {
                "auto" => tl!("Auto"),
                "low" => tl!("Low"),
                "medium" => tl!("Medium"),
                "high" => tl!("High"),
                "xhigh" => tl!("Very high"),
                _ => tl!("Maximum"),
            };
            ((*q).to_owned(), label)
        })
        .collect();
    // A quality the model doesn't take (picked for another model) falls back to High.
    let quality = if openai::qualities(model).contains(&o.quality.as_str()) { o.quality.clone() } else { "high".into() };
    choice(ui, f, "genai-openai-quality", tl!("Quality"), "quality", &qualities, &quality);
    if openai::flexible_sizes(model) {
        ui.label(tl!("Working resolution"));
        let mut side = o.resolution;
        let sides: Vec<(u32, &str)> = openai::RESOLUTIONS.iter().map(|(s, l)| (*s, tl!(*l))).collect();
        if !sides.iter().any(|(s, _)| *s == side) {
            side = 2048;
        }
        if crate::widgets::dropdown(ui, "genai-openai-resolution", &mut side, &sides, 220.0) {
            set_opt(f, "resolution", json!(side));
        }
        ui.end_row();
    }
    let formats: Vec<(String, &str)> = vec![("png".into(), "PNG"), ("jpeg".into(), "JPEG"), ("webp".into(), "WebP")];
    choice(ui, f, "genai-openai-format", tl!("Format"), "format", &formats, &o.format);
    if o.format != "png" {
        ui.label(tl!("Quality (compression)"));
        let mut c = f32::from(o.compression);
        if crate::widgets::value_field(ui, &mut c, 0.0..=100.0, "%", 64.0).changed() {
            set_opt(f, "compression", json!(c.round().clamp(0.0, 100.0) as u64));
        }
        ui.end_row();
    }
    if openai::takes_input_fidelity(model) {
        let fidelity: Vec<(String, &str)> = vec![(String::new(), tl!("Default (low)")), ("high".into(), tl!("High")), ("low".into(), tl!("Low"))];
        choice(ui, f, "genai-openai-fidelity", tl!("Input fidelity"), "inputFidelity", &fidelity, o.input_fidelity.as_deref().unwrap_or(""));
    }
}

fn sdcpp_rows(ui: &mut egui::Ui, f: &mut Map<String, Value>, preset: &str) {
    let mut v = f.get("options").cloned().unwrap_or(Value::Null);
    if let Some(o) = v.as_object_mut() {
        o.insert("preset".into(), json!(preset));
    }
    let o = sdcpp::Options::from_json(&v);
    ui.label(tl!("Server address"));
    let mut url = opt(f, "url").and_then(Value::as_str).unwrap_or(sdcpp::DEFAULT_URL).to_owned();
    let r = ui.add(egui::TextEdit::singleline(&mut url).hint_text(sdcpp::DEFAULT_URL).desired_width(220.0));
    crate::field_tab::register(ui.ctx(), r.id);
    if r.changed() {
        set_opt(f, "url", json!(url));
    }
    ui.end_row();
    ui.label(tl!("Steps"));
    let mut steps = o.steps as f32;
    if crate::widgets::value_field(ui, &mut steps, 0.0..=100.0, "", 64.0).on_hover_text(tl!("0: the server's own setting")).changed() {
        set_opt(f, "steps", json!(steps.round().clamp(0.0, 100.0) as u64));
    }
    ui.end_row();
    ui.label(tl!("Guidance (CFG)"));
    let mut cfg = o.cfg.unwrap_or(0.0);
    if crate::widgets::value_field_prec(ui, &mut cfg, 0.0..=30.0, "", 64.0, 1).on_hover_text(tl!("0: the server's own setting")).changed() {
        set_opt(f, "cfg", if cfg > 0.0 { json!(cfg) } else { Value::Null });
    }
    ui.end_row();
    ui.label(tl!("Fill strength"));
    let mut strength = o.strength * 100.0;
    if crate::widgets::value_field(ui, &mut strength, 5.0..=100.0, "%", 64.0)
        .on_hover_text(tl!("How much of the selected area Fill makes anew; 100%: all of it"))
        .changed()
    {
        set_opt(f, "strength", json!((strength / 100.0).clamp(0.05, 1.0)));
    }
    ui.end_row();
    ui.label(tl!("Working size"));
    let mut side = o.work_side;
    let sides: Vec<(u32, String)> = [256u32, 384, 512, 640, 768, 1024, 1536, 2048].iter().map(|s| (*s, format!("{s} px"))).collect();
    if !sides.iter().any(|(s, _)| *s == side) {
        side = sdcpp::DEFAULT_SIDE;
    }
    let labels: Vec<(u32, &str)> = sides.iter().map(|(s, l)| (*s, l.as_str())).collect();
    if crate::widgets::dropdown_literal(ui, "genai-sdcpp-side", &mut side, &labels, 220.0) {
        set_opt(f, "workSide", json!(side));
    }
    ui.end_row();
    if preset == "other" {
        ui.label("");
        let mut on = o.instructions;
        if crate::widgets::checkbox(ui, &mut on, tl!("It edits from instructions and takes reference images")).changed() {
            set_opt(f, "instructions", json!(on));
        }
        ui.end_row();
    }
}

/// The notes under the grid: where to get a key, the model's licence, what is sent and to whom.
pub(super) fn notes(app: &mut PhotocraftApp, ui: &mut egui::Ui, f: &Map<String, Value>, provider: &str) {
    if provider.is_empty() {
        return;
    }
    let t = crate::theme::Tokens::get(ui.ctx());
    let model = f.get("model").and_then(Value::as_str).unwrap_or(default_model(provider)).to_owned();
    if let Some((url, label)) = key_page(provider) {
        ui.add_space(4.0);
        if ui.link(label).clicked() {
            crate::links::open(app, &ui.ctx().clone(), url);
        }
    }
    if provider == sdcpp::ID {
        ui.add_space(4.0);
        faint(
            ui,
            tl!(
                "Start sd-server with the model first; PhotoCraft sends it one request at a time. Each image takes as long as the server needs (minutes on a laptop), at the working size, scaled up to the selection."
            ),
        );
        match model.as_str() {
            "qwen-image-2.1-turbo" => {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(tl!("Qwen Image 2.1 Turbo is released under the Qwen Research licence: for non-commercial use only."))
                        .color(t.warning)
                        .size(12.0),
                );
            }
            "z-image-turbo" => {
                ui.add_space(4.0);
                faint(ui, tl!("Z-Image Turbo (Apache 2.0) fills selections from a description; it doesn't edit from instructions or take reference images."));
            }
            _ => {}
        }
    }
    if provider == openai::ID {
        let o = openai::Options::from_json(f.get("options").unwrap_or(&Value::Null));
        let size = openai::output_size(&model, &o, 1600, 900);
        ui.add_space(4.0);
        let cost = match openai::estimated_cost(&model, &o.quality, size) {
            Some(usd) => crate::i18n::fmt(
                tl!("About US${cost} a variation at {w}×{h} (OpenAI's published prices; the images sent are billed too)."),
                &[("cost", &format!("{usd:.3}")), ("w", &size.0.to_string()), ("h", &size.1.to_string())],
            ),
            None => tl!("OpenAI publishes no price for this quality; see its pricing page.").to_owned(),
        };
        faint(ui, &cost);
    }
    ui.add_space(8.0);
    crate::widgets::hairline(ui);
    ui.add_space(6.0);
    ui.label(egui::RichText::new(tl!("What is sent, and to whom")).font(crate::theme::semibold(12.5)).color(t.text));
    ui.add_space(2.0);
    match provider {
        gemini::ID => faint(
            ui,
            tl!(
                "Nothing is sent until you run Generative Fill. Each time you do, PhotoCraft sends the selected area and the image around it, a black-and-white mask of the selection, and your prompt to Google (generativelanguage.googleapis.com) with your API key. Google's Gemini API terms and privacy policy apply, and its usage is billed to your key. Results carry Google's SynthID watermark."
            ),
        ),
        openai::ID => faint(
            ui,
            tl!(
                "Nothing is sent until you run Generative Fill. Each time you do, PhotoCraft sends the selected area and the image around it, a mask of the selection, and your prompt to OpenAI (api.openai.com) with your API key. OpenAI's API terms and privacy policy apply, and its usage is billed to your key."
            ),
        ),
        _ => {
            let url = f.get("options").and_then(|o| o.get("url")).and_then(Value::as_str).unwrap_or(sdcpp::DEFAULT_URL).to_owned();
            faint(
                ui,
                &crate::i18n::fmt(
                    tl!(
                        "Nothing is sent until you run Generative Fill. Each time you do, PhotoCraft sends the selected area and the image around it, a black-and-white mask of the selection, and your prompt to the stable-diffusion.cpp server at {url}, without a key. At 127.0.0.1 (this computer) nothing leaves it."
                    ),
                    &[("url", &url)],
                ),
            );
        }
    }
    ui.add_space(4.0);
    faint(ui, tl!("In Edit mode the selected area is sent as it is. Reference images you add are sent too."));
    ui.add_space(4.0);
    if needs_key(provider) {
        faint(ui, tl!("Each Generative Fill sends one request per variation (1 to 4, chosen in its dialog), each billed to your key."));
        ui.add_space(4.0);
        let store = app.services.genai.as_ref().map_or("", |s| s.key_store_name);
        faint(ui, &crate::i18n::fmt(tl!("Your key is kept in {store}, never in PhotoCraft's settings files."), &[("store", tl!(store))]));
    } else {
        faint(ui, tl!("Each Generative Fill sends one request per variation (1 to 4, chosen in its dialog), one after another."));
    }
}
