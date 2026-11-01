//! The built-in feature list.
//!
//! Local recipes are only listed where the source, size and checksum were checked against the
//! real file. A feature with no recipe says so, and the screen shows it as not installable yet.
//! Cloud options are only listed where an adapter exists in this workspace; none of them has been
//! run against its real service.

use crate::feature::{AppStatus, AppUse, Feature};
use crate::recipe::CloudOption;

mod florence2;
mod inspyrenet;
mod sam21;
mod sam3;
use craft_ai_core::{CapabilityId, InputRole};
pub use florence2::FLORENCE2_LARGE_DIR;
use inspyrenet::inspyrenet;
use sam3::sam3;
use sam21::sam21;

fn uses(list: &[(&str, &str)]) -> Vec<AppUse> {
    list.iter()
        .map(|(app, tool)| AppUse {
            app: (*app).to_owned(),
            tool: (*tool).to_owned(),
            status: AppStatus::Proposed,
        })
        .collect()
}

/// A hosted option for a feature: the preset key, the label shown and a short note.
fn cloud(preset: &str, label: &str, note: &str) -> CloudOption {
    CloudOption {
        preset: preset.to_owned(),
        label: label.to_owned(),
        note: note.to_owned(),
    }
}

/// Builds a feature with no inputs, uses, local recipes or hosted options; the caller fills in
/// what the feature offers. Returns `None` when `capability` is not a valid capability id.
fn feature(id: &str, title: &str, summary: &str, capability: &str) -> Option<Feature> {
    Some(Feature {
        id: id.to_owned(),
        title: title.to_owned(),
        summary: summary.to_owned(),
        capability: CapabilityId::parse(capability).ok()?,
        inputs: Vec::new(),
        uses: Vec::new(),
        locals: Vec::new(),
        cloud: Vec::new(),
    })
}

/// Every feature the launcher offers, in the order it shows them. A test pins the count and
/// every capability id, so the empty list this falls back to cannot ship.
pub fn builtin() -> Vec<Feature> {
    try_builtin().unwrap_or_default()
}

/// Builds the whole catalogue, or `None` if any capability id fails to parse. `builtin` turns
/// `None` into an empty list and a test pins the count, so a bad id cannot ship.
fn try_builtin() -> Option<Vec<Feature>> {
    let mut select = feature(
        "select-subject",
        "Select subject",
        "Pick out the main subject, or anything you describe in words, and turn it into a selection or a mask.",
        "image.segment",
    )?;
    select.uses = uses(&[
        ("PhotoCraft", "Select Subject and Object Selection"),
        ("LightCraft", "Subject, sky and people masks"),
        ("EffectCraft", "Roto and matte"),
    ]);
    select.inputs = vec![InputRole::Source, InputRole::Prompt];
    select.locals = vec![sam21(), sam3()];

    let mut background = feature(
        "remove-background",
        "Remove background",
        "Cut the subject out and leave the background transparent.",
        "image.matte",
    )?;
    background.inputs = vec![InputRole::Source];
    background.uses = uses(&[("PhotoCraft", "Remove Background")]);
    // One model, built in only (Burn), and no hosted route yet.
    background.locals = vec![inspyrenet()];

    let mut remove = feature(
        "remove-object",
        "Remove objects",
        "Mark something and the area fills in to match its surroundings.",
        "image.inpaint",
    )?;
    remove.inputs = vec![InputRole::Source, InputRole::Mask];
    remove.uses = uses(&[
        ("PhotoCraft", "Remove tool"),
        ("LightCraft", "Generative remove"),
    ]);

    let mut fill = feature(
        "generative-fill",
        "Generative fill",
        "Select an area, describe what should be there, and get new content that blends in.",
        "image.inpaint",
    )?;
    fill.inputs = vec![InputRole::Source, InputRole::Mask, InputRole::Prompt];
    fill.uses = uses(&[("PhotoCraft", "Generative Fill")]);
    fill.cloud = vec![cloud(
        "openai",
        "OpenAI image editing",
        "Your own OpenAI key, billed by OpenAI. Built and tested offline only.",
    )];

    let mut expand = feature(
        "generative-expand",
        "Generative expand",
        "Make the canvas bigger and fill the new area to match the picture.",
        "image.outpaint",
    )?;
    expand.inputs = vec![InputRole::Source, InputRole::Prompt];
    expand.uses = uses(&[("PhotoCraft", "Generative Expand")]);

    let mut extend = feature(
        "extend-sequence",
        "Extend sequence",
        "Add frames to the end of a clip so the shot carries on.",
        "video.generate",
    )?;
    extend.inputs = vec![InputRole::Source, InputRole::Prompt];
    extend.uses = uses(&[
        ("FilmCraft", "Generative video"),
        ("EffectCraft", "Generative video"),
    ]);

    let mut image = feature(
        "generate-image",
        "Generate images",
        "Make a picture from a description.",
        "image.generate",
    )?;
    image.inputs = vec![InputRole::Prompt];
    image.uses = uses(&[
        ("PhotoCraft", "Generate Image"),
        ("DesignCraft", "Image generation"),
        ("DeckCraft", "Slide images"),
    ]);
    image.cloud = vec![
        cloud("openai", "OpenAI", "Your own OpenAI key, billed by OpenAI."),
        cloud(
            "bfl",
            "Black Forest Labs FLUX",
            "Your own BFL key, billed by Black Forest Labs.",
        ),
        cloud(
            "vertex-express",
            "Google Gemini image (Nano Banana)",
            "Express-mode API key only. The key cannot be tested without a paid request.",
        ),
        cloud(
            "artcraft-omni",
            "ArtCraft",
            "Staff-enabled API key. Whether a subscription covers it is unverified.",
        ),
    ];

    let mut video = feature(
        "generate-video",
        "Generate video",
        "Make a clip from a description.",
        "video.generate",
    )?;
    video.inputs = vec![InputRole::Prompt];
    video.uses = uses(&[
        ("FilmCraft", "Generative video"),
        ("EffectCraft", "Generative video"),
    ]);
    video.cloud = vec![
        cloud(
            "bfl",
            "Black Forest Labs",
            "Your own BFL key, billed by Black Forest Labs.",
        ),
        cloud(
            "seedance",
            "Seedance (BytePlus ModelArk)",
            "Text to video with your BytePlus ModelArk key.",
        ),
        cloud(
            "artcraft-omni",
            "ArtCraft",
            "Staff-enabled API key. Whether a subscription covers it is unverified.",
        ),
    ];

    Some(vec![
        select, background, remove, fill, expand, extend, image, video,
    ])
}
