//! Which Craft apps the gateway can reach, and how to start each one's MCP server.
//!
//! The ArtCraft Launcher knows what is installed, so it writes [`APPS_FILE`] into the gateway's
//! folder ([`mcp_dir`]) when a person turns agents on:
//!
//! ```json
//! { "version": 1, "apps": [
//!   { "id": "photocraft", "name": "PhotoCraft",
//!     "command": "C:/Program Files/PhotoCraft/photocraft-cli.exe", "args": ["mcp"] } ] }
//! ```
//!
//! Without that file (a developer build, a CLI installed by hand) the gateway looks for each known
//! app's `<id>-cli` on `PATH` and starts it as `<id>-cli mcp`, the form every Craft CLI accepts.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The app list the launcher writes into [`mcp_dir`].
pub const APPS_FILE: &str = "apps.json";
/// Overrides [`mcp_dir`] (tests, portable installs).
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

/// The Craft apps that ship an MCP server through `<id>-cli mcp`, with their display names. Used
/// only when the launcher has not written an app list.
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

/// One app the gateway can start.
#[derive(Clone, Debug, PartialEq, Eq)]
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

/// The gateway's folder: [`DIR_ENV`], otherwise `ArtCraft/mcp` beside the AI layer's `ArtCraft/ai`
/// in the per-user local configuration folder.
pub fn mcp_dir() -> Option<PathBuf> {
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
            "{APPS_FILE} has version {}, this gateway reads version {APPS_VERSION}",
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

/// Each known app whose `<id>-cli` is found in one of `path_dirs`, started as `<id>-cli mcp`.
pub fn from_path_dirs(path_dirs: &[PathBuf]) -> Vec<AppEntry> {
    KNOWN_APPS
        .iter()
        .filter_map(|(id, name)| {
            let file = format!("{id}-cli{}", std::env::consts::EXE_SUFFIX);
            path_dirs
                .iter()
                .map(|d| d.join(&file))
                .find(|p| p.is_file())
                .map(|command| AppEntry {
                    id: (*id).to_string(),
                    name: (*name).to_string(),
                    command,
                    args: vec!["mcp".to_string()],
                })
        })
        .collect()
}

/// The apps to offer: the launcher's list when there is one, otherwise the known apps on `PATH`.
/// The second value says what was left out and why.
pub fn discover() -> Found {
    let mut notes = Vec::new();
    if let Some(dir) = mcp_dir() {
        match load(&dir) {
            Ok(Some(found)) => return found,
            Ok(None) => {}
            Err(e) => notes.push(e),
        }
    }
    let path_dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    (from_path_dirs(&path_dirs), notes)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn finds_known_clis_on_path() {
        let dir = tempfile::tempdir().unwrap();
        let file = format!("pdfcraft-cli{}", std::env::consts::EXE_SUFFIX);
        std::fs::write(dir.path().join(&file), b"").unwrap();
        let apps = from_path_dirs(&[dir.path().to_path_buf()]);
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].id, "pdfcraft");
        assert_eq!(apps[0].name, "PdfCraft");
        assert_eq!(apps[0].args, vec!["mcp"]);
    }
}
