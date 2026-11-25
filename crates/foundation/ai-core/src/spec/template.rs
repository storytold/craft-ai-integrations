//! The only substitutions a spec may make: `{prompt}`, `{model}` and `{job_id}`.
//!
//! The structure of a request always comes from the spec. A substitution replaces text inside a
//! string value of a JSON template, so a prompt can never add a field or change a key, and the
//! inserted text is not read again for more substitutions. `{{` and `}}` write a literal brace.

use serde_json::Value;

use super::MAX_TEMPLATE_BYTES;

/// A value a template may name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Prompt,
    Model,
    JobId,
}

impl Slot {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "prompt" => Some(Self::Prompt),
            "model" => Some(Self::Model),
            "job_id" => Some(Self::JobId),
            _ => None,
        }
    }
}

/// The values for one call. A slot left `None` cannot be used by that call.
#[derive(Clone, Copy, Debug, Default)]
pub struct Values<'a> {
    pub prompt: Option<&'a str>,
    pub model: Option<&'a str>,
    pub job_id: Option<&'a str>,
}

impl Values<'_> {
    fn get(&self, slot: Slot) -> Option<&str> {
        match slot {
            Slot::Prompt => self.prompt,
            Slot::Model => self.model,
            Slot::JobId => self.job_id,
        }
    }

    /// The most bytes one call's substitutions may add in total: each value once, plus a
    /// template's worth of slack for short values used in several places. A template that names
    /// `{prompt}` many times cannot multiply a large prompt past this.
    pub fn budget(&self) -> usize {
        MAX_TEMPLATE_BYTES
            + [self.prompt, self.model, self.job_id]
                .iter()
                .map(|v| v.map_or(0, str::len))
                .sum::<usize>()
    }
}

enum Piece {
    Text(String),
    Slot(Slot, String),
}

/// Splits `text` into literal text and slots, or says why its braces are wrong.
fn pieces(text: &str) -> Result<Vec<Piece>, String> {
    let mut out = Vec::new();
    let mut literal = String::new();
    let mut rest = text;
    while let Some(i) = rest.find(['{', '}']) {
        literal.push_str(&rest[..i]);
        let here = &rest[i..];
        let after = &rest[i + 1..];
        if here.starts_with("{{") || here.starts_with("}}") {
            literal.push_str(&here[..1]);
            rest = &after[1..];
        } else if here.starts_with('}') {
            return Err("a lone } (write }} for a literal brace)".into());
        } else {
            let end = after
                .find('}')
                .ok_or("a { that is never closed (write {{ for a literal brace)")?;
            let name = &after[..end];
            let slot =
                Slot::parse(name).ok_or_else(|| format!("unsupported: placeholder {{{name}}}"))?;
            if !literal.is_empty() {
                out.push(Piece::Text(std::mem::take(&mut literal)));
            }
            out.push(Piece::Slot(slot, name.to_owned()));
            rest = &after[end + 1..];
        }
    }
    literal.push_str(rest);
    if !literal.is_empty() {
        out.push(Piece::Text(literal));
    }
    Ok(out)
}

/// The slots `text` uses, or why its braces are wrong.
pub fn slots(text: &str) -> Result<Vec<Slot>, String> {
    Ok(pieces(text)?
        .into_iter()
        .filter_map(|p| match p {
            Piece::Slot(slot, _) => Some(slot),
            Piece::Text(_) => None,
        })
        .collect())
}

/// Every slot used by a string value anywhere in `template`. Object keys must be literal.
pub fn value_slots(template: &Value) -> Result<Vec<Slot>, String> {
    let mut out = Vec::new();
    match template {
        Value::String(s) => out.extend(slots(s)?),
        Value::Array(items) => {
            for item in items {
                out.extend(value_slots(item)?);
            }
        }
        Value::Object(map) => {
            for (key, item) in map {
                if key.contains(['{', '}']) {
                    return Err(format!("a key cannot hold a brace ({key:?})"));
                }
                out.extend(value_slots(item)?);
            }
        }
        _ => {}
    }
    Ok(out)
}

/// Why a call is refused when its substitutions do not fit the call budget. A caller can tell
/// this apart from a spec's own mistakes, since it is the caller's value that is too large.
pub const TOO_LARGE: &str = "the call is too large once its values are filled in";

/// Renders `text`, taking every inserted value's bytes from `budget` (start one call at
/// [`Values::budget`]) so a call cannot grow past it however many slots it names.
pub fn render_text(text: &str, values: &Values<'_>, budget: &mut usize) -> Result<String, String> {
    let mut out = String::new();
    for piece in pieces(text)? {
        match piece {
            Piece::Text(t) => out.push_str(&t),
            Piece::Slot(slot, name) => {
                let value = values
                    .get(slot)
                    .ok_or_else(|| format!("{{{name}}} is not available in this call"))?;
                *budget = budget.checked_sub(value.len()).ok_or(TOO_LARGE)?;
                out.push_str(value);
            }
        }
    }
    Ok(out)
}

/// Renders every string value of `template`. Keys are never substituted.
pub fn render_value(
    template: &Value,
    values: &Values<'_>,
    budget: &mut usize,
) -> Result<Value, String> {
    Ok(match template {
        Value::String(s) => Value::String(render_text(s, values, budget)?),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|i| render_value(i, values, budget))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| Ok((k.clone(), render_value(v, values, budget)?)))
                .collect::<Result<_, String>>()?,
        ),
        other => other.clone(),
    })
}
