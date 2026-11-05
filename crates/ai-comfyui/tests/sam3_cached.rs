//! Reproduce ComfyUI's fully cached history lacking output entries on a repeated graph.
mod sam3_support;
use craft_ai_comfyui::Sam3Session;
use sam3_support::mask;
use sam3_support::{Reply, Server};
use serde_json::json;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// Asking for the same grounding twice gives the sink a new cache key each time while the
/// source, model and inference nodes keep theirs, so the repeat still returns a mask.
#[test]
fn repeated_grounding_has_a_fresh_sink_without_invalidating_inference_nodes() {
    let seen = Arc::new(Mutex::new(HashSet::<String>::new()));
    let cached = Arc::new(Mutex::new(false));
    let server_mask = mask(&[255, 0]);
    let server = Server::start(move |r| match r.path.as_str() {
        "/object_info" => Reply::Json(
            200,
            json!({
                "LoadImage":{},"LoadSAM3Model":{},"SAM3InteractiveCollector":{},
                "SAM3Grounding":{},"MaskToImage":{},"PreviewImage":{}
            })
            .to_string(),
        ),
        "/upload/image" => Reply::Json(
            200,
            json!({"name":"source.png","type":"temp","subfolder":""}).to_string(),
        ),
        "/prompt" => {
            let graph = r.body_json()["prompt"].clone();
            if let Some(prefix) = graph
                .pointer("/5/inputs/filename_prefix")
                .and_then(|v| v.as_str())
            {
                *cached.lock().unwrap() = !seen.lock().unwrap().insert(prefix.to_owned());
            }
            Reply::Json(200, json!({"prompt_id":"cached-1"}).to_string())
        }
        "/history/cached-1" => {
            let outputs = if *cached.lock().unwrap() {
                json!({})
            } else {
                json!({"5":{"images":[{"filename":"mask.png","type":"output","subfolder":""}]}})
            };
            Reply::Json(
                200,
                json!({"cached-1":{"status":{"completed":true},"outputs":outputs}}).to_string(),
            )
        }
        "/view?filename=mask.png&type=output" => Reply::Raw(
            [
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    server_mask.len()
                )
                .into_bytes(),
                server_mask.clone(),
            ]
            .concat(),
        ),
        _ => Reply::Json(404, "{}".into()),
    });
    let mut session = Sam3Session::new(server.url.trim_end_matches("/v1")).unwrap();
    session.prepare(&mask(&[255, 255])).unwrap();
    let first = session.ground("eye").unwrap();
    let second = session.ground("eye").unwrap();
    assert_eq!(first, second);
    let graphs: Vec<_> = server
        .recorded()
        .into_iter()
        .filter(|r| r.path == "/prompt")
        .map(|r| r.body_json()["prompt"].clone())
        .collect();
    assert_eq!(graphs.len(), 3);
    for node in ["1", "2", "3", "4"] {
        assert_eq!(
            graphs[1][node], graphs[2][node],
            "only the output sink should change"
        );
    }
    assert_ne!(
        graphs[1]["5"]["inputs"]["filename_prefix"],
        graphs[2]["5"]["inputs"]["filename_prefix"]
    );
}
