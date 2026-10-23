use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use photocraft_engine::Session;
use photocraft_genai::{ApiKey, Capabilities, EditRequest, GeneratedImage, ImageProvider, ProviderInfo};

use egui_kittest::kittest::Queryable;

use super::*;
use crate::{GenAiServices, Services};

const SECRET: &str = "AIza-ui-test-secret";

/// Fills with solid red at the request's size and remembers the key it was built with.
struct Fake {
    id: &'static str,
    model: String,
}

impl ImageProvider for Fake {
    fn info(&self) -> ProviderInfo {
        let name = if self.id == "openai" { "OpenAI" } else { "Google Gemini" };
        ProviderInfo { id: self.id, name, model: self.model.clone(), host: "example.invalid" }
    }
    fn capabilities(&self) -> Capabilities {
        // As the real ones: OpenAI takes 15 reference images, Gemini 4. A model named
        // `fills-only` (none is) fills from a description only, for the window's notes.
        let edits = self.model != "fills-only";
        let refs = if self.id == "openai" { 15 } else { 4 };
        Capabilities { generate: false, edit: true, variations: false, edit_in_place: edits, max_references: if edits { refs } else { 0 } }
    }
    fn edit(&self, r: &EditRequest) -> photocraft_genai::Result<GeneratedImage> {
        let (w, h) = photocraft_genai::png_size(&r.image).unwrap();
        let px: Vec<u8> = (0..w * h).flat_map(|_| [255u8, 0, 0, 255]).collect();
        let img = photocraft_codecs::Image::from_u8(w, h, photocraft_codecs::ChannelLayout::Rgba, px).unwrap();
        let bytes = photocraft_codecs::encode(&img, photocraft_codecs::Format::Png, &Default::default()).unwrap();
        Ok(GeneratedImage { bytes, mime: "image/png".into() })
    }
}

type Store = Rc<RefCell<HashMap<String, String>>>;

fn services(store: &Store) -> GenAiServices {
    let (a, b, c) = (store.clone(), store.clone(), store.clone());
    GenAiServices {
        load_key: Box::new(move |p| Ok(a.borrow().get(p).cloned())),
        save_key: Box::new(move |p, k| {
            b.borrow_mut().insert(p.into(), k.into());
            Ok(())
        }),
        delete_key: Box::new(move |p| {
            c.borrow_mut().remove(p);
            Ok(())
        }),
        provider: Box::new(|id, model, _options, key: Option<ApiKey>| {
            let id = if id == "openai" { "openai" } else { "gemini" };
            assert_eq!(key.expect("a provider is never built without its key").expose(), SECRET);
            Ok(Arc::new(Fake { id, model: model.into() }))
        }),
        key_store_name: "the test store",
    }
}

fn app_with(store: &Store) -> PhotocraftApp {
    let mut app = PhotocraftApp::new(Session::new(), Services { genai: Some(services(store)), ..Default::default() });
    app.run("file.new", json!({"width": 64, "height": 64, "background": "#ffffff"})).unwrap();
    app
}

fn fill_in_menu(app: &PhotocraftApp) -> bool {
    crate::menus::menu_items(app).iter().any(|i| i.id == FILL)
}

fn settings_dialog(app: &PhotocraftApp) -> Option<u64> {
    app.ui.dialogs.iter().find(|d| d.fields.contains_key(SETTINGS_MARK)).map(|d| d.id)
}

#[test]
fn off_by_default_with_no_menu_item_and_no_key_read() {
    let store = Store::default();
    let app = app_with(&store);
    assert!(app.session.image_provider().is_none());
    assert!(!fill_in_menu(&app), "no \"coming soon\" item before a provider is set up");
    // The web build (no services) has no settings at all.
    let mut web = PhotocraftApp::new(Session::new(), Services::default());
    assert!(restore(&mut web).is_ok());
    assert!(web.session.image_provider().is_none());
}

#[test]
fn settings_save_the_key_outside_the_dialog_and_install_the_provider() {
    let store = Store::default();
    let mut app = app_with(&store);
    // Clicking Generative Fill before setup opens the settings.
    let ctx = egui::Context::default();
    crate::menus::invoke(&mut app, &ctx, FILL, json!({})).unwrap();
    let id = settings_dialog(&app).expect("settings open");
    app.genai.key_input = format!("  {SECRET}\n");
    let fields = serde_json::to_string(&app.ui.dialogs).unwrap();
    assert!(!fields.contains(SECRET), "the key is never in dialog fields (ui.inspect)");
    let r = crate::dialogs::confirm(&mut app, id).unwrap();
    assert_eq!(r["ready"], true);
    assert_eq!(store.borrow().get("gemini").map(String::as_str), Some(SECRET));
    assert!(app.genai.key_input.is_empty());
    assert!(fill_in_menu(&app));
    let prefs = serde_json::to_string(&app.session.prefs().to_json()).unwrap();
    assert!(!prefs.contains(SECRET), "never in preferences");
    assert!(prefs.contains(photocraft_genai::gemini::DEFAULT_MODEL));

    // A fresh launch restores it from the store.
    let mut next = PhotocraftApp::new(Session::new(), Services { genai: Some(services(&store)), ..Default::default() });
    next.session.prefs.edit(|p| p.dialogs = app.session.prefs().dialogs.clone());
    restore(&mut next).unwrap();
    assert!(next.session.image_provider().is_some());
}

#[test]
fn choosing_none_turns_it_off_and_cancel_forgets_the_typed_key() {
    let store = Store::default();
    store.borrow_mut().insert("gemini".into(), SECRET.into());
    let mut app = app_with(&store);
    let id = open_settings(&mut app);
    assert_eq!(app.ui.dialogs.iter().find(|d| d.id == id).unwrap().fields["hasKey"], true);
    crate::dialogs::confirm(&mut app, id).unwrap();
    assert!(app.session.image_provider().is_some(), "the saved key is used");

    let id = open_settings(&mut app);
    app.genai.key_input = "typed but cancelled".into();
    crate::dialogs::cancel(&mut app, id).unwrap();
    assert!(app.genai.key_input.is_empty());

    let id = open_settings(&mut app);
    app.ui.dialog_mut(id).unwrap().fields.insert("provider".into(), json!(""));
    crate::dialogs::confirm(&mut app, id).unwrap();
    assert!(app.session.image_provider().is_none());
    assert!(!fill_in_menu(&app));
}

/// Picks `provider` in the open settings dialog `id`, as its dropdown does.
fn pick_provider(app: &mut PhotocraftApp, id: u64, provider: &str) {
    let mut f = app.ui.dialogs.iter().find(|d| d.id == id).unwrap().fields.clone();
    providers::load_fields(app, &mut f, provider);
    app.ui.dialog_mut(id).unwrap().fields = f;
    app.genai.key_provider = provider.into();
}

/// A key typed for one provider is never saved under another, even if the dialog's fields
/// (which automation can set) name another by the time OK is pressed; and automation can't
/// change these settings at all (it could send the user's fills to a server of its choosing).
#[test]
fn keys_stay_with_their_provider_and_automation_cannot_change_the_settings() {
    let store = Store::default();
    let mut app = app_with(&store);
    let id = open_settings(&mut app);
    app.genai.key_input = SECRET.into();
    // As `ui.dialog.set` would: the provider field changed under the typed key.
    app.ui.dialog_mut(id).unwrap().fields.insert("provider".into(), json!("openai"));
    let _ = crate::dialogs::confirm(&mut app, id);
    assert!(store.borrow().get("openai").is_none(), "typed for Gemini, never saved for OpenAI");
    assert!(store.borrow().get("gemini").is_none());
    assert!(app.genai.key_input.is_empty());

    // Through the control channel (a session gate installed): refused, nothing kept.
    store.borrow_mut().insert("openai".into(), SECRET.into());
    let id = open_settings(&mut app);
    pick_provider(&mut app, id, "openai");
    let before = app.session.prefs().dialogs.get(PREF).cloned();
    app.session.authorize = Some(|_, _| Ok(()));
    let e = crate::dialogs::confirm(&mut app, id).unwrap_err();
    assert!(e.contains("automation"), "{e}");
    app.session.authorize = None;
    assert_eq!(app.session.prefs().dialogs.get(PREF).cloned(), before);
    assert!(app.session.image_provider().is_none());
}

/// OpenAI has its own model, options and key; going back to Gemini finds Gemini's as they were.
#[test]
fn openai_keeps_its_own_model_options_and_key() {
    let store = Store::default();
    store.borrow_mut().insert("gemini".into(), SECRET.into());
    let mut app = app_with(&store);
    let id = open_settings(&mut app);
    app.ui.dialog_mut(id).unwrap().fields.insert("model".into(), json!("gemini-3-pro-image"));
    crate::dialogs::confirm(&mut app, id).unwrap();

    let id = open_settings(&mut app);
    pick_provider(&mut app, id, "openai");
    assert_eq!(fields(&app, id)["model"], photocraft_genai::openai::DEFAULT_MODEL);
    assert_eq!(fields(&app, id)["hasKey"], false);
    let options = json!({"quality": "medium", "resolution": 3840, "format": "jpeg", "compression": 80});
    app.ui.dialog_mut(id).unwrap().fields.insert("options".into(), options.clone());
    app.genai.key_input = SECRET.into();
    let r = crate::dialogs::confirm(&mut app, id).unwrap();
    assert_eq!(r["ready"], true);
    assert_eq!(store.borrow().get("openai").map(String::as_str), Some(SECRET));
    assert_eq!(app.session.image_provider().unwrap().info().id, "openai");
    let pref = app.session.prefs().dialogs.get(PREF).unwrap().clone();
    assert_eq!(pref["provider"], "openai");
    assert_eq!(pref["options"]["openai"]["quality"], "medium");
    assert_eq!(pref["options"]["openai"]["model"], photocraft_genai::openai::DEFAULT_MODEL);
    assert!(!serde_json::to_string(&app.session.prefs().to_json()).unwrap().contains(SECRET));

    // Opened again: OpenAI with its options; back to Gemini: its model and saved key.
    let id = open_settings(&mut app);
    assert_eq!(fields(&app, id)["options"]["format"], "jpeg");
    assert_eq!(fields(&app, id)["hasKey"], true);
    pick_provider(&mut app, id, "gemini");
    assert_eq!(fields(&app, id)["model"], "gemini-3-pro-image");
    assert_eq!(fields(&app, id)["hasKey"], true);
    crate::dialogs::confirm(&mut app, id).unwrap();
    assert_eq!(app.session.image_provider().unwrap().info().id, "gemini");
    assert_eq!(app.session.prefs().dialogs.get(PREF).unwrap()["options"]["openai"]["quality"], "medium", "kept for next time");
}

#[test]
fn the_prompt_dialog_runs_the_fill_and_remembers_the_prompt() {
    let store = Store::default();
    store.borrow_mut().insert("gemini".into(), SECRET.into());
    let mut app = app_with(&store);
    app.session.prefs.edit(|p| p.dialogs.insert(PREF.into(), json!({"provider": "gemini", "model": "m"})));
    restore(&mut app).unwrap();
    let ctx = egui::Context::default();
    // No selection: Generative Fill can't run, and says why.
    assert!(crate::menus::invoke(&mut app, &ctx, FILL, json!({})).is_err());
    app.run("select.rect", json!({"x": 8, "y": 8, "width": 16, "height": 16})).unwrap();
    let r = crate::menus::invoke(&mut app, &ctx, FILL, json!({})).unwrap();
    let id = r["dialog"].as_u64().unwrap();
    assert_eq!(crate::genai_ui::ok_label(&app.ui.dialogs.iter().find(|d| d.id == id).unwrap().fields), Some("Generate"));
    app.ui.dialog_mut(id).unwrap().fields.insert("prompt".into(), json!("a red square"));
    let layers = app.session.active().unwrap().doc.layers.len();
    crate::dialogs::confirm(&mut app, id).unwrap();
    let doc = &app.session.active().unwrap().doc;
    assert_eq!(doc.layers.len(), layers + 1);
    assert_eq!(doc.layers.last().unwrap().name, "a red square");
    assert_eq!(app.session.prefs().dialogs.get(FILL), Some(&json!({"prompt": "a red square", "mode": "fill", "separate": true, "variations": {"gemini": 3}})));
    let (cmd, params) = app.session.journal.iter().rev().find(|(c, _)| c == FILL).unwrap();
    assert_eq!((cmd.as_str(), params), (FILL, &json!({"prompt": "a red square", "mode": "fill", "variations": 3})), "no references: none sent");
}

/// An app with the provider set up, an open 64² document with a selection, and the prompt
/// dialog's id.
fn prompt_ready(store: &Store) -> PhotocraftApp {
    store.borrow_mut().insert("gemini".into(), SECRET.into());
    let mut app = app_with(store);
    app.session.prefs.edit(|p| p.dialogs.insert(PREF.into(), json!({"provider": "gemini", "model": "m"})));
    restore(&mut app).unwrap();
    app
}

fn open_prompt_dialog(app: &mut PhotocraftApp) -> u64 {
    let ctx = egui::Context::default();
    crate::menus::invoke(app, &ctx, FILL, json!({})).unwrap()["dialog"].as_u64().unwrap()
}

fn fields(app: &PhotocraftApp, id: u64) -> &Map<String, Value> {
    &app.ui.dialogs.iter().find(|d| d.id == id).unwrap().fields
}

fn last_fill(app: &PhotocraftApp) -> Value {
    app.session.journal.iter().rev().find(|(c, _)| c == FILL).map(|(_, p)| p.clone()).unwrap()
}

#[test]
fn edit_mode_and_reference_documents_reach_the_command_and_are_remembered() {
    let store = Store::default();
    let mut app = prompt_ready(&store);
    // A green document to use as a reference, then the one to edit.
    app.run("file.new", json!({"width": 32, "height": 32, "background": "#00ff00"})).unwrap();
    let green = app.session.active().unwrap().doc.id.0;
    app.run("file.new", json!({"width": 64, "height": 64, "background": "#ffffff"})).unwrap();
    app.run("select.rect", json!({"x": 8, "y": 8, "width": 16, "height": 16})).unwrap();
    let id = open_prompt_dialog(&mut app);
    assert_eq!(fields(&app, id)["mode"], "fill", "Fill until Edit is chosen");
    assert_eq!(fields(&app, id)["references"], json!([]));
    let f = &mut app.ui.dialog_mut(id).unwrap().fields;
    f.insert("mode".into(), json!("edit"));
    f.insert("prompt".into(), json!("  make it blue "));
    f.insert("references".into(), json!([{"document": green}]));
    crate::dialogs::confirm(&mut app, id).unwrap();
    assert_eq!(last_fill(&app), json!({"prompt": "make it blue", "mode": "edit", "variations": 3, "references": [{"document": green}]}));
    assert_eq!(app.session.prefs().dialogs.get(FILL), Some(&json!({"prompt": "make it blue", "mode": "edit", "separate": true, "variations": {"gemini": 3}})));
    assert_eq!(app.session.active().unwrap().doc.layers.last().unwrap().name, "make it blue");

    // The next prompt starts in Edit with the same reference.
    let id = open_prompt_dialog(&mut app);
    assert_eq!(fields(&app, id)["mode"], "edit");
    assert_eq!(fields(&app, id)["references"], json!([{"document": green}]));
    // Edit with no prompt removes, as Fill does: Edit needs something to change.
    app.ui.dialog_mut(id).unwrap().fields.insert("prompt".into(), json!(""));
    app.ui.dialog_mut(id).unwrap().fields.insert("references".into(), json!([]));
    crate::dialogs::confirm(&mut app, id).unwrap();
    assert_eq!(last_fill(&app), json!({"prompt": "", "mode": "fill", "variations": 3}));
    assert_eq!(
        app.session.prefs().dialogs.get(FILL),
        Some(&json!({"prompt": "", "mode": "edit", "separate": true, "variations": {"gemini": 3}})),
        "the tab chosen is kept"
    );

    // A reference to a document that has been closed since is dropped.
    app.genai.references = vec![json!({"document": 987_654_321}), json!({"document": green})];
    let id = open_prompt_dialog(&mut app);
    assert_eq!(fields(&app, id)["references"], json!([{"document": green}]));
}

/// The number of variations is chosen in the dialog (1 to 4), sent with the fill and remembered
/// for the provider it was chosen with; Generate More asks for as many.
#[test]
fn the_number_of_variations_is_chosen_and_remembered_per_provider() {
    let store = Store::default();
    let mut app = prompt_ready(&store);
    app.run("select.rect", json!({"x": 8, "y": 8, "width": 16, "height": 16})).unwrap();
    let id = open_prompt_dialog(&mut app);
    assert_eq!(fields(&app, id)["variations"], 3, "the provider's default at first");
    app.ui.dialog_mut(id).unwrap().fields.insert("variations".into(), json!(1));
    crate::dialogs::confirm(&mut app, id).unwrap();
    assert_eq!(last_fill(&app)["variations"], 1);
    let group = app.session.generative_layer().unwrap();
    assert_eq!(app.session.active().unwrap().doc.layer(group).unwrap().children().unwrap().len(), 1);
    assert_eq!(app.session.prefs().dialogs.get(FILL).unwrap()["variations"], json!({"gemini": 1}));
    app.run("select.rect", json!({"x": 8, "y": 8, "width": 16, "height": 16})).unwrap();
    let id = open_prompt_dialog(&mut app);
    assert_eq!(fields(&app, id)["variations"], 1, "the last choice is offered again");
    // Out of range (a hand-edited preference) falls back to the default; another provider's
    // choice is kept alongside.
    app.session.prefs.edit(|p| p.dialogs.insert(FILL.into(), json!({"variations": {"gemini": 9, "other": 2}})));
    crate::dialogs::cancel(&mut app, id).unwrap();
    let id = open_prompt_dialog(&mut app);
    assert_eq!(fields(&app, id)["variations"], 3);
    app.ui.dialog_mut(id).unwrap().fields.insert("variations".into(), json!(4));
    crate::dialogs::confirm(&mut app, id).unwrap();
    assert_eq!(app.session.prefs().dialogs.get(FILL).unwrap()["variations"], json!({"gemini": 4, "other": 2}));
    assert_eq!(chosen_variations(&app), 4);
}

#[test]
fn image_files_are_added_as_references_by_path_after_they_open() {
    let dir = std::env::temp_dir().join(format!("photocraft-genai-ui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let good = dir.join("bottle.png");
    let px: Vec<u8> = (0..40 * 20).flat_map(|_| [200u8, 30, 40, 255]).collect();
    let img = photocraft_codecs::Image::from_u8(40, 20, photocraft_codecs::ChannelLayout::Rgba, px).unwrap();
    std::fs::write(&good, photocraft_codecs::encode(&img, photocraft_codecs::Format::Png, &Default::default()).unwrap()).unwrap();
    let bad = dir.join("broken.png");
    std::fs::write(&bad, b"not an image").unwrap();
    let store = Store::default();
    let mut app = prompt_ready(&store);
    app.run("select.rect", json!({"x": 8, "y": 8, "width": 16, "height": 16})).unwrap();
    let answers = [&bad, &good].map(|p| Some(crate::file_dialog::FileDialogAnswer::Paths(vec![p.to_string_lossy().into_owned()])));
    app.services.file_dialog = Some(crate::file_dialog::fake(answers.into()).0);
    let id = open_prompt_dialog(&mut app);
    let ctx = egui::Context::default();
    // A file that doesn't open is reported now and not added.
    pick_reference_file(&mut app).unwrap();
    app.poll_file_dialog(&ctx, None);
    assert!(app.ui.status.contains("broken.png"), "{}", app.ui.status);
    assert_eq!(fields(&app, id)["references"], json!([]));
    pick_reference_file(&mut app).unwrap();
    app.poll_file_dialog(&ctx, None);
    let entry = json!({"path": good.to_string_lossy()});
    assert_eq!(fields(&app, id)["references"], json!([entry.clone()]));
    assert!(matches!(app.genai.previews.get(&reference_key(&entry)), Some(Preview::Image(i)) if i.size == [40, 20]), "a preview, never enlarged");
    assert_eq!(reference_name(&app, &entry).as_deref(), Some("bottle.png"));
    app.ui.dialog_mut(id).unwrap().fields.insert("prompt".into(), json!("this bottle"));
    crate::dialogs::confirm(&mut app, id).unwrap();
    assert_eq!(last_fill(&app)["references"], json!([entry]));
    let _ = std::fs::remove_dir_all(dir);
}

/// Renders Generative AI Settings and the prompt for a look (`GENAI_SNAPSHOT_DIR=… cargo test`).
#[test]
fn the_dialogs_render() {
    let store = Store::default();
    store.borrow_mut().insert("gemini".into(), SECRET.into());
    let store2 = store.clone();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1440.0, 760.0)).with_max_steps(64).build_eframe(move |cc| {
        PhotocraftApp::setup_context(&cc.egui_ctx, Default::default());
        let mut app = app_with(&store2);
        open_settings(&mut app);
        app
    });
    h.run_steps(6);
    h.get_by_label("Provider");
    h.get_by_label("Remove Key");
    h.get_by_label("Get a Gemini API key from Google AI Studio");
    let dir = std::env::var_os("GENAI_SNAPSHOT_DIR");
    if let Some(dir) = &dir
        && let Ok(img) = h.render()
    {
        img.save(std::path::Path::new(dir).join("genai-settings.png")).unwrap();
    }
    let id = settings_dialog(h.state()).unwrap();
    crate::dialogs::confirm(h.state_mut(), id).unwrap();
    h.state_mut().run("select.rect", json!({"x": 8, "y": 8, "width": 16, "height": 16})).unwrap();
    let ctx = h.ctx.clone();
    crate::menus::invoke(h.state_mut(), &ctx, FILL, json!({})).unwrap();
    h.run_steps(6);
    h.get_by_label("Generate");
    h.get_by_label("Fill selection");
    h.get_by_label("Add Reference…");
    if let Some(dir) = &dir
        && let Ok(img) = h.render()
    {
        img.save(std::path::Path::new(dir).join("genai-prompt.png")).unwrap();
    }
    // Edit, with the document itself as a reference.
    h.get_by_label("Edit selection").click();
    h.run_steps(2);
    let doc = h.state().session.active().unwrap().doc.id.0;
    let prompt = h.state().ui.dialogs[0].id;
    let f = &mut h.state_mut().ui.dialog_mut(prompt).unwrap().fields;
    assert_eq!(f["mode"], "edit");
    f.insert("prompt".into(), json!("make the label black and the text white"));
    f.insert("references".into(), json!([{"document": doc}]));
    h.run_steps(6);
    h.get_by_label("Remove");
    if let Some(dir) = &dir
        && let Ok(img) = h.render()
    {
        img.save(std::path::Path::new(dir).join("genai-prompt-edit.png")).unwrap();
    }
    // Add Reference… offers an image file or an open document.
    h.get_by_label("Add Reference…").click();
    h.run_steps(4);
    h.get_by_label("Image File…");
    h.get_by_label("Open Documents");
    if let Some(dir) = &dir
        && let Ok(img) = h.render()
    {
        img.save(std::path::Path::new(dir).join("genai-add-reference.png")).unwrap();
    }
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    // The options bar of a selection tool: the button sits after Select and Mask….
    let prompt = h.state().ui.dialogs[0].id;
    crate::dialogs::cancel(h.state_mut(), prompt).unwrap();
    h.state_mut().ui.tool = crate::state::Tool::RectMarquee;
    h.run_steps(6);
    h.get_by_label("Generative Fill…");
    if let Some(dir) = &dir
        && let Ok(img) = h.render()
    {
        img.save(std::path::Path::new(dir).join("genai-options-bar.png")).unwrap();
    }
}

/// File › Automate › Generative Batch…: hidden until a provider that edits from instructions is
/// set up; its dialog lists the open documents, runs the batch with what was chosen, remembers it,
/// and reports what was written.
#[test]
fn the_batch_dialog_runs_a_batch_and_remembers_it() {
    let batch_in_menu = |app: &PhotocraftApp| crate::menus::menu_items(app).iter().any(|i| i.id == photocraft_engine::genai_cmds::BATCH);
    let store = Store::default();
    let off = app_with(&store);
    assert!(!batch_in_menu(&off), "no item before a provider is set up");
    let mut app = prompt_ready(&store);
    assert!(batch_in_menu(&app));
    let first = app.session.active().unwrap().doc.id.0;
    app.run("file.new", json!({"width": 40, "height": 30, "background": "#00ff00"})).unwrap();
    let second = app.session.active().unwrap().doc.id.0;
    let ctx = egui::Context::default();
    let id = crate::menus::invoke(&mut app, &ctx, photocraft_engine::genai_cmds::BATCH, json!({})).unwrap()["dialog"].as_u64().unwrap();
    assert_eq!(fields(&app, id)["source"], "documents");
    assert_eq!(fields(&app, id)["documents"], json!([{"id": first, "on": true}, {"id": second, "on": true}]));
    assert_eq!(crate::genai_ui::ok_label(fields(&app, id)), Some("Start Batch"));
    // No prompt: it says so and runs nothing.
    assert!(crate::dialogs::confirm(&mut app, id).is_err());
    let out = std::env::temp_dir().join(format!("photocraft-genai-ui-batch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let id = crate::menus::invoke(&mut app, &ctx, photocraft_engine::genai_cmds::BATCH, json!({})).unwrap()["dialog"].as_u64().unwrap();
    let f = &mut app.ui.dialog_mut(id).unwrap().fields;
    f.insert("prompt".into(), json!("warmer"));
    f.insert("documents".into(), json!([{"id": first, "on": false}, {"id": second, "on": true}]));
    f.insert("output".into(), json!(out.to_string_lossy()));
    f.insert("variations".into(), json!(2));
    f.insert("sameLook".into(), json!(true));
    let r = crate::dialogs::confirm(&mut app, id).unwrap();
    assert_eq!(r["written"].as_array().unwrap().len(), 2, "{r}");
    let written: Vec<String> = std::fs::read_dir(&out).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(written.len(), 2, "{written:?}");
    let last = app.session.journal.iter().rev().find(|(c, _)| c == photocraft_engine::genai_cmds::BATCH).map(|(_, p)| p.clone());
    assert!(last.is_none(), "a batch isn't recorded in the journal");
    let pref = app.session.prefs().dialogs.get(photocraft_engine::genai_cmds::BATCH).unwrap().clone();
    assert_eq!((pref["prompt"].as_str(), pref["sameLook"].as_bool()), (Some("warmer"), Some(true)));
    // Opened again: the prompt and Same look are kept, with its words.
    let id = crate::menus::invoke(&mut app, &ctx, photocraft_engine::genai_cmds::BATCH, json!({})).unwrap()["dialog"].as_u64().unwrap();
    assert_eq!(fields(&app, id)["prompt"], "warmer");
    assert_eq!(fields(&app, id)["sameLookPrompt"], photocraft_engine::genai_cmds::batch::SAME_LOOK_PROMPT);
    // A batch's end is reported; failed files are kept for the next time.
    let done = photocraft_engine::jobs::JobEvent {
        id: photocraft_engine::jobs::JobId(1),
        command: photocraft_engine::genai_cmds::BATCH.into(),
        label: "Generative Batch".into(),
        document: None,
        outcome: photocraft_engine::jobs::JobOutcome::Done(
            json!({"written": ["a"], "failed": [{"name": "b.png", "path": "/x/b.png", "error": "no"}], "folders": ["/x/IA"]}),
        ),
    };
    assert!(crate::genai_ui::on_job_event(&mut app, &ctx, &done));
    assert_eq!(app.genai.batch_failed, ["/x/b.png"]);
    let _ = std::fs::remove_dir_all(out);
}

/// Renders the batch dialog for a look (`GENAI_SNAPSHOT_DIR=… cargo test`).
#[test]
fn the_batch_dialog_renders() {
    let store = Store::default();
    store.borrow_mut().insert("gemini".into(), SECRET.into());
    let store2 = store.clone();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1440.0, 900.0)).with_max_steps(64).build_eframe(move |cc| {
        PhotocraftApp::setup_context(&cc.egui_ctx, Default::default());
        let mut app = prompt_ready(&store2);
        app.run("file.new", json!({"width": 40, "height": 30, "background": "#00ff00"})).unwrap();
        let ctx = egui::Context::default();
        let id = crate::menus::invoke(&mut app, &ctx, photocraft_engine::genai_cmds::BATCH, json!({})).unwrap()["dialog"].as_u64().unwrap();
        let f = &mut app.ui.dialog_mut(id).unwrap().fields;
        f.insert("prompt".into(), json!("make the light warmer"));
        f.insert("sameLook".into(), json!(true));
        app
    });
    h.run_steps(6);
    h.get_by_label("Start Batch");
    h.get_by_label("Same look in all");
    h.get_by_label("Open documents");
    if let Some(dir) = std::env::var_os("GENAI_SNAPSHOT_DIR")
        && let Ok(img) = h.render()
    {
        img.save(std::path::Path::new(&dir).join("genai-batch.png")).unwrap();
    }
}

/// Renders OpenAI's settings for a look (`GENAI_SNAPSHOT_DIR=… cargo test`).
#[test]
fn openai_settings_render() {
    let store = Store::default();
    let store2 = store.clone();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1440.0, 900.0)).with_max_steps(64).build_eframe(move |cc| {
        PhotocraftApp::setup_context(&cc.egui_ctx, Default::default());
        let mut app = app_with(&store2);
        let id = open_settings(&mut app);
        pick_provider(&mut app, id, "openai");
        app.ui.dialog_mut(id).unwrap().fields.insert("options".into(), json!({"format": "jpeg"}));
        app
    });
    h.run_steps(6);
    h.get_by_label("Get an OpenAI API key");
    h.get_by_label("Working resolution");
    h.get_by_label("Quality (compression)");
    if let Some(dir) = std::env::var_os("GENAI_SNAPSHOT_DIR")
        && let Ok(img) = h.render()
    {
        img.save(std::path::Path::new(&dir).join("genai-settings-openai.png")).unwrap();
    }
}

/// Properties shows a generative layer's variations: clicking one shows it, and Generate More
/// adds three (`GENAI_SNAPSHOT_DIR=… cargo test` saves a look).
#[test]
fn properties_show_the_variations() {
    let store = Store::default();
    store.borrow_mut().insert("gemini".into(), SECRET.into());
    let store2 = store.clone();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1440.0, 760.0)).with_max_steps(64).build_eframe(move |cc| {
        PhotocraftApp::setup_context(&cc.egui_ctx, Default::default());
        let mut app = app_with(&store2);
        app.session.prefs.edit(|p| p.dialogs.insert(PREF.into(), json!({"provider": "gemini", "model": "m"})));
        restore(&mut app).unwrap();
        app.run("select.rect", json!({"x": 8, "y": 8, "width": 32, "height": 24})).unwrap();
        app
    });
    h.state_mut().run(FILL, json!({"prompt": "a red kite"})).unwrap();
    for _ in 0..200 {
        h.run_steps(1);
        if h.state().session.generative_layer().is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let group = h.state().session.generative_layer().expect("the fill finished");
    h.run_steps(4);
    let shown = |h: &egui_kittest::Harness<'_, PhotocraftApp>| -> Vec<bool> {
        h.state().session.active().unwrap().doc.layer(group).unwrap().children().unwrap().iter().map(|l| l.visible).collect()
    };
    assert_eq!(shown(&h), [true, false, false]);
    h.get_by_label("Variations");
    h.get_by_label("Generate More");
    if let Some(dir) = std::env::var_os("GENAI_SNAPSHOT_DIR")
        && let Ok(img) = h.render()
    {
        img.save(std::path::Path::new(&dir).join("genai-variations.png")).unwrap();
    }
    // The Layers panel lists "Variation 3" too; the thumbnail is the one in Properties, above it.
    let thumb = h.query_all_by_role_and_label(egui::accesskit::Role::Button, "Variation 3").min_by(|a, b| a.rect().min.y.total_cmp(&b.rect().min.y)).unwrap();
    thumb.click();
    h.run_steps(4);
    assert_eq!(shown(&h), [false, false, true]);
    h.get_by_label("Generate More").click();
    for _ in 0..200 {
        h.run_steps(1);
        if shown(&h).len() == 6 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(shown(&h), [false, false, false, true, false, false]);
}

// ---- Fills that run side by side --------------------------------------------------------------

/// Answers after `delay` with solid blue, or (with `fail`) says the service can't be reached.
struct Slow {
    delay: std::time::Duration,
    fail: bool,
}

impl ImageProvider for Slow {
    fn info(&self) -> ProviderInfo {
        ProviderInfo { id: "gemini", name: "Google Gemini", model: "slow".into(), host: "example.invalid" }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities { generate: false, edit: true, variations: false, edit_in_place: true, max_references: 4 }
    }
    fn edit(&self, r: &EditRequest) -> photocraft_genai::Result<GeneratedImage> {
        std::thread::sleep(self.delay);
        if self.fail {
            return Err(photocraft_genai::GenError::Network("offline".into()));
        }
        let (w, h) = photocraft_genai::png_size(&r.image).unwrap();
        let px: Vec<u8> = (0..w * h).flat_map(|_| [0u8, 0, 255, 255]).collect();
        let img = photocraft_codecs::Image::from_u8(w, h, photocraft_codecs::ChannelLayout::Rgba, px).unwrap();
        let bytes = photocraft_codecs::encode(&img, photocraft_codecs::Format::Png, &Default::default()).unwrap();
        Ok(GeneratedImage { bytes, mime: "image/png".into() })
    }
}

/// [`prompt_ready`], running jobs in the background as the desktop app does, with `provider`.
fn background(store: &Store, provider: Slow) -> PhotocraftApp {
    let mut app = prompt_ready(store);
    app.background_jobs = true;
    app.session.set_image_provider(Some(Arc::new(provider)));
    app
}

fn generate(app: &mut PhotocraftApp, prompt: &str) {
    let id = open_prompt_dialog(app);
    app.ui.dialog_mut(id).unwrap().fields.insert("prompt".into(), json!(prompt));
    crate::dialogs::confirm(app, id).unwrap();
}

/// Tick the shell until `done` holds (at most 10 s).
fn tick_until(app: &mut PhotocraftApp, done: impl Fn(&PhotocraftApp) -> bool) {
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !done(app) {
        assert!(std::time::Instant::now() < deadline, "timed out: {:?}", app.session.generative_fills());
        crate::jobs_ui::tick(app, &ctx);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn groups(app: &PhotocraftApp) -> Vec<(String, photocraft_geom::Rect)> {
    let doc = &app.session.active().unwrap().doc;
    doc.layers.iter().filter(|l| l.is_group()).map(|l| (l.name.clone(), l.mask.as_ref().unwrap().surface.content_bounds())).collect()
}

#[test]
fn separate_areas_are_filled_one_by_one_unless_asked_otherwise() {
    let store = Store::default();
    let mut app = prompt_ready(&store);
    app.run("select.rect", json!({"x": 4, "y": 4, "width": 12, "height": 12})).unwrap();
    app.run("select.rect", json!({"x": 40, "y": 40, "width": 16, "height": 16, "mode": "add"})).unwrap();
    let id = open_prompt_dialog(&mut app);
    assert_eq!(fields(&app, id)["__areas"], 2);
    assert_eq!(fields(&app, id)["separate"], true, "on unless turned off");
    app.ui.dialog_mut(id).unwrap().fields.insert("prompt".into(), json!("x"));
    crate::dialogs::confirm(&mut app, id).unwrap();
    let r = |x, y, w, h| photocraft_geom::Rect::from_xywh(x, y, w, h);
    assert_eq!(groups(&app), [("x".into(), r(4, 4, 12, 12)), ("x".into(), r(40, 40, 16, 16))]);
    // Turned off, the selection is filled as one; the choice is remembered.
    let id = open_prompt_dialog(&mut app);
    let f = &mut app.ui.dialog_mut(id).unwrap().fields;
    f.insert("prompt".into(), json!("y"));
    f.insert("separate".into(), json!(false));
    crate::dialogs::confirm(&mut app, id).unwrap();
    assert_eq!(groups(&app).last().unwrap(), &("y".into(), r(4, 4, 52, 52)));
    assert_eq!(app.session.prefs().dialogs.get(FILL).unwrap()["separate"], false);
    let id = open_prompt_dialog(&mut app);
    assert_eq!(fields(&app, id)["separate"], false);
}

#[test]
fn in_the_background_fills_block_nothing_and_land_where_they_were_started() {
    let store = Store::default();
    let mut app = background(&store, Slow { delay: std::time::Duration::from_millis(300), fail: false });
    let bg = app.session.active().unwrap().active_layer;
    app.run("select.rect", json!({"x": 4, "y": 4, "width": 12, "height": 12})).unwrap();
    generate(&mut app, "a");
    // No progress window, no lock, and the selection let go for the next one.
    assert!(app.session.active_job().is_none());
    assert!(crate::jobs_ui::job_in_view(&app).is_none(), "the status bar's job readout leaves it to its own");
    assert!(app.session.active().unwrap().doc.selection.is_none());
    app.run("select.rect", json!({"x": 40, "y": 40, "width": 16, "height": 16})).unwrap();
    generate(&mut app, "b");
    let fills = app.session.generative_fills();
    assert_eq!(fills.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    // The status bar counts them.
    let mut h = egui_kittest::Harness::new_ui_state(|ui, app: &mut PhotocraftApp| crate::genai_ui::status_fills(app, ui), app);
    h.run_steps(2);
    let shown = h.query_by_label("2 generating").is_some() || h.query_by_label("1 generating · 1 waiting").is_some();
    assert!(shown, "status readout");
    let mut app = h.into_state();
    tick_until(&mut app, |app| app.session.generative_fills().is_empty() && !app.session.has_jobs());
    assert_eq!(groups(&app).len(), 2);
    assert_eq!(app.session.active().unwrap().active_layer, bg, "the user's layer stays active");
    assert!(app.ui.status.starts_with("Generated"), "{}", app.ui.status);
}

#[test]
fn a_failed_fill_keeps_its_row_offers_retry_and_can_be_discarded() {
    let store = Store::default();
    let mut app = background(&store, Slow { delay: std::time::Duration::from_millis(20), fail: true });
    app.run("select.rect", json!({"x": 4, "y": 4, "width": 12, "height": 12})).unwrap();
    generate(&mut app, "lamp");
    // Failed, and its job's end reported.
    tick_until(&mut app, |app| app.session.generative_fills().first().is_some_and(|f| f.state == "failed") && !app.session.has_jobs());
    assert!(app.ui.status_error && app.ui.status.contains("offline"), "{}", app.ui.status);
    let doc = app.session.active().unwrap().doc.id;
    // Its row stands where the layer will land, with Retry and Discard.
    let mut h = egui_kittest::Harness::new_ui_state(move |ui, app: &mut PhotocraftApp| crate::genai_ui::layer_rows_unplaced(app, ui, doc, &[]), app);
    h.run_steps(2);
    h.get_by_label("Failed: lamp");
    let mut app = h.into_state();
    // Retry with a working service lands it.
    app.session.set_image_provider(Some(Arc::new(Slow { delay: std::time::Duration::from_millis(20), fail: false })));
    let id = app.session.generative_fills()[0].id;
    app.run(photocraft_engine::genai_cmds::RETRY, json!({"fill": id})).unwrap();
    tick_until(&mut app, |app| app.session.generative_fills().is_empty() && !app.session.has_jobs());
    assert_eq!(groups(&app).len(), 1);
    // Discard stops one that is still generating; nothing lands.
    app.session.set_image_provider(Some(Arc::new(Slow { delay: std::time::Duration::from_millis(300), fail: false })));
    app.run("select.rect", json!({"x": 30, "y": 30, "width": 12, "height": 12})).unwrap();
    generate(&mut app, "gone");
    let id = app.session.generative_fills()[0].id;
    app.run(photocraft_engine::genai_cmds::DISCARD, json!({"fill": id})).unwrap();
    app.session.join_cancelled_jobs();
    tick_until(&mut app, |app| !app.session.has_jobs());
    assert!(app.session.generative_fills().is_empty());
    assert_eq!(groups(&app).len(), 1);
}

#[test]
fn the_canvas_draws_fills_in_progress() {
    let store = Store::default();
    let mut app = background(&store, Slow { delay: std::time::Duration::from_millis(500), fail: false });
    app.run("select.rect", json!({"x": 4, "y": 4, "width": 12, "height": 12})).unwrap();
    generate(&mut app, "a");
    let doc = app.session.active().unwrap().doc.id;
    let mut h = egui_kittest::Harness::new_ui_state(
        move |ui, app: &mut PhotocraftApp| {
            let xf = crate::canvas::ViewXform { rect: ui.max_rect(), zoom: 4.0, center: [32.0, 32.0], flip: false, rotation: 0.0, aspect: 1.0 };
            let painter = ui.painter().clone();
            crate::genai_ui::paint_pending(app, &ui.ctx().clone(), &painter, &xf, doc);
        },
        app,
    );
    h.run_steps(2);
    // Its tag: what it is doing, with Cancel.
    assert!(h.query_by_label("Generating").is_some() || h.query_by_label_contains("Waiting").is_some());
    h.get_by_label("Cancel this Generative Fill").click();
    h.run_steps(2);
    let mut app = h.into_state();
    app.session.join_cancelled_jobs();
    tick_until(&mut app, |app| !app.session.has_jobs());
    assert!(app.session.generative_fills().is_empty());
}

/// Renders fills side by side for a look (`GENAI_SNAPSHOT_DIR=… cargo test`): six objects
/// selected together and filled each on its own (five generating, one waiting its turn), one
/// that failed, the Layers panel's rows and the status bar's list.
#[test]
fn fills_side_by_side_render() {
    let Some(dir) = std::env::var_os("GENAI_SNAPSHOT_DIR") else { return };
    let dir = std::path::PathBuf::from(dir);
    let store = Store::default();
    store.borrow_mut().insert("gemini".into(), SECRET.into());
    let store2 = store.clone();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1440.0, 860.0)).with_max_steps(64).wgpu().build_eframe(move |cc| {
        PhotocraftApp::setup_context(&cc.egui_ctx, Default::default());
        let mut app = background(&store2, Slow { delay: std::time::Duration::from_millis(30), fail: true });
        app.run("file.new", json!({"width": 1200, "height": 800, "background": "#d9d2c3"})).unwrap();
        // A floor, and seven "objects" to remove.
        app.run("select.rect", json!({"x": 0, "y": 520, "width": 1200, "height": 280})).unwrap();
        app.run("edit.fill", json!({"color": "#8a7a66"})).unwrap();
        for (i, (x, y, w, hh, c)) in [
            (90, 380, 90, 160, "#2f5d8a"),
            (300, 430, 70, 110, "#a8432f"),
            (470, 300, 60, 60, "#e0b03c"),
            (640, 420, 120, 120, "#3c7d4a"),
            (860, 360, 80, 180, "#6b3d8a"),
            (1020, 460, 100, 80, "#c76a2b"),
            (560, 600, 140, 60, "#444444"),
        ]
        .into_iter()
        .enumerate()
        {
            let _ = i;
            app.run("select.rect", json!({"x": x, "y": y, "width": w, "height": hh})).unwrap();
            app.run("edit.fill", json!({"color": c})).unwrap();
        }
        app.run("select.deselect", json!({})).unwrap();
        app
    });
    h.run_steps(4);
    let save = |h: &mut egui_kittest::Harness<PhotocraftApp>, name: &str| {
        if let Ok(img) = h.render() {
            img.save(dir.join(name)).unwrap();
        }
    };
    // One that failed: the service couldn't be reached.
    h.state_mut().run("select.rect", json!({"x": 545, "y": 590, "width": 170, "height": 80})).unwrap();
    generate(h.state_mut(), "remove the rug");
    for _ in 0..200 {
        h.run_steps(1);
        if h.state().session.generative_fills().first().is_some_and(|f| f.state == "failed") && !h.state().session.has_jobs() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // Six objects selected together, filled each on its own: five at a time, one waits.
    h.state_mut().session.set_image_provider(Some(Arc::new(Slow { delay: std::time::Duration::from_secs(30), fail: false })));
    for (i, (x, y, w, hh)) in [(80, 370, 110, 180), (290, 420, 90, 130), (460, 290, 80, 80), (630, 410, 140, 140), (850, 350, 100, 200), (1010, 450, 120, 100)]
        .into_iter()
        .enumerate()
    {
        let mode = if i == 0 { "replace" } else { "add" };
        h.state_mut().run("select.rect", json!({"x": x, "y": y, "width": w, "height": hh, "mode": mode})).unwrap();
    }
    let ctx = h.ctx.clone();
    crate::menus::invoke(h.state_mut(), &ctx, FILL, json!({})).unwrap();
    let id = h.state().ui.dialogs[0].id;
    h.state_mut().ui.dialog_mut(id).unwrap().fields.insert("prompt".into(), json!(""));
    h.run_steps(6);
    h.get_by_label("Fill each of the 6 separate areas on its own");
    save(&mut h, "genai-areas-prompt.png");
    crate::dialogs::confirm(h.state_mut(), id).unwrap();
    for _ in 0..30 {
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(h.state().session.generative_fills().len(), 7);
    save(&mut h, "genai-side-by-side.png");
    // The status bar's list.
    h.get_by_label_contains("generating").click();
    h.run_steps(4);
    save(&mut h, "genai-side-by-side-list.png");
    h.state_mut().session.cancel_all_jobs();
}

/// The tags sit over the canvas: clicking one's Cancel stops that fill and nothing else; the
/// tool under it (the Brush here) never sees the click.
#[test]
fn a_tags_cancel_takes_the_click_from_the_canvas() {
    let store = Store::default();
    store.borrow_mut().insert("gemini".into(), SECRET.into());
    let store2 = store.clone();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1200.0, 800.0)).with_max_steps(64).build_eframe(move |cc| {
        PhotocraftApp::setup_context(&cc.egui_ctx, Default::default());
        let mut app = background(&store2, Slow { delay: std::time::Duration::from_secs(20), fail: false });
        app.run("file.new", json!({"width": 600, "height": 400, "background": "#ffffff"})).unwrap();
        app.ui.tool = crate::state::Tool::Brush;
        app
    });
    h.run_steps(4);
    for x in [100, 350] {
        h.state_mut().run("select.rect", json!({"x": x, "y": 200, "width": 60, "height": 60})).unwrap();
        generate(h.state_mut(), "");
        h.run_steps(2);
    }
    h.run_steps(4);
    assert_eq!(h.state().session.generative_fills().len(), 2);
    let history = h.state().session.active().unwrap().history.past_len();
    h.get_all_by_label("Cancel this Generative Fill").next().unwrap().click();
    h.run_steps(4);
    assert_eq!(h.state().session.generative_fills().len(), 1, "that one stopped");
    assert_eq!(h.state().session.active().unwrap().history.past_len(), history, "no brush stroke under the tag");
    // The same click on the canvas itself paints.
    let at = h.get_all_by_label("Cancel this Generative Fill").next().unwrap().rect().center() + egui::vec2(0.0, 80.0);
    h.input_mut().events.push(egui::Event::PointerMoved(at));
    for pressed in [true, false] {
        h.input_mut().events.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
        h.run_steps(2);
    }
    h.run_steps(4);
    assert!(h.state().session.active().unwrap().history.past_len() > history, "a click on the canvas paints");
    h.state_mut().session.cancel_all_jobs();
}

/// Gemini (its key saved) in use, with a selection.
fn gemini_ready(store: &Store) -> PhotocraftApp {
    let mut app = prompt_ready(store);
    app.run("select.rect", json!({"x": 8, "y": 8, "width": 16, "height": 16})).unwrap();
    app
}

/// The Generative Fill window chooses the provider and model among those set up (a key saved),
/// for every fill to come (kept as the settings' choice, each provider's options as they were),
/// and says there what the model won't do of what was asked; automation can't choose.
#[test]
fn the_fill_window_chooses_the_provider_and_model() {
    let store = Store::default();
    let mut app = gemini_ready(&store);
    open_prompt_dialog(&mut app);
    assert_eq!(app.genai.configured, ["gemini"], "OpenAI has no key yet: not offered");
    // A key saved for OpenAI since: offered the next time the window opens.
    store.borrow_mut().insert("openai".into(), SECRET.into());
    let id = open_prompt_dialog(&mut app);
    assert_eq!(app.genai.configured, ["gemini"], "the window was open: the list is from when it opened");
    crate::dialogs::cancel(&mut app, id).unwrap();
    let id = open_prompt_dialog(&mut app);
    assert_eq!(app.genai.configured, ["gemini", "openai"]);

    switch_provider(&mut app, "openai", "gpt-image-2").unwrap();
    let p = app.session.image_provider().unwrap().info();
    assert_eq!((p.id, p.model.as_str()), ("openai", "gpt-image-2"));
    let pref = app.session.prefs().dialogs.get(PREF).unwrap().clone();
    assert_eq!((pref["provider"].as_str(), pref["model"].as_str()), (Some("openai"), Some("gpt-image-2")));
    assert_eq!(pref["options"]["gemini"]["model"], "m", "Gemini's model is kept for switching back");

    // Six references with OpenAI (it takes 15), then Gemini (4): the first 4 are sent, and the
    // window keeps all six for next time.
    let doc = app.session.active().unwrap().doc.id.0;
    let dir = std::env::temp_dir().join(format!("photocraft-genai-refs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut refs = vec![json!({"document": doc})];
    for i in 0..5 {
        let path = dir.join(format!("ref-{i}.png"));
        let px = photocraft_codecs::Image::from_u8(4, 4, photocraft_codecs::ChannelLayout::Rgba, vec![200; 64]).unwrap();
        std::fs::write(&path, photocraft_codecs::encode(&px, photocraft_codecs::Format::Png, &Default::default()).unwrap()).unwrap();
        refs.push(json!({"path": path.to_string_lossy()}));
    }
    let f = &mut app.ui.dialog_mut(id).unwrap().fields;
    f.insert("prompt".into(), json!("a red boat"));
    f.insert("references".into(), Value::Array(refs.clone()));
    switch_provider(&mut app, "gemini", "m").unwrap();
    crate::dialogs::confirm(&mut app, id).unwrap();
    assert_eq!(last_fill(&app)["references"].as_array().unwrap().len(), 4);
    assert_eq!(app.genai.references, refs);
    let _ = std::fs::remove_dir_all(dir);

    // Not from automation; a provider that can't be built leaves the one in use.
    app.automation_input = true;
    assert!(switch_provider(&mut app, "openai", "gpt-image-2").unwrap_err().contains("automation"));
    app.automation_input = false;
    store.borrow_mut().remove("openai");
    assert!(switch_provider(&mut app, "openai", "gpt-image-2").is_err());
    assert_eq!(app.session.image_provider().unwrap().info().id, "gemini");
    assert_eq!(app.session.prefs().dialogs.get(PREF).unwrap()["provider"], "gemini");
}

/// A provider saved by an earlier build that no longer offers it (the local server some test
/// builds had) reads as none: off, no error, and the settings open on Gemini.
#[test]
fn a_provider_no_longer_offered_reads_as_none() {
    let store = Store::default();
    store.borrow_mut().insert("gemini".into(), SECRET.into());
    let mut app = app_with(&store);
    app.session.prefs.edit(|p| p.dialogs.insert(PREF.into(), json!({"provider": "sdcpp", "model": "z-image-turbo"})));
    restore(&mut app).unwrap();
    assert!(app.session.image_provider().is_none());
    assert!(!fill_in_menu(&app));
    let id = open_settings(&mut app);
    assert_eq!(fields(&app, id)["provider"], "gemini");
}

/// File › Automate › Generative Batch… opens with any provider, says in the window when the model
/// can't edit images from instructions, and won't start until another is chosen there.
#[test]
fn the_batch_window_says_when_the_model_cannot_run_it() {
    let store = Store::default();
    let mut app = gemini_ready(&store);
    switch_provider(&mut app, "gemini", "fills-only").unwrap();
    assert!(crate::menus::menu_items(&app).iter().any(|i| i.id == photocraft_engine::genai_cmds::BATCH));
    let ctx = egui::Context::default();
    let id = crate::menus::invoke(&mut app, &ctx, photocraft_engine::genai_cmds::BATCH, json!({})).unwrap()["dialog"].as_u64().unwrap();
    app.ui.dialog_mut(id).unwrap().fields.insert("prompt".into(), json!("warmer"));
    let e = crate::dialogs::confirm(&mut app, id).unwrap_err();
    assert!(e.contains("doesn't edit images from instructions"), "{e}");
    switch_provider(&mut app, "gemini", "m").unwrap();
    let id = crate::menus::invoke(&mut app, &ctx, photocraft_engine::genai_cmds::BATCH, json!({})).unwrap()["dialog"].as_u64().unwrap();
    let out = std::env::temp_dir().join(format!("photocraft-genai-batch-model-{}", std::process::id()));
    let f = &mut app.ui.dialog_mut(id).unwrap().fields;
    f.insert("prompt".into(), json!("warmer"));
    f.insert("output".into(), json!(out.to_string_lossy()));
    crate::dialogs::confirm(&mut app, id).unwrap();
    let _ = std::fs::remove_dir_all(out);
}

/// Renders both windows with Gemini and OpenAI set up and a model that only fills chosen, its
/// notes showing (`GENAI_SNAPSHOT_DIR=… cargo test` saves a look).
#[test]
fn the_provider_pickers_render() {
    let store = Store::default();
    store.borrow_mut().insert("openai".into(), SECRET.into());
    let store2 = store.clone();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1440.0, 900.0)).with_max_steps(64).build_eframe(move |cc| {
        PhotocraftApp::setup_context(&cc.egui_ctx, Default::default());
        let mut app = gemini_ready(&store2);
        let id = open_prompt_dialog(&mut app);
        let doc = app.session.active().unwrap().doc.id.0;
        let f = &mut app.ui.dialog_mut(id).unwrap().fields;
        f.insert("mode".into(), json!("edit"));
        f.insert("references".into(), json!([{"document": doc}]));
        switch_provider(&mut app, "gemini", "fills-only").unwrap();
        app
    });
    h.run_steps(6);
    h.get_by_label("Provider");
    h.get_by_label("Model");
    h.get_by_label("fills-only doesn't edit what is in the selection: it fills the selection from the description.");
    h.get_by_label("fills-only doesn't take reference images: they aren't sent.");
    assert!(h.query_by_label("Edit selection").is_none());
    h.get_by_label("Empty: removes what is selected and continues the image through it.");
    assert!(h.query_by_label("The reference images are sent too, at most 1024 pixels on their long side.").is_none());
    let dir = std::env::var_os("GENAI_SNAPSHOT_DIR");
    if let Some(dir) = &dir
        && let Ok(img) = h.render()
    {
        img.save(std::path::Path::new(dir).join("genai-prompt-pickers.png")).unwrap();
    }
    let prompt = h.state().ui.dialogs[0].id;
    crate::dialogs::cancel(h.state_mut(), prompt).unwrap();
    switch_provider(h.state_mut(), "openai", "gpt-image-2").unwrap();
    let ctx = h.ctx.clone();
    crate::menus::invoke(h.state_mut(), &ctx, photocraft_engine::genai_cmds::BATCH, json!({})).unwrap();
    h.run_steps(6);
    h.get_by_label("Start Batch");
    if let Some(dir) = &dir
        && let Ok(img) = h.render()
    {
        img.save(std::path::Path::new(dir).join("genai-batch-pickers.png")).unwrap();
    }
}

/// Keep what is sent and returned: off by default; on, the session keeps each fill's request and
/// results under Documents/PhotoCraft/generative, through launches and provider switches.
#[test]
fn keeping_what_is_sent_is_off_until_ticked_in_the_settings() {
    let store = Store::default();
    store.borrow_mut().insert("openai".into(), SECRET.into());
    let mut app = gemini_ready(&store);
    assert!(app.session.generative_keep().is_none());
    let id = open_settings(&mut app);
    assert_eq!(fields(&app, id)["keep"], false);
    app.ui.dialog_mut(id).unwrap().fields.insert("keep".into(), json!(true));
    crate::dialogs::confirm(&mut app, id).unwrap();
    let kept = std::path::PathBuf::from(app.session.generative_keep().expect("on"));
    assert!(kept.ends_with(std::path::Path::new("Documents").join("PhotoCraft").join("generative")), "{kept:?}");
    switch_provider(&mut app, "openai", "gpt-image-2").unwrap();
    assert!(app.session.generative_keep().is_some(), "a provider switch keeps it");
    let mut next = PhotocraftApp::new(Session::new(), Services { genai: Some(services(&store)), ..Default::default() });
    next.session.prefs.edit(|p| p.dialogs = app.session.prefs().dialogs.clone());
    restore(&mut next).unwrap();
    assert!(next.session.generative_keep().is_some(), "and so does a launch");
    let id = open_settings(&mut app);
    app.ui.dialog_mut(id).unwrap().fields.insert("keep".into(), json!(false));
    crate::dialogs::confirm(&mut app, id).unwrap();
    assert!(app.session.generative_keep().is_none());
}
