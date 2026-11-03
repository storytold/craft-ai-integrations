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
/// duplicates it. If there is no `servers` list yet, one is created.
pub fn install_mcp_server(path: &Path, port: u16) -> Result<(), String> {
    let raw = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => "{}".to_string(), // file missing -> start fresh
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
    let mut servers: Vec<Value> = doc
        .get("servers")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().cloned().collect())
        .unwrap_or_default();

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
    fs::write(path, format!("{out}\n"))
        .map_err(|e| format!("could not write {}: {e}", path.display()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

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
