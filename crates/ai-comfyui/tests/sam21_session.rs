#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;
use craft_ai_comfyui::Sam21Session;
use serde_json::{Value, json};
use support::{Reply, Server};

/// A small grey PNG used as both the source image and the returned mask.
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
/// Installed sam21 returns a real nonempty selection mask.
#[ignore = "requires a separate running local ComfyUI with SAM 2.1 and Florence-2"]
fn installed_sam21_returns_a_real_nonempty_selection_mask() {
    let url = std::env::var("CRAFT_TEST_COMFYUI_URL").expect("local server address");
    let image =
        std::fs::read(std::env::var("CRAFT_TEST_SAM21_IMAGE").expect("test photo path")).unwrap();
    let source = png::Decoder::new(std::io::Cursor::new(image.as_slice()))
        .read_info()
        .unwrap();
    let mut session = Sam21Session::new(&url).unwrap();
    session.verify_server().unwrap();
    session.prepare(&image).unwrap();
    let mask = session.ground("person").unwrap();
    let mut decoder = png::Decoder::new(std::io::Cursor::new(mask.as_slice()))
        .read_info()
        .unwrap();
    assert_eq!(
        (decoder.info().width, decoder.info().height),
        (source.info().width, source.info().height)
    );
    let mut pixels = vec![0; decoder.output_buffer_size().unwrap()];
    let frame = decoder.next_frame(&mut pixels).unwrap();
    assert!(pixels[..frame.buffer_size()].iter().any(|p| *p > 0));
    assert!(pixels[..frame.buffer_size()].contains(&0));
}

#[test]
/// Selected sam21 segments the image with florence and returns an exact mask.
fn selected_sam21_segments_the_image_with_florence_and_returns_an_exact_mask() {
    let mask = png();
    let server_mask = mask.clone();
    let server = Server::start(move |r| {
        match r.path.as_str() {
        "/system_stats" => Reply::Json(200, json!({"system":{"comfyui_version":"0.22.3"},"devices":[{"type":"cuda"}]}).to_string()),
        "/object_info" => Reply::Json(200, json!({"LoadImage":{},"Florence2ModelLoader":{"input":{"required":{"model":[["Florence-2-large"],{}]}}},"Florence2Run":{},"Florence2toCoordinates":{},"DownloadAndLoadSAM2Model":{},"Sam2Segmentation":{},"MaskToImage":{},"PreviewImage":{}}).to_string()),
        "/upload/image" => Reply::Json(200,json!({"name":"source.png","type":"temp","subfolder":""}).to_string()),
        "/prompt" => Reply::Json(200,json!({"prompt_id":"sam21-job"}).to_string()),
        "/history/sam21-job" => Reply::Json(200,json!({"sam21-job":{"status":{"completed":true},"outputs":{"8":{"images":[{"filename":"mask.png","type":"output","subfolder":""}]}}}}).to_string()),
        "/view?filename=mask.png&type=output" => Reply::Raw([format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",server_mask.len()).into_bytes(),server_mask.clone()].concat()),
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
/// Cancelling sam21 removes only its queued prompt.
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
        "/system_stats" => Reply::Json(200, json!({"system":{"comfyui_version":"0.22.3"},"devices":[{"type":"cuda"}]}).to_string()),
        "/object_info" => Reply::Json(200, json!({"LoadImage":{},"Florence2ModelLoader":{"input":{"required":{"model":[["Florence-2-large"],{}]}}},"Florence2Run":{},"Florence2toCoordinates":{},"DownloadAndLoadSAM2Model":{},"Sam2Segmentation":{},"MaskToImage":{},"PreviewImage":{}}).to_string()),
        "/upload/image" => Reply::Json(200,json!({"name":"source.png","type":"temp","subfolder":""}).to_string()),
        "/prompt" => Reply::Json(200,json!({"prompt_id":"sam21-job"}).to_string()),
        "/history/sam21-job" if server_polls.fetch_add(1, Ordering::SeqCst) == 0 => Reply::Json(200,"{}".to_owned()),
        "/history/sam21-job" => Reply::Json(200,json!({"sam21-job":{"status":{"completed":true},"outputs":{"8":{"images":[{"filename":"mask.png","type":"output","subfolder":""}]}}}}).to_string()),
        "/view?filename=mask.png&type=output" => Reply::Raw([format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",server_mask.len()).into_bytes(),server_mask.clone()].concat()),
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
/// Sam21 refuses before uploading when the detector is not in comfyui.
fn sam21_refuses_before_uploading_when_the_detector_is_not_in_comfyui() {
    // Another Florence-2 variant is installed, but not the one the graph asks for.
    let server = Server::start(|r| {
        match r.path.as_str() {
        "/system_stats" => Reply::Json(200, json!({"system":{"comfyui_version":"0.22.3"},"devices":[{"type":"cuda"}]}).to_string()),
        "/object_info" => Reply::Json(200, json!({"LoadImage":{},"Florence2ModelLoader":{"input":{"required":{"model":[["Florence-2-base"],{}]}}},"Florence2Run":{},"Florence2toCoordinates":{},"DownloadAndLoadSAM2Model":{},"Sam2Segmentation":{},"MaskToImage":{},"PreviewImage":{}}).to_string()),
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

/// A scripted ComfyUI that reports `device` (or none) and serves one mask.
fn device_server(device: Option<&'static str>, view_type: &'static str) -> Server {
    let mask = png();
    Server::start(move |r| {
        match r.path.as_str() {
        "/system_stats" => {
            let mut stats = json!({"system":{"comfyui_version":"0.22.3"}});
            if let Some(device) = device {
                stats["devices"] = json!([{"type": device}]);
            }
            Reply::Json(200, stats.to_string())
        }
        "/object_info" => Reply::Json(200, json!({"LoadImage":{},"Florence2ModelLoader":{"input":{"required":{"model":[["Florence-2-large"],{}]}}},"Florence2Run":{},"Florence2toCoordinates":{},"DownloadAndLoadSAM2Model":{},"Sam2Segmentation":{},"MaskToImage":{},"PreviewImage":{}}).to_string()),
        "/upload/image" => Reply::Json(200, json!({"name":"source.png","type":"temp","subfolder":""}).to_string()),
        "/prompt" => Reply::Json(200, json!({"prompt_id":"job"}).to_string()),
        "/history/job" => Reply::Json(200, json!({"job":{"status":{"completed":true},"outputs":{"8":{"images":[{"filename":"mask.png","type":view_type,"subfolder":""}]}}}}).to_string()),
        "/view?filename=mask.png&type=output" => Reply::Raw([format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", mask.len()).into_bytes(), mask.clone()].concat()),
        _ => Reply::Json(404, "{}".into()),
    }
    })
}

/// A session that has verified the server and prepared a test image.
fn prepared(server: &Server) -> Result<Sam21Session, craft_ai_core::AiError> {
    let mut session = Sam21Session::new(server.url.trim_end_matches("/v1")).unwrap();
    session.verify_server().unwrap();
    session.prepare(&png())?;
    Ok(session)
}

#[test]
/// The graph uses the device comfyui reports and full precision off cuda.
fn the_graph_uses_the_device_comfyui_reports_and_full_precision_off_cuda() {
    for (device, precision) in [("cuda", "fp16"), ("cpu", "fp32"), ("mps", "fp32")] {
        let server = device_server(Some(device), "output");
        prepared(&server).unwrap().ground("person").unwrap();
        let graph = server
            .recorded()
            .into_iter()
            .find(|r| r.path == "/prompt")
            .unwrap()
            .body_json()["prompt"]
            .clone();
        assert_eq!(graph["5"]["inputs"]["device"], device);
        assert_eq!(graph["5"]["inputs"]["precision"], precision, "{device}");
        assert_eq!(graph["2"]["inputs"]["precision"], precision, "{device}");
    }
}

#[test]
/// An unreported or unsupported device is refused before the image is uploaded.
fn an_unreported_or_unsupported_device_is_refused_before_the_image_is_uploaded() {
    for device in [None, Some("xpu"), Some("")] {
        let server = device_server(device, "output");
        let error = prepared(&server).err().expect("must be refused");
        assert_eq!(error.kind, craft_ai_core::ErrorKind::Unsupported);
        assert!(error.message.contains("CUDA, Apple (MPS) or CPU"));
        assert!(!server.recorded().iter().any(|r| r.path == "/upload/image"));
        assert!(!server.recorded().iter().any(|r| r.path == "/prompt"));
    }
}

#[test]
/// A bad result is reported as sam 2 1 not sam 3.
fn a_bad_result_is_reported_as_sam_2_1_not_sam_3() {
    let server = device_server(Some("cuda"), "user");
    let error = prepared(&server).unwrap().ground("person").unwrap_err();
    assert!(error.message.contains("SAM 2.1"), "{}", error.message);
    assert!(!error.message.contains("SAM 3"), "{}", error.message);
}

/// Every matching subject is sent to the segmenter. With `individual_objects` false the node keeps only
/// the last box, so a second match is silently dropped. A scripted server cannot see that, so this pins
/// the graph and the ignored live test below proves the result.
#[test]
fn every_detected_box_is_sent_to_the_segmenter() {
    let server = device_server(Some("cuda"), "output");
    prepared(&server).unwrap().ground("person").unwrap();
    let graph = server
        .recorded()
        .into_iter()
        .find(|r| r.path == "/prompt")
        .unwrap()
        .body_json()["prompt"]
        .clone();
    assert_eq!(graph["6"]["inputs"]["individual_objects"], true);
    assert_eq!(graph["4"]["inputs"]["index"], "", "all boxes, not one");
}

/// Two identical selections must not share a sink name, or ComfyUI serves the second from its cache
/// with no outputs and the selection fails.
#[test]
fn asking_twice_gives_each_selection_its_own_sink_name() {
    let server = device_server(Some("cuda"), "output");
    let session = prepared(&server).unwrap();
    session.ground("person").unwrap();
    session.ground("person").unwrap();
    let prefixes: Vec<Value> = server
        .recorded()
        .into_iter()
        .filter(|r| r.path == "/prompt")
        .map(|r| r.body_json()["prompt"]["8"]["inputs"]["filename_prefix"].clone())
        .collect();
    assert_eq!(prefixes.len(), 2);
    assert_ne!(prefixes[0], prefixes[1]);
}

/// Two copies of the sample photo side by side: a selection of "person" must light up both halves.
/// This needs a real ComfyUI, because only the real segmenter drops all but one box.
#[test]
#[ignore = "requires a separate running local ComfyUI with SAM 2.1 and Florence-2"]
fn installed_sam21_selects_every_matching_subject() {
    let url = std::env::var("CRAFT_TEST_COMFYUI_URL").expect("local server address");
    let image =
        std::fs::read(std::env::var("CRAFT_TEST_SAM21_IMAGE").expect("test photo path")).unwrap();
    let mut decoder = png::Decoder::new(std::io::Cursor::new(image.as_slice()));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().unwrap();
    let (w, h) = (reader.info().width as usize, reader.info().height as usize);
    let (color, _) = reader.output_color_type();
    let channels = color.samples();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    reader.next_frame(&mut pixels).unwrap();
    let mut two = Vec::new();
    for row in pixels.chunks(w * channels).take(h) {
        two.extend_from_slice(row);
        two.extend_from_slice(row);
    }
    let mut doubled = Vec::new();
    let mut encoder = png::Encoder::new(&mut doubled, (w * 2) as u32, h as u32);
    encoder.set_color(color);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&two)
        .unwrap();
    let mut session = Sam21Session::new(&url).unwrap();
    session.verify_server().unwrap();
    session.prepare(&doubled).unwrap();
    let mask = session.ground("person").unwrap();
    let mut decoder = png::Decoder::new(std::io::Cursor::new(mask.as_slice()))
        .read_info()
        .unwrap();
    let mut out = vec![0; decoder.output_buffer_size().unwrap()];
    let frame = decoder.next_frame(&mut out).unwrap();
    let samples = frame.color_type.samples();
    let lit = |from: usize, to: usize| {
        out.chunks(frame.line_size)
            .flat_map(|row| row[from * samples..to * samples].iter())
            .filter(|p| **p > 0)
            .count()
    };
    assert!(lit(0, w) > 0, "the left subject was dropped");
    assert!(lit(w, w * 2) > 0, "the right subject was dropped");
}
