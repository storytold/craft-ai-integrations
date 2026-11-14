//! Keeps everything these helpers hand to ComfyUI in its scratch area, so nothing of ours piles up
//! in the user's `input/` or `output/` folders.
//!
//! Invariants:
//! - A source image is uploaded with `type=temp` (see `Http::upload_image`), never `input`.
//! - A graph is queued only after [`route_sinks`] has turned every `SaveImage` into a `PreviewImage`,
//!   which writes to the scratch area ComfyUI clears when it starts.
//! - Each sink keeps a unique `filename_prefix`. A `PreviewImage` ignores it, but it changes the
//!   node's cache key. Without it, asking for the same selection twice runs from cache and the
//!   second history record has no outputs at all.
use craft_ai_core::AiError;
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};

/// The ComfyUI tag that points a loader at the scratch area instead of `input/`.
pub(crate) const TEMP_TAG: &str = " [temp]";

/// Turn every `SaveImage` in `graph` into a scratch-only `PreviewImage` with a unique cache key.
pub(crate) fn route_sinks(graph: &mut Value) -> Result<(), AiError> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nodes = graph
        .as_object_mut()
        .ok_or_else(|| AiError::invalid("workflow graph is invalid"))?;
    for node in nodes.values_mut() {
        if node.get("class_type").and_then(Value::as_str) != Some("SaveImage") {
            continue;
        }
        node["class_type"] = Value::String("PreviewImage".into());
        let inputs = node
            .get_mut("inputs")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| AiError::invalid("workflow sink has no inputs"))?;
        let base = match inputs.get("filename_prefix") {
            None => "craft",
            Some(prefix) => prefix
                .as_str()
                .ok_or_else(|| AiError::invalid("workflow output prefix is invalid"))?,
        };
        let unique = format!(
            "{base}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        inputs.insert("filename_prefix".into(), Value::String(unique));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A `SaveImage` becomes a `PreviewImage`; the loader and its scratch input are left alone.
    #[test]
    fn save_becomes_preview_and_other_nodes_are_untouched() {
        let mut g = json!({
            "1":{"class_type":"LoadImage","inputs":{"image":"a.png [temp]"}},
            "2":{"class_type":"SaveImage","inputs":{"images":["1",0],"filename_prefix":"mask"}}
        });
        route_sinks(&mut g).unwrap();
        assert_eq!(g["2"]["class_type"], "PreviewImage");
        assert_eq!(g["1"]["class_type"], "LoadImage");
        assert_eq!(g["1"]["inputs"]["image"], "a.png [temp]");
    }

    /// Two routings of the same graph give the sink different prefixes that keep the original base.
    #[test]
    fn each_sink_gets_its_own_cache_key() {
        let one = || json!({"2":{"class_type":"SaveImage","inputs":{"images":["1",0],"filename_prefix":"mask"}}});
        let (mut a, mut b) = (one(), one());
        route_sinks(&mut a).unwrap();
        route_sinks(&mut b).unwrap();
        assert_ne!(
            a["2"]["inputs"]["filename_prefix"],
            b["2"]["inputs"]["filename_prefix"]
        );
        assert!(
            a["2"]["inputs"]["filename_prefix"]
                .as_str()
                .unwrap()
                .starts_with("mask-")
        );
    }

    /// A sink without a `filename_prefix` still gets a unique one, based on `craft`.
    #[test]
    fn a_sink_with_no_prefix_still_gets_a_key() {
        let mut g = json!({"2":{"class_type":"SaveImage","inputs":{"images":["1",0]}}});
        route_sinks(&mut g).unwrap();
        assert!(
            g["2"]["inputs"]["filename_prefix"]
                .as_str()
                .unwrap()
                .starts_with("craft-")
        );
    }

    /// A prefix that is not text is an invalid graph and is refused rather than overwritten.
    #[test]
    fn a_non_text_prefix_is_refused() {
        let mut g =
            json!({"2":{"class_type":"SaveImage","inputs":{"images":["1",0],"filename_prefix":5}}});
        assert!(route_sinks(&mut g).is_err());
    }
}
