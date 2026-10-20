//! The Florence-2 detector that finds the words of a prompt in a picture, for SAM 2 and SAM 2.1.
//!
//! ComfyUI's Florence 2 add-on shows the folders of `models/LLM` as the `model` choices of its
//! `Florence2ModelLoader` node and loads only one of those; this loader never downloads. So the
//! detector must already sit in `models/LLM/Florence-2-large`, and the check below refuses a run
//! before any picture is uploaded when ComfyUI cannot see it.
use craft_ai_core::AiError;
use serde_json::Value;

/// The folder name the graph asks for. The shared catalogue installs the files into this folder.
pub const FLORENCE2_DETECTOR: &str = "Florence-2-large";

/// Fails unless the loader node lists the detector among its choices.
pub(crate) fn require_detector(nodes: &Value) -> Result<(), AiError> {
    let listed = nodes
        .get("Florence2ModelLoader")
        .and_then(|node| node.pointer("/input/required/model/0"))
        .and_then(Value::as_array)
        .is_some_and(|choices| {
            choices
                .iter()
                .any(|c| c.as_str() == Some(FLORENCE2_DETECTOR))
        });
    if listed {
        Ok(())
    } else {
        Err(AiError::unsupported(format!(
            "ComfyUI cannot see the {FLORENCE2_DETECTOR} detector in its LLM models folder; set the model up first"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn nodes(choices: Value) -> Value {
        json!({"Florence2ModelLoader": {"input": {"required": {"model": [choices, {}]}}}})
    }

    #[test]
    fn accepts_a_loader_that_lists_the_detector() {
        assert!(require_detector(&nodes(json!(["other", "Florence-2-large"]))).is_ok());
    }

    #[test]
    fn refuses_a_loader_without_it_or_without_choices() {
        assert!(require_detector(&nodes(json!(["Florence-2-Flux-Large"]))).is_err());
        assert!(require_detector(&nodes(json!([]))).is_err());
        assert!(require_detector(&json!({"Florence2ModelLoader": {}})).is_err());
        assert!(require_detector(&json!({})).is_err());
    }
}
