#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;
use craft_ai_comfyui::Sam3Point;
use serde_json::json;
use support::{Reply, Server};

fn grey_png(value: u8) -> Vec<u8> {
    let mut bytes = vec![];
    let mut writer = png::Encoder::new(&mut bytes, 2, 2);
    writer.set_color(png::ColorType::Grayscale);
    writer.set_depth(png::BitDepth::Eight);
    let mut writer = writer.write_header().unwrap();
    writer.write_image_data(&[value; 4]).unwrap();
    drop(writer);
    bytes
}

/// A server without the SAM 3 nodes is refused after the node check, before any upload or queue.
#[test]
fn a_missing_sam_node_never_uploads_or_queues() {
    let s = Server::start(|r| match r.path.as_str() {
        "/object_info" => Reply::Json(200, json!({"LoadImage":{},"PreviewImage":{}}).to_string()),
        _ => Reply::Json(500, "{}".into()),
    });
    let p = Sam3Point::new(s.url.trim_end_matches("/v1")).unwrap();
    assert!(
        p.segment(&grey_png(128), 0.5, 0.5)
            .unwrap_err()
            .message
            .contains("LoadSAM3Model")
    );
    assert_eq!(
        s.recorded()
            .iter()
            .map(|r| r.path.as_str())
            .collect::<Vec<_>>(),
        ["/object_info"]
    );
}

/// One point queues the installed SAM 3 workflow with a scratch-only result node and returns its mask.
#[test]
fn a_point_queues_the_installed_sam3_workflow_and_returns_a_mask() {
    let mask = grey_png(255);
    let reply = mask.clone();
    let s = Server::start(move |r| {
        match r.path.as_str() {
        "/object_info" => Reply::Json(200, json!({
            "LoadImage":{}, "LoadSAM3Model":{}, "SAM3CreatePoint":{},
            "SAM3CombinePoints":{}, "SAM3Segmentation":{}, "MaskToImage":{}, "PreviewImage":{}
        }).to_string()),
        "/upload/image" => Reply::Json(200, json!({"name":"uploaded.png","subfolder":"","type":"temp"}).to_string()),
        "/prompt" => Reply::Json(200, json!({"prompt_id":"point-job"}).to_string()),
        "/history/point-job" => Reply::Json(200, json!({"point-job":{"status":{"completed":true},"outputs":{"7":{"images":[{"filename":"mask.png","type":"output","subfolder":""}]}}}}).to_string()),
        "/view?filename=mask.png&type=output" => Reply::Raw([format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", reply.len()).into_bytes(), reply.clone()].concat()),
        _ => Reply::Json(404, "{}".into()),
    }
    });
    let p = Sam3Point::new(s.url.trim_end_matches("/v1")).unwrap();
    let result = p.segment(&grey_png(128), 0.25, 0.75).unwrap();
    assert_eq!(result, mask);
    let calls = s.recorded();
    let graph = &calls
        .iter()
        .find(|r| r.path == "/prompt")
        .unwrap()
        .body_json()["prompt"];
    assert_eq!(graph["1"]["inputs"]["image"], "uploaded.png [temp]");
    assert_eq!(graph["3"]["inputs"]["x"], 0.25);
    assert_eq!(graph["3"]["inputs"]["y"], 0.75);
    assert_eq!(graph["5"]["class_type"], "SAM3Segmentation");
    assert_eq!(graph["6"]["inputs"]["mask"], json!(["5", 0]));
    assert_eq!(graph["7"]["inputs"]["images"], json!(["6", 0]));
    assert_eq!(graph["7"]["class_type"], "PreviewImage");
}

#[test]
fn bad_coordinates_do_not_touch_the_server() {
    let s = Server::start(|_| Reply::Json(200, "{}".into()));
    let p = Sam3Point::new(s.url.trim_end_matches("/v1")).unwrap();
    assert!(p.segment(&grey_png(128), f32::NAN, 0.5).is_err());
    assert!(p.segment(&grey_png(128), 1.01, 0.5).is_err());
    assert_eq!(s.count(), 0);
}

/// Live and opt-in: SAM 3 selects the generated disc at its centre.
#[test]
#[ignore = "requires the person's running ComfyUI; never starts or modifies it"]
fn existing_comfyui_segments_the_disc_at_its_centre() {
    let url = std::env::var("CRAFT_TEST_COMFYUI_URL")
        .expect("set to the existing loopback ComfyUI address");
    let mut image = Vec::new();
    let mut encoder = png::Encoder::new(&mut image, 256, 256);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    let pixels: Vec<_> = (0..256)
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
    writer.write_image_data(&pixels).unwrap();
    drop(writer);
    let result = Sam3Point::new(&url)
        .unwrap()
        .segment(&image, 0.5, 0.5)
        .unwrap();
    let decoder = png::Decoder::new(std::io::Cursor::new(result.as_slice()));
    let mut reader = decoder.read_info().unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let frame = reader.next_frame(&mut pixels).unwrap();
    assert_eq!((frame.width, frame.height), (256, 256));
    assert_eq!(frame.color_type, png::ColorType::Grayscale);
    assert!(
        pixels.iter().any(|v| *v > 0),
        "SAM 3 should select something"
    );
    assert!(pixels.contains(&0), "SAM 3 should leave a background");
}

#[test]
fn a_remote_server_is_not_a_local_comfyui() {
    assert!(Sam3Point::new("https://example.org").is_err());
}
