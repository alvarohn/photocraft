//! File › Automate › Generative Batch…: pick the images (open documents, or a folder's, ticking
//! which), say what to change, add reference images, choose the variations and Same look, and
//! where the results go; Start Batch runs `edit.generativeBatch` in the background, its progress
//! and Cancel in the status bar like any job. When it ends a notice says what was written and what
//! failed, and the dialog opened again ticks only the files that failed.

use serde_json::{Map, Value, json};

use crate::PhotocraftApp;
use crate::state::DialogKind;
use photocraft_engine::genai_cmds::batch as gb;

pub(super) const MARK: &str = "__genaiBatch";
const BATCH: &str = gb::BATCH;

/// File › Automate › Generative Batch…: the dialog (or, before a provider is set up, the settings).
pub(super) fn menu(app: &mut PhotocraftApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    if id != BATCH || params.get("prompt").is_some() {
        return None;
    }
    if app.session.image_provider().is_none() {
        return Some(Ok(json!({"dialog": super::open_settings(app)})));
    }
    if let Some(why) = app.session.disabled_reason(BATCH) {
        return Some(Err(why));
    }
    Some(Ok(json!({"dialog": open(app)})))
}

/// The image files of `folder`, ticked (all, or only those that failed last time there).
fn folder_files(app: &PhotocraftApp, folder: &str) -> Vec<Value> {
    let files = photocraft_engine::file_cmds::list_images(folder).unwrap_or_default();
    let failed: Vec<&String> = app.genai.batch_failed.iter().filter(|p| files.contains(p)).collect();
    files.iter().map(|p| json!({"path": p, "on": failed.is_empty() || failed.contains(&p)})).collect()
}

fn open(app: &mut PhotocraftApp) -> u64 {
    if let Some(d) = app.ui.dialogs.iter().find(|d| d.fields.contains_key(MARK)) {
        return d.id;
    }
    let last = app.session.prefs().dialogs.get(BATCH).cloned().unwrap_or(Value::Null);
    let text = |k: &str| last.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    let documents: Vec<Value> = app.session.documents().iter().map(|d| json!({"id": d.doc.id.0, "on": true})).collect();
    let folder = text("folder");
    let source = match text("source").as_str() {
        "folder" => "folder",
        _ if documents.is_empty() => "folder",
        _ => "documents",
    };
    app.genai.configured = super::set_up_providers(app);
    let look = last.get("sameLookPrompt").and_then(Value::as_str).filter(|t| !t.trim().is_empty()).unwrap_or(gb::SAME_LOOK_PROMPT).to_owned();
    let mut f = Map::new();
    f.insert(MARK.into(), json!(true));
    f.insert("__label".into(), json!("Generative Batch"));
    f.insert("source".into(), json!(source));
    f.insert("documents".into(), Value::Array(documents));
    f.insert("files".into(), Value::Array(if folder.is_empty() { Vec::new() } else { folder_files(app, &folder) }));
    f.insert("folder".into(), json!(folder));
    f.insert("prompt".into(), json!(text("prompt")));
    f.insert("references".into(), Value::Array(app.genai.references.iter().filter(|r| super::reference_name(app, r).is_some()).cloned().collect()));
    f.insert("variations".into(), json!(super::chosen_variations(app)));
    f.insert("sameLook".into(), json!(last.get("sameLook").and_then(Value::as_bool).unwrap_or(false)));
    f.insert("sameLookPrompt".into(), json!(look));
    f.insert("output".into(), json!(text("output")));
    app.ui.open_dialog(DialogKind::Command, f)
}

/// Why the chosen model can't run a batch (it doesn't edit whole images from instructions), if so.
fn cannot_run(app: &PhotocraftApp) -> Option<String> {
    let p = app.session.image_provider()?;
    if p.capabilities().edit_in_place {
        return None;
    }
    Some(crate::i18n::fmt(
        tl!(
            "{model} doesn't edit images from instructions, which Generative Batch needs: choose a model that does, such as Gemini, OpenAI or Qwen Image 2.1 Turbo."
        ),
        &[("model", &p.info().model)],
    ))
}

/// How many reference images the user may add: the provider's, less one for Same look's.
pub(super) fn room(app: &PhotocraftApp, f: &Map<String, Value>) -> usize {
    let (_, max) = super::prompt_capabilities(app);
    if f.get("sameLook").and_then(Value::as_bool) == Some(true) { max.saturating_sub(1) } else { max }
}

/// The ticked entries of list `key`, by `field`.
fn ticked(f: &Map<String, Value>, key: &str, field: &str) -> Vec<Value> {
    let list = f.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
    list.iter().filter(|e| e.get("on").and_then(Value::as_bool) == Some(true)).filter_map(|e| e.get(field).cloned()).collect()
}

/// A ticked list (documents or files), at most a few rows tall, with All and None.
fn checklist(app: &PhotocraftApp, ui: &mut egui::Ui, f: &mut Map<String, Value>, key: &str) {
    let mut list = f.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
    let before = list.clone();
    let name = |e: &Value| -> String {
        if let Some(p) = e.get("path").and_then(Value::as_str) {
            return p.rsplit(['/', '\\']).next().unwrap_or(p).to_owned();
        }
        let id = e.get("id").and_then(Value::as_u64).unwrap_or(0);
        app.session.documents().iter().find(|d| d.doc.id.0 == id).map_or_else(String::new, |d| d.doc.name.clone())
    };
    // Documents closed since the dialog opened are dropped.
    list.retain(|e| !name(e).is_empty());
    ui.horizontal(|ui| {
        let on = list.iter().filter(|e| e.get("on").and_then(Value::as_bool) == Some(true)).count();
        super::faint(ui, &crate::i18n::fmt(tl!("{on} of {n} ticked"), &[("on", &on.to_string()), ("n", &list.len().to_string())]));
        if ui.small_button(tl!("All")).clicked() {
            list.iter_mut().for_each(|e| e["on"] = json!(true));
        }
        if ui.small_button(tl!("None")).clicked() {
            list.iter_mut().for_each(|e| e["on"] = json!(false));
        }
    });
    egui::ScrollArea::vertical().id_salt(("genai-batch-list", key)).max_height(140.0).auto_shrink([false, true]).show(ui, |ui| {
        for e in &mut list {
            let mut on = e.get("on").and_then(Value::as_bool) == Some(true);
            let label = name(e);
            let r = ui.checkbox(&mut on, egui::RichText::new(&label).size(12.0));
            if r.changed() {
                e["on"] = json!(on);
            }
        }
    });
    if list != before {
        f.insert(key.into(), Value::Array(list));
    }
}

pub(super) fn body(app: &mut PhotocraftApp, ui: &mut egui::Ui, f: &mut Map<String, Value>) {
    let t = crate::theme::Tokens::get(ui.ctx());
    let section = |ui: &mut egui::Ui, text: &str| {
        ui.label(egui::RichText::new(text).font(crate::theme::semibold(12.5)).color(t.text));
    };
    ui.set_min_width(460.0);
    super::provider_rows(app, ui, f, "batch");
    if let Some(why) = cannot_run(app) {
        super::warn(ui, &why);
    } else {
        super::references_note(app, ui, f, room(app, f));
    }
    ui.add_space(8.0);
    section(ui, tl!("Images"));
    let folder_mode = f.get("source").and_then(Value::as_str) == Some("folder");
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (source, label, on) in [("documents", tl!("Open documents"), !folder_mode), ("folder", tl!("A folder"), folder_mode)] {
            let r = crate::widgets::pill_tab(ui, label, on);
            r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, label));
            if r.clicked() {
                f.insert("source".into(), json!(source));
            }
        }
    });
    ui.add_space(4.0);
    let mut choose_folder = false;
    if folder_mode {
        ui.horizontal(|ui| {
            let folder = f.get("folder").and_then(Value::as_str).unwrap_or("").to_owned();
            let shown = if folder.is_empty() { tl!("No folder chosen").to_owned() } else { folder.clone() };
            ui.add(egui::Label::new(egui::RichText::new(shown).size(12.0).color(t.text_dim)).truncate()).on_hover_text(&folder);
            choose_folder = crate::widgets::secondary_button(ui, tl!("Choose Folder…"), 0.0).clicked();
        });
        checklist(app, ui, f, "files");
    } else {
        checklist(app, ui, f, "documents");
    }
    ui.add_space(8.0);
    section(ui, tl!("What to change"));
    let mut prompt = f.get("prompt").and_then(Value::as_str).unwrap_or("").to_owned();
    let r = ui.add(
        egui::TextEdit::singleline(&mut prompt)
            .hint_text(tl!("What to change in each image"))
            .desired_width(f32::INFINITY)
            .char_limit(photocraft_genai::MAX_PROMPT),
    );
    crate::field_tab::register(ui.ctx(), r.id);
    if r.changed() {
        f.insert("prompt".into(), json!(prompt));
    }
    super::faint(ui, tl!("Each image is edited whole, as Edit selection does with all of it selected."));
    let max = room(app, f);
    if max > 0 {
        ui.add_space(8.0);
        super::references_section(app, ui, f, max);
    }
    ui.add_space(8.0);
    super::variations_row(ui, f);
    if super::prompt_capabilities(app).1 > 0 {
        ui.add_space(6.0);
        let mut look = f.get("sameLook").and_then(Value::as_bool) == Some(true);
        if crate::widgets::checkbox(ui, &mut look, tl!("Same look in all")).changed() {
            f.insert("sameLook".into(), json!(look));
        }
        if look {
            super::faint(
                ui,
                tl!("The first image is edited first; its result goes to the others as their last reference image, with this added to their prompt:"),
            );
            let mut words = f.get("sameLookPrompt").and_then(Value::as_str).unwrap_or("").to_owned();
            let r = ui.add(egui::TextEdit::multiline(&mut words).desired_rows(2).desired_width(f32::INFINITY).char_limit(photocraft_genai::MAX_PROMPT));
            crate::field_tab::register(ui.ctx(), r.id);
            if r.changed() {
                f.insert("sameLookPrompt".into(), json!(words));
            }
        }
    }
    ui.add_space(8.0);
    section(ui, tl!("Results"));
    let output = f.get("output").and_then(Value::as_str).unwrap_or("").to_owned();
    let mut choose_output = false;
    ui.horizontal(|ui| {
        let shown = if output.is_empty() {
            crate::i18n::fmt(tl!("JPEG files in an {folder} folder beside each image"), &[("folder", gb::OUTPUT_FOLDER)])
        } else {
            output.clone()
        };
        ui.add(egui::Label::new(egui::RichText::new(shown).size(12.0).color(t.text_dim)).truncate()).on_hover_text(&output);
        choose_output = crate::widgets::secondary_button(ui, tl!("Choose Folder…"), 0.0).clicked();
        if !output.is_empty() && ui.small_button(tl!("Beside each image")).clicked() {
            f.insert("output".into(), json!(""));
        }
    });
    if let Some(info) = app.session.image_provider().map(|p| p.info()) {
        ui.add_space(6.0);
        super::faint(
            ui,
            &crate::i18n::fmt(
                tl!("Each image is sent to {name} ({model}) once per variation, with the reference images."),
                &[("name", info.name), ("model", &info.model)],
            ),
        );
    }
    if choose_folder
        && let Err(e) = app.ask_folder(|app, path| {
            let files = folder_files(app, &path);
            if let Some(d) = app.ui.dialogs.iter_mut().find(|d| d.fields.contains_key(MARK)) {
                d.fields.insert("folder".into(), json!(path));
                d.fields.insert("files".into(), Value::Array(files));
            }
            Ok(Value::Null)
        })
    {
        app.ui.status = e;
    }
    if choose_output
        && let Err(e) = app.ask_folder(|app, path| {
            if let Some(d) = app.ui.dialogs.iter_mut().find(|d| d.fields.contains_key(MARK)) {
                d.fields.insert("output".into(), json!(path));
            }
            Ok(Value::Null)
        })
    {
        app.ui.status = e;
    }
}

/// Start Batch: remember the choices and run the batch in the background.
pub(super) fn confirm(app: &mut PhotocraftApp, f: &Map<String, Value>) -> Result<Value, String> {
    let text = |k: &str| f.get(k).and_then(Value::as_str).unwrap_or("").trim().to_owned();
    let (prompt, output, folder) = (text("prompt"), text("output"), text("folder"));
    if let Some(why) = cannot_run(app) {
        return Err(why);
    }
    if prompt.is_empty() {
        return Err(tl!("Say what to change in each image.").into());
    }
    let folder_mode = text("source") == "folder";
    let same_look = f.get("sameLook").and_then(Value::as_bool) == Some(true) && super::prompt_capabilities(app).1 > 0;
    let references: Vec<Value> = super::references_of(f).into_iter().filter(|r| super::reference_name(app, r).is_some()).take(room(app, f)).collect();
    let variations = f.get("variations").and_then(Value::as_u64).unwrap_or_else(|| super::chosen_variations(app));
    let remembered = json!({"prompt": prompt, "source": text("source"), "folder": folder, "output": output, "sameLook": same_look, "sameLookPrompt": text("sameLookPrompt")});
    app.session.prefs.edit(|p| p.dialogs.insert(BATCH.into(), remembered));
    app.genai.references = super::references_of(f).into_iter().filter(|r| super::reference_name(app, r).is_some()).collect();
    let mut params = Map::new();
    params.insert("prompt".into(), json!(prompt));
    params.insert("variations".into(), json!(variations));
    if folder_mode {
        params.insert("files".into(), Value::Array(ticked(f, "files", "path")));
    } else {
        params.insert("documents".into(), Value::Array(ticked(f, "documents", "id")));
    }
    if !references.is_empty() {
        params.insert("references".into(), Value::Array(references));
    }
    if same_look {
        params.insert("sameLook".into(), json!(true));
        params.insert("sameLookPrompt".into(), json!(text("sameLookPrompt")));
    }
    if !output.is_empty() {
        params.insert("output".into(), json!(output));
    }
    app.run(BATCH, Value::Object(params))
}

/// The batch's job ended: say what it wrote and what failed, and keep the failed files for the
/// next time the dialog opens. True if it was a batch.
pub(super) fn on_job_event(app: &mut PhotocraftApp, e: &photocraft_engine::jobs::JobEvent) -> bool {
    use photocraft_engine::jobs::JobOutcome;
    if e.command != BATCH {
        return false;
    }
    match &e.outcome {
        JobOutcome::Done(v) => {
            let failed = v.get("failed").and_then(Value::as_array).cloned().unwrap_or_default();
            app.genai.batch_failed = failed.iter().filter_map(|f| f.get("path").and_then(Value::as_str).map(str::to_owned)).collect();
            let written = v.get("written").and_then(Value::as_array).map_or(0, Vec::len);
            let folders: Vec<&str> = v.get("folders").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            let mut text = crate::i18n::fmt(
                tl!("Generative Batch wrote {n} files to {folder}."),
                &[
                    ("n", &written.to_string()),
                    (
                        "folder",
                        &match folders.as_slice() {
                            [one] => (*one).to_owned(),
                            _ => crate::i18n::fmt(tl!("{n} folders"), &[("n", &folders.len().to_string())]),
                        },
                    ),
                ],
            );
            if !failed.is_empty() {
                let first = failed.first().and_then(|f| f.get("error")).and_then(Value::as_str).unwrap_or("");
                text.push(' ');
                text.push_str(&crate::i18n::fmt(tl!("{n} failed ({error})."), &[("n", &failed.len().to_string()), ("error", first)]));
            }
            if let Some(why) = v.get("stopped").and_then(Value::as_str) {
                text.push(' ');
                text.push_str(&crate::i18n::fmt(tl!("Stopped: {why}"), &[("why", why)]));
            }
            if failed.is_empty() {
                app.ui.status = text;
                app.ui.status_error = false;
            } else {
                crate::notices::error(app, text);
            }
        }
        JobOutcome::Failed(err) => crate::notices::error(app, crate::i18n::fmt(tl!("Generative Batch failed: {error}"), &[("error", err)])),
        JobOutcome::Cancelled => {
            app.ui.status = tl!("Generative Batch cancelled; the files already written are kept.").to_owned();
            app.ui.status_error = false;
        }
    }
    true
}
