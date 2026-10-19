//! Generative image providers (Generative Fill and friends, issue #41).
//!
//! [`ImageProvider`] is the one interface the engine talks to; each backend (Gemini today, others
//! later) is a separate implementation and none is privileged. Providers never open a socket
//! themselves: every request goes through an [`HttpTransport`] the app injects, so this crate (and
//! the engine above it) stays network-free, builds for wasm, and tests run against a fake.
//!
//! Images cross this interface as encoded bytes (PNG in, PNG or JPEG out) in sRGB. Masks are PNGs
//! of the same size as their image: white marks the pixels to change, black the pixels to keep.
//!
//! Credentials: an [`ApiKey`] never prints (its `Debug` is redacted) and no error message carries
//! a key or a response body; a service's own error text is passed through only after any copy of
//! the key is scrubbed from it.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod gemini;
pub mod openai;

/// Largest response body a provider accepts (a 4K PNG is well under this).
pub const MAX_RESPONSE: usize = 48 * 1024 * 1024;
/// Largest prompt, in bytes.
pub const MAX_PROMPT: usize = 8_000;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GenError {
    #[error("{0} can't do that")]
    Unsupported(&'static str),
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("the API key was rejected; check it in Generative AI Settings")]
    Auth,
    #[error("the image service's quota or rate limit was reached; try again later")]
    Quota,
    /// The account can't make this request until its owner acts (credit used up, a spending limit,
    /// no access to the model); trying again doesn't help.
    #[error("the image service account can't make this request: {0}")]
    Account(String),
    #[error("the image service declined this request: {0}")]
    Refused(String),
    #[error("the image service failed (HTTP {status}): {message}")]
    Http { status: u16, message: String },
    #[error("could not reach the image service: {0}")]
    Network(String),
    #[error("the image service sent a response PhotoCraft can't read: {0}")]
    BadResponse(&'static str),
    #[error("cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, GenError>;

/// A secret in process memory. It never prints; read it only to build a request.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    /// `None` for a blank key. Surrounding whitespace (a pasted newline) is dropped.
    pub fn new(key: &str) -> Option<ApiKey> {
        let k = key.trim();
        (!k.is_empty()).then(|| ApiKey(k.to_owned()))
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
    /// `text` with every copy of the key replaced, for error messages that echo a request.
    pub fn scrub(&self, text: &str) -> String {
        text.replace(&self.0, "[key]")
    }
}

/// A `multipart/form-data` body (RFC 7578), for services that take files as form parts.
pub struct Multipart {
    boundary: String,
    body: Vec<u8>,
}

impl Multipart {
    /// `seed` makes the boundary; any value works, as the boundary is checked against the parts.
    pub fn new(seed: u64) -> Multipart {
        Multipart { boundary: format!("----PhotoCraftBoundary{seed:016x}"), body: Vec::new() }
    }

    fn head(&mut self, name: &str, file: Option<(&str, &str)>) {
        // Names and file names are ours (ASCII, no quotes), never user text.
        let disposition = match file {
            Some((filename, _)) => format!("form-data; name=\"{name}\"; filename=\"{filename}\""),
            None => format!("form-data; name=\"{name}\""),
        };
        self.body.extend_from_slice(format!("--{}\r\nContent-Disposition: {disposition}\r\n", self.boundary).as_bytes());
        if let Some((_, mime)) = file {
            self.body.extend_from_slice(format!("Content-Type: {mime}\r\n").as_bytes());
        }
        self.body.extend_from_slice(b"\r\n");
    }

    pub fn text(&mut self, name: &str, value: &str) -> &mut Multipart {
        self.head(name, None);
        self.body.extend_from_slice(value.as_bytes());
        self.body.extend_from_slice(b"\r\n");
        self
    }

    pub fn file(&mut self, name: &str, filename: &str, mime: &str, bytes: &[u8]) -> &mut Multipart {
        self.head(name, Some((filename, mime)));
        self.body.extend_from_slice(bytes);
        self.body.extend_from_slice(b"\r\n");
        self
    }

    /// The `content-type` header value and the body. `Err` in the (astronomically unlikely) case
    /// that a part contains the boundary.
    pub fn finish(mut self) -> Result<(String, Vec<u8>)> {
        let marker = format!("--{}", self.boundary);
        if self.body.windows(marker.len()).filter(|w| *w == marker.as_bytes()).count() != self.parts() {
            return Err(GenError::Invalid("could not encode the request".into()));
        }
        self.body.extend_from_slice(format!("--{}--\r\n", self.boundary).as_bytes());
        Ok((format!("multipart/form-data; boundary={}", self.boundary), self.body))
    }

    fn parts(&self) -> usize {
        let head = format!("--{}\r\nContent-Disposition", self.boundary);
        self.body.windows(head.len()).filter(|w| *w == head.as_bytes()).count()
    }
}

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ApiKey(…)")
    }
}

/// One HTTPS POST. Header values may hold a credential, so a transport must not log them.
pub struct HttpRequest {
    pub url: String,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

/// Any HTTP status comes back as a response (the provider reads the service's error body);
/// only a failure to talk to the server at all is an `Err`.
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// The network seam. The desktop app implements it over HTTPS; tests use a fake; the web build
/// has none, so generative commands are simply unavailable there.
pub trait HttpTransport: Send + Sync {
    /// Blocking: called from an engine background job, never the UI thread.
    fn post(&self, request: HttpRequest) -> Result<HttpResponse>;
}

/// What a long request reports while it runs, and how it learns that it should stop. The engine
/// passes its job's; a provider that waits minutes (one that polls a job) uses it, others ignore it.
pub trait Progress: Send + Sync {
    /// How far this request is (0..1), with a short message ("Step 3 of 8").
    fn report(&self, fraction: f32, message: &str);
    fn cancelled(&self) -> bool;
}

/// A [`Progress`] that reports nowhere and is never cancelled.
pub struct NoProgress;

impl Progress for NoProgress {
    fn report(&self, _: f32, _: &str) {}
    fn cancelled(&self) -> bool {
        false
    }
}

/// How a provider wants the mask of an [`EditRequest`]. The engine builds it, so a provider never
/// decodes an image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MaskFormat {
    /// Opaque black and white: white marks the pixels to change (Gemini, stable-diffusion.cpp).
    #[default]
    Luma,
    /// Transparent where the pixels are to change, opaque black elsewhere (OpenAI).
    Alpha,
}

/// What a provider can do; the UI and the engine's `enabled` checks read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Capabilities {
    pub generate: bool,
    pub edit: bool,
    pub variations: bool,
    /// Edits in [`EditMode::Edit`]: changes what is in the masked area instead of refilling it.
    pub edit_in_place: bool,
    /// How many [`ReferenceImage`]s an edit takes (0: none).
    pub max_references: usize,
}

/// Who a request goes to, for the privacy notice and error messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderInfo {
    /// Stable id stored in preferences (`gemini`).
    pub id: &'static str,
    /// Display name (`Google Gemini`).
    pub name: &'static str,
    /// Model the requests use.
    pub model: String,
    /// Host the image, mask and prompt are sent to.
    pub host: &'static str,
}

/// Output size class. Providers map it to their nearest supported size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageSize {
    /// About 1024 px on the long side.
    #[default]
    Small,
    /// About 2048 px.
    Medium,
    /// About 4096 px.
    Large,
}

impl ImageSize {
    /// The class that covers `long_side` pixels without upscaling much.
    pub fn for_long_side(long_side: u32) -> ImageSize {
        match long_side {
            0..=1200 => ImageSize::Small,
            1201..=2400 => ImageSize::Medium,
            _ => ImageSize::Large,
        }
    }
}

/// Text to image.
#[derive(Debug, Clone)]
pub struct GenerateRequest {
    pub prompt: String,
    /// Wanted output width and height (only their ratio matters).
    pub aspect: (u32, u32),
    pub size: ImageSize,
}

/// Most reference images one edit sends. Gemini takes up to 14, but each one adds to the cost and
/// the time of every variation, and a few cover what people use them for.
pub const MAX_REFERENCES: usize = 4;
/// Most reference images any provider is given ([`Capabilities::max_references`] says how many
/// each takes): OpenAI takes 16 images, the one edited and 15 more.
pub const REFERENCES_LIMIT: usize = 15;

/// What an edit does with the masked area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EditMode {
    /// Generative Fill: the engine sends `image` with the masked area blurred out, so the provider
    /// fills a hole rather than repainting what was there. An empty prompt asks it to continue the
    /// surroundings through the hole (removing what was there).
    #[default]
    Fill,
    /// Change what is there: the masked area arrives as it is and the provider changes only what
    /// the prompt asks for (a colour, a text, a material), keeping the rest. Needs a prompt.
    Edit,
}

/// An image the user supplies to guide an edit: an object or product to show, a design or look to
/// match. Encoded (PNG or JPEG) in sRGB.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceImage {
    pub bytes: Vec<u8>,
    /// `image/png` or `image/jpeg`.
    pub mime: String,
}

/// Change the masked part of `image` (Generative Fill), as `mode` says, optionally guided by
/// reference images.
#[derive(Debug, Clone)]
pub struct EditRequest {
    pub image: Vec<u8>,
    pub mask: Vec<u8>,
    pub prompt: String,
    pub size: ImageSize,
    pub mode: EditMode,
    /// At most [`MAX_REFERENCES`].
    pub references: Vec<ReferenceImage>,
}

/// Another take on a whole image, optionally steered by a prompt.
#[derive(Debug, Clone)]
pub struct VariationRequest {
    pub image: Vec<u8>,
    pub prompt: String,
    pub size: ImageSize,
}

/// An encoded image a provider returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedImage {
    pub bytes: Vec<u8>,
    /// `image/png` or `image/jpeg`.
    pub mime: String,
}

/// A generative image backend. Methods block (they run in a background job) and default to
/// [`GenError::Unsupported`], so a backend implements only what its [`Capabilities`] report.
pub trait ImageProvider: Send + Sync {
    fn info(&self) -> ProviderInfo;
    fn capabilities(&self) -> Capabilities;
    /// The width:height an edit of a `w`×`h` image comes back at. Providers with a fixed set of
    /// output shapes return the nearest one, so the caller can grow its context area to match
    /// instead of stretching the result.
    fn edit_aspect(&self, w: u32, h: u32) -> (u32, u32) {
        (w, h)
    }
    /// How many variations a fill makes when nobody chose: three, as Photoshop makes, for a
    /// service that answers in seconds. A provider that takes minutes an image says fewer.
    fn default_variations(&self) -> usize {
        3
    }
    /// The pixel size an edit's image is sent at, from the size the engine would send (the
    /// context area, at most 2048 px on its long side). A provider that works at a smaller size or
    /// needs multiples of some step says so; the engine scales the result back up.
    fn request_size(&self, w: u32, h: u32) -> (u32, u32) {
        (w, h)
    }
    /// How the mask is to be sent ([`EditRequest::mask`]).
    fn mask_format(&self) -> MaskFormat {
        MaskFormat::Luma
    }
    /// Most requests sent to it at once (a server that works one at a time); `None`: as many as
    /// the engine allows.
    fn max_parallel(&self) -> Option<usize> {
        None
    }
    /// About what one variation of an edit of a `w`×`h` area costs, in US dollars, when the
    /// provider bills per image and publishes its prices; `None` otherwise (free, or unknown).
    fn estimated_cost(&self, _w: u32, _h: u32) -> Option<f64> {
        None
    }
    /// [`ImageProvider::edit`], reporting progress and stopping when cancelled, for providers that
    /// can (others just edit).
    fn edit_with_progress(&self, request: &EditRequest, _progress: &dyn Progress) -> Result<GeneratedImage> {
        self.edit(request)
    }
    fn generate(&self, _request: &GenerateRequest) -> Result<GeneratedImage> {
        Err(GenError::Unsupported(self.info().name))
    }
    fn edit(&self, _request: &EditRequest) -> Result<GeneratedImage> {
        Err(GenError::Unsupported(self.info().name))
    }
    fn variations(&self, _request: &VariationRequest) -> Result<GeneratedImage> {
        Err(GenError::Unsupported(self.info().name))
    }
}

/// Checks shared by every provider, run before anything is sent.
pub fn check_prompt(prompt: &str, required: bool) -> Result<()> {
    if prompt.len() > MAX_PROMPT {
        return Err(GenError::Invalid(format!("the prompt is longer than {MAX_PROMPT} bytes")));
    }
    if required && prompt.trim().is_empty() {
        return Err(GenError::Invalid("the prompt is empty".into()));
    }
    Ok(())
}

/// Largest encoded reference image, in bytes.
pub const MAX_REFERENCE_BYTES: usize = 8 * 1024 * 1024;

/// Checks on an edit's reference images, run before anything is sent: no more than `max`, each a
/// non-empty PNG or JPEG of a reasonable size.
pub fn check_references(references: &[ReferenceImage], max: usize) -> Result<()> {
    if references.len() > max {
        return Err(GenError::Invalid(format!("at most {max} reference images can be sent")));
    }
    for r in references {
        if !matches!(r.mime.as_str(), "image/png" | "image/jpeg") {
            return Err(GenError::Invalid("reference images must be PNG or JPEG".into()));
        }
        if r.bytes.is_empty() || r.bytes.len() > MAX_REFERENCE_BYTES {
            return Err(GenError::Invalid("a reference image is empty or too large".into()));
        }
    }
    Ok(())
}

/// The entry of `ratios` (width, height) closest to `w`×`h`, compared in log space so 2:1 and
/// 1:2 are equally far from 1:1.
pub fn nearest_ratio(w: u32, h: u32, ratios: &[(u32, u32)]) -> Option<(u32, u32)> {
    let target = (f64::from(w.max(1)) / f64::from(h.max(1))).ln();
    ratios.iter().copied().filter(|&(a, b)| a > 0 && b > 0).min_by(|&(a, b), &(c, d)| {
        let da = ((f64::from(a) / f64::from(b)).ln() - target).abs();
        let dc = ((f64::from(c) / f64::from(d)).ln() - target).abs();
        da.total_cmp(&dc)
    })
}

/// Width and height of a PNG from its header, without decoding it.
pub fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || !bytes.starts_with(SIG) || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    Some((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_keys_never_print_and_are_scrubbed() {
        let k = ApiKey::new("  sk-secret-123\n").unwrap();
        assert_eq!(k.expose(), "sk-secret-123");
        assert!(!format!("{k:?}").contains("secret"));
        assert_eq!(k.scrub("bad key sk-secret-123!"), "bad key [key]!");
        assert!(ApiKey::new(" \n").is_none());
    }

    #[test]
    fn nearest_ratio_is_symmetric_in_log_space() {
        let r = [(1, 1), (16, 9), (9, 16), (4, 3), (3, 4)];
        assert_eq!(nearest_ratio(1000, 1000, &r), Some((1, 1)));
        assert_eq!(nearest_ratio(1920, 1080, &r), Some((16, 9)));
        assert_eq!(nearest_ratio(1080, 1920, &r), Some((9, 16)));
        assert_eq!(nearest_ratio(400, 310, &r), Some((4, 3)));
        assert_eq!(nearest_ratio(0, 0, &r), Some((1, 1)));
        assert_eq!(nearest_ratio(5, 5, &[]), None);
    }

    #[test]
    fn prompt_and_size_checks() {
        assert!(check_prompt("", false).is_ok());
        assert!(check_prompt(" ", true).is_err());
        assert!(check_prompt(&"x".repeat(MAX_PROMPT + 1), false).is_err());
        assert_eq!(ImageSize::for_long_side(900), ImageSize::Small);
        assert_eq!(ImageSize::for_long_side(2000), ImageSize::Medium);
        assert_eq!(ImageSize::for_long_side(5000), ImageSize::Large);
    }

    #[test]
    fn reference_checks() {
        let jpeg = |n: usize| ReferenceImage { bytes: vec![1; n], mime: "image/jpeg".into() };
        assert!(check_references(&[], 0).is_ok());
        assert!(check_references(&[jpeg(10), jpeg(10)], 2).is_ok());
        assert!(check_references(&[jpeg(10)], 0).is_err());
        assert!(check_references(&[jpeg(0)], 4).is_err());
        assert!(check_references(&[jpeg(MAX_REFERENCE_BYTES + 1)], 4).is_err());
        assert!(check_references(&[ReferenceImage { bytes: vec![1], mime: "image/gif".into() }], 4).is_err());
    }

    #[test]
    fn png_size_reads_the_header_and_rejects_junk() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend_from_slice(&640u32.to_be_bytes());
        png.extend_from_slice(&480u32.to_be_bytes());
        assert_eq!(png_size(&png), Some((640, 480)));
        assert_eq!(png_size(b"GIF89a"), None);
        assert_eq!(png_size(&png[..20]), None);
    }
}
