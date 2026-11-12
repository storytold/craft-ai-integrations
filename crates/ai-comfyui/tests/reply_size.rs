//! A ComfyUI reply's cost once parsed is bounded, not only its bytes. A reply of many tiny
//! objects costs about a hundred times its size as a plain `serde_json::Value`; the readers
//! refuse it within their budget. Scripted localhost server only.

#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;

use craft_ai_comfyui::{Binding, ComfyConfig, ComfyProvider, Workflow};
use craft_ai_core::{ErrorKind, Provider};
use serde_json::json;
use support::{Reply, Server};

/// An object holding `n` tiny objects: about 7 bytes each on the wire, about 800 once parsed.
fn tiny_objects(n: usize) -> String {
    format!(r#"{{"extra":[{}]}}"#, vec![r#"{"":0}"#; n].join(","))
}

fn installed() -> String {
    json!({
        "CLIPTextEncode":{"input":{"required":{"text":["STRING",{}]}}},
        "SaveOutput":{"input":{"required":{"data":["IMAGE",{}]}}}
    })
    .to_string()
}

fn provider(s: &Server) -> ComfyProvider {
    let workflow = Workflow::import(
        "local-workflow",
        "image.generate",
        json!({
            "1":{"class_type":"CLIPTextEncode","inputs":{"text":"old"}},
            "2":{"class_type":"SaveOutput","inputs":{"data":["1",0]}}
        }),
        Binding::new("1", "CLIPTextEncode", "text"),
        "2",
    )
    .unwrap();
    ComfyProvider::new(ComfyConfig::new(s.url.trim_end_matches("/v1")).with_workflow(workflow))
        .unwrap()
}

fn models_error(stats: String, nodes: String) -> craft_ai_core::AiError {
    let s = Server::start(move |r| match r.path.as_str() {
        "/system_stats" => Reply::Json(200, stats.clone()),
        "/object_info" => Reply::Json(200, nodes.clone()),
        _ => Reply::Json(404, "{}".into()),
    });
    provider(&s).models().unwrap_err()
}

#[test]
fn a_small_reply_of_tiny_objects_is_refused() {
    // Just under the 1 MiB byte cap: some 110 MiB as a plain `Value`.
    let e = models_error(tiny_objects(140_000), installed());
    assert_eq!(e.kind, ErrorKind::Provider);
    assert!(e.message.contains("too large"), "{}", e.message);
}

#[test]
fn a_node_list_of_tiny_objects_is_refused() {
    // The node list has the largest budget (a real one with many add-ons is many MiB of small
    // objects), so it takes about 3 MiB of tiny objects, some 360 MiB as a plain `Value`.
    let e = models_error("{}".into(), tiny_objects(430_000));
    assert_eq!(e.kind, ErrorKind::Provider);
    assert!(e.message.contains("too large"), "{}", e.message);
}

#[test]
fn a_real_node_list_still_reads() {
    let s = Server::start(|r| match r.path.as_str() {
        "/system_stats" => Reply::Json(200, "{}".into()),
        "/object_info" => Reply::Json(200, installed()),
        _ => Reply::Json(404, "{}".into()),
    });
    assert_eq!(provider(&s).models().unwrap().len(), 1);
}
