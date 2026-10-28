//! The models a FANTASTICO account offers, and what each can do.
//!
//! The account service lists the chat models of the model provider behind it, entry for entry, with the `:batch` variants already removed because they cannot be called on
//! `chat/completions`. Each entry is read for what it says rather than guessed from its id:
//!
//! - `architecture.output_modalities` without `text` means it is not a chat model: left out;
//! - `supported_parameters` says whether it takes `tools` and `response_format`. When the list is
//!   absent both are assumed, and the model refuses at run time if it cannot;
//! - `pricing` is US dollars per token as a string, kept as millionths per million tokens;
//! - `created` is when the provider listed it, used as the release date.
//!
//! Every model offers `text.complete` with streaming. A model id the list does not include can
//! still be asked for (see [`assumed`]): the list changes daily and a saved choice should not
//! break because of it.

use craft_ai_core::{
    CapabilityId, CapabilitySpec, Features, InputRole, ModelId, ModelInfo, Pricing,
};
use serde_json::Value;

/// More entries than this in one list is not a catalogue but a hostile reply.
pub(crate) const MAX_MODELS: usize = 4096;
/// Labels are shown in a picker: longer ones are cut.
const MAX_LABEL_CHARS: usize = 120;
/// Most conversation turns in one request: a long agent session, with room to spare.
pub(crate) const MAX_TURNS: u32 = 1024;

/// No input is required: a request has prompts or conversation turns, and `chat::plan` refuses
/// one with neither.
fn text_spec(features: Features) -> Option<CapabilitySpec> {
    Some(
        CapabilitySpec::new(CapabilityId::parse("text.complete").ok()?)
            .accepting(InputRole::Prompt, 64)
            .accepting(InputRole::Context, 64)
            .accepting(InputRole::System, 1)
            .accepting(InputRole::parse(crate::chat::TURN).ok()?, MAX_TURNS)
            .with_features(features),
    )
}

/// What a model the list does not describe is taken to be: a chat model that streams, calls
/// tools and returns structured output.
pub(crate) fn assumed(id: &ModelId) -> Option<ModelInfo> {
    Some(
        ModelInfo::new(id.clone(), id.as_str()).with(text_spec(Features {
            streaming: true,
            tools: true,
            structured_output: true,
        })?),
    )
}

fn strings(v: Option<&Value>) -> Option<Vec<&str>> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
}

/// Dollars per token as a JSON string or number, to millionths of a dollar per million tokens.
fn micros_per_mtok(v: Option<&Value>) -> Option<u64> {
    let dollars = match v? {
        Value::String(s) => s.trim().parse::<f64>().ok()?,
        Value::Number(n) => n.as_f64()?,
        _ => return None,
    };
    // One dollar a token is already absurd; anything beyond is a unit mix-up, not a price.
    if !dollars.is_finite() || !(0.0..=1.0).contains(&dollars) {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some((dollars * 1.0e12).round() as u64)
}

/// One entry of the account's model list, or `None` when it is not a usable chat model.
pub(crate) fn from_entry(entry: &Value) -> Option<ModelInfo> {
    let id = ModelId::new(entry.get("id").and_then(Value::as_str)?).ok()?;
    let outputs = strings(entry.pointer("/architecture/output_modalities"));
    if outputs.is_some_and(|o| !o.contains(&"text")) {
        return None;
    }
    let features = match strings(entry.get("supported_parameters")) {
        Some(p) => Features {
            streaming: true,
            tools: p.contains(&"tools"),
            structured_output: p.contains(&"response_format") || p.contains(&"structured_outputs"),
        },
        None => Features {
            streaming: true,
            tools: true,
            structured_output: true,
        },
    };
    let label: String = entry
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(id.as_str())
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_LABEL_CHARS)
        .collect();
    let mut info = ModelInfo::new(id, label).with(text_spec(features)?);
    let pricing = entry.get("pricing");
    let price = Pricing {
        input_micros_per_mtok: micros_per_mtok(pricing.and_then(|p| p.get("prompt"))),
        output_micros_per_mtok: micros_per_mtok(pricing.and_then(|p| p.get("completion"))),
        per_image_micros: None,
    };
    if price.input_micros_per_mtok.is_some() || price.output_micros_per_mtok.is_some() {
        info.pricing = Some(price);
    }
    info.released = entry
        .get("created")
        .and_then(Value::as_u64)
        .filter(|t| *t > 0);
    Some(info)
}

/// The whole list, sorted by id, without duplicates.
pub(crate) fn from_list(entries: &[Value]) -> Vec<ModelInfo> {
    let mut found: Vec<ModelInfo> = entries
        .iter()
        .take(MAX_MODELS)
        .filter_map(from_entry)
        .collect();
    found.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
    found.dedup_by(|a, b| a.id == b.id);
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cap() -> CapabilityId {
        CapabilityId::parse("text.complete").unwrap()
    }

    #[test]
    fn a_catalogue_entry_becomes_a_chat_model_with_its_price_and_features() {
        let m = from_entry(&json!({
            "id": "anthropic/claude-sonnet-5", "name": "Anthropic: Claude Sonnet 5",
            "created": 1_760_000_000u64,
            "architecture": {"input_modalities": ["text", "image"], "output_modalities": ["text"]},
            "supported_parameters": ["tools", "tool_choice", "max_tokens"],
            "pricing": {"prompt": "0.000003", "completion": "0.000015"}
        }))
        .unwrap();
        assert_eq!(m.label, "Anthropic: Claude Sonnet 5");
        let spec = m.spec_for(&cap()).unwrap();
        assert!(spec.features.streaming && spec.features.tools);
        assert!(!spec.features.structured_output);
        let p = m.pricing.unwrap();
        assert_eq!(
            (p.input_micros_per_mtok, p.output_micros_per_mtok),
            (Some(3_000_000), Some(15_000_000))
        );
        assert_eq!(m.released, Some(1_760_000_000));
    }

    #[test]
    fn entries_that_are_not_chat_models_or_are_malformed_are_left_out() {
        for bad in [
            json!({"id": "x/image-only", "architecture": {"output_modalities": ["image"]}}),
            json!({"id": "has space"}),
            json!({"id": 7}),
            json!({}),
            json!("nope"),
        ] {
            assert!(from_entry(&bad).is_none(), "{bad}");
        }
        let both = from_entry(
            &json!({"id": "g/img", "architecture": {"output_modalities": ["image", "text"]}}),
        );
        assert!(both.is_some());
    }

    #[test]
    fn unknown_parameters_assume_everything_and_odd_fields_are_ignored() {
        let m = from_entry(&json!({
            "id": "meta/llama:free", "name": "  \u{7}x  ",
            "pricing": {"prompt": "free", "completion": "-1"}, "created": "yesterday"
        }))
        .unwrap();
        let f = m.spec_for(&cap()).unwrap().features;
        assert!(f.tools && f.structured_output);
        assert_eq!(m.label, "x");
        assert_eq!((m.pricing, m.released), (None, None));
    }

    #[test]
    fn the_list_is_sorted_deduplicated_and_capped() {
        let list = from_list(&[
            json!({"id": "b/m"}),
            json!({"id": "a/m"}),
            json!({"id": "b/m"}),
        ]);
        let ids: Vec<_> = list.iter().map(|m| m.id.as_str().to_owned()).collect();
        assert_eq!(ids, ["a/m", "b/m"]);
        let many: Vec<Value> = (0..MAX_MODELS + 10)
            .map(|i| json!({"id": format!("m/{i}")}))
            .collect();
        assert_eq!(from_list(&many).len(), MAX_MODELS);
    }

    #[test]
    fn an_unlisted_id_is_assumed_to_be_a_full_chat_model() {
        let m = assumed(&ModelId::new("new/model").unwrap()).unwrap();
        assert!(m.spec_for(&cap()).unwrap().features.tools);
    }
}
