//! Updates for this fork's own Windows builds: the official PhotoCraft plus Generative Fill
//! (storytold/photocraft#2878, #2879, #3703).
//!
//! The fork's release workflow (`.github/workflows/fork-release.yml`, see `docs/fork.md`) merges
//! the official repository every few hours, builds an installer and publishes it as a GitHub
//! release, next to a source pack that rebuilds it offline (the git history plus every
//! dependency). At startup this module asks GitHub for the latest release. When it is newer than
//! the running build it asks the user, downloads the installer and the source pack, keeps the last
//! [`KEEP_SOURCES`] source packs in `Documents\PhotoCraft\codigo-fuente`, and starts the installer.
//! The running build's own source pack is saved there too, so the code survives even if the
//! repositories disappear.
//!
//! Inert unless the build sets `PHOTOCRAFT_FORK_REPO` (`owner/repo`) and `PHOTOCRAFT_FORK_BUILD`
//! (the workflow does), so official and local builds never contact GitHub. Windows only; set
//! `PHOTOCRAFT_NO_UPDATE_CHECK=1` to skip the check.

use std::path::{Path, PathBuf};
use std::time::Duration;

use eframe::egui;
use serde_json::Value;

/// Source packs kept on disk, newest first.
pub const KEEP_SOURCES: usize = 3;
/// Release tags are `gf-<build>`, builds `YYYYMMDD.HHMM-<commit>` (UTC).
const TAG_PREFIX: &str = "gf-";
const API: &str = "https://api.github.com/repos";
/// API answers (release lists) are small; anything bigger is not GitHub.
const MAX_API_BYTES: u64 = 4 << 20;
/// This fork's repository and build id, set by its release workflow.
const FORK_BUILD: Option<(&str, &str)> = match (option_env!("PHOTOCRAFT_FORK_REPO"), option_env!("PHOTOCRAFT_FORK_BUILD")) {
    (Some(repo), Some(build)) => Some((repo, build)),
    _ => None,
};

/// This fork's builds take their updates from this fork's releases ([`start`]). The official
/// release check (at startup and Help › Check for Updates) would offer the official build, without
/// Generative Fill, so it is off in them, as in the official Flatpak builds.
pub fn hand_over_updates(updates: &mut photocraft_engine::updates_cmds::Updates) {
    hand_over(updates, FORK_BUILD.is_some());
}

fn hand_over(updates: &mut photocraft_engine::updates_cmds::Updates, fork_build: bool) {
    if fork_build {
        updates.check = None;
    }
}

pub fn start(ctx: &egui::Context) {
    let Some((repo, build)) = FORK_BUILD else { return };
    if !cfg!(target_os = "windows") || std::env::var_os("PHOTOCRAFT_NO_UPDATE_CHECK").is_some() {
        return;
    }
    let ctx = ctx.clone();
    let spawned = std::thread::Builder::new().name("fork-update".into()).spawn(move || {
        if let Err(e) = run(repo, build, &ctx) {
            log::warn!("update check: {e}");
        }
    });
    if let Err(e) = spawned {
        log::warn!("update check: could not start: {e}");
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub build: String,
    pub assets: Vec<Asset>,
}

/// The sortable part of a build id (`YYYYMMDD.HHMM`), or `None` when it isn't one.
pub fn build_stamp(build: &str) -> Option<&str> {
    let stamp = build.get(..13)?;
    let ok = stamp.char_indices().all(|(i, c)| if i == 8 { c == '.' } else { c.is_ascii_digit() });
    let rest_ok = build.get(13..).is_some_and(|r| r.is_empty() || r.starts_with('-'));
    (ok && rest_ok).then_some(stamp)
}

/// Whether `candidate` was built after `current`.
pub fn is_newer(current: &str, candidate: &str) -> bool {
    match (build_stamp(current), build_stamp(candidate)) {
        (Some(a), Some(b)) => b > a,
        _ => false,
    }
}

/// `12/10/2026 06:17 UTC` for `20261012.0617-…`.
pub fn display_date(build: &str) -> String {
    let Some(s) = build_stamp(build) else { return build.to_owned() };
    let part = |r: std::ops::Range<usize>| s.get(r).unwrap_or_default();
    format!("{}/{}/{} {}:{} UTC", part(6..8), part(4..6), part(0..4), part(9..11), part(11..13))
}

pub fn source_name(build: &str) -> String {
    format!("photocraft-source-{build}.zip")
}

/// A GitHub release object, if it is one of this fork's builds.
pub fn parse_release(v: &Value) -> Option<Release> {
    let build = v.get("tag_name")?.as_str()?.strip_prefix(TAG_PREFIX)?;
    build_stamp(build)?;
    let assets = v
        .get("assets")?
        .as_array()?
        .iter()
        .filter_map(|a| {
            Some(Asset {
                name: a.get("name")?.as_str()?.to_owned(),
                url: a.get("browser_download_url")?.as_str()?.to_owned(),
                size: a.get("size")?.as_u64()?,
            })
        })
        // Names become file names: no paths.
        .filter(|a| !a.name.is_empty() && !a.name.contains(['/', '\\', ':']) && a.name != "." && a.name != "..")
        .collect();
    Some(Release { build: build.to_owned(), assets })
}

pub fn asset<'a>(r: &'a Release, suffix: &str) -> Option<&'a Asset> {
    r.assets.iter().find(|a| a.name.ends_with(suffix))
}

/// Deletes all but the newest `keep` source packs in `dir`; returns what it deleted.
pub fn prune_sources(dir: &Path, keep: usize) -> std::io::Result<Vec<PathBuf>> {
    let mut packs: Vec<(String, PathBuf)> = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let build = name.strip_prefix("photocraft-source-")?.strip_suffix(".zip")?;
            Some((build_stamp(build)?.to_owned(), e.path()))
        })
        .collect();
    packs.sort_by(|a, b| b.0.cmp(&a.0));
    let mut removed = Vec::new();
    for (_, path) in packs.into_iter().skip(keep) {
        std::fs::remove_file(&path)?;
        removed.push(path);
    }
    Ok(removed)
}

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// `Documents\PhotoCraft\codigo-fuente`.
fn source_dir() -> Result<PathBuf, String> {
    let home = env_dir("USERPROFILE").ok_or("USERPROFILE is not set")?;
    Ok(home.join("Documents").join("PhotoCraft").join("codigo-fuente"))
}

/// `%LOCALAPPDATA%\PhotoCraft\updates`.
fn updates_dir() -> Result<PathBuf, String> {
    let base = env_dir("LOCALAPPDATA").unwrap_or_else(std::env::temp_dir);
    Ok(base.join("PhotoCraft").join("updates"))
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(true)
        // Release downloads redirect to GitHub's asset host.
        .max_redirects(5)
        .user_agent("PhotoCraft-fork-updater")
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .timeout_recv_body(Some(Duration::from_secs(2 * 60 * 60)))
        .build()
        .new_agent()
}

fn fetch_release(agent: &ureq::Agent, url: &str) -> Result<Release, String> {
    let mut response = agent.get(url).header("Accept", "application/vnd.github+json").call().map_err(|e| format!("{url}: {e}"))?;
    let bytes = response.body_mut().with_config().limit(MAX_API_BYTES).read_to_vec().map_err(|e| format!("{url}: {e}"))?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|e| format!("{url}: not JSON: {e}"))?;
    parse_release(&value).ok_or_else(|| format!("{url}: not a release of this fork"))
}

/// Downloads `asset` to `dest` through a `.part` file, checking the size GitHub reported.
fn download(agent: &ureq::Agent, asset: &Asset, dest: &Path) -> Result<(), String> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let part = dest.with_extension("part");
    let result = (|| {
        let mut response = agent.get(&asset.url).call().map_err(|e| format!("{}: {e}", asset.name))?;
        let mut file = std::fs::File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?;
        let written = std::io::copy(&mut response.body_mut().as_reader(), &mut file).map_err(|e| format!("{}: {e}", asset.name))?;
        file.sync_all().map_err(|e| format!("{}: {e}", part.display()))?;
        if written != asset.size {
            return Err(format!("{}: got {written} bytes, expected {}", asset.name, asset.size));
        }
        Ok(())
    })();
    match result {
        Ok(()) => std::fs::rename(&part, dest).map_err(|e| format!("{}: {e}", dest.display())),
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

/// Saves `release`'s source pack (unless it is already there) and prunes old ones.
fn save_source(agent: &ureq::Agent, release: &Release, dir: &Path) -> Result<PathBuf, String> {
    let name = source_name(&release.build);
    let dest = dir.join(&name);
    let have = |a: &Asset| dest.metadata().is_ok_and(|m| m.len() == a.size);
    let pack = asset(release, &name).ok_or_else(|| format!("release {} has no {name}", release.build))?;
    if !have(pack) {
        download(agent, pack, &dest)?;
    }
    if let Err(e) = prune_sources(dir, KEEP_SOURCES) {
        log::warn!("pruning old source packs: {e}");
    }
    Ok(dest)
}

fn ask(title: &str, text: &str) -> bool {
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Info)
        .set_title(title)
        .set_description(text)
        .set_buttons(rfd::MessageButtons::YesNo)
        .show()
        == rfd::MessageDialogResult::Yes
}

fn tell(title: &str, text: &str) {
    let _ = rfd::MessageDialog::new().set_level(rfd::MessageLevel::Warning).set_title(title).set_description(text).set_buttons(rfd::MessageButtons::Ok).show();
}

const TITLE: &str = "Actualización de PhotoCraft";

fn run(repo: &str, current: &str, ctx: &egui::Context) -> Result<(), String> {
    let agent = agent();
    let sources = source_dir()?;

    // This build's own source code, once.
    if !sources.join(source_name(current)).exists() {
        match fetch_release(&agent, &format!("{API}/{repo}/releases/tags/{TAG_PREFIX}{current}")).and_then(|r| save_source(&agent, &r, &sources)) {
            Ok(path) => log::info!("saved this build's source code to {}", path.display()),
            Err(e) => log::warn!("saving this build's source code: {e}"),
        }
    }

    let latest = fetch_release(&agent, &format!("{API}/{repo}/releases/latest"))?;
    if !is_newer(current, &latest.build) {
        return Ok(());
    }
    let msi = asset(&latest, ".msi").ok_or_else(|| format!("release {} has no installer", latest.build))?;
    let updates = updates_dir()?;
    let installer = updates.join(&msi.name);
    let downloaded = installer.metadata().is_ok_and(|m| m.len() == msi.size);

    if !downloaded {
        let question = format!(
            "Hay una versión nueva de PhotoCraft con Relleno generativo, armada con los últimos cambios del repositorio \
             oficial (storytold/photocraft).\n\nInstalada: {}\nNueva: {}\n\n¿La descargo e instalo? También se guarda su \
             código fuente completo en:\n{}",
            display_date(current),
            display_date(&latest.build),
            sources.display()
        );
        if !ask(TITLE, &question) {
            return Ok(());
        }
        // Only the newest installer is worth keeping.
        if let Ok(entries) = std::fs::read_dir(&updates) {
            for e in entries.filter_map(Result::ok) {
                if e.path().extension().is_some_and(|x| x == "msi" || x == "part") {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        if let Err(e) = download(&agent, msi, &installer) {
            tell(TITLE, &format!("No se pudo descargar la actualización. Se va a volver a intentar la próxima vez que abras PhotoCraft.\n\n{e}"));
            return Err(e);
        }
        if let Err(e) = save_source(&agent, &latest, &sources) {
            tell(TITLE, &format!("La actualización se descargó, pero no su código fuente. Se va a volver a intentar la próxima vez.\n\n{e}"));
        }
    }

    let question = "La actualización está lista.\n\n¿La instalo ahora? PhotoCraft se va a cerrar (si tenés algo sin guardar, \
                    te va a preguntar antes) y se abre el instalador. Si elegís No, te lo vuelvo a ofrecer la próxima vez.";
    if !ask(TITLE, question) {
        return Ok(());
    }
    std::process::Command::new("msiexec").arg("/i").arg(&installer).spawn().map_err(|e| format!("msiexec: {e}"))?;
    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    ctx.request_repaint();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn build_ids_sort_by_their_stamp() {
        assert_eq!(build_stamp("20261012.0617-abc1234"), Some("20261012.0617"));
        assert_eq!(build_stamp("20261012.0617"), Some("20261012.0617"));
        assert_eq!(build_stamp("20261012-0617-abc"), None);
        assert_eq!(build_stamp("2026101.20617"), None);
        assert_eq!(build_stamp("20261012.0617abc"), None);
        assert_eq!(build_stamp("ñ"), None);
        assert_eq!(build_stamp(""), None);
        assert!(is_newer("20261012.0617-aaaaaaa", "20261012.1217-bbbbbbb"));
        assert!(is_newer("20261012.0617-aaaaaaa", "20261101.0000-aaaaaaa"));
        assert!(!is_newer("20261012.0617-aaaaaaa", "20261012.0617-bbbbbbb"));
        assert!(!is_newer("20261012.0617-aaaaaaa", "20261011.2359-bbbbbbb"));
        assert!(!is_newer("dev", "20261012.0617-aaaaaaa"));
        assert!(!is_newer("20261012.0617-aaaaaaa", "garbage"));
        assert_eq!(display_date("20261012.0617-abc"), "12/10/2026 06:17 UTC");
        assert_eq!(display_date("dev"), "dev");
    }

    #[test]
    fn releases_parse_and_reject_other_tags_and_unsafe_names() {
        let v = json!({
            "tag_name": "gf-20261012.0617-abc1234",
            "assets": [
                {"name": "photocraft-0.7.0-gf.20261012.0617-abc1234-windows-x64.msi", "browser_download_url": "https://github.com/a/b/x.msi", "size": 10},
                {"name": "photocraft-source-20261012.0617-abc1234.zip", "browser_download_url": "https://github.com/a/b/s.zip", "size": 20},
                {"name": "..\\evil.msi", "browser_download_url": "https://github.com/a/b/e.msi", "size": 1},
                {"name": "no-size.msi", "browser_download_url": "https://github.com/a/b/n.msi"}
            ]
        });
        let r = parse_release(&v).unwrap();
        assert_eq!(r.build, "20261012.0617-abc1234");
        assert_eq!(r.assets.len(), 2);
        assert_eq!(asset(&r, ".msi").unwrap().size, 10);
        assert_eq!(asset(&r, &source_name(&r.build)).unwrap().size, 20);
        assert!(parse_release(&json!({"tag_name": "v0.7.0", "assets": []})).is_none());
        assert!(parse_release(&json!({"tag_name": "gf-nope", "assets": []})).is_none());
        assert!(parse_release(&json!({"tag_name": 3})).is_none());
        assert!(parse_release(&json!(null)).is_none());
    }

    #[test]
    fn fork_builds_hand_the_official_release_check_over() {
        let transport = || -> photocraft_engine::updates_cmds::CheckFn { Box::new(|| Err("unused".into())) };
        let mut official = photocraft_engine::updates_cmds::Updates { check: Some(transport()), ..Default::default() };
        hand_over(&mut official, false);
        assert!(official.check.is_some());
        let mut fork = photocraft_engine::updates_cmds::Updates { check: Some(transport()), ..Default::default() };
        hand_over(&mut fork, true);
        assert!(fork.check.is_none());
        // Tests aren't built by the release workflow, so this build keeps the official check.
        hand_over_updates(&mut official);
        assert!(official.check.is_some());
    }

    #[test]
    fn pruning_keeps_the_newest_source_packs_and_ignores_other_files() {
        let dir = std::env::temp_dir().join(format!("photocraft-fork-update-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let builds = ["20261010.0000-a", "20261012.0600-b", "20261011.1200-c", "20261012.1200-d", "20261009.0000-e"];
        for b in builds {
            std::fs::write(dir.join(source_name(b)), b).unwrap();
        }
        std::fs::write(dir.join("notes.txt"), "keep me").unwrap();
        std::fs::write(dir.join("photocraft-source-garbage.zip"), "keep me").unwrap();
        let removed = prune_sources(&dir, 3).unwrap();
        assert_eq!(removed.len(), 2);
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        left.sort();
        assert_eq!(
            left,
            [
                "notes.txt",
                "photocraft-source-20261011.1200-c.zip",
                "photocraft-source-20261012.0600-b.zip",
                "photocraft-source-20261012.1200-d.zip",
                "photocraft-source-garbage.zip"
            ]
        );
        assert!(prune_sources(&dir.join("missing"), 3).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
