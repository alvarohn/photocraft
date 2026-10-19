//! The providers Generative AI Settings offers and each one's own settings: Google Gemini and
//! OpenAI (an API key each), and a stable-diffusion.cpp server the user runs (an address, no
//! key). The provider, its model and its options are kept in preferences (`dialogs` entry
//! `ui.generativeAiSettings`: `{"provider", "model", "options": {<provider>: {…}}}`); keys only in
//! the credential store.

use photocraft_genai::{gemini, openai};
use serde_json::{Map, Value, json};

use crate::PhotocraftApp;

/// Providers the settings offer: (id, name). Only providers the app can build appear.
pub(super) fn list() -> Vec<(String, &'static str)> {
    vec![(String::new(), tl!("None")), (gemini::ID.into(), gemini::NAME), (openai::ID.into(), openai::NAME)]
}

/// `provider`'s models, best default first.
fn models(provider: &str) -> &'static [(&'static str, &'static str)] {
    match provider {
        openai::ID => openai::MODELS,
        _ => gemini::MODELS,
    }
}

/// `provider`'s models, as dropdown options.
pub(super) fn model_list(provider: &str) -> Vec<(String, &'static str)> {
    models(provider).iter().map(|(id, name)| ((*id).to_owned(), *name)).collect()
}

pub(super) fn default_model(provider: &str) -> &'static str {
    match provider {
        openai::ID => openai::DEFAULT_MODEL,
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
/// (the chosen one's replaced, with its model, so switching back finds it). Keep what is sent and
/// returned stays as it was.
pub(super) fn remembered(app: &PhotocraftApp, provider: &str, model: &str, options: &Value) -> Value {
    let keep = super::keeps(app);
    let mut all = app.session.prefs().dialogs.get(super::PREF).and_then(|v| v.get("options")).and_then(Value::as_object).cloned().unwrap_or_default();
    // The provider in use until now keeps its model for switching back: settings saved before each
    // provider kept its own options held only the one in use.
    let (current, current_model) = super::saved(app);
    if !current.is_empty() && current != provider && !current_model.is_empty() {
        let entry = all.entry(current).or_insert_with(|| json!({}));
        if let Some(o) = entry.as_object_mut() {
            o.insert("model".into(), json!(current_model));
        }
    }
    if !provider.is_empty() {
        let mut o = options.as_object().cloned().unwrap_or_default();
        o.insert("model".into(), json!(model));
        all.insert(provider.into(), Value::Object(o));
    }
    json!({"provider": provider, "model": model, "options": all, "keep": keep})
}

/// Fill the dialog's fields for `provider` (on open, and when another provider is picked).
pub(super) fn load_fields(app: &PhotocraftApp, f: &mut Map<String, Value>, provider: &str) {
    let has_key = app.services.genai.as_ref().and_then(|s| (s.load_key)(provider).ok().flatten()).is_some();
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
    if crate::widgets::dropdown(ui, "genai-model", &mut model, &model_list(provider), 220.0) {
        f.insert("model".into(), json!(model));
    }
    ui.end_row();
    key_row(app, ui, f, provider);
    if provider == openai::ID {
        openai_rows(ui, f, &model);
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
        openai::ID => faint(
            ui,
            tl!(
                "Nothing is sent until you run Generative Fill. Each time you do, PhotoCraft sends the selected area and the image around it, a mask of the selection, and your prompt to OpenAI (api.openai.com) with your API key. OpenAI's API terms and privacy policy apply, and its usage is billed to your key."
            ),
        ),
        _ => faint(
            ui,
            tl!(
                "Nothing is sent until you run Generative Fill. Each time you do, PhotoCraft sends the selected area and the image around it, a black-and-white mask of the selection, and your prompt to Google (generativelanguage.googleapis.com) with your API key. Google's Gemini API terms and privacy policy apply, and its usage is billed to your key. Results carry Google's SynthID watermark."
            ),
        ),
    }
    ui.add_space(4.0);
    faint(ui, tl!("In Edit mode the selected area is sent as it is. Reference images you add are sent too."));
    ui.add_space(4.0);
    faint(ui, tl!("Each Generative Fill sends one request per variation (1 to 4, chosen in its dialog), each billed to your key."));
    ui.add_space(4.0);
    let store = app.services.genai.as_ref().map_or("", |s| s.key_store_name);
    faint(ui, &crate::i18n::fmt(tl!("Your key is kept in {store}, never in PhotoCraft's settings files."), &[("store", tl!(store))]));
}
