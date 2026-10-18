//! OpenAI's GPT Image models through the Images API (`/v1/images/edits`), with the user's own API
//! key. Model ids and parameters follow the API reference as of October 2026 (`openai/openai-openapi`):
//! GPT Image 2.5 comes as two models, Sunburst (editing fidelity) and Flare (speed).
//!
//! Unlike Gemini, the API takes a real mask: a PNG the size of the image, transparent where the
//! image is to change. The engine builds it in that form ([`MaskFormat::Alpha`]). Reference images
//! follow the edited image in the same request (`image[]`, at most 16 images in all). The engine
//! still keeps only the masked pixels of the result.

use base64::Engine as _;
use serde_json::Value;

use crate::{
    ApiKey, Capabilities, EditMode, EditRequest, GenError, GeneratedImage, HttpRequest, HttpTransport, ImageProvider, MaskFormat, Multipart, ProviderInfo,
    Result,
};

pub const ID: &str = "openai";
pub const NAME: &str = "OpenAI";
pub const HOST: &str = "api.openai.com";
const ENDPOINT: &str = "https://api.openai.com/v1/images/edits";

/// Image models offered in settings, best default first: (id, description).
pub const MODELS: &[(&str, &str)] = &[
    ("gpt-image-2.5-sunburst", "GPT Image 2.5 Sunburst (most faithful edits)"),
    ("gpt-image-2.5-flare", "GPT Image 2.5 Flare (faster)"),
    ("gpt-image-2", "GPT Image 2"),
    ("gpt-image-1.5", "GPT Image 1.5"),
    ("gpt-image-1", "GPT Image 1"),
    ("gpt-image-1-mini", "GPT Image 1 Mini (cheapest)"),
];
pub const DEFAULT_MODEL: &str = "gpt-image-2.5-sunburst";

/// Reference images one edit takes: the API takes 16 images, and the first is the one edited.
pub const MAX_REFERENCES: usize = 15;

/// Output shapes of the models with fixed sizes (GPT Image 1, 1 Mini, 1.5).
const STANDARD: &[(u32, u32)] = &[(1, 1), (3, 2), (2, 3)];
/// Flexible sizes (GPT Image 2 and 2.5): both sides multiples of 16, aspect between 1:3 and 3:1,
/// at most 3840 px a side, and between these many pixels.
const STEP: u32 = 16;
const MAX_EDGE: u32 = 3840;
const MIN_PIXELS: u64 = 655_360;
const MAX_PIXELS: u64 = 8_294_400;

/// What a model family accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    /// GPT Image 1 and 1.5: fixed sizes, input fidelity high or low.
    One,
    /// GPT Image 1 Mini: fixed sizes, input fidelity low only.
    Mini,
    /// GPT Image 2: any size within limits, no input fidelity (always high).
    Two,
    /// GPT Image 2.5 (Sunburst, Flare): as 2, plus the xhigh and max qualities.
    TwoFive,
}

fn family(model: &str) -> Family {
    if model.starts_with("gpt-image-2.5") {
        Family::TwoFive
    } else if model.starts_with("gpt-image-2") {
        Family::Two
    } else if model.starts_with("gpt-image-1-mini") {
        Family::Mini
    } else {
        Family::One
    }
}

/// Is `model` one with flexible output sizes (the working resolution applies)?
pub fn flexible_sizes(model: &str) -> bool {
    matches!(family(model), Family::Two | Family::TwoFive)
}

/// The qualities `model` takes, best last.
pub fn qualities(model: &str) -> &'static [&'static str] {
    match family(model) {
        Family::TwoFive => &["auto", "low", "medium", "high", "xhigh", "max"],
        _ => &["auto", "low", "medium", "high"],
    }
}

/// Does `model` take an input fidelity (how closely it keeps faces and details of the input)?
pub fn takes_input_fidelity(model: &str) -> bool {
    family(model) == Family::One
}

/// Output file formats the API returns.
pub const FORMATS: &[&str] = &["png", "jpeg", "webp"];

/// Working resolutions offered for flexible models: (long side, label).
pub const RESOLUTIONS: &[(u32, &str)] = &[(1024, "1K"), (2048, "2K"), (3840, "4K (experimental)")];

/// Settings that shape each request, from Generative AI Settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// `auto`, `low`, `medium`, `high`, and for GPT Image 2.5 `xhigh` and `max`.
    pub quality: String,
    /// Long side of the output for flexible models, in pixels (1024 to 3840).
    pub resolution: u32,
    /// `png`, `jpeg` or `webp`.
    pub format: String,
    /// 0 to 100, for `jpeg` and `webp`.
    pub compression: u8,
    /// `high` or `low` (GPT Image 1 and 1.5); `None` leaves the model's default.
    pub input_fidelity: Option<String>,
}

impl Default for Options {
    fn default() -> Options {
        Options { quality: "high".into(), resolution: 2048, format: "png".into(), compression: 90, input_fidelity: None }
    }
}

impl Options {
    /// Read from the settings' JSON (`{"quality", "resolution", "format", "compression",
    /// "inputFidelity"}`); missing or unknown values take their defaults.
    pub fn from_json(v: &Value) -> Options {
        let d = Options::default();
        let text = |k: &str, allowed: &[&str], default: &str| v.get(k).and_then(Value::as_str).filter(|s| allowed.contains(s)).unwrap_or(default).to_owned();
        Options {
            quality: text("quality", &["auto", "low", "medium", "high", "xhigh", "max"], &d.quality),
            resolution: v.get("resolution").and_then(Value::as_u64).map_or(d.resolution, |r| r.clamp(1024, u64::from(MAX_EDGE)) as u32),
            format: text("format", FORMATS, &d.format),
            compression: v.get("compression").and_then(Value::as_u64).map_or(d.compression, |c| c.min(100) as u8),
            input_fidelity: v.get("inputFidelity").and_then(Value::as_str).filter(|f| matches!(*f, "high" | "low")).map(str::to_owned),
        }
    }
}

pub struct OpenAiProvider<T: HttpTransport> {
    key: ApiKey,
    model: String,
    options: Options,
    transport: T,
}

impl<T: HttpTransport> OpenAiProvider<T> {
    /// `model` must look like an OpenAI model id (letters, digits, `-`, `.`); it is not checked
    /// against [`MODELS`] so a newer model works without a PhotoCraft update. A quality the model
    /// doesn't take (chosen for another model) becomes `high`.
    pub fn new(key: ApiKey, model: &str, mut options: Options, transport: T) -> Result<Self> {
        let model = model.trim();
        let model = if model.is_empty() { DEFAULT_MODEL } else { model };
        if model.len() > 80 || !model.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.') {
            return Err(GenError::Invalid(format!("`{model}` is not an OpenAI model id")));
        }
        if !qualities(model).contains(&options.quality.as_str()) {
            options.quality = "high".into();
        }
        Ok(Self { key, model: model.to_owned(), options, transport })
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// The `size` asked for an edit of a `w`×`h` image: the nearest fixed size, or for flexible
    /// models that shape at the working resolution, within the API's limits.
    fn size(&self, w: u32, h: u32) -> String {
        if !flexible_sizes(&self.model) {
            return match crate::nearest_ratio(w, h, STANDARD) {
                Some((3, 2)) => "1536x1024".into(),
                Some((2, 3)) => "1024x1536".into(),
                _ => "1024x1024".into(),
            };
        }
        let (w, h) = flexible_size(w, h, self.options.resolution);
        format!("{w}x{h}")
    }

    fn http_error(&self, status: u16, body: &[u8]) -> GenError {
        let error = serde_json::from_slice::<Value>(body).ok().and_then(|v| v.get("error").cloned());
        let field = |k: &str| error.as_ref().and_then(|e| e.get(k)).and_then(Value::as_str).unwrap_or("").to_owned();
        let (message, code, kind) = (field("message"), field("code"), field("type"));
        let message = clip(&self.key.scrub(if message.is_empty() { &code } else { &message }), 300);
        // Retrying a used-up balance or a spending limit doesn't restore access.
        const BILLING: &[&str] = &[
            "insufficient_quota",
            "credit_balance_exhausted",
            "organization_spend_limit_exceeded",
            "project_spend_limit_exceeded",
            "organization_usage_limit_exceeded",
            "billing_hard_limit_reached",
            "billing_not_active",
        ];
        let billing = BILLING.contains(&code.as_str()) || kind == "insufficient_quota";
        match status {
            401 => GenError::Auth,
            _ if code == "invalid_api_key" => GenError::Auth,
            _ if billing => GenError::Account(message),
            // No access to the model (or the organization isn't verified for it).
            403 => GenError::Account(message),
            429 => GenError::Quota,
            400 if code.contains("moderation") || code.contains("content_policy") || code.contains("safety") => GenError::Refused(message),
            _ => GenError::Http { status, message },
        }
    }
}

/// A flexible model's output size for a `w`×`h` request: that shape (kept within 1:3 to 3:1), its
/// long side at `long` pixels, both sides multiples of 16, between the API's least and most pixels
/// and at most 3840 px a side.
pub fn flexible_size(w: u32, h: u32, long: u32) -> (u32, u32) {
    let aspect = (f64::from(w.max(1)) / f64::from(h.max(1))).clamp(1.0 / 3.0, 3.0);
    let long = f64::from(long.clamp(STEP, MAX_EDGE));
    let (mut fw, mut fh) = if aspect >= 1.0 { (long, long / aspect) } else { (long * aspect, long) };
    let pixels = fw * fh;
    let k = if pixels > MAX_PIXELS as f64 {
        (MAX_PIXELS as f64 / pixels).sqrt()
    } else if pixels < MIN_PIXELS as f64 {
        (MIN_PIXELS as f64 / pixels).sqrt()
    } else {
        1.0
    };
    fw *= k;
    fh *= k;
    let step = f64::from(STEP);
    let snap = |v: f64| ((v / step).round() * step).clamp(step, f64::from(MAX_EDGE)) as u32;
    let (mut sw, mut sh) = (snap(fw), snap(fh));
    // Rounding can cross a limit by a step. Each fix moves the shape toward square (the longer side
    // shorter, or the shorter longer), so they don't undo each other.
    for _ in 0..1024 {
        let pixels = u64::from(sw) * u64::from(sh);
        let (long, short) = if sw >= sh { (&mut sw, &mut sh) } else { (&mut sh, &mut sw) };
        if pixels > MAX_PIXELS && *long > STEP {
            *long -= STEP;
        } else if (pixels < MIN_PIXELS || *long > 3 * *short) && *short < MAX_EDGE {
            *short += STEP;
        } else {
            break;
        }
    }
    (sw, sh)
}

impl<T: HttpTransport> ImageProvider for OpenAiProvider<T> {
    fn info(&self) -> ProviderInfo {
        ProviderInfo { id: ID, name: NAME, model: self.model.clone(), host: HOST }
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { generate: false, edit: true, variations: false, edit_in_place: true, max_references: MAX_REFERENCES }
    }

    fn edit_aspect(&self, w: u32, h: u32) -> (u32, u32) {
        if flexible_sizes(&self.model) { flexible_size(w, h, self.options.resolution) } else { crate::nearest_ratio(w, h, STANDARD).unwrap_or((1, 1)) }
    }

    fn mask_format(&self) -> MaskFormat {
        MaskFormat::Alpha
    }

    fn estimated_cost(&self, w: u32, h: u32) -> Option<f64> {
        estimated_cost(&self.model, &self.options.quality, output_size(&self.model, &self.options, w, h))
    }

    fn edit(&self, r: &EditRequest) -> Result<GeneratedImage> {
        crate::check_prompt(&r.prompt, r.mode == EditMode::Edit)?;
        crate::check_references(&r.references, MAX_REFERENCES)?;
        let (w, h) = crate::png_size(&r.image).ok_or_else(|| GenError::Invalid("the image must be a PNG".into()))?;
        if crate::png_size(&r.mask) != Some((w, h)) {
            return Err(GenError::Invalid("the mask must be a PNG the size of the image".into()));
        }
        let n = r.references.len();
        let instruction = match r.mode {
            EditMode::Fill => fill_instruction(&r.prompt, n),
            EditMode::Edit => edit_instruction(&r.prompt, n),
        };
        let mut form = Multipart::new(seed(&r.image));
        form.text("model", &self.model).text("prompt", &instruction);
        // One image goes as `image`; with references, all of them as `image[]`, the edited first.
        let image_field = if n == 0 { "image" } else { "image[]" };
        form.file(image_field, "image.png", "image/png", &r.image);
        for (i, reference) in r.references.iter().enumerate() {
            let ext = if reference.mime == "image/png" { "png" } else { "jpg" };
            form.file("image[]", &format!("reference-{}.{ext}", i + 1), &reference.mime, &reference.bytes);
        }
        form.file("mask", "mask.png", "image/png", &r.mask);
        form.text("size", &self.size(w, h)).text("quality", &self.options.quality).text("output_format", &self.options.format);
        if self.options.format != "png" {
            form.text("output_compression", &self.options.compression.to_string());
        }
        if let Some(f) = &self.options.input_fidelity {
            match family(&self.model) {
                Family::One => {
                    form.text("input_fidelity", f);
                }
                // GPT Image 1 Mini takes only `low`, its default; later models refuse the field.
                Family::Mini | Family::Two | Family::TwoFive => {}
            }
        }
        let (content_type, body) = form.finish()?;
        let response = self.transport.post(HttpRequest {
            url: ENDPOINT.into(),
            headers: vec![("authorization", format!("Bearer {}", self.key.expose())), ("content-type", content_type)],
            body,
        })?;
        if response.body.len() > crate::MAX_RESPONSE {
            return Err(GenError::BadResponse("the response is too large"));
        }
        if !(200..300).contains(&response.status) {
            return Err(self.http_error(response.status, &response.body));
        }
        let value: Value = serde_json::from_slice(&response.body).map_err(|_| GenError::BadResponse("not JSON"))?;
        let data = value.get("data").and_then(Value::as_array).and_then(|d| d.first()).and_then(|d| d.get("b64_json")).and_then(Value::as_str);
        let Some(data) = data else { return Err(GenError::Refused("it returned no image".into())) };
        let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|_| GenError::BadResponse("the image data is not base64"))?;
        let mime = match value.get("output_format").and_then(Value::as_str).unwrap_or(&self.options.format) {
            "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            _ => "image/png",
        };
        Ok(GeneratedImage { bytes, mime: mime.into() })
    }
}

/// A boundary seed from the image itself, so requests differ without a clock or random source.
fn seed(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3))
}

/// What the instructions say about the images after the first, if any.
fn references_note(references: usize, use_for: &str) -> String {
    match references {
        0 => String::new(),
        1 => format!("The second image is a reference the user supplied: {use_for} "),
        n => format!("The {n} images after the first are references the user supplied: {use_for} "),
    }
}

/// The instruction for [`EditMode::Fill`]: the masked area arrives blurred out (a dimmed
/// placeholder made of the colours around it), so the model can't paint back what was there. An
/// empty prompt means "what would be there without whatever stood in it", unless reference images
/// came with it: then it means "put what they show there". Edits apply to the first image only.
pub fn fill_instruction(prompt: &str, references: usize) -> String {
    let refs = references_note(
        references,
        "use them for what the request asks (an object, product, person, material or style to show in the placeholder) and match them closely.",
    );
    let what = if prompt.trim().is_empty() && references > 0 {
        "Replace the placeholder with the subject of the reference images. Make it belong to the scene: the right size for \
         its place in the perspective, lit from the same direction, casting and receiving shadows and reflections, and \
         interacting with whatever touches it. Around it, continue the background and anything that crosses the \
         placeholder's edge."
            .to_owned()
    } else if prompt.trim().is_empty() {
        "Replace the placeholder with what would be there if whatever stood in it had never existed: continue the \
         background, surfaces, textures and lighting from around it, and carry on anything that crosses its edge (lines, \
         edges, horizon, patterns, objects partly outside it) the way it would plausibly continue. Do not add a new \
         object, light or shape that nothing around it suggests."
            .to_owned()
    } else {
        format!(
            "Replace the placeholder with: {}. Make it belong to the scene: the right size for its place in the \
             perspective, lit from the same direction, casting and receiving shadows and reflections, and interacting \
             with whatever touches it. Around the new content, continue the background and anything that crosses the \
             placeholder's edge.",
            prompt.trim().trim_end_matches('.')
        )
    };
    format!(
        "Edit the first image inside the masked area only. That area has been blurred out as a placeholder. {refs}The blur \
         is not part of the scene: replace all of it with sharp, detailed content that has the same focus, noise and grain \
         as its surroundings. Before editing, study the whole image: what the scene is, where the light comes from, the \
         perspective and depth, and what runs up to the placeholder. {what} Keep everything outside the masked area, the \
         framing, the scale and the position of every object exactly as they are."
    )
}

/// The instruction for [`EditMode::Edit`]: the masked area arrives as it is and the model changes
/// only what the prompt asks for there.
pub fn edit_instruction(prompt: &str, references: usize) -> String {
    let refs =
        references_note(references, "use them as the request says (for example a colour, material, design, logo or object to match) and match them closely.");
    format!(
        "Edit the first image inside the masked area only. {refs}In the masked area: {}. Change only what that asks for. \
         Keep the shape, size, position, perspective, lighting, texture and material of everything there, and keep any \
         text, logo or pattern with the same lettering, layout and alignment, except where the request changes it. The \
         edited area must look like part of the same photograph, with the same focus, noise and grain. Keep everything \
         outside the masked area, the framing and the scale exactly as they are.",
        prompt.trim().trim_end_matches('.')
    )
}

/// About what one variation costs in US dollars, from OpenAI's published per-image prices
/// (October 2026) for the model, quality and output size; GPT Image 2.5 is priced by tokens at
/// the same rates as GPT Image 2, so its figure is GPT Image 2's. `None` where no price is
/// published (`auto`, `xhigh`, `max`). Input images (the one edited, references) add a little.
pub fn estimated_cost(model: &str, quality: &str, size: (u32, u32)) -> Option<f64> {
    // (square 1024², portrait or landscape 1024×1536) for low, medium, high.
    let table: [(f64, f64); 3] = match family(model) {
        Family::Two | Family::TwoFive => [(0.006, 0.005), (0.053, 0.041), (0.211, 0.165)],
        Family::Mini => [(0.005, 0.006), (0.011, 0.015), (0.036, 0.052)],
        Family::One if model.starts_with("gpt-image-1.5") => [(0.009, 0.013), (0.034, 0.05), (0.133, 0.2)],
        Family::One => [(0.011, 0.016), (0.042, 0.063), (0.167, 0.25)],
    };
    let (square, oblong) = match quality {
        "low" => table[0],
        "medium" => table[1],
        "high" => table[2],
        _ => return None,
    };
    let (w, h) = (f64::from(size.0.max(1)), f64::from(size.1.max(1)));
    if w == h { Some(square * w * h / (1024.0 * 1024.0)) } else { Some(oblong * w * h / (1024.0 * 1536.0)) }
}

/// The output size an edit of a `w`×`h` area asks for with `model` and `options` (for the cost
/// estimate and the settings' notes).
pub fn output_size(model: &str, options: &Options, w: u32, h: u32) -> (u32, u32) {
    if flexible_sizes(model) {
        flexible_size(w, h, options.resolution)
    } else {
        match crate::nearest_ratio(w, h, STANDARD) {
            Some((3, 2)) => (1536, 1024),
            Some((2, 3)) => (1024, 1536),
            _ => (1024, 1024),
        }
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
    use serde_json::json;
    use std::sync::Mutex;

    /// Records each request and answers with the next canned response.
    struct Fake {
        sent: Mutex<Vec<HttpRequest>>,
        replies: Mutex<Vec<Result<HttpResponse>>>,
    }

    impl Fake {
        fn new(replies: Vec<Result<HttpResponse>>) -> Self {
            Self { sent: Mutex::new(Vec::new()), replies: Mutex::new(replies) }
        }
        fn status(status: u16, body: Value) -> Result<HttpResponse> {
            Ok(HttpResponse { status, body: serde_json::to_vec(&body).unwrap() })
        }
        fn image(bytes: &[u8], format: &str) -> Result<HttpResponse> {
            Fake::status(200, json!({"created": 1, "data": [{"b64_json": b64(bytes)}], "output_format": format, "size": "1024x1024"}))
        }
    }

    impl HttpTransport for Fake {
        fn post(&self, r: HttpRequest) -> Result<HttpResponse> {
            self.sent.lock().unwrap().push(r);
            self.replies.lock().unwrap().remove(0)
        }
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

    const KEY: &str = "sk-proj-test-secret";

    fn provider(model: &str, options: Options, replies: Vec<Result<HttpResponse>>) -> OpenAiProvider<Fake> {
        OpenAiProvider::new(ApiKey::new(KEY).unwrap(), model, options, Fake::new(replies)).unwrap()
    }

    fn edit(prompt: &str, w: u32, h: u32) -> EditRequest {
        EditRequest {
            image: png_of(w, h),
            mask: png_of(w, h),
            prompt: prompt.into(),
            size: crate::ImageSize::Medium,
            mode: EditMode::Fill,
            references: Vec::new(),
        }
    }

    /// A multipart part: name, file name, content type, body.
    type Part = (String, Option<String>, Option<String>, Vec<u8>);

    /// The multipart parts of a request.
    fn parts(r: &HttpRequest) -> Vec<Part> {
        let ct = &r.headers.iter().find(|(n, _)| *n == "content-type").unwrap().1;
        let boundary = format!("--{}", ct.split("boundary=").nth(1).unwrap());
        let body = &r.body;
        let mut out = Vec::new();
        let mut starts: Vec<usize> = (0..body.len().saturating_sub(boundary.len())).filter(|&i| body[i..].starts_with(boundary.as_bytes())).collect();
        assert!(body.ends_with(format!("{boundary}--\r\n").as_bytes()), "closed");
        starts.push(body.len());
        for pair in starts.windows(2) {
            let chunk = &body[pair[0] + boundary.len()..pair[1]];
            if chunk.starts_with(b"--") {
                break;
            }
            let split = chunk.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
            let head = String::from_utf8(chunk[2..split].to_vec()).unwrap();
            let data = chunk[split + 4..chunk.len() - 2].to_vec();
            let name = head.split("name=\"").nth(1).unwrap().split('"').next().unwrap().to_owned();
            let filename = head.split("filename=\"").nth(1).map(|f| f.split('"').next().unwrap().to_owned());
            let ctype = head.split("Content-Type: ").nth(1).map(|c| c.trim().to_owned());
            out.push((name, filename, ctype, data));
        }
        out
    }

    fn text_part(r: &HttpRequest, name: &str) -> Option<String> {
        parts(r).into_iter().find(|p| p.0 == name).map(|p| String::from_utf8(p.3).unwrap())
    }

    #[test]
    fn an_edit_sends_image_mask_and_prompt_with_the_key_in_a_header() {
        let p = provider(DEFAULT_MODEL, Options::default(), vec![Fake::image(b"result", "png")]);
        let out = p.edit(&edit("a red balloon", 1600, 900)).unwrap();
        assert_eq!(out, GeneratedImage { bytes: b"result".to_vec(), mime: "image/png".into() });
        let sent = p.transport().sent.lock().unwrap();
        let r = &sent[0];
        assert_eq!(r.url, ENDPOINT);
        assert!(!r.url.contains("secret"));
        assert!(r.headers.contains(&("authorization", format!("Bearer {KEY}"))));
        let parts = parts(r);
        let names: Vec<&str> = parts.iter().map(|p| p.0.as_str()).collect();
        assert_eq!(names, ["model", "prompt", "image", "mask", "size", "quality", "output_format"]);
        assert_eq!(text_part(r, "model").unwrap(), DEFAULT_MODEL);
        let prompt = text_part(r, "prompt").unwrap();
        assert!(prompt.contains("placeholder with: a red balloon.") && prompt.contains("masked area only"), "{prompt}");
        assert_eq!(parts[2].3, png_of(1600, 900));
        assert_eq!((parts[2].1.as_deref(), parts[2].2.as_deref()), (Some("image.png"), Some("image/png")));
        assert_eq!(parts[3].1.as_deref(), Some("mask.png"));
        // 16:9 at the 2K working resolution, both sides multiples of 16.
        assert_eq!(text_part(r, "size").unwrap(), "2048x1152");
        assert_eq!(text_part(r, "quality").unwrap(), "high");
        assert!(!String::from_utf8_lossy(&r.body).contains(KEY), "the key is only in the header");
        assert_eq!(p.mask_format(), MaskFormat::Alpha);
    }

    #[test]
    fn references_follow_the_image_as_an_array_and_the_mask_applies_to_the_first() {
        let p = provider(DEFAULT_MODEL, Options::default(), vec![Fake::image(b"r", "png")]);
        let jpeg = |b: &[u8]| ReferenceImage { bytes: b.to_vec(), mime: "image/jpeg".into() };
        let r = EditRequest { mode: EditMode::Edit, references: vec![jpeg(b"a"), jpeg(b"b")], ..edit("make it like these", 64, 64) };
        p.edit(&r).unwrap();
        let sent = p.transport().sent.lock().unwrap();
        let parts = parts(&sent[0]);
        let images: Vec<_> = parts.iter().filter(|p| p.0 == "image[]").map(|p| (p.1.clone().unwrap(), p.3.clone())).collect();
        assert_eq!(images, [("image.png".into(), png_of(64, 64)), ("reference-1.jpg".into(), b"a".to_vec()), ("reference-2.jpg".into(), b"b".to_vec())]);
        assert!(parts.iter().all(|p| p.0 != "image"));
        let prompt = text_part(&sent[0], "prompt").unwrap();
        assert!(prompt.contains("The 2 images after the first are references") && prompt.contains("In the masked area: make it like these."), "{prompt}");
        assert!(!prompt.contains("blurred out"));
    }

    #[test]
    fn fixed_size_models_get_the_nearest_size_and_input_fidelity() {
        let options = Options { quality: "medium".into(), format: "jpeg".into(), compression: 80, input_fidelity: Some("high".into()), ..Options::default() };
        let p = provider("gpt-image-1.5", options.clone(), vec![Fake::image(b"j", "jpeg"), Fake::image(b"k", "jpeg")]);
        assert_eq!(p.edit(&edit("", 900, 1400)).unwrap().mime, "image/jpeg");
        assert_eq!(p.edit_aspect(900, 1400), (2, 3));
        let mini = provider("gpt-image-1-mini", options, vec![Fake::image(b"m", "jpeg")]);
        mini.edit(&edit("", 1000, 1000)).unwrap();
        let sent = p.transport().sent.lock().unwrap();
        assert_eq!(text_part(&sent[0], "size").unwrap(), "1024x1536");
        assert_eq!(text_part(&sent[0], "input_fidelity").unwrap(), "high");
        assert_eq!(text_part(&sent[0], "output_compression").unwrap(), "80");
        let sent = mini.transport().sent.lock().unwrap();
        assert_eq!(text_part(&sent[0], "size").unwrap(), "1024x1024");
        assert_eq!(text_part(&sent[0], "input_fidelity"), None, "Mini takes only its default");
        // Only GPT Image 2.5 takes xhigh and max; another model asks for high instead.
        let max = || Options { quality: "max".into(), ..Options::default() };
        let one = OpenAiProvider::new(ApiKey::new("k").unwrap(), "gpt-image-1", max(), Fake::new(vec![Fake::image(b"r", "png")])).unwrap();
        one.edit(&edit("", 64, 64)).unwrap();
        assert_eq!(text_part(&one.transport().sent.lock().unwrap()[0], "quality").unwrap(), "high");
        let flare = OpenAiProvider::new(ApiKey::new("k").unwrap(), "gpt-image-2.5-flare", max(), Fake::new(vec![Fake::image(b"r", "png")])).unwrap();
        flare.edit(&edit("", 64, 64)).unwrap();
        assert_eq!(text_part(&flare.transport().sent.lock().unwrap()[0], "quality").unwrap(), "max");
    }

    #[test]
    fn flexible_sizes_stay_inside_the_apis_limits() {
        for (w, h) in [(1, 1), (1600, 900), (900, 1600), (5000, 100), (100, 5000), (4000, 3000), (777, 333)] {
            for long in [1024, 2048, 3840, 0, 99_999] {
                let (sw, sh) = flexible_size(w, h, long);
                let px = u64::from(sw) * u64::from(sh);
                assert!(sw % 16 == 0 && sh % 16 == 0, "{w}x{h}@{long}: {sw}x{sh}");
                assert!(sw <= 3840 && sh <= 3840, "{sw}x{sh}");
                assert!((MIN_PIXELS..=MAX_PIXELS).contains(&px), "{w}x{h}@{long}: {sw}x{sh} = {px}");
                assert!(sw <= 3 * sh && sh <= 3 * sw, "{w}x{h}@{long}: {sw}x{sh}");
            }
        }
        assert_eq!(flexible_size(1920, 1080, 3840), (3840, 2160));
        assert_eq!(flexible_size(1000, 1000, 1024), (1024, 1024));
        // A square at 4K would be too many pixels: as large as allowed.
        let (w, h) = flexible_size(1000, 1000, 3840);
        assert!(w == h && u64::from(w) * u64::from(h) <= MAX_PIXELS && w > 2800);
    }

    #[test]
    fn errors_map_to_actionable_messages_without_the_key() {
        let err = |status: u16, code: &str, message: &str| {
            Fake::status(status, json!({"error": {"message": message, "type": "invalid_request_error", "param": null, "code": code}}))
        };
        let cases = [
            (err(401, "invalid_api_key", "Incorrect API key provided: sk-proj-***"), GenError::Auth),
            (err(429, "rate_limit_exceeded", "Rate limit reached"), GenError::Quota),
            (err(429, "insufficient_quota", "You exceeded your current quota"), GenError::Account("You exceeded your current quota".into())),
            (err(429, "credit_balance_exhausted", "Your credit balance is exhausted"), GenError::Account("Your credit balance is exhausted".into())),
            (
                err(403, "", "Your organization must be verified to use the model"),
                GenError::Account("Your organization must be verified to use the model".into()),
            ),
            (
                err(400, "moderation_blocked", "Your request was rejected by the safety system"),
                GenError::Refused("Your request was rejected by the safety system".into()),
            ),
            (err(400, "invalid_value", &format!("bad size {KEY}")), GenError::Http { status: 400, message: "bad size [key]".into() }),
            (err(400, &format!("odd_{KEY}"), ""), GenError::Http { status: 400, message: "odd_[key]".into() }),
            (
                Fake::status(
                    503,
                    json!({"error": {"message": "overloaded", "type": "service_unavailable_error", "param": null, "code": "server_is_overloaded"}}),
                ),
                GenError::Http { status: 503, message: "overloaded".into() },
            ),
            (Fake::status(200, json!({"data": []})), GenError::Refused("it returned no image".into())),
            (Ok(HttpResponse { status: 200, body: b"<html>".to_vec() }), GenError::BadResponse("not JSON")),
            (Fake::status(200, json!({"data": [{"b64_json": "%%%"}]})), GenError::BadResponse("the image data is not base64")),
            (Err(GenError::Network("certificate not trusted".into())), GenError::Network("certificate not trusted".into())),
        ];
        for (reply, want) in cases {
            let p = provider(DEFAULT_MODEL, Options::default(), vec![reply]);
            let got = p.edit(&edit("x", 64, 64)).unwrap_err();
            assert_eq!(got, want);
            assert!(!format!("{got} {got:?}").contains(KEY));
            // A network failure (TLS included) is reported once, never sent again some other way.
            assert_eq!(p.transport().sent.lock().unwrap().len(), 1);
        }
    }

    #[test]
    fn bad_input_fails_before_anything_is_sent() {
        let p = provider(DEFAULT_MODEL, Options::default(), Vec::new());
        assert!(matches!(p.edit(&EditRequest { image: b"jpeg".to_vec(), ..edit("x", 8, 8) }), Err(GenError::Invalid(_))));
        assert!(matches!(p.edit(&EditRequest { mask: png_of(9, 8), ..edit("x", 8, 8) }), Err(GenError::Invalid(_))));
        assert!(matches!(p.edit(&EditRequest { mode: EditMode::Edit, ..edit(" ", 8, 8) }), Err(GenError::Invalid(_))));
        let jpeg = ReferenceImage { bytes: b"x".to_vec(), mime: "image/jpeg".into() };
        assert!(matches!(p.edit(&EditRequest { references: vec![jpeg; MAX_REFERENCES + 1], ..edit("x", 8, 8) }), Err(GenError::Invalid(_))));
        assert!(p.transport().sent.lock().unwrap().is_empty());
        assert!(OpenAiProvider::new(ApiKey::new("k").unwrap(), "gpt image/../x", Options::default(), Fake::new(Vec::new())).is_err());
        assert_eq!(OpenAiProvider::new(ApiKey::new("k").unwrap(), " ", Options::default(), Fake::new(Vec::new())).unwrap().info().model, DEFAULT_MODEL);
    }

    #[test]
    fn options_read_leniently_and_costs_are_estimated() {
        assert_eq!(Options::from_json(&Value::Null), Options::default());
        let o = Options::from_json(&json!({"quality": "max", "resolution": 99999, "format": "gif", "compression": 300, "inputFidelity": "ultra"}));
        assert_eq!(o, Options { quality: "max".into(), resolution: 3840, format: "png".into(), compression: 100, input_fidelity: None });
        assert_eq!(Options::from_json(&json!({"resolution": 5})).resolution, 1024);
        let near = |a: Option<f64>, b: f64| a.is_some_and(|a| (a - b).abs() < 1e-9);
        assert!(near(estimated_cost("gpt-image-1", "high", (1024, 1024)), 0.167));
        assert!(near(estimated_cost("gpt-image-1-mini", "low", (1536, 1024)), 0.006));
        assert!(estimated_cost(DEFAULT_MODEL, "high", (2048, 1152)).unwrap() > 0.2);
        assert_eq!(estimated_cost(DEFAULT_MODEL, "max", (1024, 1024)), None);
        assert_eq!(estimated_cost(DEFAULT_MODEL, "auto", (1024, 1024)), None);
        assert_eq!(output_size("gpt-image-1", &Options::default(), 1600, 900), (1536, 1024));
        assert_eq!(qualities("gpt-image-2"), ["auto", "low", "medium", "high"]);
        assert!(takes_input_fidelity("gpt-image-1") && !takes_input_fidelity("gpt-image-2.5-sunburst"));
    }

    #[test]
    fn multipart_bodies_are_well_formed() {
        let mut m = Multipart::new(7);
        m.text("a", "1").file("f", "x.png", "image/png", b"\x00\x01");
        let (ct, body) = m.finish().unwrap();
        assert_eq!(ct, "multipart/form-data; boundary=----PhotoCraftBoundary0000000000000007");
        let text = String::from_utf8_lossy(&body);
        assert!(text.starts_with("------PhotoCraftBoundary0000000000000007\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\n1\r\n"), "{text}");
        assert!(text.ends_with("------PhotoCraftBoundary0000000000000007--\r\n"));
        // A part that holds the boundary can't be sent.
        let mut bad = Multipart::new(7);
        bad.text("a", "------PhotoCraftBoundary0000000000000007");
        assert!(bad.finish().is_err());
    }
}
