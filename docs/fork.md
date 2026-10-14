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
  requests. Details: [generative-ai.md](generative-ai.md).
- **Edit selection and reference images**, the fork's own additions to that Generative Fill
  (2026-10-14; not in texjer's pull requests). The dialog has two tabs: *Fill selection* is
  texjer's fill, unchanged (it blurs the selection out first, so an empty prompt removes what is
  there); *Edit selection* sends the selection as it is and asks for only what the prompt says
  to change (a colour, a text, a material), like Photoshop's Prompt to Edit. Either can take up
  to 4 reference images, from files or open documents. These live in texjer's files
  (`crates/genai`, `genai_cmds.rs`, `genai_ui.rs`), so a new push to those pull requests has to
  be merged by hand.
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
- **In-app updates** (`apps/photocraft/src/fork_update.rs`, Windows): at startup the app asks GitHub
  for the latest release. When it is newer, it offers to download it, saves its source pack, and
  starts the installer (closing PhotoCraft first, which asks about unsaved work). The source packs
  of the running build and of every update go to `Documents\PhotoCraft\codigo-fuente`, newest 3
  kept. Only builds made by the workflow (which sets `PHOTOCRAFT_FORK_REPO` and
  `PHOTOCRAFT_FORK_BUILD`) do this; set `PHOTOCRAFT_NO_UPDATE_CHECK=1` to skip it.

## Merging the official repository

The workflow merges in the runner and does not push the merge: the default `GITHUB_TOKEN` cannot
push commits that change the official repository's workflow files. `main` holds the fork's own
changes on top of some official commit, and every run merges the current official `main` again.
Expected conflicts are resolved by the script:

- Markdown docs take the official text;
- `Cargo.lock` takes the official lockfile, then `cargo metadata` adds the fork's extra crates;
- translation tables (`crates/ui-egui/src/i18n/*.tsv`) keep both sides (`.gitattributes`), and
  lines both sides added identically are dropped once (duplicates the official file already has
  stay);
- Rust and TOML files where both sides only added lines at the same place (a new match arm in
  `dialogs.rs`, a `mod` line) keep both (`scripts/fork/union-additions.py`, diff3 markers with an
  empty base). The build that follows catches a union that doesn't compile, so nothing broken is
  published.

Any other conflict stops the run, opens an issue in the fork (if issues are enabled) and fails the
workflow, so GitHub emails the owner. Fix it by merging `upstream/main` into `main` by hand,
resolving those files, and pushing; the next run publishes again. The installed app keeps working
in the meantime.

The workflow also turns off the official repository's own workflows in the fork (they need the
official signing secrets and would only fail here), and re-enables itself each run so GitHub doesn't
pause the schedule after 60 days without commits.

## Setting it up on a fresh fork

1. Actions tab → enable workflows. If "Fork release" shows as disabled (GitHub pauses schedules in
   forks), open it and click "Enable workflow".
2. Run "Fork release" once by hand (Run workflow) or push to `main`.
3. Optional: Settings → General → Features → Issues, so failed merges open an issue.
