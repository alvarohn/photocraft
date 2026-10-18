//! A stable-diffusion.cpp server (`sd-server`) the user runs, usually on the same computer: free,
//! private, no key, and slow (minutes an image on a laptop GPU).
//!
//! It uses the server's native asynchronous API (`/sdcpp/v1`, stable-diffusion.cpp
//! `examples/server/api.md`): `POST /sdcpp/v1/img_gen` queues a job with the image as
//! `init_image`, the mask as `mask_image` (white = regenerate) and, for models that edit from
//! instructions (Qwen Image 2.1, Flux Kontext), the image and any references as `ref_images`;
//! `GET /sdcpp/v1/jobs/{id}` is polled until it is done, its preview giving the sampling step;
//! `POST …/cancel` withdraws a job still waiting. The server works one job at a time, so the engine
//! sends it one request at a time ([`ImageProvider::max_parallel`]). It decodes images with
//! stb_image, at the size sent: the engine sends a reduced working size ([`Options::work_side`],
//! multiples of 32) and scales the result back up.

use base64::Engine as _;
use serde_json::{Value, json};

use crate::{
    Capabilities, EditMode, EditRequest, GenError, GeneratedImage, HttpRequest, HttpTransport, ImageProvider, MaskFormat, NoProgress, Progress, ProviderInfo,
    Result,
};

pub const ID: &str = "sdcpp";
pub const NAME: &str = "stable-diffusion.cpp (local)";
/// Where requests go, for the privacy notice: the server's address is the user's own setting.
pub const HOST: &str = "your stable-diffusion.cpp server";
/// `sd-server`'s own default address (`--listen-ip 127.0.0.1 --listen-port 1234`).
pub const DEFAULT_URL: &str = "http://127.0.0.1:1234";

/// Model presets: (id, name). The server loads the model when it starts; the preset says what it
/// is, so PhotoCraft knows what to ask of it and how.
pub const PRESETS: &[(&str, &str)] =
    &[("z-image-turbo", "Z-Image Turbo"), ("qwen-image-2.1-turbo", "Qwen Image 2.1 Turbo (edits, references)"), ("other", "Other model")];
pub const DEFAULT_PRESET: &str = "z-image-turbo";

/// Qwen Image 2.1 Turbo's fixed 8-step schedule (the `--sigmas qwen-image-2.1-turbo` preset, which
/// requests can't name), so it applies even when the server was started without it.
const QWEN_TURBO_SIGMAS: [f64; 9] = [1.0, 0.978453, 0.95418, 0.926626, 0.89508, 0.845148, 0.704534, 0.414568, 0.0];

/// Working size: the long side sent, between these, in steps of [`STEP`].
pub const MIN_SIDE: u32 = 256;
pub const MAX_SIDE: u32 = 2048;
pub const DEFAULT_SIDE: u32 = 512;
/// Sizes are multiples of this (Qwen Image 2.1 needs 32; others align to 8 or 64 themselves).
const STEP: u32 = 32;
/// How long to wait between looks at a job, and between looks at Cancel, in milliseconds.
#[cfg(all(not(test), not(target_arch = "wasm32")))]
const POLL_MS: u64 = 1_000;
#[cfg(all(test, not(target_arch = "wasm32")))]
const POLL_MS: u64 = 1;
#[cfg(not(target_arch = "wasm32"))]
const CANCEL_MS: u64 = 100;
/// Polls that may fail in a row (the server busy loading, a dropped connection) before the job is
/// given up.
const POLL_RETRIES: u32 = 5;

/// Settings that shape each request, from Generative AI Settings.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    /// The server's address: `http://host:port`, optionally with a path prefix.
    pub url: String,
    /// One of [`PRESETS`].
    pub preset: String,
    /// Sampling steps (1 to 100); 0 leaves the server's `--steps`.
    pub steps: u32,
    /// Guidance (CFG) scale; `None` leaves the server's `--cfg-scale`.
    pub cfg: Option<f32>,
    /// Sampling method (`euler`, …); empty leaves the server's.
    pub sampler: String,
    /// How much of the selected area Fill regenerates, 0.05 to 1 (1: from noise).
    pub strength: f32,
    /// Long side of the image sent, in pixels ([`MIN_SIDE`] to [`MAX_SIDE`], a multiple of 32).
    pub work_side: u32,
    /// For `other`: the model edits from instructions and takes reference images.
    pub instructions: bool,
    /// How far past the selection the model may draw, as a share of the selection's longer side
    /// (0 to 1; 0: only inside it). See [`ImageProvider::mask_room`].
    pub room: f32,
}

impl Options {
    /// A preset's own settings.
    pub fn preset(preset: &str) -> Options {
        let base = Options {
            url: DEFAULT_URL.into(),
            preset: DEFAULT_PRESET.into(),
            steps: 8,
            cfg: Some(1.0),
            sampler: String::new(),
            strength: 1.0,
            work_side: DEFAULT_SIDE,
            instructions: false,
            room: 0.0,
        };
        match preset {
            "qwen-image-2.1-turbo" => Options { preset: preset.into(), sampler: "euler".into(), ..base },
            "other" => Options { preset: preset.into(), steps: 0, cfg: None, ..base },
            _ => base,
        }
    }

    /// Read from the settings' JSON (`{"url", "preset", "steps", "cfg", "sampler", "strength",
    /// "workSide", "instructions", "room"}`); missing or out-of-range values take the preset's.
    pub fn from_json(v: &Value) -> Options {
        let preset = v.get("preset").and_then(Value::as_str).filter(|p| PRESETS.iter().any(|(id, _)| id == p)).unwrap_or(DEFAULT_PRESET);
        let d = Options::preset(preset);
        let url = v.get("url").and_then(Value::as_str).map(str::trim).filter(|u| !u.is_empty()).unwrap_or(DEFAULT_URL).to_owned();
        let side = v.get("workSide").and_then(Value::as_u64).map_or(d.work_side, |s| {
            let s = s.clamp(u64::from(MIN_SIDE), u64::from(MAX_SIDE)) as u32;
            s / STEP * STEP
        });
        let cfg = match v.get("cfg") {
            Some(Value::Null) => None,
            Some(c) => c.as_f64().filter(|c| c.is_finite() && (0.0..=30.0).contains(c)).map(|c| c as f32).or(d.cfg),
            None => d.cfg,
        };
        let sampler = v.get("sampler").and_then(Value::as_str).map(str::trim).unwrap_or(&d.sampler);
        let sampler = if sampler.len() <= 32 && sampler.bytes().all(|b| b.is_ascii_alphanumeric() || b"+_-".contains(&b)) { sampler } else { "" };
        Options {
            url,
            preset: preset.into(),
            steps: v.get("steps").and_then(Value::as_u64).map_or(d.steps, |s| s.min(100) as u32),
            cfg,
            sampler: sampler.to_owned(),
            strength: v.get("strength").and_then(Value::as_f64).filter(|s| s.is_finite()).map_or(d.strength, |s| s.clamp(0.05, 1.0) as f32),
            work_side: side,
            instructions: v.get("instructions").and_then(Value::as_bool).unwrap_or(d.instructions),
            room: v.get("room").and_then(Value::as_f64).filter(|r| r.is_finite()).map_or(d.room, |r| r.clamp(0.0, 1.0) as f32),
        }
    }

    /// Does the model edit from instructions and take reference images?
    pub fn edits(&self) -> bool {
        self.preset == "qwen-image-2.1-turbo" || (self.preset == "other" && self.instructions)
    }
}

/// Checks a server address: `http` or `https`, a host, an optional port and path, nothing else
/// (no user name or password, query or fragment). Returns it without a trailing `/`.
pub fn check_url(url: &str) -> Result<String> {
    let url = url.trim().trim_end_matches('/');
    // The address isn't repeated: a pasted one may hold a user name and password.
    let bad = || GenError::Invalid(format!("the server address must be like {DEFAULT_URL}: http or https, a host, a port and a path, nothing else"));
    let rest = url.strip_prefix("http://").or_else(|| url.strip_prefix("https://")).ok_or_else(bad)?;
    let host = rest.split('/').next().unwrap_or("");
    let path_ok = rest[host.len()..].bytes().all(|b| b.is_ascii_alphanumeric() || b"/-._~".contains(&b));
    let host_ok = !host.is_empty() && host.bytes().all(|b| b.is_ascii_alphanumeric() || b"-.:[]".contains(&b));
    if url.len() > 200 || !host_ok || !path_ok {
        return Err(bad());
    }
    Ok(url.to_owned())
}

pub struct SdCppProvider<T: HttpTransport> {
    base: String,
    options: Options,
    transport: T,
}

impl<T: HttpTransport> SdCppProvider<T> {
    pub fn new(options: Options, transport: T) -> Result<Self> {
        let base = check_url(&options.url)?;
        Ok(Self { base, options, transport })
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    fn sample_params(&self) -> Value {
        let mut p = serde_json::Map::new();
        if self.options.steps > 0 {
            p.insert("sample_steps".into(), json!(self.options.steps));
        }
        if let Some(cfg) = self.options.cfg {
            p.insert("guidance".into(), json!({"txt_cfg": cfg}));
        }
        if !self.options.sampler.is_empty() {
            p.insert("sample_method".into(), json!(self.options.sampler));
        }
        if self.options.preset == "qwen-image-2.1-turbo" {
            p.insert("custom_sigmas".into(), json!(QWEN_TURBO_SIGMAS));
        }
        Value::Object(p)
    }

    /// The request body for an edit (`/sdcpp/v1/img_gen`).
    fn body(&self, r: &EditRequest, (w, h): (u32, u32)) -> Value {
        let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
        let edits = self.options.edits();
        let prompt = if edits { instruction(&r.prompt, r.mode, r.references.len()) } else { r.prompt.trim().to_owned() };
        let mut body = json!({
            "prompt": prompt,
            "negative_prompt": "",
            "width": w,
            "height": h,
            // A new seed each time: the server's default (42) would make every variation the same.
            "seed": -1,
            "batch_count": 1,
            // Edit regenerates the area from the instruction, the image itself as a reference.
            "strength": if r.mode == EditMode::Edit { 1.0 } else { self.options.strength },
            "init_image": b64(&r.image),
            "mask_image": b64(&r.mask),
            "sample_params": self.sample_params(),
            "output_format": "png",
            "embed_image_metadata": false,
            // The cheapest preview: it carries the sampling step, the only progress the API gives.
            "preview": "proj",
            "preview_interval": 1,
        });
        if edits && let Some(o) = body.as_object_mut() {
            let refs: Vec<String> = std::iter::once(b64(&r.image)).chain(r.references.iter().map(|i| b64(&i.bytes))).collect();
            o.insert("ref_images".into(), json!(refs));
        }
        body
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn unreachable(&self, e: GenError) -> GenError {
        match e {
            GenError::Network(why) => GenError::Network(format!("{why} (is sd-server running at {}?)", self.base)),
            e => e,
        }
    }

    fn post(&self, path: &str, body: &Value) -> Result<crate::HttpResponse> {
        let body = serde_json::to_vec(body).map_err(|_| GenError::Invalid("could not encode the request".into()))?;
        self.transport
            .post(HttpRequest { url: self.url(path), headers: vec![("content-type", "application/json".into())], body })
            .map_err(|e| self.unreachable(e))
    }

    /// Withdraw job `id` (it works only while the job waits; the server can't stop one generating).
    fn cancel(&self, id: &str) {
        let _ = self.post(&format!("/sdcpp/v1/jobs/{id}/cancel"), &json!({}));
    }

    /// Wait for job `id`, reporting its place in line and sampling step, until it is done.
    fn wait(&self, id: &str, progress: &dyn Progress) -> Result<GeneratedImage> {
        let mut failures = 0;
        loop {
            if !pause(progress) {
                self.cancel(id);
                return Err(GenError::Cancelled);
            }
            let r = self.transport.get(HttpRequest { url: self.url(&format!("/sdcpp/v1/jobs/{id}")), headers: Vec::new(), body: Vec::new() });
            let r = match r {
                Ok(r) => r,
                Err(e @ GenError::Unsupported(_)) => return Err(e),
                Err(e) => {
                    failures += 1;
                    if failures > POLL_RETRIES {
                        return Err(self.unreachable(e));
                    }
                    continue;
                }
            };
            failures = 0;
            if r.body.len() > crate::MAX_RESPONSE {
                return Err(GenError::BadResponse("the response is too large"));
            }
            match r.status {
                200 => {}
                404 | 410 => return Err(GenError::BadResponse("the server no longer has the job (was it restarted?)")),
                status => return Err(http_error(status, &r.body)),
            }
            let job: Value = serde_json::from_slice(&r.body).map_err(|_| GenError::BadResponse("not JSON"))?;
            match job.get("status").and_then(Value::as_str).unwrap_or("") {
                "queued" => {
                    let ahead = job.get("queue_position").and_then(Value::as_u64).unwrap_or(0);
                    let msg = if ahead > 0 { format!("Waiting for the local server ({ahead} ahead)") } else { "Waiting for the local server".into() };
                    progress.report(0.0, &msg);
                }
                "generating" => {
                    let preview = job.get("preview");
                    let step = preview.and_then(|p| p.get("step")).and_then(Value::as_u64);
                    let total = preview.and_then(|p| p.get("total_steps")).and_then(Value::as_u64).filter(|t| *t > 0);
                    match (step, total) {
                        (Some(s), Some(t)) => progress.report((s as f32 / t as f32).min(1.0) * 0.95, &format!("Step {s} of {t}")),
                        _ => progress.report(0.02, "Generating"),
                    }
                }
                "completed" => {
                    let images = job.get("result").and_then(|r| r.get("images")).and_then(Value::as_array);
                    let data = images.and_then(|i| i.first()).and_then(|i| i.get("b64_json")).and_then(Value::as_str);
                    let Some(data) = data else { return Err(GenError::Refused("it returned no image".into())) };
                    if data.len() > crate::MAX_RESPONSE {
                        return Err(GenError::BadResponse("the image is too large"));
                    }
                    let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|_| GenError::BadResponse("the image data is not base64"))?;
                    return Ok(GeneratedImage { bytes, mime: "image/png".into() });
                }
                "failed" => {
                    let message = job.get("error").and_then(|e| e.get("message")).and_then(Value::as_str).unwrap_or("the generation failed");
                    return Err(GenError::Http { status: 500, message: clip(message, 300) });
                }
                "cancelled" => return Err(GenError::Cancelled),
                _ => return Err(GenError::BadResponse("the job's status is unknown")),
            }
        }
    }
}

/// Sleep one poll interval, looking at Cancel as it goes. False if cancelled.
#[cfg(not(target_arch = "wasm32"))]
fn pause(progress: &dyn Progress) -> bool {
    let end = std::time::Instant::now() + std::time::Duration::from_millis(POLL_MS);
    while std::time::Instant::now() < end {
        if progress.cancelled() {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(CANCEL_MS.min(POLL_MS)));
    }
    !progress.cancelled()
}

/// The web build has no transport, so this never runs there; it can't sleep either.
#[cfg(target_arch = "wasm32")]
fn pause(progress: &dyn Progress) -> bool {
    !progress.cancelled()
}

fn http_error(status: u16, body: &[u8]) -> GenError {
    let v = serde_json::from_slice::<Value>(body).ok();
    let field = |k: &str| v.as_ref().and_then(|v| v.get(k)).and_then(Value::as_str).unwrap_or("").to_owned();
    let (error, message) = (field("error"), field("message"));
    let message = match (error.is_empty(), message.is_empty()) {
        (false, false) => format!("{error}: {message}"),
        (false, true) => error,
        (true, _) => message,
    };
    match status {
        // "job queue is full": the engine waits and tries again.
        429 => GenError::Quota,
        _ => GenError::Http { status, message: clip(&message, 300) },
    }
}

/// The prompt for a model that edits from instructions. The image to edit is the first reference
/// (in Fill with the selected area blurred out); any of the user's follow.
pub fn instruction(prompt: &str, mode: EditMode, references: usize) -> String {
    let p = prompt.trim().trim_end_matches('.');
    let refs = match references {
        0 => String::new(),
        1 => " Use the second image as a reference, as the request says, and match it closely.".to_owned(),
        n => format!(" Use the other {n} images as references, as the request says, and match them closely."),
    };
    match mode {
        EditMode::Edit => format!("{p}. Change only that; keep everything else in the first image exactly as it is.{refs}"),
        EditMode::Fill if p.is_empty() && references > 0 => {
            format!("Replace the blurred area of the first image with the subject of the reference images, sized, lit and placed to belong to the scene.{refs}")
        }
        EditMode::Fill if p.is_empty() => "Remove the blurred area of the first image: fill it with what would be there if nothing stood in it, continuing \
                                            the background, surfaces, lines and light around it. Add nothing new."
            .to_owned(),
        EditMode::Fill => {
            format!("In the blurred area of the first image, put: {p}. Make it belong to the scene: the same light, perspective, focus and grain.{refs}")
        }
    }
}

/// The size an image of `w`×`h` is sent at: its long side at most `side`, both sides multiples of
/// 32 and at least 64.
pub fn work_size(w: u32, h: u32, side: u32) -> (u32, u32) {
    let side = side.clamp(MIN_SIDE, MAX_SIDE);
    let k = (f64::from(side) / f64::from(w.max(h).max(1))).min(1.0);
    let snap = |v: u32| (((f64::from(v) * k / f64::from(STEP)).round() as u32) * STEP).clamp(2 * STEP, side);
    (snap(w.max(1)), snap(h.max(1)))
}

impl<T: HttpTransport> ImageProvider for SdCppProvider<T> {
    fn info(&self) -> ProviderInfo {
        let model = PRESETS.iter().find(|(id, _)| *id == self.options.preset).map_or("model", |(_, name)| name);
        ProviderInfo { id: ID, name: NAME, model: model.to_owned(), host: HOST }
    }

    fn capabilities(&self) -> Capabilities {
        let edits = self.options.edits();
        Capabilities { generate: false, edit: true, variations: false, edit_in_place: edits, max_references: if edits { crate::MAX_REFERENCES } else { 0 } }
    }

    fn edit_aspect(&self, w: u32, h: u32) -> (u32, u32) {
        work_size(w, h, self.options.work_side)
    }

    fn default_variations(&self) -> usize {
        1
    }

    fn request_size(&self, w: u32, h: u32) -> (u32, u32) {
        work_size(w, h, self.options.work_side)
    }

    fn mask_format(&self) -> MaskFormat {
        MaskFormat::Luma
    }

    fn max_parallel(&self) -> Option<usize> {
        Some(1)
    }

    fn mask_room(&self) -> f32 {
        self.options.room
    }

    fn edit(&self, r: &EditRequest) -> Result<GeneratedImage> {
        self.edit_with_progress(r, &NoProgress)
    }

    fn edit_with_progress(&self, r: &EditRequest, progress: &dyn Progress) -> Result<GeneratedImage> {
        let edits = self.options.edits();
        crate::check_prompt(&r.prompt, r.mode == EditMode::Edit)?;
        if r.mode == EditMode::Edit && !edits {
            return Err(GenError::Unsupported("this model (it doesn't edit from instructions; use Fill)"));
        }
        crate::check_references(&r.references, if edits { crate::MAX_REFERENCES } else { 0 })?;
        let size = crate::png_size(&r.image).ok_or_else(|| GenError::Invalid("the image must be a PNG".into()))?;
        if crate::png_size(&r.mask) != Some(size) {
            return Err(GenError::Invalid("the mask must be a PNG the size of the image".into()));
        }
        progress.report(0.0, "Sending to the local server");
        let r = self.post("/sdcpp/v1/img_gen", &self.body(r, size))?;
        if !(200..300).contains(&r.status) {
            return Err(http_error(r.status, &r.body));
        }
        let job: Value = serde_json::from_slice(&r.body).map_err(|_| GenError::BadResponse("not JSON"))?;
        let id = job.get("id").and_then(Value::as_str).ok_or(GenError::BadResponse("the server sent no job id"))?;
        // Ids are the server's own (`job_<hex>_<hex>`); anything else is not put in a URL.
        if id.is_empty() || id.len() > 100 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
            return Err(GenError::BadResponse("the server sent an odd job id"));
        }
        self.wait(id, progress)
    }
}

fn clip(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HttpResponse, ReferenceImage};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// Plays a server: records each request (method, URL, JSON body) and answers with the next
    /// canned response.
    struct Fake {
        sent: Mutex<Vec<(&'static str, String, Value)>>,
        replies: Mutex<Vec<Result<HttpResponse>>>,
    }

    impl Fake {
        fn new(replies: Vec<Result<HttpResponse>>) -> Self {
            Self { sent: Mutex::new(Vec::new()), replies: Mutex::new(replies) }
        }
        fn reply(&self, method: &'static str, r: HttpRequest) -> Result<HttpResponse> {
            let body = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
            assert!(r.headers.iter().all(|(n, _)| *n == "content-type"), "nothing but the content type: no key goes to a local server");
            self.sent.lock().unwrap().push((method, r.url, body));
            let mut replies = self.replies.lock().unwrap();
            if replies.is_empty() { Err(GenError::Network("no more replies".into())) } else { replies.remove(0) }
        }
    }

    impl HttpTransport for Fake {
        fn post(&self, r: HttpRequest) -> Result<HttpResponse> {
            self.reply("POST", r)
        }
        fn get(&self, r: HttpRequest) -> Result<HttpResponse> {
            self.reply("GET", r)
        }
    }

    fn json_reply(status: u16, body: Value) -> Result<HttpResponse> {
        Ok(HttpResponse { status, body: serde_json::to_vec(&body).unwrap() })
    }

    fn queued() -> Result<HttpResponse> {
        json_reply(202, json!({"id": "job_1_00000001", "kind": "img_gen", "status": "queued", "poll_url": "/sdcpp/v1/jobs/job_1_00000001"}))
    }

    fn done(bytes: &[u8]) -> Result<HttpResponse> {
        json_reply(
            200,
            json!({"id": "job_1_00000001", "status": "completed", "result": {"output_format": "png", "images": [{"index": 0, "b64_json": b64(bytes)}]}}),
        )
    }

    fn png_of(w: u32, h: u32) -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(b"rest");
        v
    }

    fn b64(b: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(b)
    }

    fn provider(preset: &str, replies: Vec<Result<HttpResponse>>) -> SdCppProvider<Fake> {
        SdCppProvider::new(Options::preset(preset), Fake::new(replies)).unwrap()
    }

    fn edit(prompt: &str, w: u32, h: u32) -> EditRequest {
        EditRequest {
            image: png_of(w, h),
            mask: png_of(w, h),
            prompt: prompt.into(),
            size: crate::ImageSize::Small,
            mode: EditMode::Fill,
            references: Vec::new(),
        }
    }

    /// Records progress; cancels once `cancel_after` reports have come.
    #[derive(Default)]
    struct Seen {
        reports: Mutex<Vec<(f32, String)>>,
        cancel: AtomicBool,
        cancel_after: Option<usize>,
        count: AtomicUsize,
    }

    impl Progress for Seen {
        fn report(&self, fraction: f32, message: &str) {
            self.reports.lock().unwrap().push((fraction, message.to_owned()));
            if self.cancel_after.is_some_and(|n| self.count.fetch_add(1, Ordering::SeqCst) + 1 >= n) {
                self.cancel.store(true, Ordering::SeqCst);
            }
        }
        fn cancelled(&self) -> bool {
            self.cancel.load(Ordering::SeqCst)
        }
    }

    #[test]
    fn a_fill_queues_a_job_with_image_and_mask_and_polls_it_with_progress() {
        let generating = json_reply(200, json!({"status": "generating", "preview": {"pass": 0, "step": 4, "total_steps": 8, "b64_json": "AAAA"}}));
        let p = provider("z-image-turbo", vec![queued(), json_reply(200, json!({"status": "queued", "queue_position": 1})), generating, done(b"png!")]);
        let seen = Seen::default();
        let out = p.edit_with_progress(&edit("  a red boat ", 512, 384), &seen).unwrap();
        assert_eq!(out, GeneratedImage { bytes: b"png!".to_vec(), mime: "image/png".into() });
        let sent = p.transport().sent.lock().unwrap();
        let (method, url, body) = &sent[0];
        assert_eq!((*method, url.as_str()), ("POST", "http://127.0.0.1:1234/sdcpp/v1/img_gen"));
        assert_eq!(body["prompt"], "a red boat", "a text-to-image model gets a description");
        assert_eq!((body["width"].as_u64(), body["height"].as_u64()), (Some(512), Some(384)));
        assert_eq!(body["seed"], -1, "a new seed each variation");
        assert_eq!(body["strength"], 1.0);
        assert_eq!(body["init_image"], b64(&png_of(512, 384)));
        assert_eq!(body["mask_image"], b64(&png_of(512, 384)));
        assert!(body.get("ref_images").is_none(), "Z-Image takes no references");
        assert_eq!(body["sample_params"], json!({"sample_steps": 8, "guidance": {"txt_cfg": 1.0}}));
        assert_eq!(body["preview"], "proj");
        assert!(sent[1..].iter().all(|(m, u, _)| *m == "GET" && u == "http://127.0.0.1:1234/sdcpp/v1/jobs/job_1_00000001"));
        let reports = seen.reports.lock().unwrap();
        assert!(reports.iter().any(|(_, m)| m == "Waiting for the local server (1 ahead)"), "{reports:?}");
        assert!(reports.iter().any(|(f, m)| m == "Step 4 of 8" && (*f - 0.475).abs() < 1e-3), "{reports:?}");
    }

    #[test]
    fn qwen_edits_send_the_image_and_references_as_ref_images_with_its_schedule() {
        let p = provider("qwen-image-2.1-turbo", vec![queued(), done(b"x")]);
        let jpeg = |b: &[u8]| ReferenceImage { bytes: b.to_vec(), mime: "image/jpeg".into() };
        let r = EditRequest { mode: EditMode::Edit, references: vec![jpeg(b"logo")], ..edit("make the label black", 256, 256) };
        p.edit(&r).unwrap();
        let sent = p.transport().sent.lock().unwrap();
        let body = &sent[0].2;
        assert_eq!(body["ref_images"], json!([b64(&png_of(256, 256)), b64(b"logo")]));
        assert_eq!(body["strength"], 1.0);
        let prompt = body["prompt"].as_str().unwrap();
        assert!(prompt.starts_with("make the label black. Change only that") && prompt.contains("second image as a reference"), "{prompt}");
        assert_eq!(body["sample_params"]["sample_method"], "euler");
        assert_eq!(body["sample_params"]["custom_sigmas"].as_array().unwrap().len(), 9);
        assert_eq!(
            p.capabilities(),
            Capabilities { generate: false, edit: true, variations: false, edit_in_place: true, max_references: crate::MAX_REFERENCES }
        );
        drop(sent);
        // An empty Fill prompt asks for a removal in words.
        let p = provider("qwen-image-2.1-turbo", vec![queued(), done(b"x")]);
        p.edit(&edit("", 64, 64)).unwrap();
        assert!(p.transport().sent.lock().unwrap()[0].2["prompt"].as_str().unwrap().starts_with("Remove the blurred area"));
    }

    #[test]
    fn a_text_to_image_model_refuses_edit_mode_and_references_before_sending() {
        let p = provider("z-image-turbo", Vec::new());
        assert!(p.edit(&EditRequest { mode: EditMode::Edit, ..edit("blue", 64, 64) }).is_err());
        let jpeg = ReferenceImage { bytes: b"r".to_vec(), mime: "image/jpeg".into() };
        assert!(matches!(p.edit(&EditRequest { references: vec![jpeg], ..edit("x", 64, 64) }), Err(GenError::Invalid(_))));
        assert!(matches!(p.edit(&EditRequest { mask: png_of(32, 64), ..edit("x", 64, 64) }), Err(GenError::Invalid(_))));
        assert!(p.transport().sent.lock().unwrap().is_empty());
        assert_eq!(p.capabilities().max_references, 0);
        assert!(!p.capabilities().edit_in_place);
    }

    #[test]
    fn cancelling_withdraws_the_job() {
        let still = || json_reply(200, json!({"status": "queued", "queue_position": 2}));
        let p = provider("z-image-turbo", vec![queued(), still(), json_reply(200, json!({}))]);
        let seen = Seen { cancel_after: Some(2), ..Seen::default() };
        assert_eq!(p.edit_with_progress(&edit("x", 64, 64), &seen), Err(GenError::Cancelled));
        let sent = p.transport().sent.lock().unwrap();
        let (method, url, _) = sent.last().unwrap();
        assert_eq!((*method, url.as_str()), ("POST", "http://127.0.0.1:1234/sdcpp/v1/jobs/job_1_00000001/cancel"));
    }

    #[test]
    fn server_errors_are_reported() {
        let failed = json_reply(200, json!({"status": "failed", "error": {"code": "generation_failed", "message": "out of VRAM"}}));
        let cases = [
            (
                vec![Err(GenError::Network("connection refused".into()))],
                GenError::Network("connection refused (is sd-server running at http://127.0.0.1:1234?)".into()),
            ),
            (
                vec![json_reply(400, json!({"error": "invalid generation parameters"}))],
                GenError::Http { status: 400, message: "invalid generation parameters".into() },
            ),
            (vec![json_reply(429, json!({"error": "job queue is full"}))], GenError::Quota),
            (vec![queued(), failed], GenError::Http { status: 500, message: "out of VRAM".into() }),
            (vec![queued(), json_reply(410, json!({"error": "expired"}))], GenError::BadResponse("the server no longer has the job (was it restarted?)")),
            (vec![queued(), json_reply(200, json!({"status": "cancelled"}))], GenError::Cancelled),
            (vec![queued(), json_reply(200, json!({"status": "completed", "result": {"images": []}}))], GenError::Refused("it returned no image".into())),
            (vec![json_reply(202, json!({"id": "../../etc"}))], GenError::BadResponse("the server sent an odd job id")),
            (vec![Ok(HttpResponse { status: 202, body: b"<html>".to_vec() })], GenError::BadResponse("not JSON")),
        ];
        for (replies, want) in cases {
            let p = provider("z-image-turbo", replies);
            assert_eq!(p.edit(&edit("x", 64, 64)), Err(want));
        }
        // A few failed polls in a row are tolerated (a dropped connection); more give up.
        let flaky = vec![queued(), Err(GenError::Network("reset".into())), Err(GenError::Network("reset".into())), done(b"ok")];
        assert!(provider("z-image-turbo", flaky).edit(&edit("x", 64, 64)).is_ok());
        let gone = std::iter::once(queued()).chain((0..=POLL_RETRIES).map(|_| Err(GenError::Network("refused".into())))).collect();
        assert!(matches!(provider("z-image-turbo", gone).edit(&edit("x", 64, 64)), Err(GenError::Network(_))));
    }

    #[test]
    fn work_sizes_are_multiples_of_32_within_the_working_size() {
        assert_eq!(work_size(1600, 900, 512), (512, 288));
        assert_eq!(work_size(900, 1600, 512), (288, 512));
        assert_eq!(work_size(300, 200, 512), (288, 192), "never enlarged");
        assert_eq!(work_size(5000, 10, 512), (512, 64));
        assert_eq!(work_size(0, 0, 0), (64, 64));
        for (w, h) in [(1, 1), (777, 333), (4096, 4096), (31, 2000)] {
            let (a, b) = work_size(w, h, 1024);
            assert!(a % 32 == 0 && b % 32 == 0 && a.max(b) <= 1024 && a.min(b) >= 64, "{w}x{h} -> {a}x{b}");
        }
        let p = provider("z-image-turbo", Vec::new());
        assert_eq!((p.default_variations(), p.max_parallel(), p.mask_format()), (1, Some(1), MaskFormat::Luma));
        assert_eq!(p.request_size(2048, 1536), (512, 384));
    }

    #[test]
    fn options_and_addresses_are_checked() {
        assert_eq!(Options::from_json(&Value::Null), Options::preset(DEFAULT_PRESET));
        let o = Options::from_json(
            &json!({"preset": "qwen-image-2.1-turbo", "url": " http://192.168.1.20:7860/ ", "steps": 500, "cfg": 99, "sampler": "euler a;rm", "strength": 7, "workSide": 1000}),
        );
        assert_eq!((o.steps, o.cfg, o.sampler.as_str(), o.strength, o.work_side), (100, Some(1.0), "", 1.0, 992));
        assert_eq!(SdCppProvider::new(o, Fake::new(Vec::new())).unwrap().base, "http://192.168.1.20:7860");
        assert_eq!(Options::from_json(&json!({"preset": "nope"})).preset, DEFAULT_PRESET);
        assert_eq!(Options::from_json(&json!({"preset": "other", "cfg": null})).cfg, None);
        assert!(Options::from_json(&json!({"preset": "other", "instructions": true})).edits());
        let room = |v: Value| Options::from_json(&json!({"room": v})).room;
        assert_eq!((room(json!(0.25)), room(json!(7)), room(json!(-1)), room(json!("x")), room(Value::Null)), (0.25, 1.0, 0.0, 0.0, 0.0));
        assert_eq!(SdCppProvider::new(Options { room: 0.5, ..Options::preset("z-image-turbo") }, Fake::new(Vec::new())).unwrap().mask_room(), 0.5);
        assert_eq!(provider("z-image-turbo", Vec::new()).mask_room(), 0.0, "none by default");
        for bad in ["ftp://x", "http://", "http://user:pw@host", "http://h/?q=1", "http://h/#f", "javascript:alert(1)", "http://h h"] {
            assert!(check_url(bad).is_err(), "{bad}");
        }
        assert!(!check_url("http://me:hunter2@host:1234").unwrap_err().to_string().contains("hunter2"), "never repeated");
        assert_eq!(check_url("https://gpu.local:1234/sd/").unwrap(), "https://gpu.local:1234/sd");
        assert!(SdCppProvider::new(Options { url: "nonsense".into(), ..Options::preset(DEFAULT_PRESET) }, Fake::new(Vec::new())).is_err());
    }
}
