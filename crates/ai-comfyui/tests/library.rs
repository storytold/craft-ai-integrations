use craft_ai_comfyui::{Binding, WorkflowLibrary, WorkflowSpec};
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

fn graph(prompt_text: &str) -> serde_json::Value {
    json!({
        "1":{"class_type":"CLIPTextEncode","inputs":{"text":prompt_text}},
        "2":{"class_type":"SaveImage","inputs":{"images":["1",0]}}
    })
}

fn spec(id: &str) -> WorkflowSpec {
    WorkflowSpec::import(
        id,
        "image.generate",
        graph("original"),
        Some(Binding::new("1", "CLIPTextEncode", "text")),
        Some("2".to_owned()),
    )
    .unwrap()
}

fn library() -> (WorkflowLibrary, TempDir) {
    let dir = TempDir::new().unwrap();
    let lib = WorkflowLibrary::at(dir.path().join("workflows")).unwrap();
    (lib, dir)
}

fn path_in(dir: &TempDir, name: &str) -> PathBuf {
    dir.path().join("workflows").join(name)
}

#[test]
fn inpaint_bindings_survive_the_shared_workflow_library() {
    let (lib, _dir) = library();
    let bytes = json!({
        "id":"repair", "capability":"image.inpaint",
        "graph":{
            "1":{"class_type":"CLIPTextEncode","inputs":{"text":"old"}},
            "2":{"class_type":"LoadImage","inputs":{"image":"old-source.png"}},
            "3":{"class_type":"LoadImageMask","inputs":{"image":"old-mask.png"}},
            "4":{"class_type":"SaveImage","inputs":{"images":["5",0]}},
            "5":{"class_type":"Inpaint","inputs":{"image":["2",0],"mask":["3",0],"prompt":["1",0]}}
        },
        "prompt":{"node":"1","class_type":"CLIPTextEncode","input":"text"},
        "source":{"node":"2","class_type":"LoadImage","input":"image"},
        "mask":{"node":"3","class_type":"LoadImageMask","input":"image"},
        "output_node":"4"
    })
    .to_string();
    let spec = WorkflowSpec::parse(bytes.as_bytes(), None).unwrap();
    lib.save(&spec).unwrap();
    let loaded = lib.load().unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].capability().as_str(), "image.inpaint");
    assert!(
        loaded[0].info().capabilities[0]
            .requires
            .contains(&craft_ai_core::model::InputRole::Mask)
    );
}

#[test]
fn a_json_file_lands_in_the_library_and_loads_back() {
    let (lib, dir) = library();
    let file = dir.path().join("kite.json");
    fs::write(
        &file,
        json!({
            "id":"kite-image",
            "capability":"image.generate",
            "graph":graph("original"),
            "prompt":{"node":"1","class_type":"CLIPTextEncode","input":"text"},
            "output_node":"2"
        })
        .to_string(),
    )
    .unwrap();
    lib.import_file(&file).unwrap();
    let workflows = lib.load().unwrap();
    assert_eq!(workflows.len(), 1);
    assert_eq!(workflows[0].id().as_str(), "kite-image");
    let rendered = workflows[0]
        .render(
            &craft_ai_core::Request::new(
                craft_ai_core::capability::CapabilityId::parse("image.generate").unwrap(),
            )
            .with_input(
                craft_ai_core::model::InputRole::Prompt,
                craft_ai_core::payload::Payload::text("a kite"),
            ),
        )
        .unwrap();
    assert_eq!(rendered["1"]["inputs"]["text"], "a kite");
}

#[test]
fn a_bare_api_graph_gets_defaults_and_a_derived_id() {
    let (lib, dir) = library();
    let file = dir.path().join("bare.json");
    fs::write(&file, graph("bare template").to_string()).unwrap();
    let stored = lib.import_file(&file).unwrap();
    assert_eq!(stored, "bare");
    let workflows = lib.load().unwrap();
    assert_eq!(workflows.len(), 1);
    assert_eq!(workflows[0].id().as_str(), "bare");
    assert_eq!(workflows[0].capability().as_str(), "image.generate");
    // Default binding: the only text-input node found.
    let rendered = workflows[0]
        .render(
            &craft_ai_core::Request::new(
                craft_ai_core::capability::CapabilityId::parse("image.generate").unwrap(),
            )
            .with_input(
                craft_ai_core::model::InputRole::Prompt,
                craft_ai_core::payload::Payload::text("a kite"),
            ),
        )
        .unwrap();
    assert_eq!(rendered["1"]["inputs"]["text"], "a kite");
}

#[test]
fn a_ui_format_export_is_refused_with_a_clear_message() {
    let (lib, dir) = library();
    let file = dir.path().join("ui.json");
    fs::write(
        &file,
        json!({"nodes":[{"id":1,"type":"CLIPTextEncode"}],"links":[[1,1,0,2,0,"*"]]}).to_string(),
    )
    .unwrap();
    let err = lib.import_file(&file).unwrap_err();
    assert!(err.message.contains("API Format"), "message was: {err}");
    assert!(lib.load().unwrap().is_empty());
}

#[test]
fn a_second_import_with_the_same_id_replaces_the_first() {
    let (lib, dir) = library();
    let first = dir.path().join("one.json");
    fs::write(
        &first,
        json!({
            "id":"same","capability":"image.generate","graph":graph("old"),
            "prompt":{"node":"1","class_type":"CLIPTextEncode","input":"text"},
            "output_node":"2"
        })
        .to_string(),
    )
    .unwrap();
    let second = dir.path().join("two.json");
    fs::write(
        &second,
        json!({
            "id":"same","capability":"video.generate","graph":graph("new"),
            "prompt":{"node":"1","class_type":"CLIPTextEncode","input":"text"},
            "output_node":"2"
        })
        .to_string(),
    )
    .unwrap();
    lib.import_file(&first).unwrap();
    lib.import_file(&second).unwrap();
    let workflows = lib.load().unwrap();
    assert_eq!(workflows.len(), 1);
    assert_eq!(workflows[0].capability().as_str(), "video.generate");
}

#[test]
fn remove_deletes_the_file_and_only_that_file() {
    let (lib, _dir) = library();
    lib.save(&spec("keep")).unwrap();
    lib.save(&spec("drop")).unwrap();
    lib.remove("keep").unwrap();
    let ids: Vec<_> = lib
        .load()
        .unwrap()
        .iter()
        .map(|w| w.id().as_str().to_owned())
        .collect();
    assert_eq!(ids, ["drop"]);
    // What remains in the directory is the one file that was kept, under its derived name.
    let mut remaining: Vec<String> = fs::read_dir(lib.dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    remaining.sort();
    assert_eq!(remaining.len(), 1);
    assert!(remaining[0].ends_with(".json"));
}

#[test]
fn the_library_directory_is_created_on_demand() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("nested").join("workflows");
    let lib = WorkflowLibrary::at(&target).unwrap();
    lib.save(&spec("fresh")).unwrap();
    // The file lands under a derived name, so the directory is checked, not one path.
    let mut names: Vec<String> = fs::read_dir(&target)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names.len(), 1);
    assert!(names[0].ends_with(".json"));
    assert_eq!(lib.load().unwrap().len(), 1);
}

#[test]
fn a_damaged_library_file_is_reported_and_skipped_not_fatal() {
    let (lib, dir) = library();
    lib.save(&spec("good")).unwrap();
    fs::write(path_in(&dir, "bad.json"), "{ not json").unwrap();
    let workflows = lib.load().unwrap();
    assert_eq!(workflows.len(), 1);
    assert_eq!(workflows[0].id().as_str(), "good");
}

#[test]
fn oversized_files_are_refused_before_reading_the_rest() {
    let (lib, dir) = library();
    let file = dir.path().join("huge.json");
    let mut big = String::from("{\"id\":\"big\",\"capability\":\"image.generate\",\"graph\":");
    big.push_str(&"0".repeat(3 * 1024 * 1024));
    big.push('}');
    fs::write(&file, big).unwrap();
    assert!(lib.import_file(&file).is_err());
}

#[test]
fn workflow_specs_round_trip_through_the_library_file() {
    let (lib, _dir) = library();
    let original = spec("round");
    lib.save(&original).unwrap();
    let loaded = lib.load().unwrap();
    assert_eq!(loaded.len(), 1);
    // The graph and binding survive the round trip, not just the id.
    assert_eq!(
        loaded[0]
            .render(
                &craft_ai_core::Request::new(
                    craft_ai_core::capability::CapabilityId::parse("image.generate").unwrap(),
                )
                .with_input(
                    craft_ai_core::model::InputRole::Prompt,
                    craft_ai_core::payload::Payload::text("checked"),
                )
            )
            .unwrap()["1"]["inputs"]["text"],
        "checked"
    );
}

#[test]
fn hostile_ids_cannot_escape_the_library_directory() {
    let (lib, dir) = library();
    // A file just outside the library directory that a traversal id must not touch.
    let canary = dir.path().join("connections.json");
    fs::write(&canary, "keep").unwrap();
    for id in ["../connections", "C:/evil", "a/b", "."] {
        lib.save(&spec(id)).unwrap();
    }
    assert_eq!(
        fs::read_to_string(&canary).unwrap(),
        "keep",
        "an id escaped the library directory"
    );
    // Everything written stays inside the library, under plain file names.
    for entry in fs::read_dir(lib.dir()).unwrap() {
        let file_name = entry.unwrap().file_name();
        let name = file_name.to_string_lossy();
        assert!(!name.contains('/'), "name escaped into a folder: {name}");
        assert!(
            !name.starts_with('.'),
            "name escaped into a dotfile: {name}"
        );
    }
    // The raw ids survive inside the files: the graph still loads under each of them.
    let ids: Vec<String> = lib
        .load()
        .unwrap()
        .iter()
        .map(|w| w.id().as_str().to_owned())
        .collect();
    for id in ["../connections", "C:/evil", "a/b", "."] {
        assert!(ids.contains(&id.to_owned()), "{id} did not survive");
    }
}

#[test]
fn ids_that_clean_alike_stay_distinct() {
    let (lib, _dir) = library();
    // `a/b` and `a.b` both clean to `a_b`; the file name must still tell them apart.
    lib.save(&spec("a/b")).unwrap();
    lib.save(&spec("a.b")).unwrap();
    let ids: Vec<String> = lib
        .load()
        .unwrap()
        .iter()
        .map(|w| w.id().as_str().to_owned())
        .collect();
    assert!(ids.contains(&"a/b".to_owned()), "a/b was lost");
    assert!(ids.contains(&"a.b".to_owned()), "a.b was lost");
    assert_eq!(ids.len(), 2);
}

#[test]
fn remove_only_reaches_files_inside_the_library() {
    let (lib, dir) = library();
    // A file just outside the library directory that a traversal id must not delete.
    let canary = dir.path().join("evil.json");
    fs::write(&canary, "keep").unwrap();
    lib.remove("../evil").unwrap();
    assert_eq!(
        fs::read_to_string(&canary).unwrap(),
        "keep",
        "remove reached outside the library directory"
    );
    // A saved workflow really is removed by its raw id.
    lib.save(&spec("gone")).unwrap();
    lib.remove("gone").unwrap();
    assert!(lib.load().unwrap().is_empty());
}

#[test]
fn a_legacy_file_named_after_the_id_is_replaced_by_a_save() {
    let (lib, dir) = library();
    // A file from before ids stopped being file names: the id inside, the id as the name.
    let legacy = path_in(&dir, "legacy.json");
    fs::write(
        &legacy,
        json!({
            "id":"legacy",
            "capability":"image.generate",
            "graph":graph("original"),
            "prompt":{"node":"1","class_type":"CLIPTextEncode","input":"text"},
            "output_node":"2"
        })
        .to_string(),
    )
    .unwrap();
    lib.save(&spec("legacy")).unwrap();
    // One workflow per id: the legacy file must not survive as a second copy.
    let stored: Vec<String> = fs::read_dir(dir.path().join("workflows"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(stored.len(), 1, "save left more than one file: {stored:?}");
    assert!(!legacy.exists(), "the legacy file survived the save");
    assert_eq!(lib.load().unwrap().len(), 1);
}

#[test]
fn remove_reaches_a_legacy_file_named_after_the_id() {
    let (lib, dir) = library();
    let legacy = path_in(&dir, "legacy.json");
    fs::write(
        &legacy,
        json!({
            "id":"legacy",
            "capability":"image.generate",
            "graph":graph("original"),
            "prompt":{"node":"1","class_type":"CLIPTextEncode","input":"text"},
            "output_node":"2"
        })
        .to_string(),
    )
    .unwrap();
    assert_eq!(
        lib.load().unwrap().len(),
        1,
        "the legacy file does not load"
    );
    lib.remove("legacy").unwrap();
    assert!(!legacy.exists(), "remove missed the legacy file");
    assert!(lib.load().unwrap().is_empty());
}

#[test]
fn ids_that_share_a_cleaned_prefix_stay_distinct() {
    let (lib, _dir) = library();
    // Both ids clean to the same 40 characters; only the hash tells the files apart.
    let long = "x".repeat(45);
    let other = format!("{}y", "x".repeat(44));
    lib.save(&spec(&long)).unwrap();
    lib.save(&spec(&other)).unwrap();
    let ids: Vec<String> = lib
        .load()
        .unwrap()
        .iter()
        .map(|w| w.id().as_str().to_owned())
        .collect();
    assert!(ids.contains(&long), "the first long id was lost");
    assert!(ids.contains(&other), "the second long id was lost");
    assert_eq!(ids.len(), 2);
}

#[test]
fn non_json_files_are_not_workflows_even_when_they_hold_one() {
    let (lib, dir) = library();
    // A file the library never shows — load_specs only reads `json` files — but that
    // happens to contain a valid workflow, stored under the id reconcile would look for.
    let notes = path_in(&dir, "notes.txt");
    fs::write(
        &notes,
        json!({
            "id":"legacy",
            "capability":"image.generate",
            "graph":graph("original"),
            "prompt":{"node":"1","class_type":"CLIPTextEncode","input":"text"},
            "output_node":"2"
        })
        .to_string(),
    )
    .unwrap();
    assert!(
        lib.load().unwrap().is_empty(),
        "a non-json file must not load as a workflow"
    );
    lib.remove("legacy").unwrap();
    assert!(notes.exists(), "remove deleted a file it never showed");
    lib.save(&spec("legacy")).unwrap();
    assert!(notes.exists(), "save tidied away a file it never showed");
}

#[test]
fn save_refuses_when_the_directory_cannot_be_read() {
    let (lib, dir) = library();
    lib.save(&spec("early")).unwrap();
    fs::remove_dir_all(dir.path().join("workflows")).unwrap();
    // The scan fails before anything is written: the error names the scan, not the write,
    // and nothing recreates the directory — `at` created it, not `save`.
    let err = lib.save(&spec("later")).unwrap_err();
    assert!(
        err.message
            .contains("could not read the workflow directory"),
        "{err}"
    );
    assert!(
        !dir.path().join("workflows").exists(),
        "save recreated the directory it refused to scan"
    );
}

#[test]
fn remove_stays_quiet_when_the_directory_cannot_be_read() {
    let (lib, dir) = library();
    fs::remove_dir_all(dir.path().join("workflows")).unwrap();
    // Missing is fine: an unreadable directory is not an error, nothing is deleted
    // and remove keeps its best-effort contract.
    lib.remove("anything").unwrap();
}

// Linux only: the premise is a name the filesystem accepts but `to_str` refuses, and ext4 is
// byte-transparent while macOS (APFS) refuses to create a non-UTF-8 name at all (EILSEQ), so
// the scenario cannot be set up there. Windows is excluded by the byte-level OsString import.
#[cfg(target_os = "linux")]
#[test]
fn remove_reaches_a_file_whose_name_is_not_utf8() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let (lib, dir) = library();
    // A name a unix filesystem accepts but `to_str` refuses; the id inside decides what it is.
    let file = dir
        .path()
        .join("workflows")
        .join(OsString::from_vec(b"bad\xff.json".to_vec()));
    fs::write(
        &file,
        json!({
            "id":"legacy",
            "capability":"image.generate",
            "graph":graph("original"),
            "prompt":{"node":"1","class_type":"CLIPTextEncode","input":"text"},
            "output_node":"2"
        })
        .to_string(),
    )
    .unwrap();
    assert_eq!(
        lib.load().unwrap().len(),
        1,
        "the non-utf8-named file does not load"
    );
    lib.remove("legacy").unwrap();
    assert!(!file.exists(), "remove missed the non-utf8-named file");
}

#[test]
fn save_refuses_a_destination_storing_another_workflow() {
    let (lib, dir) = library();
    lib.save(&spec("a")).unwrap();
    // The id inside the destination file is hand-edited to another workflow's; the file name
    // stays derived from `a`, so writing to it would overwrite the other workflow.
    let stored: Vec<PathBuf> = fs::read_dir(dir.path().join("workflows"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    assert_eq!(stored.len(), 1);
    fs::write(
        &stored[0],
        json!({
            "id":"b",
            "capability":"image.generate",
            "graph":graph("other"),
            "prompt":{"node":"1","class_type":"CLIPTextEncode","input":"text"},
            "output_node":"2"
        })
        .to_string(),
    )
    .unwrap();
    assert_eq!(
        lib.load().unwrap().len(),
        1,
        "the hand-edited workflow does not load"
    );
    let err = lib.save(&spec("a")).unwrap_err();
    assert!(err.message.contains("another workflow"), "{}", err.message);
    // The refusal path writes nothing at all: no temporary file is left behind either.
    let entries = fs::read_dir(dir.path().join("workflows"))
        .unwrap()
        .flatten()
        .count();
    assert_eq!(entries, 1, "the refusal left {entries} entries behind");
    // The hand-edited workflow survives, and remove("a") does not reach it either.
    let ids: Vec<String> = lib
        .load()
        .unwrap()
        .iter()
        .map(|w| w.id().as_str().to_owned())
        .collect();
    assert_eq!(ids, ["b".to_owned()]);
    lib.remove("a").unwrap();
    assert_eq!(lib.load().unwrap().len(), 1);
}

#[test]
fn a_spec_with_an_unknown_capability_is_refused() {
    let err = WorkflowSpec::import(
        "weird",
        "mesh.sculpt",
        graph("x"),
        Some(Binding::new("1", "CLIPTextEncode", "text")),
        Some("2".to_owned()),
    )
    .unwrap_err();
    assert_eq!(err.kind, craft_ai_core::ErrorKind::Unsupported);
}
