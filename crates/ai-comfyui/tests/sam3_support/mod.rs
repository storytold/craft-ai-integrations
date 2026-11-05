//! Scripted localhost SAM 3 fixtures; no model, GPU, external service or real key is used.
#![allow(dead_code)]
#[path = "../../../ai-openai/tests/support/mod.rs"]
mod http_support;
pub use http_support::Reply;
pub use http_support::Server;
use serde_json::{Value, json};

pub fn mask(pixels: &[u8]) -> Vec<u8> {
    sized_mask(2, 1, pixels)
}

pub fn sized_mask(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, width, height);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(pixels)
        .unwrap();
    bytes
}

/// The pixel bytes of a PNG.
pub fn pixels(bytes: &[u8]) -> Vec<u8> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let frame = reader.next_frame(&mut buf).unwrap();
    assert_eq!((frame.width, frame.height), (2, 1));
    assert_eq!(frame.color_type, png::ColorType::Grayscale);
    buf.truncate(frame.buffer_size());
    buf
}

pub fn server(images: Value, files: Vec<(&'static str, Vec<u8>)>) -> Server {
    server_with(images, files, json!({}), false)
}

/// Like `server`, with extra history outputs (for example PreviewAny text) and optionally the
/// PreviewAny and StringConcatenate nodes advertised by `/object_info`. The literal `{nonce}`
/// in extra outputs is replaced by the nonce the last grounding request sent in node 6.
pub fn server_with(
    images: Value,
    files: Vec<(&'static str, Vec<u8>)>,
    extra_outputs: Value,
    preview_any: bool,
) -> Server {
    let nodes: &[&str] = if preview_any {
        &["PreviewAny", "StringConcatenate"]
    } else {
        &[]
    };
    server_nodes(images, files, extra_outputs, nodes)
}

/// Like `server_with`, advertising exactly `extra_nodes` beyond the grounding basics.
pub fn server_nodes(
    images: Value,
    files: Vec<(&'static str, Vec<u8>)>,
    extra_outputs: Value,
    extra_nodes: &[&str],
) -> Server {
    let extra_nodes: Vec<String> = extra_nodes.iter().map(|n| (*n).to_owned()).collect();
    let nonce = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    Server::start(move |r| match r.path.as_str() {
        "/object_info" => {
            let mut nodes = json!({
                "LoadImage":{},"LoadSAM3Model":{},"SAM3InteractiveCollector":{},
                "SAM3Grounding":{},"MaskToImage":{},"PreviewImage":{}
            });
            for name in &extra_nodes {
                nodes[name.as_str()] = json!({});
            }
            Reply::Json(200, nodes.to_string())
        }
        "/upload/image" => Reply::Json(
            200,
            json!({
                "name":"source.png","type":"temp","subfolder":""
            })
            .to_string(),
        ),
        "/prompt" => {
            if let Some(sent) = r.body_json()["prompt"]
                .pointer("/6/inputs/string_a")
                .and_then(Value::as_str)
            {
                *nonce.lock().unwrap() = sent.to_owned();
            }
            Reply::Json(200, json!({"prompt_id":"batch-1"}).to_string())
        }
        "/history/batch-1" => {
            let substituted = extra_outputs
                .to_string()
                .replace("{nonce}", &nonce.lock().unwrap());
            let mut outputs: Value = serde_json::from_str(&substituted).unwrap();
            outputs["5"] = json!({"images":images});
            Reply::Json(
                200,
                json!({"batch-1":{"status":{"completed":true},"outputs":outputs}}).to_string(),
            )
        }
        path => files
            .iter()
            .find(|(name, _)| path == format!("/view?filename={name}&type=output"))
            .map(|(_, data)| {
                Reply::Raw(
                    [
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            data.len()
                        )
                        .into_bytes(),
                        data.clone(),
                    ]
                    .concat(),
                )
            })
            .unwrap_or_else(|| Reply::Json(404, "{}".into())),
    })
}

pub fn two_masks() -> Value {
    json!([
        {"filename":"left.png","type":"output","subfolder":""},
        {"filename":"right.png","type":"output","subfolder":""}
    ])
}
