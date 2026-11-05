//! Nothing these helpers send to ComfyUI may stay in the user's `input/` or `output/` folders.
//! Uploads go to the scratch area (`type=temp`), result nodes are `PreviewImage`, and each queued
//! sink has its own cache key so a repeat of the same request still produces an output.
#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;
use craft_ai_comfyui::Sam21Session;
use serde_json::{Value, json};
use support::{Reply, Server};

/// A plain white 64 by 64 greyscale PNG, used as both source image and mask.
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

/// The `/object_info` reply of a server with every node the SAM 2.1 graph needs.
fn installed() -> String {
    json!({
        "LoadImage":{},
        "Florence2ModelLoader":{"input":{"required":{"model":[["Florence-2-large"],{}]}}},
        "Florence2Run":{},"Florence2toCoordinates":{},"DownloadAndLoadSAM2Model":{},
        "Sam2Segmentation":{},"MaskToImage":{},"PreviewImage":{}
    })
    .to_string()
}

/// A server whose mask lives only in the scratch area. `upload_type` is what it claims on upload.
fn server(upload_type: &'static str) -> Server {
    let mask = png();
    Server::start(move |r| match r.path.as_str() {
        "/system_stats" => Reply::Json(
            200,
            json!({"system":{"comfyui_version":"0.22.3"},"devices":[{"type":"cuda"}]}).to_string(),
        ),
        "/object_info" => Reply::Json(200, installed()),
        "/upload/image" => Reply::Json(
            200,
            json!({"name":"source.png","type":upload_type,"subfolder":""}).to_string(),
        ),
        "/prompt" => Reply::Json(200, json!({"prompt_id":"job"}).to_string()),
        "/history/job" => Reply::Json(
            200,
            json!({"job":{"status":{"completed":true},"outputs":{"8":{"images":[
                {"filename":"ComfyUI_temp_abc_00001_.png","type":"temp","subfolder":""}
            ]}}}})
            .to_string(),
        ),
        "/view?filename=ComfyUI_temp_abc_00001_.png&type=temp" => Reply::Raw(
            [
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    mask.len()
                )
                .into_bytes(),
                mask.clone(),
            ]
            .concat(),
        ),
        _ => Reply::Json(404, "{}".into()),
    })
}

/// A SAM 2.1 session on `server` that has checked the server and uploaded the source image.
fn session(server: &Server) -> Sam21Session {
    let url = server.url.trim_end_matches("/v1");
    let mut session = Sam21Session::new(url).unwrap();
    session.verify_server().unwrap();
    session.prepare(&png()).unwrap();
    session
}

/// Every graph `server` received on `/prompt`, in order.
fn queued(server: &Server) -> Vec<Value> {
    server
        .recorded()
        .iter()
        .filter(|r| r.path == "/prompt")
        .map(|r| r.body_json()["prompt"].clone())
        .collect()
}

/// The upload asks for the scratch area (`type=temp`) and never for `input`.
#[test]
fn the_source_image_is_uploaded_to_the_scratch_area_not_input() {
    let s = server("temp");
    let _ = session(&s);
    let upload = s
        .recorded()
        .into_iter()
        .find(|r| r.path == "/upload/image")
        .unwrap();
    let body = String::from_utf8_lossy(&upload.body).into_owned();
    assert!(
        body.contains("name=\"type\"\r\n\r\ntemp\r\n"),
        "{body:.200}"
    );
    assert!(!body.contains("\r\n\r\ninput\r\n"));
}

/// If the server says the upload landed in `input/`, preparing the image fails.
#[test]
fn a_reply_that_says_the_file_landed_in_input_is_refused() {
    let s = server("input");
    let url = s.url.trim_end_matches("/v1");
    let mut session = Sam21Session::new(url).unwrap();
    session.verify_server().unwrap();
    assert!(session.prepare(&png()).is_err());
}

/// The loader reads the ` [temp]` copy and no node in the queued graph is a `SaveImage`.
#[test]
fn the_loader_reads_the_scratch_copy_and_the_result_is_a_preview_not_a_saved_file() {
    let s = server("temp");
    let session = session(&s);
    session.ground("person").unwrap();
    let graph = queued(&s).remove(0);
    assert_eq!(graph["1"]["inputs"]["image"], "source.png [temp]");
    assert_eq!(graph["8"]["class_type"], "PreviewImage");
    assert!(
        graph
            .as_object()
            .unwrap()
            .values()
            .all(|n| n["class_type"] != "SaveImage"),
        "no node may write to the user's output folder"
    );
}

/// Two identical selections give the result node different prefixes, so the repeat is not served
/// from cache with no output.
#[test]
fn asking_twice_gives_each_result_node_its_own_cache_key() {
    let s = server("temp");
    let session = session(&s);
    session.ground("person").unwrap();
    session.ground("person").unwrap();
    let graphs = queued(&s);
    assert_eq!(graphs.len(), 2);
    assert_ne!(
        graphs[0]["8"]["inputs"]["filename_prefix"], graphs[1]["8"]["inputs"]["filename_prefix"],
        "a cached result node returns no output on the repeat"
    );
}
