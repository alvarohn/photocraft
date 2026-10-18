# Generative AI

PhotoCraft can fill a selection from a prompt (Edit › Generative Fill…) through an image service
the user picks and pays for with their own API key. It is off by default: until a provider is set
up, the command is hidden, nothing is contacted, and no key is read. This page follows the shape
the maintainers asked for in issue #41.

## Using it

1. Edit › Preferences › AI Integrations… › Generative AI Settings… (or the ▾ next to the selection
   tools' Generative Fill… button). Pick a provider (Google Gemini, OpenAI, or a stable-diffusion.cpp
   server of your own) and model, paste an API key (the local server takes none), set the
   provider's options, and read what is sent before pressing OK. Each provider
   keeps its own model, options and key, so switching back and forth loses nothing.
2. Make a selection, then Edit › Generative Fill… (or the options bar button). The dialog has two
   modes, as tabs:
   - **Fill selection** (the default): describe what to put there, or leave the prompt empty to
     continue the surrounding image through it (anything that runs into the selection carries on
     and partly covered objects are completed). This is what removes things.
   - **Edit selection**: describe what to change (a colour, a text, a material); what is in the
     selection is kept otherwise, like Photoshop's Prompt to Edit (or its partner models in
     Generative Fill). It needs a prompt: with none, the fill runs as Fill. The tab chosen is
     remembered.

   Under the prompt, **Reference images** (optional, up to 4; 15 with OpenAI): Add Reference… › Image File… or
   one of the open documents. They show the provider an object or product to put in the
   selection, or a design, colour or look to match ("this bottle", "a label like this one"). With
   references and an empty prompt, Fill places what they show. A file is checked when it is added
   (one that doesn't open is reported then) and read again when the fill runs. The references of
   the last fill are offered again the next time, for this session only.
   **Variations** (1 to 4): how many versions to generate, one request each. The default is the
   provider's (3, as Photoshop makes); the number chosen is remembered for each provider, so a
   costly or slow one can stay at 1. With a provider that publishes per-image prices (OpenAI) the
   dialog shows about what the fill will cost.
3. The variations come back as in Photoshop. They land in a new group named after the prompt
   and masked by the selection, one layer per variation (Variation 1, 2, 3…), with the first
   showing. Nothing under it changes; one undo removes it. The requests run side by side as one
   background job that can be cancelled; if some fail and others succeed, the successes are kept.

   Fills don't wait for each other, and nothing waits for them (Photoshop holds the
   window until each one is done). Press Generate and the selection is let go: select the next
   thing and start another while the first generates. Each fill in progress shows:
   - on the canvas, its area under a moving shimmer with a running outline, and a tag
     (Generating, Waiting (n) or Failed, the seconds it has taken, ✕ to cancel, ↻ to retry);
   - in the Layers panel, a locked "Generating: …" row right above the layer that was active
     when it started, where its group will land;
   - in the status bar, how many are generating, waiting and failed; click it for the list (go
     to an area, retry, cancel).

   At most 5 fills talk to the provider at once (each sends one request per variation, and the
   service limits how many it takes a minute); the rest wait their turn in order. A request the service turns
   down for its rate limit is sent again after 4, 8 and 16 s. The result lands where it was
   started, on its own document, without becoming the active layer, so whatever the user is
   doing meanwhile goes on; it is one undo step of its own, and a later one lands right above the
   same layer, under the earlier ones. It is dropped if the canvas changed size meanwhile (it
   would no longer line up) or the document was closed. A fill that failed (no network, the
   service refused it) keeps its tag and row until Retry (sent again exactly as started) or ✕.

   **Several objects at once.** Select them together (⇧ adds to the selection) and the dialog
   offers *Fill each of the n separate areas on its own* (on by default, up to 16 areas): each
   area becomes its own fill and layer, generated side by side, from one prompt. Off, the whole
   selection is filled as one, as Photoshop does.
4. With the group (or one of its variations) active, Properties › Variations shows a thumbnail
   per variation: click one to show it alone (`layer.generative.selectVariation`, one undo step).
   Generate More sends the same image, mask, prompt, mode and reference images again and adds
   as many more as the last fill made with that provider (`layer.generative.more`).

5. **File › Automate › Generative Batch…** (once a provider that edits from instructions is set
   up: Gemini, OpenAI, or a local edit model such as Qwen Image 2.1) runs the same edit on many
   images:
   - *Images*: the open documents, or a folder's image files; tick which.
   - *What to change*: the prompt (required). Each image is edited whole, as Edit selection does
     with all of it selected; reference images and the number of variations as in the prompt
     dialog.
   - *Same look in all*: the first image is edited first, alone; its result goes to every other
     one as their last reference image, and a sentence (shown, editable) is added to their prompt
     saying to match it. So a set of photos comes out with one look. It takes one of the
     provider's reference places.
   - *Results*: JPEG files (quality 92) the size of each image, in an `IA` folder beside it
     (`<folder>/IA/<name>.jpg`, then `<name>-2.jpg`… for more variations), or in a folder chosen.
     An open document that was never saved needs a folder chosen. A result is never written over
     the image it came from.

   Start Batch runs it as one background job: its progress (*3 of 12 images: name*) and Cancel in
   the status bar like any job; files already written stay when it is cancelled. It shares the
   provider's turns with the fills (up to 5 images at once; one for a local server). An image that
   fails is reported and the batch goes on, but two failures in a row that the account causes
   (the key, the balance or spending limit, the rate limit after its retries) stop it, as the
   rest would fail too. When it ends, a notice says how many files it wrote, where, and what
   failed; opened again, the dialog ticks only the files that failed, to retry them.

Photoshop keeps its variations inside one generative layer. PhotoCraft uses a group of ordinary
layers instead, so a PSD round trip needs no format changes and other apps see plain layers. What
each group was generated from is kept in the session only: after reopening a file the group is an
ordinary group (the variations are all still there; show one by hand), and Generate More is off.

Providers today:

- **Google Gemini** (the Nano Banana image models, default `gemini-nano-banana-2.1`). Keys come
  from [Google AI Studio](https://aistudio.google.com/apikey).
- **OpenAI** (GPT Image, through the Images API's `/v1/images/edits`). Keys come from
  [platform.openai.com](https://platform.openai.com/api-keys). Models, checked against the API
  reference (`openai/openai-openapi`, October 2026): `gpt-image-2.5-sunburst` (the default: GPT
  Image 2.5 for the most faithful edits), `gpt-image-2.5-flare` (GPT Image 2.5, faster),
  `gpt-image-2`, `gpt-image-1.5`, `gpt-image-1`, `gpt-image-1-mini`. There is no bare
  `gpt-image-2.5`: Sunburst and Flare are its two models. Options (Generative AI Settings):
  - *Quality*: auto, low, medium, high (the default); GPT Image 2.5 adds very high (`xhigh`) and
    maximum (`max`). A quality the model doesn't take becomes high.
  - *Working resolution* (GPT Image 2 and 2.5, which take any size): 1K, 2K (default) or 4K, the
    long side of the result (sides multiples of 16, between 1:3 and 3:1, 655,360 to 8,294,400
    pixels; above 2560×1440 OpenAI calls it experimental). GPT Image 1, 1.5 and Mini make
    1024×1024, 1536×1024 or 1024×1536, whichever is nearest the area's shape.
  - *Format*: PNG (default), JPEG or WebP, with a compression quality for the last two.
  - *Input fidelity* (GPT Image 1 and 1.5 only): high or low; the others take only their own.
  - The settings show about what one variation costs at that quality and size, from OpenAI's
    published per-image prices (GPT Image 2.5 is billed by tokens at GPT Image 2's rates, so its
    figure is GPT Image 2's; no figure for auto, very high or maximum). It is an estimate: the
    images sent are billed too.

  Not offered: *moderation* (the API takes it only in the JSON form of an edit, and PhotoCraft
  sends the multipart form, the one the official SDKs send), *background* (a fill's result is
  matted into the photo, so transparency isn't used) and `n` (each variation is its own request,
  so they run side by side and a failed one doesn't lose the others; the image sent is billed once
  per variation).

### A local server: stable-diffusion.cpp

**stable-diffusion.cpp (local)** sends fills to an [`sd-server`](https://github.com/leejet/stable-diffusion.cpp/tree/master/examples/server)
the user runs, usually on the same computer: free, private, no key, and slow (minutes an image on
a laptop GPU). It speaks the server's native asynchronous API (checked against release
`master-973-f0aac52`, 2026-10-17, `examples/server/api.md` and `routes_sdcpp.cpp`):

- `POST /sdcpp/v1/img_gen` queues a job: `init_image` (the image sent), `mask_image` (its mask;
  white = regenerate), `width`/`height` (the working size), `strength`, `seed: -1` (a new seed
  per variation: the server's default, 42, would make them all the same), `sample_params`
  (steps, `guidance.txt_cfg`, sampler, and for Qwen Image 2.1 Turbo its 8-step `custom_sigmas`,
  which a request can't name by preset), `preview: "proj"` (the cheapest preview: the only
  progress the API gives) and, for a model that edits from instructions, `ref_images` (the image
  sent, then the user's references). Images are base64 PNG or JPEG.
- `GET /sdcpp/v1/jobs/{id}` is polled every second: its place in the server's queue, then the
  sampling step (`preview.step` of `total_steps`), shown on the fill's tag as a percentage, then
  the result (`result.images[0].b64_json`). No call stays open for minutes, so no timeout cuts a
  slow image short; a few failed polls in a row are tolerated.
- Cancel withdraws a job still waiting (`POST /sdcpp/v1/jobs/{id}/cancel`). The server can't stop
  one already generating: PhotoCraft lets go of it at once, the server finishes it, and the next
  request waits behind it.

The server works one job at a time, so PhotoCraft sends it one request at a time: a fill's
variations go one after another and other fills wait their turn (at most one at the provider).
The default is 1 variation. The image goes at the **working size** (Generative AI Settings: 256 to
2048 px on the long side, 512 by default, both sides multiples of 32 as Qwen Image 2.1 needs) and
the result is scaled back up to the area, so a large selection comes back soft: raise the working
size if the machine can take it.

Settings: the server's address (default `http://127.0.0.1:1234`, `sd-server`'s own; `http` or
`https`, a host, a port and a path, nothing else), the model it runs (a preset), steps and
guidance (0: the server's own `--steps` and `--cfg-scale`), Fill strength and the working size.

| Preset | What it does | Fill | Edit, references, Batch | Licence |
|---|---|---|---|---|
| Z-Image Turbo | text to image; inpaints through the mask | yes, from a description (an empty prompt continues the image) | no | Apache 2.0 |
| Qwen Image 2.1 Turbo | edits from instructions, with reference images | yes | yes | Qwen Research: non-commercial use only (the settings say so) |
| Other model | whatever the server runs; tick *It edits from instructions…* for an edit model (Qwen Image Edit, Flux Kontext) | yes | if ticked | the model's |

**What Fill sends to a local model, and why.** The same as to Gemini: the image with the selection
blurred out (the dimmed placeholder) and the mask. stable-diffusion.cpp inpaints any model by
blending latents: at every step the area outside the mask is put back from the image sent, and
inside it the model generates (`src/pipeline/diffusion_engine.cpp`, `denoised * mask + init *
(1 - mask)`). At Fill strength 100% (the default) the masked area starts from pure noise, so what
the image shows there doesn't reach the result through the latents. It still matters twice: below
100% the area starts from the image's own pixels there, and an object left in them comes back
(the placeholder makes removal work, as with Gemini); and a model that edits from instructions
gets the image as a reference too, and would see (and copy back) what was there. So Fill sends
the placeholder, and Edit the selection as it is. This is from reading the server's code; it
couldn't be measured against a real model here (the models' hosts aren't reachable from the
cloud build), so the strength setting is there to tune it.

The mask is black and white; the server rounds it at 50% and widens it to its 8-pixel latent grid,
and PhotoCraft keeps only the selection's own pixels of the result (with its soft edge), as for
every provider.

Running `sd-server` (from stable-diffusion.cpp's releases; Vulkan builds run on most GPUs). Which
device runs each part (the text encoder `te`, the diffusion model and the VAE) and any other option
go in this command, since PhotoCraft doesn't start the server, and each model and card wants its own
split. These are the ones that worked on a 2 GB GTX 1050 (Windows, Vulkan, NVIDIA driver 582.78,
stable-diffusion.cpp master-973-f0aac52, 18 October 2026, 512×512 and 8 steps); `Vulkan1` is that
card's number on that PC, another PC may number it differently:

```text
sd-server --listen-port 1234 --diffusion-model z_image_turbo-Q3_K.gguf --llm Qwen3-4B-Instruct-2507-Q4_K_M.gguf ^
  --vae ae.safetensors --cfg-scale 1.0 --steps 8 ^
  --backend Vulkan1 --offload-to-cpu --vae-tiling -v

sd-server --listen-port 1234 --diffusion-model <Qwen Image 2.1 Turbo>.gguf --llm Qwen3VL-8B-Instruct-Q4_K_M.gguf ^
  --llm_vision mmproj-Qwen3VL-8B-Instruct-F16.gguf --vae qwen_image_2.1_vae_bf16.safetensors ^
  --sigmas qwen-image-2.1-turbo --steps 8 --cfg-scale 1.0 --sampling-method euler ^
  --backend "te=Vulkan1,diffusion=Vulkan1,vae=CPU" --vae-tiling --max-vram 1.0 --mmap ^
  --model-args qwen_image_2_1_prefix_cache=false -v
```

| On the GTX 1050 | Total | Per step | VAE |
|---|---|---|---|
| Z-Image Turbo | 3.8 min | 23 s | 11 s, on the card |
| Qwen Image 2.1 Turbo | 5.9 min | 23 s | 149 s, on the processor with tiling (73 s without it, in a processor-only run) |

On the processor alone they took 9.3 and 11 minutes. For Qwen Image 2.1 on that card, leave out
`--offload-to-cpu`: with it, and also with `--max-vram 1.5`, the card runs out of memory
(`ErrorOutOfDeviceMemory`). Its VAE (`wan_vae`) on the card's Vulkan driver crashes the driver
(an access violation, 0xC0000005, in `nvoglv64.dll`, as it starts decoding the tiles), hence
`vae=CPU`.

`--llm_vision` (the text encoder's vision weights) is what lets Qwen Image 2.1 read reference
images. With the Qwen Image 2.1 Turbo preset PhotoCraft always sends the image as a reference, so
a server started without it fails every fill ("Qwen Image 2.1 editing requires Qwen3-VL vision
weights"); to use such a server as a plain inpainter, pick *Other model* with *It edits from
instructions* unticked (and set its steps to 8, guidance to 1).

## What is sent

Only when Generative Fill or Generate More runs, and only to the configured provider, once per
variation (1 to 4, three by default, each billed to the user's key). In Fill:

- the selection's bounds plus a margin of context (twice the selection's longer side, 64 to
  2048 px, so the model can follow a pattern or trend that runs through the selection, not
  just what touches its edge), grown toward the provider's output shape and kept inside the
  canvas, rendered from all visible layers in sRGB and scaled to at most 2048 px on the long
  side, with the selection itself replaced by a dimmed soft blur of the colours around it
  (at full brightness, bright content below an empty area smeared up through it and the
  model carried that content to the selection's far edge). The provider never sees
  what is being replaced, as Photoshop's fill doesn't, so it can't paint it back: an empty prompt
  removes what was selected and continues the surroundings, including anything that crosses the
  selection's edge. (A flat grey hole was tried first; Gemini drew it in as a grey box or a lamp.);
- a PNG mask of the selection at the same size: black and white (white = fill) for Gemini and
  stable-diffusion.cpp;
  for OpenAI, which takes a real mask, transparent wherever any of the selection is (OpenAI edits
  where the mask's alpha is zero) and opaque elsewhere;
- the prompt.

Generative Batch sends each image whole: its composite at most 2048 px on its long side (the
provider's working size if smaller), a mask of all of it, the prompt (with Same look's sentence
after the first) and the reference images (with Same look, the first image's result, at most
1024 px, as the last one), once per variation.

In Edit, the same, except that the selection is sent as it is (not blurred), since the provider is
to change what is there; the instruction asks it to change only what the prompt says and keep
shapes, texture, lettering and light. Without reference images, Fill sends exactly what it sent
before Edit and references existed (a test pins the instruction text).

Reference images, in either mode, follow the mask: each one's composite (all visible layers),
reduced to at most 1024 px on its long side, converted to sRGB, transparent parts matted over
white, as a JPEG. The instruction says what they are for. Gemini takes up to 14; PhotoCraft sends
it at most 4, as each one adds to every variation's cost and time. OpenAI takes 16 images in all
(`image[]`, the one edited first, the mask applying to it), so PhotoCraft sends it up to 15
references.

Gemini takes no mask parameter, so the mask is a second image and the instruction says what it
means. OpenAI takes the mask as a parameter; its instruction says only that the masked area is
the one to edit (and, in Fill, that it was blurred out). PhotoCraft keeps only the masked pixels of the result, so anything the model changes
outside the selection is discarded. The model redraws the whole image and relights it,
differently each time (Gemini often lifts dark shadows to nearly double), which would leave a
seam at the selection's edge; the area outside the selection should come back unchanged, so
comparing it with the original measures the change (`match_surroundings`). A tone curve on
brightness, hue kept, undoes the relighting; then what is left is carried smoothly inward from
the edge by push-pull. Pixels the model actually changed (different fine detail, as where it
moved an object, or a far different colour) are left out of both. A result that doesn't resemble
the original around the selection (over 25% mean difference) is left as it came.

To debug a provider, start the app with `PHOTOCRAFT_GENAI_DUMP=<folder>`: each request's image,
mask, prompt, mode, reference images, raw results and colour-matching figures are written to a new
subfolder there. Nothing
is written without it. For tuning, the ignored test `live_fill` in `apps/photocraft/src/genai.rs`
runs one fill from the command line with the saved key, or replays saved results without a
request (`PHOTOCRAFT_LIVE_REPLAY`). Gemini returns JPEG (its only output format) and its results carry Google's SynthID watermark.

## Keys and privacy

- A local stable-diffusion.cpp server gets no key and no credential is read for it. Its
  transport alone allows plain HTTP (it is this computer or one on the local network) and skips
  any system proxy; the cloud providers' stays HTTPS only.
- Keys live in the OS credential store: the macOS Keychain, Windows Credential Manager. On Linux
  and the BSDs a key stays in process memory and is asked for again after a restart; it is never
  written to a file.
- Preferences hold only the provider id and model (`dialogs["ui.generativeAiSettings"]`).
- The key field is not a dialog field, so `ui.inspect` and the control channel never see it. An
  `ApiKey` never prints (`Debug` is redacted), errors never carry a response body, and a
  service's own error text is passed on only after any copy of the key is scrubbed from it.
- The key goes in a request header (`x-goog-api-key` for Gemini, `Authorization: Bearer` for
  OpenAI), never in a URL or a body. The transport is HTTPS only and follows no redirects. TLS
  is always verified (rustls): a request whose certificate doesn't check out fails, and is never
  sent again without verification.

## Automation

`edit.generativeBatch` (`prompt`, `files` and/or `documents`, `variations`, `references`,
`sameLook`, `sameLookPrompt`, `output`; result `{written, failed: [{name, error, path |
document}], stopped, skipped, images, folders}`) is the batch, a background job like the others.

`edit.generativeFill` is a normal engine command (one undo step, a cancellable job; params
`prompt`, `variations` (1 to 4; by default the provider's, 3 for the cloud services), `mode` (`"fill"`, the default, or `"edit"`, which
needs a prompt), `references` (up to 4 file paths, 15 with OpenAI, `{"path": …}` or `{"document": id}` of an
open document) and `area` (fill only that one of the selection's separate areas, numbered from
0 top to bottom, as `layer.generative.areas` lists them)) so scripts and recorded actions can run
it locally, but automation (the control channel and MCP) may not: `authorize_engine_command`
refuses it, `layer.generative.more`, `layer.generative.retry` and `edit.generativeBatch`
(`genai_cmds::uploads_pixels`), because they upload document pixels. `layer.generative.selectVariation` sends nothing and is allowed. An explicit opt-in for
automation can come later.

Started in the background, a fill returns `{job, fill}` at once. `layer.generative.fills` lists
those that haven't landed (`id`, `job`, `document`, `anchor` (the layer it lands above), `name`,
`prompt`, `bounds`, `state` (`queued`, `running` or `failed`), `queue_position`, `progress`,
`elapsed_ms`, `error`); `layer.generative.retry {"fill": id}` sends a failed one again and
`layer.generative.discard {"fill": id}` stops one or forgets a failed one. Run inline (a script,
the CLI) a fill still lands before the command returns, and its group becomes the active layer as
before.

## Code

| Where | What |
|---|---|
| `crates/genai` | `ImageProvider` (generate, edit with mask in Fill or Edit mode with reference images, variations; capabilities, including `edit_in_place` and `max_references`; output shape, working size, mask form, how many requests at once, default number of variations, cost estimate, progress), `HttpTransport` (the network seam: POST, and GET for polling), `ApiKey`, `Multipart`, and the backends: `gemini.rs`, `openai.rs`, `sdcpp.rs` (a local stable-diffusion.cpp server). No network code: it builds for wasm and its tests use a fake transport. |
| `crates/engine/src/genai_cmds.rs` | `edit.generativeFill`, `layer.generative.selectVariation`, `layer.generative.more`, `.retry`, `.discard`, `.fills`, `.areas`: context area, pixel interchange (after #730's approach), reference images (`reference_image`), the job, the masked group of variations, the fills that haven't landed (`GenState`). `Session::set_image_provider` installs a provider. |
| `crates/engine/src/genai_cmds/batch.rs` | `edit.generativeBatch`: the images, one job, Same look, the account-failure stop, the JPEGs written. |
| `crates/engine/src/genai_cmds/queue.rs` | Turns at the provider (at most 5 fills at once, in order) and the pauses after a rate-limit answer. |
| `crates/engine/src/genai_cmds/areas.rs` | A selection's separate areas (8-connected parts, on a grid of at most a million cells) and the mask of one of them. |
| `crates/engine/src/jobs.rs` (`run_on`) | A job that edits a document without locking it: the fills' jobs. |
| `crates/ui-egui/src/genai_ui.rs` | Generative AI Settings, the prompt dialog (Fill / Edit tabs, reference images, each area on its own, variations, cost), the options bar button, Properties › Variations, `restore` at launch. |
| `crates/ui-egui/src/genai_ui/batch.rs` | File › Automate › Generative Batch…: the dialog, and the notice when a batch ends. |
| `crates/ui-egui/src/genai_ui/providers.rs` | The providers the settings offer and each one's rows (model, key, options), notes and what is sent; preferences `{"provider", "model", "options": {<provider>: {…}}}`. |
| `crates/ui-egui/src/genai_ui/pending.rs` | Fills in progress: the canvas shimmer, outline and tags, the Layers panel's "Generating…" rows, the status bar's readout and list, the flash when one lands. |
| `apps/photocraft/src/genai.rs` | The desktop `GenAiServices`: credential store (`keyring`), HTTPS transport (`ureq` + rustls; 3 minutes for Gemini, 10 for OpenAI), the local server's transport (plain HTTP allowed, no proxy, short calls) and `build`, which makes a provider from its id, model, options and key (none for the local server). |

### Adding a provider

Implement `ImageProvider` in `crates/genai` over an `HttpTransport` (any local endpoint, ComfyUI,
a generic HTTP contract), add it to `build` in `apps/photocraft/src/genai.rs` and to
`genai_ui/providers.rs` (the list, its rows and notes), and test it against a fake transport the
way `gemini.rs` and `openai.rs` do.
No backend is privileged; the engine only sees the trait.
