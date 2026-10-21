//! The `image.segment` contract: what every engine reads from a selection request and what every
//! app reads back, so a click, a box or "nothing there" means the same on every route.
//!
//! A request carries the picture as its `source` input, an optional description as its `prompt`
//! input, and clicks or a box in `params`. Positions are fractions of the picture as a person sees
//! it (0 to 1, from the top left), so they survive any resizing on the way:
//! - `points`: `[{"x": 0.5, "y": 0.4, "positive": true}]`; a negative click marks what to leave out;
//! - `box`: `{"x0": 0.1, "y0": 0.1, "x1": 0.6, "y1": 0.9}`.
//!
//! The answer is a grayscale PNG mask the size of the picture, white where selected, then one
//! `application/json` output, [`Found`]: whether anything matched, how sure the model is and,
//! when a model can tell, what the selection is. A route that finds nothing still returns an
//! empty mask and `matched: false`, so "no match" is never confused with a failed job.
//!
//! Everything read here may come from another program, so it is checked: positions inside the
//! picture, a bounded number of clicks, a short plain name.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::AiError;
use crate::job::Output;
use crate::payload::Payload;
use crate::request::Request;

/// The capability id this module describes.
pub const CAPABILITY: &str = "image.segment";
/// Most clicks one request may carry.
pub const MAX_POINTS: usize = 32;
/// Longest name a result may give its selection, in characters.
pub const MAX_NAME_CHARS: usize = 64;
/// The media type of the [`Found`] output.
pub const FOUND_MEDIA_TYPE: &str = "application/json";

/// One click, as a fraction of the picture's width and height.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Click {
    pub x: f32,
    pub y: f32,
    /// `true` on the thing to select, `false` on something to leave out.
    pub positive: bool,
}

/// A box around the thing to select, as fractions of the picture: top left, then bottom right.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Area {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

/// What a selection found, sent beside the mask.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Found {
    /// Whether anything was selected. `false` comes with an empty mask.
    pub matched: bool,
    /// How sure the model is, 0 to 1, when it says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
    /// What the selection is ("person", "sky"), when the model or a labeller can tell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Whether `v` is a usable position: a finite number from 0 to 1.
fn fraction(v: f32) -> bool {
    v.is_finite() && (0.0..=1.0).contains(&v)
}

/// `request` with `clicks` written as its `points` parameter.
pub fn with_clicks(request: Request, clicks: &[Click]) -> Request {
    let list: Vec<Value> = clicks
        .iter()
        .map(|c| json!({"x": c.x, "y": c.y, "positive": c.positive}))
        .collect();
    request.with_param("points", Value::Array(list))
}

/// `request` with `area` written as its `box` parameter.
pub fn with_area(request: Request, area: Area) -> Request {
    request.with_param(
        "box",
        json!({"x0": area.x0, "y0": area.y0, "x1": area.x1, "y1": area.y1}),
    )
}

/// The clicks in `request`, empty when it has none. Refused when `points` is not a list of at
/// most [`MAX_POINTS`] clicks inside the picture.
pub fn clicks(request: &Request) -> Result<Vec<Click>, AiError> {
    let Some(value) = request.params.get("points") else {
        return Ok(Vec::new());
    };
    let wrong = || AiError::invalid("points must be a list of {x, y, positive} inside the picture");
    let list = value.as_array().ok_or_else(wrong)?;
    if list.len() > MAX_POINTS {
        return Err(AiError::invalid(format!("at most {MAX_POINTS} points")));
    }
    list.iter()
        .map(|p| {
            if !p.is_object() {
                return Err(wrong());
            }
            let click: Click = serde_json::from_value(p.clone()).map_err(|_| wrong())?;
            if fraction(click.x) && fraction(click.y) {
                Ok(click)
            } else {
                Err(wrong())
            }
        })
        .collect()
}

/// The box in `request`, if any. Refused when it is not inside the picture with its top left
/// above and left of its bottom right.
pub fn area(request: &Request) -> Result<Option<Area>, AiError> {
    let Some(value) = request.params.get("box") else {
        return Ok(None);
    };
    let wrong =
        || AiError::invalid("box must be {x0, y0, x1, y1} inside the picture, top left first");
    // Only the object form: serde would also read a bare list, which other routes do not.
    if !value.is_object() {
        return Err(wrong());
    }
    let area: Area = serde_json::from_value(value.clone()).map_err(|_| wrong())?;
    let inside = [area.x0, area.y0, area.x1, area.y1]
        .into_iter()
        .all(fraction);
    if inside && area.x0 < area.x1 && area.y0 < area.y1 {
        Ok(Some(area))
    } else {
        Err(wrong())
    }
}

/// The description in `request`'s prompt input, if it has one that is not blank.
pub fn description(request: &Request) -> Option<&str> {
    request
        .inputs
        .iter()
        .find_map(|i| match (&i.role, &i.payload) {
            (crate::model::InputRole::Prompt, Payload::Text { text })
                if !text.trim().is_empty() =>
            {
                Some(text.as_str())
            }
            _ => None,
        })
}

/// `found` as the output that goes beside the mask.
pub fn found_output(found: &Found) -> Result<Output, AiError> {
    let data = serde_json::to_vec(found)
        .map_err(|e| AiError::invalid(format!("selection result: {e}")))?;
    Ok(Output {
        payload: Payload::inline(FOUND_MEDIA_TYPE, data)?,
        label: None,
    })
}

/// The first output that is a valid [`Found`], if any. A name that is too long or holds control
/// characters is dropped, and a score outside 0 to 1 is dropped, so neither can reach the screen.
pub fn read_found(outputs: &[Output]) -> Option<Found> {
    outputs.iter().find_map(|o| {
        let Payload::Inline { media_type, data } = &o.payload else {
            return None;
        };
        if media_type.as_str() != FOUND_MEDIA_TYPE {
            return None;
        }
        let mut found: Found = serde_json::from_slice(data).ok()?;
        found.score = found.score.filter(|s| fraction(*s));
        found.name = found.name.filter(|n| {
            !n.trim().is_empty()
                && n.chars().count() <= MAX_NAME_CHARS
                && !n.chars().any(char::is_control)
        });
        Some(found)
    })
}

/// The media type of the mask: the contract says an inline PNG.
pub const MASK_MEDIA_TYPE: &str = "image/png";

/// The first inline PNG output: the mask. A preview in another format, or an image that is only a
/// handle, is not the mask and is skipped.
pub fn mask(outputs: &[Output]) -> Option<&Payload> {
    outputs.iter().map(|o| &o.payload).find(|p| {
        matches!(
            p,
            Payload::Inline { media_type, .. } if media_type.as_str() == MASK_MEDIA_TYPE
        )
    })
}

#[cfg(test)]
mod tests;
