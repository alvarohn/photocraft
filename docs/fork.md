# This fork: PhotoCraft + Generative Fill, self-updating

`alvarohn/photocraft` is a personal fork of [storytold/photocraft](https://github.com/storytold/photocraft)
that adds Generative Fill and ships its own Windows builds, kept up to date with the official
repository, each with its complete source code. Everything here lives in files the official
repository doesn't have, so merging it stays easy.

## What the fork adds

- **Generative Fill** (Edit › Generative Fill…, and a button after Select and Mask… in the options
  bar), from texjer's open pull requests
  [storytold/photocraft#2878](https://github.com/storytold/photocraft/pull/2878),
  [#2879](https://github.com/storytold/photocraft/pull/2879) and
  [#3703](https://github.com/storytold/photocraft/pull/3703): select an area, type what should
  appear there (or nothing, to remove what is there), pick one of three variations. It uses Google
  Gemini with your own API key (Preferences › Integrations › Generative AI Settings; get a key at
  https://aistudio.google.com/apikey). Off until you set it up; each fill sends three billed
  requests. Details: [generative-ai.md](generative-ai.md). It follows #3703 as texjer last pushed
  it (`eb79ea0`, 2026-10-16: twice the selection's size of context around it and a dimmed
  placeholder, so fills follow the picture; Gemini's message when its credit runs out).
- **Edit selection and reference images**, the fork's own additions to that Generative Fill
  (2026-10-14; not in texjer's pull requests). The dialog has two tabs: *Fill selection* is
  texjer's fill, unchanged (it blurs the selection out first, so an empty prompt removes what is
  there); *Edit selection* sends the selection as it is and asks for only what the prompt says
  to change (a colour, a text, a material), like Photoshop's Prompt to Edit. Either can take up
  to 4 reference images, from files or open documents.
- **Fills side by side**, also the fork's own (2026-10-17): Generative Fill no longer holds the
  window. Press Generate, select the next thing and start another while the first generates; up
  to 5 talk to Google at once and the rest wait their turn. Each one shimmers over its area with
  a tag (✕ cancels, ↻ retries one that failed), has a locked "Generating…" row in the Layers
  panel where its layer will land, and is counted in the status bar (click for the list).
  Several objects selected together can be filled each on its own from one prompt. It touches
  official files in small places: `jobs.rs` (`run_on`, a job that doesn't lock its document),
  and one-line hooks in `canvas.rs`, `panels.rs` (Layers rows, status bar) and `jobs_ui.rs`.
  Details: [generative-ai.md](generative-ai.md).
- **More providers and a choice of variations**, also the fork's own (2026-10-18): the prompt
  dialog asks for 1 to 4 variations (remembered per provider), and besides Gemini, Generative AI
  Settings offers **OpenAI** (GPT Image 2.5 Sunburst and Flare, 2, 1.5, 1, 1 Mini; quality,
  working resolution up to 4K, output format, input fidelity, a cost estimate; up to 15 reference
  images). A local stable-diffusion.cpp server was a provider too for a day; the owner chose to
  keep local models out of PhotoCraft (2026-10-19), so it was taken out (its code is in the
  history: commit `c7876cb`).
- **Provider and model in the fill windows**, the fork's own too (2026-10-18, after a first try
  on the owner's PC): Generative Fill and Generative Batch choose the provider and model at their
  top, among those set up, kept as the settings' choice; Generative AI Settings keep the keys and
  the finer options. Each window says there what the model won't do of what it asks (fewer
  reference images). Each provider has its own turns, and Retry and Generate More go only to the
  provider and model a fill was made with.
- **Keep what is sent and returned**, the fork's own too (2026-10-18): Generative AI Settings can
  keep each fill's request and results in `Documents/PhotoCraft/generative/<date>/` (off by
  default; the `PHOTOCRAFT_GENAI_DUMP` debugging aid made a setting).
- **Generative Batch** (File › Automate › Generative Batch…), the fork's own too (2026-10-18): the
  same edit for the open documents or a folder's images, results as JPEGs in an `IA` folder
  beside each image, *Same look in all* (the first result guides the others), stopping after two
  account failures in a row; one background job with its progress and Cancel in the status bar,
  for every provider. New files: `crates/engine/src/genai_cmds/batch.rs` (and its tests),
  `crates/ui-egui/src/genai_ui/batch.rs`; one line each in the official `menus.rs` (the item shows
  once a provider is set up) and `crates/automation/src/workspace.rs` (automation may not run it). New files: `crates/genai/src/openai.rs`, `crates/ui-egui/src/genai_ui/providers.rs`;
  small changes in texjer's files (`lib.rs` of `genai`, `genai_cmds.rs`, `queue.rs`,
  `genai_ui.rs`, `apps/photocraft/src/genai.rs`) and one line in the official
  `crates/ui-egui/src/lib.rs` (`BuildProviderFn` takes the provider's options and an optional
  key). Details: [generative-ai.md](generative-ai.md).
- **Automatic builds** (`.github/workflows/fork-release.yml`): every 6 hours GitHub Actions merges
  the official `main` into this fork's code (`scripts/fork/merge-upstream.sh`), and when the result
  changed since the last release it tests Generative Fill, builds the Windows installer and
  publishes a release `gf-<YYYYMMDD.HHMM>-<official commit>` with:
  - `photocraft-<version>-gf.<build>-windows-x64.msi`: the installer;
  - `…-portable.zip`: the same app without installing;
  - `photocraft-source-<build>.zip`: the source code with its full git history, every Rust
    dependency vendored and the optional fonts, which rebuilds offline (`COMO-COMPILAR.txt` inside);
  - `SHA256SUMS.txt`.
  The newest 10 releases are kept.
- **Test builds** (`.github/workflows/fork-test-build.yml`): every push to a `claude/…` or
  `test/…` branch builds Windows from that branch and attaches the portable zip and the MSI to
  the run (Actions › Fork test build › the run › Artifacts, kept 7 days). Nothing is published
  and the installed app isn't offered it; the portable build keeps its settings beside
  `photocraft.exe`, so it runs next to the installed one (set up Generative AI in it once: the
  key saved by the installed app is already in the credential store). A way to try a change
  before it goes to `main`.
- **In-app updates** (`apps/photocraft/src/fork_update.rs`, Windows): at startup the app asks GitHub
  for the latest release. When it is newer, it offers to download it, saves its source pack, and
  starts the installer (closing PhotoCraft first, which asks about unsaved work). The source packs
  of the running build and of every update go to `Documents\PhotoCraft\codigo-fuente`, newest 3
  kept. Only builds made by the workflow (which sets `PHOTOCRAFT_FORK_REPO` and
  `PHOTOCRAFT_FORK_BUILD`) do this; set `PHOTOCRAFT_NO_UPDATE_CHECK=1` to skip it.

## Known gaps (debt)

What the 2026-10-18 additions (variations, OpenAI, Generative Batch) leave undone:

- **No complete run against OpenAI yet.** Its request shape is tested against a fake transport
  and checked against OpenAI's API reference, but the cloud build can't reach it or use a key.
  The first real try on the owner's PC got as far as OpenAI's answer that the account had no
  credit left: the key and its use are right; the request itself is still to be seen working.
- **OpenAI moderation** isn't sent: the API takes it for edits only in its JSON form, and
  PhotoCraft sends the multipart form (the one the official SDKs send). Switching an edit to the
  JSON form (`images: [{image_url: data URL}]`) would allow it.
- **OpenAI cost**: an estimate before the fill, from the published per-image prices (GPT Image
  2.5's taken as GPT Image 2's; none for auto, very high and maximum). The tokens the response
  reports (`usage`) aren't shown afterwards. Each variation is its own request, so the image sent
  is billed once per variation (`n` would bill it once).
- **Generative Batch** writes JPEG files only (no PNG, no "as layers in the open documents"), at
  each image's size; retrying is opening the dialog again (only the failed files are ticked), not
  a per-image Retry button; a batch is not listed in the fills' status-bar list (it shows as a
  job).
- **Translations**: the new strings are in the 20 languages whose catalogs are complete (the
  i18n tests require it); written by a model from each catalog's own terms, not yet read by
  native speakers.

## texjer's pull request and the `genai/side-by-side` branch

The fork's Generative Fill work lives in texjer's files (`crates/genai`, `genai_cmds.rs`,
`genai_ui.rs`), so it is kept as a branch texjer can take as it is: `genai/side-by-side` is his
#3703 head, the official `main` merged into it (which clears that pull request's conflicts), and
the fork's commits on top (Edit selection and reference images; fills side by side), with none of
the fork's own files. `main` is that branch merged in, plus the fork's own files (these docs, the
workflows, `scripts/fork/`, `fork_update.rs` and its two lines in `apps/photocraft/src/main.rs`).

When texjer pushes again: start a branch from his new head, merge `upstream/main`, cherry-pick the
fork's Generative Fill commits from `genai/side-by-side` (resolving them against his changes),
test, and make that the new `genai/side-by-side`; then merge it into `main` keeping its tree plus
the fork's own files (`git merge -s ours --no-commit`, `git checkout <branch> -- .`, restore
`main.rs`). The automatic merge below doesn't do this: it only follows the official `main`.

texjer took `genai/side-by-side` into #3703 as it was (2026-10-18: the Edit tab and reference
images, fills side by side, their translations and tests); its Generative AI files are the same as
`main`'s were before the 2026-10-18 additions. CodeRabbit's review of #3703 that day found, in that
shared code: generation sources kept by layer id alone and never dropped (a reopened `.pcraft`
keeps its document and layer ids, so its groups were taken as generative; memory grew with every
fill), the group's mask not at the document's depth, a "Generating…" row lost when its anchor layer
is hidden by Isolate Layers or the layer filter, and debug dump folders that could collide. The
fork fixed them (2026-10-19): sources by document and layer, dropped when the document closes (one
line in the official `Session::close`) or its group is gone, the mask converted as every new mask
is, the unplaced rows worked out from the rows shown (one line in the official `panels.rs`), and
dump folders numbered. The same fixes, on texjer's head, are offered to him in
`genai/coderabbit-fixes`.

## Merging the official repository

The workflow merges in the runner and does not push the merge: the default `GITHUB_TOKEN` cannot
push commits that change the official repository's workflow files. `main` holds the fork's own
changes on top of some official commit, and every run merges the current official `main` again.
Expected conflicts are resolved by the script:

- Markdown docs take the official text;
- `Cargo.lock` takes the official lockfile, then `cargo metadata` adds the fork's extra crates.
  The same happens after a merge without a lockfile conflict when `cargo --locked` would refuse
  the result: the fork's crates take the workspace version, so every official version bump left
  their entries behind (what stopped the 2026-10-20 17:30 run);
- translation tables (`crates/ui-egui/src/i18n/*.tsv`) keep both sides (`.gitattributes`), and
  lines both sides added identically are dropped once (duplicates the official file already has
  stay); a string both sides translated differently keeps the official translation;
- Rust and TOML files where both sides only added lines at the same place (a new match arm in
  `dialogs.rs`, a `mod` line) keep both (`scripts/fork/union-additions.py`, diff3 markers with an
  empty base). The build that follows catches a union that doesn't compile, so nothing broken is
  published.

Any other conflict stops the run, opens an issue in the fork (if issues are enabled) and fails the
workflow, so GitHub emails the owner. Fix it by merging `upstream/main` into `main` by hand,
resolving those files, and pushing; the next run publishes again. The installed app keeps working
in the meantime. Issues are off in this fork (a fork starts with them off), so today the failed
run's email (Settings › Notifications › Actions) is the only notice.

When `git merge-base --all main upstream/main` lists more than one commit, git merges against a
virtual base and can report conflicts in files the fork never touched (2026-10-23: five bases,
after official commits the fork had merged came back into the official history by another path;
git 2.43 reported 270 conflicted files). Merge by hand against the official commit `main` last merged, where the fork's
diff is only its own files: `git merge-tree --write-tree --merge-base=<that commit> main
upstream/main` shows the real conflicts (two that day); the merge commit `main` gets then has
`upstream/main` as a parent, so the next runs have a single base again.

The workflow also turns off the official repository's own workflows in the fork (they need the
official signing secrets and would only fail here), and re-enables itself each run so GitHub doesn't
pause the schedule after 60 days without commits.

## Setting it up on a fresh fork

1. Actions tab → enable workflows. If "Fork release" shows as disabled (GitHub pauses schedules in
   forks), open it and click "Enable workflow".
2. Run "Fork release" once by hand (Run workflow) or push to `main`.
3. Optional: Settings → General → Features → Issues, so failed merges open an issue.
