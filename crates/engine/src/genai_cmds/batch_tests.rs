use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use photocraft_genai::{Capabilities, GeneratedImage, ProviderInfo};

use super::*;

/// Answers every edit with a solid colour at the request's size and keeps what it was sent; with
/// `fail`, every call fails that way; `edits` false: a model that can't edit from instructions.
struct Fake {
    color: [u8; 3],
    fail: Option<GenError>,
    edits: bool,
    calls: AtomicUsize,
    sent: Mutex<Vec<EditRequest>>,
}

fn fake() -> Fake {
    Fake { color: [0, 0, 255], fail: None, edits: true, calls: AtomicUsize::new(0), sent: Mutex::new(Vec::new()) }
}

impl ImageProvider for Fake {
    fn info(&self) -> ProviderInfo {
        ProviderInfo { id: "fake", name: "Fake", model: "fake-1".into(), host: "example.invalid" }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities { generate: false, edit: true, variations: false, edit_in_place: self.edits, max_references: 4 }
    }
    fn edit(&self, r: &EditRequest) -> photocraft_genai::Result<GeneratedImage> {
        self.sent.lock().unwrap().push(r.clone());
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(e) = &self.fail {
            return Err(e.clone());
        }
        let (w, h) = photocraft_genai::png_size(&r.image).unwrap();
        let px: Vec<u8> = (0..w * h).flat_map(|_| [self.color[0], self.color[1], self.color[2], 255]).collect();
        let img = Image::from_u8(w, h, ChannelLayout::Rgba, px).unwrap();
        Ok(GeneratedImage { bytes: codecs::encode(&img, Format::Png, &Default::default()).unwrap(), mime: "image/png".into() })
    }
}

/// A new empty folder for one test.
fn folder(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("photocraft-genai-batch-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A `w`×`h` PNG of one colour at `path`.
fn png(path: &std::path::Path, w: u32, h: u32) {
    let px: Vec<u8> = (0..w * h).flat_map(|_| [200u8, 180, 20, 255]).collect();
    let img = Image::from_u8(w, h, ChannelLayout::Rgba, px).unwrap();
    std::fs::write(path, codecs::encode(&img, Format::Png, &Default::default()).unwrap()).unwrap();
}

fn session_with(provider: Arc<Fake>) -> Session {
    let mut s = Session::new();
    s.set_image_provider(Some(provider));
    s
}

fn paths(dir: &std::path::Path, names: &[&str]) -> Vec<String> {
    names.iter().map(|n| dir.join(n).to_string_lossy().into_owned()).collect()
}

fn decoded(path: &str) -> codecs::Image {
    codecs::decode(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn a_folder_of_images_is_edited_into_an_ia_folder_at_each_images_size() {
    let dir = folder("files");
    png(&dir.join("a.png"), 64, 48);
    png(&dir.join("b.png"), 30, 90);
    let provider = Arc::new(fake());
    let mut s = session_with(provider.clone());
    let r = s.execute(BATCH, json!({"prompt": "warmer light", "files": paths(&dir, &["a.png", "b.png"]), "variations": 2})).unwrap();
    assert_eq!(r["images"], 2);
    assert_eq!(r["failed"], json!([]));
    let written: Vec<String> = r["written"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
    let ia = dir.join(OUTPUT_FOLDER);
    let want = paths(&ia, &["a.jpg", "a-2.jpg", "b.jpg", "b-2.jpg"]);
    assert_eq!(written.iter().collect::<std::collections::BTreeSet<_>>(), want.iter().collect());
    let a = decoded(&want[0]);
    assert_eq!((a.width(), a.height()), (64, 48), "the image's own size");
    let b = decoded(&want[2]);
    assert_eq!((b.width(), b.height()), (30, 90));
    // A blue result, as a JPEG.
    let px = a.convert(ChannelLayout::Rgb, CodecSample::U8).data()[..3].to_vec();
    assert!(px[2] > 200 && px[0] < 40, "{px:?}");
    // Each image went whole, in Edit mode, with a mask of all of it, once per variation.
    let sent = provider.sent.lock().unwrap();
    assert_eq!(sent.len(), 4);
    assert!(sent.iter().all(|r| r.mode == EditMode::Edit && r.prompt == "warmer light"));
    let mask = codecs::decode(&sent[0].mask).unwrap().convert(ChannelLayout::Rgba, CodecSample::U8);
    assert!(mask.data().as_chunks::<4>().0.iter().all(|p| p[0] == 255), "all of it");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn same_look_sends_the_first_result_to_the_others_with_its_words() {
    let dir = folder("look");
    for n in ["1.png", "2.png", "3.png"] {
        png(&dir.join(n), 40, 40);
    }
    let provider = Arc::new(fake());
    let mut s = session_with(provider.clone());
    let refs = dir.join("logo.png");
    png(&refs, 10, 10);
    let r = s
        .execute(BATCH, json!({"prompt": "make it moody", "files": paths(&dir, &["1.png", "2.png", "3.png"]), "variations": 1, "sameLook": true, "references": [refs.to_string_lossy()]}))
        .unwrap();
    assert_eq!(r["written"].as_array().unwrap().len(), 3);
    let sent = provider.sent.lock().unwrap();
    assert_eq!(sent[0].references.len(), 1, "the first goes with the user's reference only");
    assert_eq!(sent[0].prompt, "make it moody");
    for later in &sent[1..] {
        assert_eq!(later.references.len(), 2, "and the first result, last");
        assert_eq!(later.references[0], sent[0].references[0]);
        assert_eq!(later.references[1].mime, "image/jpeg");
        assert_eq!(later.prompt, format!("make it moody {SAME_LOOK_PROMPT}"));
    }
    drop(sent);
    // The words can be the user's own; Same look needs a place for its reference.
    let provider = Arc::new(fake());
    let mut s = session_with(provider.clone());
    s.execute(
        BATCH,
        json!({"prompt": "x", "files": paths(&dir, &["1.png", "2.png"]), "variations": 1, "sameLook": true, "sameLookPrompt": "Igual que la última."}),
    )
    .unwrap();
    assert_eq!(provider.sent.lock().unwrap()[1].prompt, "x Igual que la última.");
    let four = vec![refs.to_string_lossy().into_owned(); 4];
    assert!(s.execute(BATCH, json!({"prompt": "x", "files": paths(&dir, &["1.png"]), "sameLook": true, "references": four})).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn two_account_failures_in_a_row_stop_the_batch() {
    let dir = folder("account");
    let names: Vec<String> = (0..8).map(|i| format!("{i}.png")).collect();
    for n in &names {
        png(&dir.join(n), 16, 16);
    }
    let files = paths(&dir, &names.iter().map(String::as_str).collect::<Vec<_>>());
    for e in [GenError::Auth, GenError::Account("credit balance exhausted".into())] {
        let provider = Arc::new(Fake { fail: Some(e), ..fake() });
        let mut s = session_with(provider.clone());
        let r = s.execute(BATCH, json!({"prompt": "x", "files": files, "variations": 1})).unwrap();
        assert!(r["stopped"].is_string(), "{r}");
        assert_eq!(r["written"], json!([]));
        let calls = provider.calls.load(Ordering::SeqCst);
        assert!((2..8).contains(&calls), "stopped early: {calls} calls");
        assert_eq!(r["skipped"].as_u64().unwrap() as usize + r["failed"].as_array().unwrap().len(), 8);
        assert!(r["failed"][0]["path"].is_string(), "failed files are named for a retry");
    }
    // Other failures don't stop it.
    let provider = Arc::new(Fake { fail: Some(GenError::Refused("no".into())), ..fake() });
    let mut s = session_with(provider.clone());
    let r = s.execute(BATCH, json!({"prompt": "x", "files": files, "variations": 1})).unwrap();
    assert_eq!(r["failed"].as_array().unwrap().len(), 8);
    assert_eq!(r["stopped"], Value::Null);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn open_documents_go_beside_their_file_or_to_the_folder_chosen() {
    let dir = folder("docs");
    let provider = Arc::new(fake());
    let mut s = session_with(provider.clone());
    s.execute("file.new", json!({"width": 32, "height": 24, "background": "#ff0000"})).unwrap();
    let unsaved = s.active().unwrap().doc.id.0;
    let e = s.execute(BATCH, json!({"prompt": "x", "documents": [unsaved]})).unwrap_err().to_string();
    assert!(e.contains("never been saved"), "{e}");
    let out = dir.join("out");
    let r = s.execute(BATCH, json!({"prompt": "x", "documents": [unsaved], "variations": 1, "output": out.to_string_lossy()})).unwrap();
    let written = r["written"][0].as_str().unwrap().to_owned();
    assert!(written.starts_with(&*out.to_string_lossy()) && written.ends_with(".jpg"), "{written}");
    assert_eq!((decoded(&written).width(), decoded(&written).height()), (32, 24));
    // A saved one: beside its file.
    let saved = dir.join("photo.png");
    png(&saved, 20, 20);
    crate::file_cmds::open_bytes_as(&mut s, "photo.png", &std::fs::read(&saved).unwrap(), None, Some(saved.to_string_lossy().into_owned())).unwrap();
    let id = s.active().unwrap().doc.id.0;
    assert!(s.active().unwrap().path.is_some());
    let r = s.execute(BATCH, json!({"prompt": "x", "documents": [id], "variations": 1})).unwrap();
    assert_eq!(r["written"][0], dir.join(OUTPUT_FOLDER).join("photo.jpg").to_string_lossy().as_ref());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_image_in_the_output_folder_is_never_overwritten() {
    let dir = folder("overwrite");
    let source = dir.join("same.jpg");
    let px: Vec<u8> = (0..16 * 16).flat_map(|_| [10u8, 20, 30]).collect();
    let img = Image::from_raw(16, 16, ChannelLayout::Rgb, CodecSample::U8, px).unwrap();
    std::fs::write(&source, codecs::encode(&img, Format::Jpeg, &Default::default()).unwrap()).unwrap();
    let before = std::fs::read(&source).unwrap();
    let mut s = session_with(Arc::new(fake()));
    let r = s.execute(BATCH, json!({"prompt": "x", "files": [source.to_string_lossy()], "variations": 1, "output": dir.to_string_lossy()})).unwrap();
    assert_eq!(std::fs::read(&source).unwrap(), before);
    assert_eq!(r["written"][0], dir.join("same (IA).jpg").to_string_lossy().as_ref());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn bad_params_and_unsuited_providers_fail_before_anything_is_sent() {
    let dir = folder("bad");
    png(&dir.join("a.png"), 8, 8);
    let file = dir.join("a.png").to_string_lossy().into_owned();
    let provider = Arc::new(fake());
    let mut s = session_with(provider.clone());
    for p in [
        json!({"files": [file]}),
        json!({"prompt": "  ", "files": [file]}),
        json!({"prompt": "x"}),
        json!({"prompt": "x", "files": []}),
        json!({"prompt": "x", "files": "a.png"}),
        json!({"prompt": "x", "files": [1]}),
        json!({"prompt": "x", "documents": ["a"]}),
        json!({"prompt": "x", "documents": [999_999]}),
        json!({"prompt": "x", "files": [file], "variations": 0}),
        json!({"prompt": "x", "files": [file], "sameLook": "yes"}),
        json!({"prompt": "x", "files": [file], "output": 3}),
        json!({"prompt": "x", "files": vec![file.clone(); MAX_ITEMS + 1]}),
        json!({"prompt": 7, "files": [file]}),
    ] {
        assert!(s.execute(BATCH, p.clone()).is_err(), "{p}");
    }
    assert!(provider.sent.lock().unwrap().is_empty());
    // A missing file fails on its own; the batch goes on.
    let r = s.execute(BATCH, json!({"prompt": "x", "files": [dir.join("nope.png").to_string_lossy(), file], "variations": 1})).unwrap();
    assert_eq!(r["failed"].as_array().unwrap().len(), 1);
    assert_eq!(r["written"].as_array().unwrap().len(), 1);
    // A model that doesn't edit from instructions can't run it, and none at all.
    let mut s = session_with(Arc::new(Fake { edits: false, ..fake() }));
    assert!(!s.is_enabled(BATCH));
    assert!(s.execute(BATCH, json!({"prompt": "x", "files": [file]})).is_err());
    let mut s = Session::new();
    assert!(s.execute(BATCH, json!({"prompt": "x", "files": [file]})).is_err());
    let _ = std::fs::remove_dir_all(dir);
}
