//! Whether a person lets AI agents use the Craft apps, and which apps they can reach.
//!
//! One switch decides it: ArtCraft Launcher › Settings › AI agents › *Let AI agents use Craft
//! apps*. While it is on, the launcher keeps an app list, [`APPS_FILE`] in [`agents_dir`]:
//!
//! ```json
//! { "version": 1, "apps": [
//!   { "id": "photocraft", "name": "PhotoCraft",
//!     "command": "C:/Program Files/PhotoCraft/photocraft-cli.exe", "args": ["mcp"] } ] }
//! ```
//!
//! Turning the switch off removes the list. Three kinds of programs use it, all through this crate:
//!
//! - the launcher writes it ([`write`], [`remove`]);
//! - the MCP gateway (`craft-ai-mcp`) reads it to know which apps it may start ([`load`],
//!   [`switched_on`]);
//! - each Craft app asks whether it is allowed to take agents into its open window
//!   ([`allowed`]): only while the list names it.
//!
//! Small on purpose (`serde`, `serde_json`, `dirs`), so every app can depend on it. Nothing here
//! panics on a bad file: a list that can't be read counts as the switch being off.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The app list the launcher writes into [`agents_dir`].
pub const APPS_FILE: &str = "apps.json";
/// Overrides [`agents_dir`] (tests, portable installs).
pub const DIR_ENV: &str = "CRAFT_MCP_DIR";
/// Largest app list read; anything bigger is not ours.
const MAX_APPS_BYTES: u64 = 1 << 20;
/// Most apps one list may name.
pub const MAX_APPS: usize = 64;
/// Most arguments one app entry may pass to its CLI.
const MAX_ARGS: usize = 32;
/// Longest app id.
const MAX_ID_LEN: usize = 40;
/// Version of the [`APPS_FILE`] format.
const APPS_VERSION: u32 = 1;

/// The Craft apps that ship an MCP server through `<id>-cli mcp`, with their display names: the
/// name a row without one gets.
pub const KNOWN_APPS: &[(&str, &str)] = &[
    ("photocraft", "PhotoCraft"),
    ("filmcraft", "FilmCraft"),
    ("effectcraft", "EffectCraft"),
    ("lightcraft", "LightCraft"),
    ("designcraft", "DesignCraft"),
    ("vectorcraft", "VectorCraft"),
    ("pdfcraft", "PdfCraft"),
    ("wordcraft", "WordCraft"),
    ("deckcraft", "DeckCraft"),
    ("gridcraft", "GridCraft"),
    ("cadcraft", "CADCraft"),
    ("soundcraft", "SoundCraft"),
    ("figcraft", "FigCraft"),
    ("craft-anything", "Craft Anything"),
];

/// The apps found, and what was left out and why.
pub type Found = (Vec<AppEntry>, Vec<String>);

/// One app agents can reach, and how to start its MCP server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AppEntry {
    /// Lower-case id agents name the app by (`photocraft`).
    pub id: String,
    /// Display name (`PhotoCraft`).
    pub name: String,
    /// The CLI to run.
    pub command: PathBuf,
    /// Its arguments (`["mcp"]`).
    pub args: Vec<String>,
}

#[derive(Deserialize)]
struct AppsFile {
    version: u32,
    #[serde(default)]
    apps: Vec<RawEntry>,
}

#[derive(Deserialize)]
struct RawEntry {
    id: String,
    #[serde(default)]
    name: String,
    command: PathBuf,
    #[serde(default)]
    args: Option<Vec<String>>,
}

/// The folder of the app list: [`DIR_ENV`], otherwise `ArtCraft/mcp` beside the AI layer's
/// `ArtCraft/ai` in the per-user local configuration folder.
pub fn agents_dir() -> Option<PathBuf> {
    match std::env::var_os(DIR_ENV) {
        Some(d) if !d.is_empty() => Some(PathBuf::from(d)),
        _ => dirs::config_local_dir().map(|b| b.join("ArtCraft").join("mcp")),
    }
}

/// An id agents can type: 1 to [`MAX_ID_LEN`] of `a-z`, `0-9` and `-`.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Parse an app list. Entries with a bad id, a relative command, too many arguments or a repeated
/// id are left out with a reason in the second value, so one bad row does not hide the others.
pub fn parse(bytes: &[u8]) -> Result<Found, String> {
    let file: AppsFile =
        serde_json::from_slice(bytes).map_err(|e| format!("{APPS_FILE} does not parse: {e}"))?;
    if file.version != APPS_VERSION {
        return Err(format!(
            "{APPS_FILE} has version {}, this reads version {APPS_VERSION}",
            file.version
        ));
    }
    let mut apps: Vec<AppEntry> = Vec::new();
    let mut skipped = Vec::new();
    for raw in file.apps {
        if apps.len() >= MAX_APPS {
            skipped.push(format!("more than {MAX_APPS} apps; the rest are left out"));
            break;
        }
        if !valid_id(&raw.id) {
            skipped.push(format!("`{}`: not a valid app id", raw.id.escape_debug()));
            continue;
        }
        if apps.iter().any(|a| a.id == raw.id) {
            skipped.push(format!("`{}`: listed twice", raw.id));
            continue;
        }
        if !raw.command.is_absolute() {
            skipped.push(format!("`{}`: command is not an absolute path", raw.id));
            continue;
        }
        let args = raw.args.unwrap_or_else(|| vec!["mcp".to_string()]);
        if args.len() > MAX_ARGS {
            skipped.push(format!("`{}`: more than {MAX_ARGS} arguments", raw.id));
            continue;
        }
        let name = if raw.name.trim().is_empty() {
            known_name(&raw.id).unwrap_or(&raw.id).to_string()
        } else {
            raw.name.trim().to_string()
        };
        apps.push(AppEntry {
            id: raw.id,
            name,
            command: raw.command,
            args,
        });
    }
    Ok((apps, skipped))
}

/// Read the app list in `dir`. `Ok(None)` when there is none.
pub fn load(dir: &Path) -> Result<Option<Found>, String> {
    let path = dir.join(APPS_FILE);
    let meta = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    if !meta.is_file() {
        return Err(format!("{}: not a regular file", path.display()));
    }
    if meta.len() > MAX_APPS_BYTES {
        return Err(format!(
            "{}: larger than {MAX_APPS_BYTES} bytes",
            path.display()
        ));
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&bytes).map(Some)
}

/// The display name of a known app id.
pub fn known_name(id: &str) -> Option<&'static str> {
    KNOWN_APPS.iter().find(|(k, _)| *k == id).map(|(_, n)| *n)
}

/// The app list for `apps` as the launcher writes it.
pub fn to_json(apps: &[AppEntry]) -> String {
    let list = serde_json::json!({ "version": APPS_VERSION, "apps": apps });
    serde_json::to_string_pretty(&list).unwrap_or_default()
}

/// Write the app list for `apps` into `dir` (made if missing): agents are turned on. The file is
/// replaced in one step, so a reader never sees half of it.
pub fn write(dir: &Path, apps: &[AppEntry]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("couldn't create {} ({e})", dir.display()))?;
    let path = dir.join(APPS_FILE);
    let tmp = dir.join(format!("{APPS_FILE}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, to_json(apps))
        .map_err(|e| format!("couldn't write {} ({e})", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("couldn't replace {} ({e})", path.display())
    })
}

/// Remove the app list in `dir`: agents are turned off. A missing list is fine.
pub fn remove(dir: &Path) -> Result<(), String> {
    let path = dir.join(APPS_FILE);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("couldn't remove {} ({e})", path.display())),
    }
}

/// Whether there is an app list in `dir`: the switch is on.
pub fn switched_on_in(dir: &Path) -> bool {
    dir.join(APPS_FILE).is_file()
}

/// Whether the launcher's switch is on ([`switched_on_in`] the [`agents_dir`]).
pub fn switched_on() -> bool {
    agents_dir().is_some_and(|d| switched_on_in(&d))
}

/// Whether the app list in `dir` names `app_id`: the person lets agents use that app. A list
/// that is missing or can't be read counts as no.
pub fn allowed_in(dir: &Path, app_id: &str) -> bool {
    matches!(load(dir), Ok(Some((apps, _))) if apps.iter().any(|a| a.id == app_id))
}

/// Whether the person lets agents use app `app_id` ([`allowed_in`] the [`agents_dir`]). Reads a
/// small file each time; an app that asks often should ask from a timer, not every frame.
pub fn allowed(app_id: &str) -> bool {
    agents_dir().is_some_and(|d| allowed_in(&d, app_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str) -> AppEntry {
        AppEntry {
            id: id.into(),
            name: known_name(id).unwrap_or(id).into(),
            command: PathBuf::from(abs(&format!("{id}-cli"))),
            args: vec!["mcp".into()],
        }
    }

    #[test]
    fn written_lists_read_back_and_removing_turns_agents_off() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("ArtCraft").join("mcp");
        assert!(!switched_on_in(&sub));
        assert!(remove(&sub).is_ok(), "nothing to remove is fine");
        let apps = [entry("photocraft"), entry("filmcraft")];
        write(&sub, &apps).unwrap();
        assert!(switched_on_in(&sub), "the folder is made");
        let (read, skipped) = load(&sub).unwrap().unwrap();
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(read, apps);
        write(&sub, &apps[..1]).unwrap();
        assert_eq!(
            load(&sub).unwrap().unwrap().0,
            apps[..1],
            "replaced, not appended"
        );
        let leftovers: Vec<_> = std::fs::read_dir(&sub)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(
            leftovers,
            [std::ffi::OsString::from(APPS_FILE)],
            "no temporary file is left"
        );
        remove(&sub).unwrap();
        assert!(!switched_on_in(&sub));
    }

    #[test]
    fn an_app_is_allowed_only_while_the_list_names_it() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!allowed_in(dir.path(), "photocraft"), "no list: off");
        write(dir.path(), &[entry("filmcraft")]).unwrap();
        assert!(!allowed_in(dir.path(), "photocraft"), "not in the list");
        assert!(allowed_in(dir.path(), "filmcraft"));
        write(dir.path(), &[entry("filmcraft"), entry("photocraft")]).unwrap();
        assert!(allowed_in(dir.path(), "photocraft"));
        std::fs::write(dir.path().join(APPS_FILE), b"not json").unwrap();
        assert!(
            !allowed_in(dir.path(), "photocraft"),
            "a broken list counts as off"
        );
        std::fs::remove_file(dir.path().join(APPS_FILE)).unwrap();
        std::fs::create_dir(dir.path().join(APPS_FILE)).unwrap();
        assert!(
            !allowed_in(dir.path(), "photocraft"),
            "a folder in its place counts as off"
        );
    }

    fn abs(name: &str) -> String {
        std::env::temp_dir()
            .join(name)
            .to_string_lossy()
            .replace('\\', "/")
    }

    #[test]
    fn parses_the_launcher_list() {
        let json = format!(
            r#"{{"version":1,"apps":[{{"id":"photocraft","name":"PhotoCraft","command":"{}","args":["mcp","--live"]}}]}}"#,
            abs("photocraft-cli")
        );
        let (apps, skipped) = parse(json.as_bytes()).unwrap();
        assert!(skipped.is_empty());
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].id, "photocraft");
        assert_eq!(apps[0].args, vec!["mcp", "--live"]);
    }

    #[test]
    fn missing_args_default_to_mcp_and_missing_name_to_the_known_one() {
        let json = format!(
            r#"{{"version":1,"apps":[{{"id":"wordcraft","command":"{}"}}]}}"#,
            abs("wordcraft-cli")
        );
        let (apps, _) = parse(json.as_bytes()).unwrap();
        assert_eq!(apps[0].args, vec!["mcp"]);
        assert_eq!(apps[0].name, "WordCraft");
    }

    #[test]
    fn bad_rows_are_left_out_with_a_reason_and_the_rest_kept() {
        let ok = abs("gridcraft-cli");
        let json = format!(
            r#"{{"version":1,"apps":[
                {{"id":"Bad Id","command":"{ok}"}},
                {{"id":"relcraft","command":"relative/cli"}},
                {{"id":"gridcraft","command":"{ok}"}},
                {{"id":"gridcraft","command":"{ok}"}}]}}"#
        );
        let (apps, skipped) = parse(json.as_bytes()).unwrap();
        assert_eq!(apps.len(), 1);
        assert_eq!(skipped.len(), 3, "{skipped:?}");
    }

    #[test]
    fn another_version_or_garbage_is_an_error() {
        assert!(parse(br#"{"version":2,"apps":[]}"#).is_err());
        assert!(parse(b"\xff\xfe").is_err());
        assert!(parse(b"[]").is_err());
    }

    #[test]
    fn the_list_is_capped() {
        let ok = abs("x-cli");
        let rows: Vec<String> = (0..MAX_APPS + 5)
            .map(|i| format!(r#"{{"id":"app{i}","command":"{ok}"}}"#))
            .collect();
        let json = format!(r#"{{"version":1,"apps":[{}]}}"#, rows.join(","));
        let (apps, skipped) = parse(json.as_bytes()).unwrap();
        assert_eq!(apps.len(), MAX_APPS);
        assert_eq!(skipped.len(), 1);
    }

    #[test]
    fn ids_are_checked() {
        assert!(valid_id("craft-anything"));
        assert!(!valid_id(""));
        assert!(!valid_id("PhotoCraft"));
        assert!(!valid_id("../x"));
        assert!(!valid_id(&"a".repeat(MAX_ID_LEN + 1)));
    }

    #[test]
    fn no_file_is_none_and_a_folder_in_its_place_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(dir.path()).unwrap().is_none());
        std::fs::create_dir(dir.path().join(APPS_FILE)).unwrap();
        assert!(load(dir.path()).is_err());
    }
}
