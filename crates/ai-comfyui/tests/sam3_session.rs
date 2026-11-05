#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;
use craft_ai_comfyui::Sam3Session;
use serde_json::{Value, json};
use support::{Reply, Server};

fn png() -> Vec<u8> {
    flat_png(255)
}

fn flat_png(value: u8) -> Vec<u8> {
    let mut bytes = vec![];
    let mut encoder = png::Encoder::new(&mut bytes, 64, 64);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&vec![value; 4096])
        .unwrap();
    bytes
}

/// Activating the tool uploads the image once; hovering returns previews and a click returns a
/// mask of the exact source size.
#[test]
fn activation_prepares_once_then_hover_previews_and_click_returns_an_exact_mask() {
    let mask = png();
    let server_mask = mask.clone();
    let preview = vec![0xff, 0xd8, 0xff, 0xd9];
    let server_preview = preview.clone();
    let server = Server::start(move |r| {
        match r.path.as_str() {
        "/system_stats" => Reply::Json(200, json!({"system":{"comfyui_version":"0.22.3"}}).to_string()),
        "/object_info" => Reply::Json(200, json!({
            "LoadImage":{},"LoadSAM3Model":{},"SAM3InteractiveCollector":{},
            "MaskToImage":{},"PreviewImage":{}
        }).to_string()),
        "/upload/image" => Reply::Json(200,json!({"name":"source.png","type":"temp","subfolder":""}).to_string()),
        "/prompt" => Reply::Json(200,json!({"prompt_id":"abc-123"}).to_string()),
        "/history/abc-123" => Reply::Json(200,json!({"abc-123":{"status":{"completed":true},"outputs":{"5":{"images":[{"filename":"mask.png","type":"output","subfolder":""}]}}}}).to_string()),
        "/sam3/interactive_segment_one" => Reply::Json(200,json!({"num_masks":1,"overlay":base64::Engine::encode(&base64::engine::general_purpose::STANDARD,&server_preview)}).to_string()),
        "/view?filename=mask.png&type=output" => Reply::Raw([format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",server_mask.len()).into_bytes(),server_mask.clone()].concat()),
        _ => Reply::Json(404,"{}".into()),
    }
    });
    let mut session = Sam3Session::new(server.url.trim_end_matches("/v1")).unwrap();
    session.prepare(&png()).unwrap();
    assert_eq!(session.preview(32.0, 32.0).unwrap(), preview);
    assert_eq!(session.commit(32.0, 32.0).unwrap(), mask);
    let calls = server.recorded();
    let graphs: Vec<Value> = calls
        .iter()
        .filter(|r| r.path == "/prompt")
        .map(|r| r.body_json()["prompt"].clone())
        .collect();
    assert_eq!(graphs.len(), 2);
    let id = graphs[0]
        .as_object()
        .unwrap()
        .iter()
        .find(|(_, v)| v["class_type"] == "SAM3InteractiveCollector")
        .unwrap()
        .0;
    assert!(graphs[0].get("5").is_none(), "activation only embeds image");
    assert_eq!(graphs[1]["5"]["inputs"]["images"], json!(["4", 0]));
    assert!(
        graphs[1][id]["inputs"]["multi_prompts_store"]
            .as_str()
            .unwrap()
            .contains("positive_points")
    );
    let preview_call = calls
        .iter()
        .find(|r| r.path == "/sam3/interactive_segment_one")
        .unwrap();
    assert_eq!(preview_call.body_json()["node_id"], id.as_str());
}

/// Several points and a box are sent together in a single queued prompt.
#[test]
fn multi_point_and_box_prompts_travel_in_one_prompt() {
    let mask = png();
    let server_mask = mask.clone();
    let server = Server::start(move |r| {
        match r.path.as_str() {
        "/system_stats" => Reply::Json(200, json!({"system":{"comfyui_version":"0.22.3"}}).to_string()),
        "/object_info" => Reply::Json(200, json!({
            "LoadImage":{},"LoadSAM3Model":{},"SAM3InteractiveCollector":{},
            "SAM3Grounding":{},"MaskToImage":{},"PreviewImage":{}
        }).to_string()),
        "/upload/image" => Reply::Json(200,json!({"name":"source.png","type":"temp","subfolder":""}).to_string()),
        "/prompt" => Reply::Json(200,json!({"prompt_id":"abc-123"}).to_string()),
        "/history/abc-123" => Reply::Json(200,json!({"abc-123":{"status":{"completed":true},"outputs":{"5":{"images":[{"filename":"mask.png","type":"output","subfolder":""}]}}}}).to_string()),
        "/view?filename=mask.png&type=output" => Reply::Raw([format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",server_mask.len()).into_bytes(),server_mask.clone()].concat()),
        _ => Reply::Json(404,"{}".into()),
    }
    });
    let mut session = Sam3Session::new(server.url.trim_end_matches("/v1")).unwrap();
    session.prepare(&png()).unwrap();
    let prompt = craft_ai_comfyui::sam3::Prompt {
        positive_points: vec![(4.0, 6.0), (10.0, 20.0)],
        negative_points: vec![(30.0, 12.0)],
        box_: Some([2.0, 3.0, 40.0, 28.0]),
    };
    session.commit_prompt(&prompt).unwrap();
    let calls = server.recorded();
    let graphs: Vec<Value> = calls
        .iter()
        .filter(|r| r.path == "/prompt")
        .map(|r| r.body_json()["prompt"].clone())
        .collect();
    assert_eq!(graphs.len(), 2);
    let store: Value = {
        let collector = graphs[1]
            .as_object()
            .and_then(|nodes| {
                nodes
                    .values()
                    .find(|v| v["class_type"] == "SAM3InteractiveCollector")
            })
            .unwrap();
        serde_json::from_str(collector["inputs"]["multi_prompts_store"].as_str().unwrap()).unwrap()
    };
    assert_eq!(store[0]["positive_points"].as_array().unwrap().len(), 2);
    assert_eq!(store[0]["negative_points"].as_array().unwrap().len(), 1);
    assert_eq!(store[0]["positive_boxes"][0]["x1"], 2.0);
    assert_eq!(store[0]["positive_boxes"][0]["y2"], 28.0);
}

/// Text grounding sends the subject text and returns the resulting mask.
#[test]
fn grounding_sends_text_and_returns_mask() {
    let mask = png();
    let server_mask = mask.clone();
    let server = Server::start(move |r| {
        match r.path.as_str() {
        "/system_stats" => Reply::Json(200, json!({"system":{"comfyui_version":"0.22.3"}}).to_string()),
        "/object_info" => Reply::Json(200, json!({
            "LoadImage":{},"LoadSAM3Model":{},"SAM3InteractiveCollector":{},
            "SAM3Grounding":{},"MaskToImage":{},"PreviewImage":{}
        }).to_string()),
        "/upload/image" => Reply::Json(200,json!({"name":"source.png","type":"temp","subfolder":""}).to_string()),
        "/prompt" => Reply::Json(200,json!({"prompt_id":"abc-123"}).to_string()),
        "/history/abc-123" => Reply::Json(200,json!({"abc-123":{"status":{"completed":true},"outputs":{"5":{"images":[{"filename":"mask.png","type":"output","subfolder":""}]}}}}).to_string()),
        "/view?filename=mask.png&type=output" => Reply::Raw([format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",server_mask.len()).into_bytes(),server_mask.clone()].concat()),
        _ => Reply::Json(404,"{}".into()),
    }
    });
    let mut session = Sam3Session::new(server.url.trim_end_matches("/v1")).unwrap();
    session.prepare(&png()).unwrap();
    session.ground("person").unwrap();
    let calls = server.recorded();
    let graphs: Vec<Value> = calls
        .iter()
        .filter(|r| r.path == "/prompt")
        .map(|r| r.body_json()["prompt"].clone())
        .collect();
    assert_eq!(graphs.len(), 2);
    let grounding = graphs[1]
        .as_object()
        .and_then(|nodes| nodes.values().find(|v| v["class_type"] == "SAM3Grounding"))
        .expect("grounding node queued");
    assert_eq!(grounding["inputs"]["text_prompt"], "person");
    assert!(grounding["inputs"]["image"][0].is_string());
}

#[test]
fn invalid_points_and_unprepared_session_never_contact_comfyui() {
    let server = Server::start(|_| Reply::Json(500, "{}".into()));
    let mut session = Sam3Session::new(server.url.trim_end_matches("/v1")).unwrap();
    assert!(session.preview(1.0, 1.0).is_err());
    assert!(session.commit(1.0, 1.0).is_err());
    assert!(session.prepare(b"bad PNG").is_err());
    assert_eq!(server.count(), 0);
}

#[test]
fn unrelated_loopback_service_is_not_accepted_as_comfyui() {
    let server = Server::start(|r| match r.path.as_str() {
        "/system_stats" => Reply::Json(200, json!({"system":{"name":"elsewhere"}}).to_string()),
        _ => Reply::Json(404, "{}".into()),
    });
    let session = Sam3Session::new(server.url.trim_end_matches("/v1")).unwrap();
    assert!(session.verify_server().is_err());
    assert_eq!(server.count(), 1);
}

#[test]
fn remote_origin_is_never_a_local_sam3_session() {
    assert!(Sam3Session::new("https://example.com").is_err());
}

/// Live and opt-in: several points select both generated discs.
#[test]
#[ignore = "requires a running local ComfyUI; does not start or modify it"]
fn installed_comfyui_multi_point_selects_both_discs() {
    let url = std::env::var("CRAFT_TEST_COMFYUI_URL").expect("set the local server address");
    let mut session = Sam3Session::new(&url).unwrap();
    let mut image = Vec::new();
    let mut encoder = png::Encoder::new(&mut image, 256, 128);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    let source: Vec<_> = (0..128)
        .flat_map(|y: i32| {
            (0..256).map(move |x: i32| {
                let big = (x - 64).pow(2) + (y - 64).pow(2) < 50 * 50;
                let small = (x - 190).pow(2) + (y - 64).pow(2) < 30 * 30;
                if big || small { 70 } else { 255 }
            })
        })
        .collect();
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&source)
        .unwrap();
    session.prepare(&image).unwrap();
    let prompt = craft_ai_comfyui::sam3::Prompt {
        positive_points: vec![(64.0, 64.0), (190.0, 64.0)],
        negative_points: vec![],
        box_: None,
    };
    let selected = session.commit_prompt(&prompt).unwrap();
    let mut reader = png::Decoder::new(std::io::Cursor::new(selected.as_slice()))
        .read_info()
        .unwrap();
    assert_eq!((reader.info().width, reader.info().height), (256, 128));
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    reader.next_frame(&mut pixels).unwrap();
    assert!(pixels[64 * 256 + 40] > 0, "big disc must be selected");
    assert!(
        pixels[64 * 256 + 188] > 0,
        "multi-point must reach the second disc"
    );
    assert_eq!(pixels[10 * 256 + 10], 0, "background must stay unselected");
}

/// Live and opt-in: prewarm, preview and select work on the installed ComfyUI.
#[test]
#[ignore = "requires a running local ComfyUI; does not start or modify it"]
fn installed_comfyui_prewarms_previews_and_selects_a_disc() {
    let url = std::env::var("CRAFT_TEST_COMFYUI_URL").expect("set the local server address");
    let mut session = Sam3Session::new(&url).unwrap();
    let mut image = Vec::new();
    let mut encoder = png::Encoder::new(&mut image, 256, 256);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    let source: Vec<_> = (0..256)
        .flat_map(|y: i32| {
            (0..256).map(move |x: i32| {
                if (x - 128).pow(2) + (y - 128).pow(2) < 70 * 70 {
                    70
                } else {
                    255
                }
            })
        })
        .collect();
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&source)
        .unwrap();
    session.prepare(&image).unwrap();
    let preview = session.preview(128.0, 128.0).unwrap();
    assert!(preview.starts_with(&[0xff, 0xd8]));
    let selected = session.commit(128.0, 128.0).unwrap();
    let mut reader = png::Decoder::new(std::io::Cursor::new(selected.as_slice()))
        .read_info()
        .unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let frame = reader.next_frame(&mut pixels).unwrap();
    assert_eq!((frame.width, frame.height), (256, 256));
    assert!(pixels.contains(&0));
    assert!(pixels.iter().any(|v| *v > 0));
}

/// A ComfyUI that serves `mask` for every graph, with the grounding node installed.
fn grounding_server(mask: Vec<u8>) -> Server {
    Server::start(move |r| {
        match r.path.as_str() {
        "/system_stats" => Reply::Json(200, json!({"system":{"comfyui_version":"0.22.3"}}).to_string()),
        "/object_info" => Reply::Json(200, json!({
            "LoadImage":{},"LoadSAM3Model":{},"SAM3InteractiveCollector":{},
            "SAM3Grounding":{},"MaskToImage":{},"PreviewImage":{}
        }).to_string()),
        "/upload/image" => Reply::Json(200, json!({"name":"source.png","type":"temp","subfolder":""}).to_string()),
        "/prompt" => Reply::Json(200, json!({"prompt_id":"abc-123"}).to_string()),
        "/history/abc-123" => Reply::Json(200, json!({"abc-123":{"status":{"completed":true},"outputs":{"5":{"images":[{"filename":"mask.png","type":"output","subfolder":""}]}}}}).to_string()),
        "/view?filename=mask.png&type=output" => Reply::Raw([format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", mask.len()).into_bytes(), mask.clone()].concat()),
        _ => Reply::Json(404, "{}".into()),
    }
    })
}

#[test]
fn grounding_a_phrase_that_matches_nothing_is_an_error_not_an_empty_selection() {
    let server = grounding_server(flat_png(0));
    let mut session = Sam3Session::new(server.url.trim_end_matches("/v1")).unwrap();
    session.prepare(&png()).unwrap();
    let error = session.ground("subject").unwrap_err();
    assert!(
        error.message.contains("Nothing matched"),
        "{}",
        error.message
    );
}

#[test]
fn grounding_does_not_refetch_the_node_list_and_ground_first_gives_up_helpfully() {
    let server = grounding_server(flat_png(0));
    let mut session = Sam3Session::new(server.url.trim_end_matches("/v1")).unwrap();
    session.prepare(&png()).unwrap();
    let before = server
        .recorded()
        .iter()
        .filter(|r| r.path == "/object_info")
        .count();
    let error = session.ground_first(&["person", "animal"]).unwrap_err();
    assert!(
        error.message.contains("Type what to select"),
        "{}",
        error.message
    );
    let after = server
        .recorded()
        .iter()
        .filter(|r| r.path == "/object_info")
        .count();
    assert_eq!(
        before, after,
        "grounding reuses the check made during prepare"
    );
}
