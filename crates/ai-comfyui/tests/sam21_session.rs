#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;
use craft_ai_comfyui::Sam21Session;
use serde_json::{Value, json};
use support::{Reply, Server};

fn png() -> Vec<u8> {
    let mut data = vec![];
    let mut encoder = png::Encoder::new(&mut data, 64, 64);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&vec![255; 4096])
        .unwrap();
    data
}

#[test]
#[ignore = "requires a separate running local ComfyUI with SAM 2.1 and Florence-2"]
fn installed_sam21_returns_a_real_nonempty_selection_mask() {
    let url = std::env::var("CRAFT_TEST_COMFYUI_URL").expect("local server address");
    let image =
        std::fs::read(std::env::var("CRAFT_TEST_SAM21_IMAGE").expect("test photo path")).unwrap();
    let source = png::Decoder::new(image.as_slice()).read_info().unwrap();
    let mut session = Sam21Session::new(&url).unwrap();
    session.verify_server().unwrap();
    session.prepare(&image).unwrap();
    let mask = session.ground("person").unwrap();
    let mut decoder = png::Decoder::new(mask.as_slice()).read_info().unwrap();
    assert_eq!(
        (decoder.info().width, decoder.info().height),
        (source.info().width, source.info().height)
    );
    let mut pixels = vec![0; decoder.output_buffer_size()];
    let frame = decoder.next_frame(&mut pixels).unwrap();
    assert!(pixels[..frame.buffer_size()].iter().any(|p| *p > 0));
    assert!(pixels[..frame.buffer_size()].contains(&0));
}

#[test]
fn selected_sam21_segments_the_image_with_florence_and_returns_an_exact_mask() {
    let mask = png();
    let server_mask = mask.clone();
    let server = Server::start(move |r| {
        match r.path.as_str() {
        "/system_stats" => Reply::Json(200, json!({"system":{"comfyui_version":"0.22.3"}}).to_string()),
        "/object_info" => Reply::Json(200, json!({"LoadImage":{},"Florence2ModelLoader":{"input":{"required":{"model":[["Florence-2-large"],{}]}}},"Florence2Run":{},"Florence2toCoordinates":{},"DownloadAndLoadSAM2Model":{},"Sam2Segmentation":{},"MaskToImage":{},"SaveImage":{}}).to_string()),
        "/upload/image" => Reply::Json(200,json!({"name":"source.png","type":"input","subfolder":""}).to_string()),
        "/prompt" => Reply::Json(200,json!({"prompt_id":"sam21-job"}).to_string()),
        "/history/sam21-job" => Reply::Json(200,json!({"sam21-job":{"status":{"completed":true},"outputs":{"8":{"images":[{"filename":"mask.png","type":"output","subfolder":""}]}}}}).to_string()),
        "/view?filename=mask.png&type=output" => Reply::Raw([format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",server_mask.len()).into_bytes(),server_mask.clone()].concat()),
        _ => Reply::Json(404,"{}".into()),
    }
    });
    let url = server.url.trim_end_matches("/v1");
    let mut session = Sam21Session::new(url).unwrap();
    session.verify_server().unwrap();
    session.prepare(&png()).unwrap();
    assert_eq!(session.ground("person").unwrap(), mask);
    let calls = server.recorded();
    let graphs: Vec<Value> = calls
        .iter()
        .filter(|r| r.path == "/prompt")
        .map(|r| r.body_json()["prompt"].clone())
        .collect();
    assert_eq!(
        graphs.len(),
        1,
        "preparation uploads but does not run a different model"
    );
    assert_eq!(graphs[0]["2"]["class_type"], "Florence2ModelLoader");
    assert_eq!(graphs[0]["2"]["inputs"]["model"], "Florence-2-large");
    assert_eq!(graphs[0]["3"]["inputs"]["text_input"], "person");
    assert_eq!(graphs[0]["4"]["inputs"]["data"], json!(["3", 3]));
    assert_eq!(
        graphs[0]["5"]["inputs"]["model"],
        "sam2.1_hiera_large.safetensors"
    );
    assert_eq!(graphs[0]["6"]["inputs"]["bboxes"], json!(["4", 1]));
    assert_eq!(graphs[0]["8"]["inputs"]["images"], json!(["7", 0]));
    assert!(!calls.iter().any(|r| r.path.starts_with("/sam3/")));
}

#[test]
fn cancelling_sam21_removes_only_its_queued_prompt() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    let polls = Arc::new(AtomicUsize::new(0));
    let server_polls = Arc::clone(&polls);
    let mask = png();
    let server_mask = mask.clone();
    let server = Server::start(move |r| {
        match r.path.as_str() {
        "/system_stats" => Reply::Json(200, json!({"system":{"comfyui_version":"0.22.3"}}).to_string()),
        "/object_info" => Reply::Json(200, json!({"LoadImage":{},"Florence2ModelLoader":{"input":{"required":{"model":[["Florence-2-large"],{}]}}},"Florence2Run":{},"Florence2toCoordinates":{},"DownloadAndLoadSAM2Model":{},"Sam2Segmentation":{},"MaskToImage":{},"SaveImage":{}}).to_string()),
        "/upload/image" => Reply::Json(200,json!({"name":"source.png","type":"input","subfolder":""}).to_string()),
        "/prompt" => Reply::Json(200,json!({"prompt_id":"sam21-job"}).to_string()),
        "/history/sam21-job" if server_polls.fetch_add(1, Ordering::SeqCst) == 0 => Reply::Json(200,"{}".to_owned()),
        "/history/sam21-job" => Reply::Json(200,json!({"sam21-job":{"status":{"completed":true},"outputs":{"8":{"images":[{"filename":"mask.png","type":"output","subfolder":""}]}}}}).to_string()),
        "/view?filename=mask.png&type=output" => Reply::Raw([format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",server_mask.len()).into_bytes(),server_mask.clone()].concat()),
        "/queue" => Reply::Json(200,json!({"deleted":true}).to_string()),
        _ => Reply::Json(404,"{}".into()),
    }
    });
    let mut session = Sam21Session::new(server.url.trim_end_matches("/v1")).unwrap();
    session.prepare(&png()).unwrap();
    let cancel = session.canceller();
    let worker = std::thread::spawn(move || session.ground("person"));
    let deadline = Instant::now() + Duration::from_secs(3);
    while !server
        .recorded()
        .iter()
        .any(|r| r.path == "/history/sam21-job")
    {
        assert!(
            Instant::now() < deadline,
            "session never polled its own prompt"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    cancel.cancel();
    let error = worker.join().unwrap().unwrap_err();
    assert!(error.message.contains("cancelled"), "{}", error.message);
    let deletion = server
        .recorded()
        .into_iter()
        .find(|r| r.path == "/queue")
        .unwrap();
    assert_eq!(deletion.body_json(), json!({"delete":["sam21-job"]}));
}

#[test]
fn sam21_refuses_before_uploading_when_the_detector_is_not_in_comfyui() {
    // Another Florence-2 variant is installed, but not the one the graph asks for.
    let server = Server::start(|r| {
        match r.path.as_str() {
        "/system_stats" => Reply::Json(200, json!({"system":{"comfyui_version":"0.22.3"}}).to_string()),
        "/object_info" => Reply::Json(200, json!({"LoadImage":{},"Florence2ModelLoader":{"input":{"required":{"model":[["Florence-2-base"],{}]}}},"Florence2Run":{},"Florence2toCoordinates":{},"DownloadAndLoadSAM2Model":{},"Sam2Segmentation":{},"MaskToImage":{},"SaveImage":{}}).to_string()),
        _ => Reply::Json(404, "{}".into()),
    }
    });
    let mut session = Sam21Session::new(server.url.trim_end_matches("/v1")).unwrap();
    session.verify_server().unwrap();
    let error = session.prepare(&png()).unwrap_err();
    assert!(
        error.message.contains("Florence-2-large"),
        "{}",
        error.message
    );
    assert!(
        !server
            .recorded()
            .iter()
            .any(|r| r.path.starts_with("/upload") || r.path == "/prompt")
    );
}
