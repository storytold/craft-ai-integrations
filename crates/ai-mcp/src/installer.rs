//! Installs the Craft AI MCP bridge into Bionic.
//!
//! Two pieces:
//!
//! 1. **MCP server registration** — merges this bridge's entry into Bionic's `ng-mcp.json`
//!    (`servers[]`). Bionic exposes no dedicated MCP-management tool, so this is a direct file
//!    write; it is idempotent (re-running updates the same entry in place) and never touches any
//!    other server.
//! 2. **Per-tool skills** — generates one `SKILL.md` folder per built-in feature. These are written
//!    to a directory *you* control (e.g. the scratchpad) so they can be installed through Bionic's
//!    own `skill.install` tool, which validates them and asks for approval. Do not move them into
//!    the global skills folder by hand — see the skill-management guidance.

use std::fs;
use std::path::{Path, PathBuf};

use craft_ai_core::InputRole;
use craft_ai_features::Feature;
use serde_json::{Map, Value};

use tempfile::NamedTempFile;

/// Stable identifier for this server in Bionic's `ng-mcp.json`. Deterministic so repeated installs
/// update the same entry instead of stacking duplicates (UUID per Bionic's schema).
pub const SERVER_ID: &str = "c55a52b4-6eca-44ac-926c-f67cceac9f1f";
/// Human-readable name shown in Bionic's MCP list.
pub const SERVER_NAME: &str = "Craft AI";
/// One-line description shown in Bionic's MCP list.
pub const SERVER_DESCRIPTION: &str = "Craft AI image and video tools exposed as MCP.";

/// The URL the server listens on for a given port (localhost only — trusted local host).
fn server_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/mcp")
}

/// One entry in Bionic's `ng-mcp.json` `servers` array for this bridge.
pub fn mcp_server_entry(port: u16) -> Value {
    // Bionic validates ng-mcp.json against a zod schema that requires `headers` and `auth` on every
    // streamableHttp connection; omitting either rejects the whole file (other servers vanish too).
    let mut auth = Map::new();
    auth.insert("type".into(), Value::String("none".into()));

    let mut connection = Map::new();
    connection.insert("type".into(), Value::String("streamableHttp".into()));
    connection.insert("url".into(), Value::String(server_url(port)));
    connection.insert("headers".into(), Value::Object(Map::new()));
    connection.insert("auth".into(), Value::Object(auth));

    let mut server = Map::new();
    server.insert("id".into(), Value::String(SERVER_ID.into()));
    server.insert("name".into(), Value::String(SERVER_NAME.into()));
    server.insert(
        "description".into(),
        Value::String(SERVER_DESCRIPTION.into()),
    );
    server.insert("enabled".into(), Value::Bool(true));
    server.insert("connection".into(), Value::Object(connection));

    Value::Object(server)
}

/// Merge this bridge's entry into an existing `ng-mcp.json` file (creating it if absent).
/// Other servers are preserved. Idempotent: re-running updates the same entry in place and never
/// duplicates it. If there is no `servers` list yet, one is created. A missing file starts fresh;
/// any other read failure, invalid JSON, a non-object root, or a non-array `servers` value returns
/// an error and leaves the file unchanged. The final write is atomic (temp file + rename), so Bionic
/// never observes a half-written file.
pub fn install_mcp_server(path: &Path, port: u16) -> Result<(), String> {
    let raw = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "{}".to_string(), // file missing -> start fresh
        Err(e) => return Err(format!("could not read {}: {e}", path.display())),
    };
    let mut doc: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => return Err(format!("existing ng-mcp.json is not valid JSON: {e}")),
    };
    if !doc.is_object() {
        return Err("ng-mcp.json root must be an object".into());
    }

    // Take the existing servers out as a Vec, modify it, and put it back. This avoids fighting
    // serde_json's immutable-by-default API for in-place array mutation. If there is no servers
    // list yet (fresh file), start from an empty one.
    let mut servers: Vec<Value> = match doc.get("servers") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => a.clone(),
        Some(_) => return Err("existing ng-mcp.json `servers` must be an array".into()),
    };

    // Replace an existing entry with the same id, or append a new one.
    if let Some(pos) = servers
        .iter()
        .position(|s| s.get("id").and_then(|i| i.as_str()) == Some(SERVER_ID))
    {
        servers[pos] = mcp_server_entry(port);
    } else {
        servers.push(mcp_server_entry(port));
    }

    doc["servers"] = Value::Array(servers);

    let out = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    // Atomic replace: serialize to a unique temp file in the same directory, then rename over
    // the target. The original stays intact until the rename succeeds, so an interrupted write never
    // leaves a half-written ng-mcp.json (Bionic would reject every server in it). A unique name
    // avoids collisions if two installs race on the same file; the temp file is cleaned up on any
    // error path and is gone after the rename. Preserve the target's permissions so any process
    // that created it keeps access after the swap.
    let tmp = NamedTempFile::new_in(path.parent().unwrap_or_else(|| Path::new(".")))
        .map_err(|e| format!("could not create temp file in {}: {e}", path.display()))?;
    fs::write(tmp.path(), out.as_bytes()).map_err(|e| format!("could not write temp file: {e}"))?;
    // Preserve the target's permissions on Unix (no-op elsewhere; NTFS ignores these bits).
    #[cfg(unix)]
    if let Ok(meta) = fs::metadata(path) {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode();
        fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(mode))
            .map_err(|e| format!("could not set permissions on temp file: {e}"))?;
    }
    // Atomic replace: rename the unique temp file over the target. The original stays intact until
    // this succeeds, so Bionic never observes a half-written ng-mcp.json. After the rename the
    // temp path no longer exists, so dropping `tmp` is a no-op (its remove() targets a gone path).
    fs::rename(tmp.path(), path)
        .map_err(|e| format!("could not replace {}: {e}", path.display()))?;
    Ok(())
}

/// Human-readable list of the JSON argument fields a feature accepts (mirrors `input_schema`).
fn input_fields(inputs: &[InputRole]) -> String {
    let mut lines = Vec::new();
    for role in inputs {
        match role {
            InputRole::Prompt => lines.push("- `prompt` — a text description of what you want."),
            InputRole::Source => lines.push(
                "- `source_path` or `source_base64` — the input image/video file path, or its base64 data.",
            ),
            InputRole::Mask => lines.push(
                "- `mask_path` or `mask_base64` — a selection/mask for the region to edit (path or base64).",
            ),
            // Reference / Context / System are app-specific (clicks, etc.) and not expressible as
            // plain JSON args in v1; the router rejects them with a clear message.
            _ => {}
        }
    }
    if lines.is_empty() {
        "No JSON arguments.".to_string()
    } else {
        lines.join("\n")
    }
}

/// Generate the SKILL.md content for one feature. Returns `(folder_name, content)`; the folder name
/// equals the front-matter `name` (required by Bionic) and is kebab-case. The `craft-ai-` prefix
/// keeps these distinct from Bionic's own skills.
pub fn skill_for(feature: &Feature) -> (String, String) {
    let name = format!("craft-ai-{0}", feature.id);
    let fields = input_fields(&feature.inputs);

    let content = format!(
        "---\nname: {name}\ndisplay-name: {title}\ndescription: {summary}\nuser-invocable: true\n---\n\nUse the `craft-ai-mcp` MCP server's `{id}` tool to accomplish this.\n\n**Inputs (JSON arguments):**\n{fields}\n\nIf it reports that a model or connection is missing, tell the user which one is needed and ask them to set it up in Craft AI before retrying.\n",
        title = feature.title,
        summary = feature.summary,
        id = feature.id,
    );

    (name, content)
}

/// Write every skill folder under `base_dir`. Returns the list of created folder paths. The caller
/// is responsible for installing them through Bionic's `skill.install` tool.
pub fn write_skills(base_dir: &Path, features: &[Feature]) -> Result<Vec<PathBuf>, String> {
    let mut written = Vec::new();
    for f in features {
        let (name, content) = skill_for(f);
        let dir = base_dir.join(&name);
        fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
        let path = dir.join("SKILL.md");
        fs::write(&path, content)
            .map_err(|e| format!("could not write {}: {e}", path.display()))?;
        written.push(dir);
    }
    Ok(written)
}

/// True when `p` is Bionic's skills folder (`.lmstudio/skills`) or anywhere inside it, whether or
/// not it exists yet. Both the path as written (`..` counts as leaving a folder) and the path with
/// links resolved are checked: a `.lmstudio/skills` that is a link to another folder is still
/// Bionic's, and a link from elsewhere into it is too.
pub fn inside_bionic_skills(p: &Path) -> bool {
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(d) => d.join(p),
            Err(_) => return true, // can't tell where it is: refuse
        }
    };
    names_bionic_skills(&lexical_parts(&abs)) || resolves_into_bionic_skills(&abs)
}

/// The lower-cased parts of `p` with `.` dropped and `..` taking one part off.
fn lexical_parts(p: &Path) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    for c in p.components() {
        let name = c.as_os_str().to_string_lossy().to_lowercase();
        match name.as_str() {
            "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(name),
        }
    }
    parts
}

fn names_bionic_skills(parts: &[String]) -> bool {
    parts
        .windows(2)
        .any(|w| matches!(w, [a, b] if a == ".lmstudio" && b == "skills"))
}

/// [`inside_bionic_skills`] on the path with links resolved in the part that exists.
fn resolves_into_bionic_skills(abs: &Path) -> bool {
    // Resolve the longest part that exists, then add the rest.
    let mut existing = abs;
    let mut rest = Vec::new();
    let resolved = loop {
        if let Ok(c) = existing.canonicalize() {
            break c;
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                existing = parent;
            }
            _ => break existing.to_path_buf(),
        }
    };
    let mut parts: Vec<String> = resolved
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    for name in rest.iter().rev() {
        let name = name.to_string_lossy().to_lowercase();
        match name.as_str() {
            "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(name),
        }
    }
    names_bionic_skills(&parts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    #[cfg(unix)]
    fn a_skills_folder_that_is_a_link_is_still_bionics() {
        let home = tempdir().unwrap();
        let target = home.path().join("elsewhere");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::create_dir_all(home.path().join(".lmstudio")).unwrap();
        let skills = home.path().join(".lmstudio").join("skills");
        std::os::unix::fs::symlink(&target, &skills).unwrap();
        assert!(
            inside_bionic_skills(&skills),
            "named as Bionic's, even though it resolves elsewhere"
        );
        assert!(inside_bionic_skills(&skills.join("x")));
        let link_in = home.path().join("into-skills");
        std::os::unix::fs::symlink(&skills, &link_in).unwrap();
        assert!(
            !inside_bionic_skills(&target),
            "the target folder itself is not named as Bionic's"
        );
    }

    #[test]
    fn bionic_skills_and_anything_inside_are_recognised_existing_or_not() {
        let home = tempdir().unwrap();
        let skills = home.path().join(".lmstudio").join("skills");
        assert!(inside_bionic_skills(&skills), "missing folder");
        assert!(
            inside_bionic_skills(&skills.join("craft-ai-x")),
            "missing nested"
        );
        std::fs::create_dir_all(skills.join("existing")).unwrap();
        assert!(inside_bionic_skills(&skills), "existing folder");
        assert!(
            inside_bionic_skills(&skills.join("existing").join("deeper")),
            "nested"
        );
        assert!(
            inside_bionic_skills(&home.path().join(".LMStudio").join("Skills")),
            "case"
        );
        assert!(!inside_bionic_skills(&home.path().join("staging")));
        assert!(
            !inside_bionic_skills(&skills.join("..").join("other")),
            "`..` leaves it"
        );
        assert!(!inside_bionic_skills(
            &home.path().join(".lmstudio").join("skills-staging")
        ));
    }

    #[test]
    fn mcp_server_entry_has_expected_fields() {
        let entry = mcp_server_entry(12345);
        assert_eq!(entry["id"], SERVER_ID);
        assert_eq!(entry["name"], "Craft AI");
        assert!(entry["enabled"].as_bool().unwrap());
        let conn = &entry["connection"];
        assert_eq!(conn["type"], "streamableHttp");
        assert_eq!(conn["url"], "http://127.0.0.1:12345/mcp");
        // Bionic's schema requires these on streamableHttp connections; missing either rejects the
        // whole ng-mcp.json file.
        assert!(conn["headers"].is_object());
        assert_eq!(conn["auth"]["type"], "none");
    }

    #[test]
    fn install_mcp_server_preserves_other_servers_and_is_idempotent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("ng-mcp.json");
        // Seed with an unrelated server.
        let seed = r#"{"servers":[{"id":"github","name":"GitHub","enabled":true,"connection":{"type":"streamableHttp","url":"https://example.com/mcp/"}}]}"#;
        fs::write(&path, seed).unwrap();

        install_mcp_server(&path, 12345).expect("install failed");

        let doc: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let servers = doc["servers"].as_array().unwrap();
        assert_eq!(servers.len(), 2); // github + craft-ai-mcp
        let ids: Vec<String> = servers
            .iter()
            .map(|s| s["id"].as_str().unwrap().to_string())
            .collect();
        assert!(ids.contains(&"github".to_string()));
        assert!(ids.contains(&SERVER_ID.to_string()));

        // Re-run with a different port: still 2 (no duplicate), url updated in place.
        install_mcp_server(&path, 9999).expect("reinstall failed");
        let doc2: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc2["servers"].as_array().unwrap().len(), 2);
        let craft = doc2["servers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == SERVER_ID)
            .expect("craft entry missing");
        assert_eq!(craft["connection"]["url"], "http://127.0.0.1:9999/mcp");
    }

    #[test]
    fn install_mcp_server_creates_missing_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("ng-mcp.json");
        assert!(!path.exists());
        install_mcp_server(&path, 12345).expect("install failed");
        assert!(path.exists());
        let doc: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc["servers"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn install_mcp_server_refuses_invalid_existing_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("ng-mcp.json");
        fs::write(&path, "not json at all").unwrap();
        let err = install_mcp_server(&path, 12345).expect_err("should refuse invalid JSON");
        assert!(err.contains("not valid JSON"));
    }

    #[test]
    fn install_mcp_server_refuses_non_array_servers() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("ng-mcp.json");
        // A `servers` value that is an object, not an array.
        let bad = "{\"servers\":{\"id\":\"github\",\"name\":\"GitHub\"}}";
        fs::write(&path, bad).unwrap();
        let err = install_mcp_server(&path, 12345)
            .expect_err("should refuse a non-array `servers` value");
        assert!(err.contains("`servers` must be an array"));

        // The file must be left unchanged.
        assert_eq!(fs::read_to_string(&path).unwrap(), bad);
    }

    #[test]
    fn skill_for_produces_valid_frontmatter_and_fields() {
        // generate-image is the 7th built-in feature (index 6).
        let features = craft_ai_features::builtin();
        let f = &features[6];
        assert_eq!(f.id, "generate-image");
        let (name, content) = skill_for(f);
        assert_eq!(name, "craft-ai-generate-image");
        // Front matter fields.
        assert!(content.starts_with("---\n"));
        assert!(content.contains("name: craft-ai-generate-image\n"));
        assert!(content.contains("display-name: Generate images\n"));
        assert!(content.contains("description: Make a picture from a description.\n"));
        assert!(content.contains("user-invocable: true\n---\n"));
        // Body references the MCP server/tool and lists the prompt input.
        assert!(content.contains("`craft-ai-mcp`"));
        assert!(content.contains("`generate-image`"));
        assert!(content.contains("- `prompt`"));
    }

    #[test]
    fn skill_for_lists_source_and_mask_fields_when_declared() {
        // remove-object (index 2) declares Source + Mask.
        let features = craft_ai_features::builtin();
        let f = &features[2];
        assert_eq!(f.id, "remove-object");
        let (_, content) = skill_for(f);
        assert!(content.contains("- `source_path`"));
        assert!(content.contains("- `mask_path`"));
    }

    #[test]
    fn write_skills_creates_folders_with_skill_md() {
        let dir = tempdir().unwrap();
        let features = craft_ai_features::builtin();
        let written = write_skills(dir.path(), &features).expect("write failed");
        assert_eq!(written.len(), features.len());
        for f in &features {
            let (name, _) = skill_for(f);
            let skill_md = dir.path().join(name).join("SKILL.md");
            assert!(skill_md.exists(), "missing {}", skill_md.display());
        }
    }
}
