//! File › Automate › Generative Batch…: the same Edit (prompt, reference images, variations) for
//! a set of images, a folder's or the open documents', each result written as a JPEG to an `IA`
//! folder beside its image (or a folder chosen), whatever the provider. One background job:
//! cancellable, with its progress in the status bar, sharing the provider's turns with the
//! fills (at most [`MAX_PARALLEL`] images at once, one for a local server).
//!
//! Each image is edited whole: its composite (all visible layers, sRGB) at most 2048 px on its
//! long side (the provider's working size if smaller) with a mask of all of it, in Edit mode; each
//! result is scaled to the image's own size, so it can stand in for it.
//!
//! *Same look*: the first image is edited first, alone; its first result goes to every other one
//! as their last reference image, with a sentence added to their prompt saying what it is for,
//! so a set of photos comes out matching.
//!
//! An image that fails is reported and the batch goes on, except that two failures in a row that
//! the account causes (the key, the balance, the rate limit) stop it: the rest would fail too.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use photocraft_codecs::{self as codecs, ChannelLayout, Format, Image, SampleType as CodecSample};
use photocraft_color::PixelFormat;
use photocraft_doc::Document;
use photocraft_genai::{EditMode, EditRequest, GenError, ImageProvider, ImageSize, ReferenceImage};
use photocraft_geom::Rect;
use photocraft_raster::Surface;
use serde_json::{Value, json};

use super::{MAX_PARALLEL, ReferenceSource, UPLOAD_SIDE, queue};
use crate::commands::CommandSpec;
use crate::jobs::JobCtx;
use crate::{EngineError, Result, Session};

pub const BATCH: &str = "edit.generativeBatch";
const LABEL: &str = "Generative Batch";
/// Most images one batch takes.
pub const MAX_ITEMS: usize = 1000;
/// The folder the results go to, beside each image, unless another is chosen.
pub const OUTPUT_FOLDER: &str = "IA";
/// What Same look adds to the prompt of every image after the first, unless the user words it.
pub const SAME_LOOK_PROMPT: &str = "Match the colours, light and overall look of the last reference image: it is the first photo of this set, \
                                    already edited the same way.";
/// Failures the account causes, in a row, that stop a batch.
const ACCOUNT_STOP: usize = 2;
const JPEG_QUALITY: u8 = 92;
/// Long side of the Same look reference as sent.
const LOOK_SIDE: u32 = 1024;

fn bad(msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: BATCH.into(), msg: msg.into() }
}

fn other(e: impl std::fmt::Display) -> EngineError {
    EngineError::Other(e.to_string())
}

/// Where an image comes from.
enum Source {
    /// A file, read when its turn comes.
    File(String),
    /// An open document, as it was when the batch started.
    Document(Arc<Document>),
}

/// One image of a batch and where its results go.
struct Item {
    source: Source,
    /// Shown in progress and results: the file's or document's name.
    name: String,
    /// The folder its results go to, and their name without extension.
    dir: String,
    stem: String,
}

/// What every image is asked for.
struct Ask {
    prompt: String,
    references: Vec<ReferenceSource>,
    count: usize,
    /// The sentence added after the first image's result when Same look is on.
    same_look: Option<String>,
}

/// Is this a failure the account causes, which the next image would hit too?
fn account_failure(e: &GenError) -> bool {
    matches!(e, GenError::Auth | GenError::Account(_) | GenError::Quota | GenError::Http { status: 402, .. })
}

fn batch_enabled(s: &Session) -> std::result::Result<(), String> {
    let p = s.image_provider().ok_or("set up a generative AI provider first (Generative AI Settings)")?;
    let caps = p.capabilities();
    if !caps.edit || !caps.edit_in_place {
        return Err(format!("{} ({}) can't edit whole images from a prompt; choose a model that edits from instructions", p.info().name, p.info().model));
    }
    Ok(())
}

/// The file name part of `path` and the folder it is in.
fn split_path(path: &str) -> (String, String) {
    let p = std::path::Path::new(path);
    let stem = p.file_stem().map_or_else(|| "image".to_owned(), |s| s.to_string_lossy().into_owned());
    let dir = p.parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
    (dir, stem)
}

fn join(dir: &str, name: &str) -> String {
    std::path::Path::new(dir).join(name).to_string_lossy().into_owned()
}

fn batch(s: &mut Session, p: &Value) -> Result<Value> {
    batch_enabled(s).map_err(bad)?;
    let provider = s.image_provider().cloned().ok_or_else(|| bad("no generative AI provider"))?;
    let prompt = match p.get("prompt") {
        Some(Value::String(t)) if !t.trim().is_empty() => t.trim().to_owned(),
        _ => return Err(bad("say what to change in each image (`prompt`)")),
    };
    photocraft_genai::check_prompt(&prompt, true).map_err(|e| bad(e.to_string()))?;
    let count = super::variations_param(p, "variations", provider.as_ref())
        .map_err(|_| bad(format!("`variations` must be a whole number from 1 to {}", super::MAX_VARIATIONS)))?;
    let max = provider.capabilities().max_references.min(photocraft_genai::REFERENCES_LIMIT);
    let same_look = match p.get("sameLook") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad("`sameLook` must be true or false")),
    };
    let same_look = if same_look {
        let words = match p.get("sameLookPrompt") {
            None | Some(Value::Null) => SAME_LOOK_PROMPT.to_owned(),
            Some(Value::String(t)) => t.trim().to_owned(),
            Some(_) => return Err(bad("`sameLookPrompt` must be text")),
        };
        if max == 0 {
            return Err(bad(format!("{} can't take reference images, which Same look needs", provider.info().name)));
        }
        photocraft_genai::check_prompt(&format!("{prompt} {words}"), true).map_err(|e| bad(e.to_string()))?;
        Some(words)
    } else {
        None
    };
    // Same look's reference takes one of the provider's places.
    let room = if same_look.is_some() { max.saturating_sub(1) } else { max };
    let references = super::references_param(s, p, room, provider.info().name).map_err(|e| match e {
        EngineError::BadParams { msg, .. } => bad(msg),
        e => e,
    })?;
    let output = match p.get("output") {
        None | Some(Value::Null) => None,
        Some(Value::String(d)) if !d.trim().is_empty() => Some(d.trim().to_owned()),
        Some(_) => return Err(bad("`output` must be a folder")),
    };
    let items = items_param(s, p, output.as_deref())?;
    let ask = Ask { prompt, references, count, same_look };
    let turns = Arc::clone(&s.genai_state.turns);
    crate::jobs::run(s, LABEL, false, move |ctx| run_batch(ctx, provider.as_ref(), &turns, &items, &ask), |_, summary| Ok(summary))
}

/// The images: `files` (paths) and `documents` (ids of open documents), each with its folder.
fn items_param(s: &Session, p: &Value, output: Option<&str>) -> Result<Vec<Item>> {
    let list = |key: &str| match p.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => Ok(a.clone()),
        Some(_) => Err(bad(format!("`{key}` must be a list"))),
    };
    let (files, documents) = (list("files")?, list("documents")?);
    if files.len().saturating_add(documents.len()) > MAX_ITEMS {
        return Err(bad(format!("a batch takes at most {MAX_ITEMS} images")));
    }
    let folder = |dir: &str| output.map_or_else(|| join(dir, OUTPUT_FOLDER), str::to_owned);
    let mut items = Vec::new();
    for f in files {
        let path = f.as_str().filter(|p| !p.trim().is_empty()).ok_or_else(|| bad("`files` must be a list of paths"))?.to_owned();
        let (dir, stem) = split_path(&path);
        let name = std::path::Path::new(&path).file_name().map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned());
        items.push(Item { source: Source::File(path), name, dir: folder(&dir), stem });
    }
    for d in documents {
        let id = d.as_u64().ok_or_else(|| bad("`documents` must be a list of document ids"))?;
        let st = s.documents().iter().find(|st| st.doc.id.0 == id).ok_or_else(|| bad(format!("no open document has id {id}")))?;
        let name = st.doc.name.clone();
        let (dir, stem) = match (&st.path, output) {
            (Some(path), _) => {
                let (dir, stem) = split_path(path);
                (folder(&dir), stem)
            }
            (None, Some(out)) => (out.to_owned(), split_path(&name).1),
            (None, None) => return Err(bad(format!("{name} has never been saved: choose a folder for the results"))),
        };
        items.push(Item { source: Source::Document(Arc::clone(&st.doc)), name, dir, stem });
    }
    if items.is_empty() {
        return Err(bad("choose at least one image (`files` or `documents`)"));
    }
    // Two images with the same name (photo.png, photo.jpg) going to one folder each get their own.
    let mut seen = std::collections::HashSet::new();
    for item in &mut items {
        let mut stem = item.stem.clone();
        let mut n = 2;
        while !seen.insert((item.dir.clone(), stem.to_lowercase())) {
            stem = format!("{} ({n})", item.stem);
            n += 1;
        }
        item.stem = stem;
    }
    Ok(items)
}

/// What became of one image.
enum Outcome {
    /// The files written, and (for Same look) the first result.
    Done(Vec<String>, Option<Surface>),
    Failed(String, bool),
}

/// The whole batch: images side by side as the provider allows, stopping when cancelled or
/// after [`ACCOUNT_STOP`] account failures in a row.
fn run_batch(ctx: &JobCtx, provider: &dyn ImageProvider, turns: &queue::Turns, items: &[Item], ask: &Ask) -> Result<Value> {
    let n = items.len();
    ctx.progress(0.0, &format!("0 of {n} images"));
    let mut references = ask.references.iter().map(ReferenceSource::image).collect::<Result<Vec<_>>>()?;
    let written = Mutex::new(Vec::<String>::new());
    let failed = Mutex::new(Vec::<Value>::new());
    let stopped = Mutex::new(None::<String>);
    let in_a_row = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let record = |item: &Item, outcome: Outcome| -> Option<Surface> {
        let mut first = None;
        match outcome {
            Outcome::Done(files, look) => {
                in_a_row.store(0, Ordering::SeqCst);
                written.lock().unwrap_or_else(PoisonError::into_inner).extend(files);
                first = look;
            }
            Outcome::Failed(error, account) => {
                let mut entry = json!({"name": item.name, "error": error});
                match &item.source {
                    Source::File(path) => entry["path"] = json!(path),
                    Source::Document(doc) => entry["document"] = json!(doc.id.0),
                }
                failed.lock().unwrap_or_else(PoisonError::into_inner).push(entry);
                if account && in_a_row.fetch_add(1, Ordering::SeqCst) + 1 >= ACCOUNT_STOP {
                    stop.store(true, Ordering::SeqCst);
                    stopped.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert(error);
                } else if !account {
                    in_a_row.store(0, Ordering::SeqCst);
                }
            }
        }
        let k = done.fetch_add(1, Ordering::SeqCst) + 1;
        ctx.progress(k as f32 / n as f32, &format!("{k} of {n} images: {}", item.name));
        first
    };
    let mut prompt = ask.prompt.clone();
    let mut start = 0;
    if let Some(words) = &ask.same_look {
        // The first image alone, first: the others follow its look.
        let first = items.first().ok_or_else(|| other("no images"))?;
        let outcome = edit_item(ctx, provider, turns, first, &prompt, &references, ask.count, true);
        let failed_first = matches!(outcome, Outcome::Failed(..));
        let look = record(first, outcome);
        ctx.check()?;
        match look {
            Some(look) => references.push(look_reference(&look)?),
            None if failed_first => {
                let why =
                    failed.lock().unwrap_or_else(PoisonError::into_inner).first().and_then(|f| f.get("error")).and_then(Value::as_str).unwrap_or("").to_owned();
                stopped.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert(format!("the first image, whose look the others follow, failed: {why}"));
                stop.store(true, Ordering::SeqCst);
            }
            None => {}
        }
        prompt = format!("{prompt} {words}");
        start = 1;
    }
    let next = AtomicUsize::new(start);
    let workers = provider.max_parallel().unwrap_or(MAX_PARALLEL).clamp(1, MAX_PARALLEL).min(n.saturating_sub(start).max(1));
    let work = || {
        loop {
            if stop.load(Ordering::SeqCst) || ctx.cancelled() {
                break;
            }
            let i = next.fetch_add(1, Ordering::SeqCst);
            let Some(item) = items.get(i) else { break };
            let outcome = edit_item(ctx, provider, turns, item, &prompt, &references, ask.count, false);
            if ctx.cancelled() {
                break;
            }
            record(item, outcome);
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::scope(|scope| {
        for _ in 1..workers {
            scope.spawn(work);
        }
        work();
    });
    #[cfg(target_arch = "wasm32")]
    {
        let _ = workers;
        work();
    }
    ctx.check()?;
    let written = written.into_inner().unwrap_or_else(PoisonError::into_inner);
    let failed = failed.into_inner().unwrap_or_else(PoisonError::into_inner);
    let stopped = stopped.into_inner().unwrap_or_else(PoisonError::into_inner);
    let skipped = n.saturating_sub(done.load(Ordering::SeqCst));
    let folders: std::collections::BTreeSet<&str> = items.iter().map(|i| i.dir.as_str()).collect();
    Ok(json!({"written": written, "failed": failed, "stopped": stopped, "skipped": skipped, "images": n, "folders": folders}))
}

/// One image: read, sent with a mask of all of it, its results written.
#[allow(clippy::too_many_arguments)]
fn edit_item(
    ctx: &JobCtx,
    provider: &dyn ImageProvider,
    turns: &queue::Turns,
    item: &Item,
    prompt: &str,
    references: &[ReferenceImage],
    count: usize,
    keep_first: bool,
) -> Outcome {
    let fail = |e: EngineError| Outcome::Failed(e.to_string(), false);
    let doc = match &item.source {
        Source::Document(doc) => Arc::clone(doc),
        Source::File(path) => match crate::file_cmds::read_file(path).and_then(|bytes| super::reference_document(path, &bytes)) {
            Ok(doc) => Arc::new(doc),
            Err(e) => return fail(e),
        },
    };
    let canvas = doc.bounds();
    if canvas.is_empty() {
        return Outcome::Failed("the image is empty".into(), false);
    }
    let request = match request_for(provider, &doc, canvas, prompt, references) {
        Ok(r) => r,
        Err(e) => return fail(e),
    };
    let Ok(turn) = turns.take(ctx, &AtomicUsize::new(0), provider.max_parallel()) else {
        return Outcome::Failed("cancelled".into(), false);
    };
    let results = match super::request_results(ctx, provider, &request, count, None) {
        Ok(Ok(results)) => results,
        Ok(Err(e)) => return Outcome::Failed(e.to_string(), account_failure(&e)),
        Err(e) => return fail(e),
    };
    drop(turn);
    match write_results(item, &results, canvas, keep_first) {
        Ok(done) => done,
        Err(e) => fail(e),
    }
}

/// The request for a whole image: its composite and a mask of all of it, in Edit mode.
fn request_for(provider: &dyn ImageProvider, doc: &Document, canvas: Rect, prompt: &str, references: &[ReferenceImage]) -> Result<EditRequest> {
    let k = (f64::from(UPLOAD_SIDE) / f64::from(canvas.width().max(canvas.height()))).min(1.0);
    let (w, h) = (((f64::from(canvas.width()) * k).round() as u32).max(1), ((f64::from(canvas.height()) * k).round() as u32).max(1));
    let (w, h) = provider.request_size(w, h);
    let (w, h) = (w.clamp(1, UPLOAD_SIDE), h.clamp(1, UPLOAD_SIDE));
    let mut all = Surface::new(PixelFormat::GRAY8);
    all.fill_rect(canvas, &[1.0]);
    let composite = super::scaled(&super::srgb_composite(doc, canvas)?, canvas, w, h);
    let (image, mask, _) = super::request_images(&composite, &all, canvas, w, h, EditMode::Edit)?;
    Ok(EditRequest { image, mask, prompt: prompt.to_owned(), size: ImageSize::for_long_side(w.max(h)), mode: EditMode::Edit, references: references.to_vec() })
}

/// Each result scaled to the image's size, as a JPEG: `<stem>.jpg`, then `<stem>-2.jpg`, … for
/// more variations. A file that is the image itself is never overwritten.
fn write_results(item: &Item, results: &[Vec<u8>], canvas: Rect, keep_first: bool) -> Result<Outcome> {
    #[cfg(not(target_arch = "wasm32"))]
    std::fs::create_dir_all(&item.dir).map_err(|e| other(format!("{}: {e}", item.dir)))?;
    let source = match &item.source {
        Source::File(path) => std::fs::canonicalize(path).ok(),
        Source::Document(_) => None,
    };
    let mut written = Vec::new();
    let mut first = None;
    for (i, bytes) in results.iter().enumerate() {
        let (got, rect) = super::decode(bytes)?;
        let fitted = super::scaled(&got, rect, canvas.width(), canvas.height());
        let name = if i == 0 { format!("{}.jpg", item.stem) } else { format!("{}-{}.jpg", item.stem, i + 1) };
        let mut path = join(&item.dir, &name);
        if source.is_some() && std::fs::canonicalize(&path).ok() == source {
            path = join(&item.dir, &format!("{} (IA).jpg", name.trim_end_matches(".jpg")));
        }
        crate::file_cmds::write_file(&path, &jpeg(&fitted, canvas.width(), canvas.height(), JPEG_QUALITY)?)?;
        written.push(path);
        if keep_first && i == 0 {
            first = Some(fitted);
        }
    }
    Ok(Outcome::Done(written, first))
}

/// The first `w`×`h` pixels of an 8-bit sRGB surface as a JPEG (alpha dropped).
fn jpeg(s: &Surface, w: u32, h: u32, quality: u8) -> Result<Vec<u8>> {
    let rgba =
        s.convert(PixelFormat::new(photocraft_color::ColorMode::Rgb, photocraft_color::SampleType::U8, true)).to_interleaved(Rect::from_xywh(0, 0, w, h));
    let rgb: Vec<u8> = rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
    let img = Image::from_raw(w, h, ChannelLayout::Rgb, CodecSample::U8, rgb).map_err(other)?;
    let opts = codecs::EncodeOptions { jpeg_quality: quality, embed_icc: false, embed_metadata: false, ..Default::default() };
    codecs::encode(&img, Format::Jpeg, &opts).map_err(other)
}

/// The first image's result as the reference the others follow: at most [`LOOK_SIDE`] px.
fn look_reference(s: &Surface) -> Result<ReferenceImage> {
    let r = s.content_bounds();
    let (w, h) = (r.width().max(1), r.height().max(1));
    let k = (f64::from(LOOK_SIDE) / f64::from(w.max(h))).min(1.0);
    let (tw, th) = (((f64::from(w) * k).round() as u32).max(1), ((f64::from(h) * k).round() as u32).max(1));
    let small = super::scaled(s, r, tw, th);
    Ok(ReferenceImage { bytes: jpeg(&small, tw, th, 90)?, mime: "image/jpeg".into() })
}

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: BATCH,
        label: "Generative Batch…",
        menu: &["File", "Automate"],
        shortcut: None,
        params: r#"{"prompt":str,"files":[path]?,"documents":[id]?,"variations":1..4?,"references":[path|{"path":str}|{"document":id}]?,"sameLook":bool=false,"sameLookPrompt":str?,"output":path?} edits each image whole through the configured provider, as Generative Fill's Edit does with a selection of all of it, and writes each result as a JPEG the image's size to `output`, else to an "IA" folder beside the image: <name>.jpg, <name>-2.jpg… for more variations. With `sameLook` the first image goes first and its result is the last reference image of the others, with `sameLookPrompt` added to their prompt. Two failures in a row that the account causes (key, balance, rate limit) stop it. Runs in the background: {written, failed:[{name, error}], stopped, skipped, images, folders}. Sends every image to the provider, once per variation."#,
        enabled: batch_enabled,
        run: batch,
        journal: false,
    }]
}

#[cfg(test)]
#[path = "batch_tests.rs"]
mod tests;
