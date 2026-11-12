//! Which steps of an imported workflow are known to keep the person's data on this computer.
//!
//! A ComfyUI workflow is a graph of steps (node classes). ComfyUI's own loading, sampling and
//! image steps work on this computer. An add-on step can do anything with what it is given,
//! including sending it to another host, and ComfyUI's own hosted-service steps do exactly that.
//! A connection to ComfyUI on this computer is local, but a workflow someone else wrote can still
//! route the picture and prompt through such a step. So every step outside the list below is
//! reported by name, and the app asks the person before the job is sent (see
//! `Disclosure::unknown_steps`).
//!
//! The list is deliberately short: only ComfyUI's own steps that read files from its folders,
//! transform images, latents, masks or text, or save results to its output folder. A step left
//! off the list is never refused; it only needs the person's agreement.

use serde_json::Value;

/// ComfyUI's own steps that keep their inputs on this computer. Exact class names.
pub const KNOWN_LOCAL_STEPS: &[&str] = &[
    // Loading from ComfyUI's own model folders.
    "CheckpointLoaderSimple",
    "CLIPLoader",
    "CLIPVisionLoader",
    "ControlNetLoader",
    "DualCLIPLoader",
    "LoraLoader",
    "LoraLoaderModelOnly",
    "StyleModelLoader",
    "UNETLoader",
    "UpscaleModelLoader",
    "VAELoader",
    // Text and conditioning.
    "CLIPSetLastLayer",
    "CLIPTextEncode",
    "CLIPVisionEncode",
    "ConditioningAverage",
    "ConditioningCombine",
    "ConditioningConcat",
    "ConditioningSetArea",
    "ConditioningZeroOut",
    "ControlNetApply",
    "ControlNetApplyAdvanced",
    "FluxGuidance",
    "InpaintModelConditioning",
    "StringConcatenate",
    "StyleModelApply",
    // Sampling.
    "BasicGuider",
    "BasicScheduler",
    "CFGGuider",
    "KSampler",
    "KSamplerAdvanced",
    "KSamplerSelect",
    "ModelSamplingDiscrete",
    "RandomNoise",
    "SamplerCustom",
    "SamplerCustomAdvanced",
    // Latents.
    "EmptyLatentImage",
    "EmptySD3LatentImage",
    "LatentBlend",
    "LatentComposite",
    "LatentFromBatch",
    "LatentUpscale",
    "LatentUpscaleBy",
    "RepeatLatentBatch",
    "SetLatentNoiseMask",
    "VAEDecode",
    "VAEDecodeTiled",
    "VAEEncode",
    "VAEEncodeForInpaint",
    "VAEEncodeTiled",
    // Images and masks.
    "CropMask",
    "EmptyImage",
    "FeatherMask",
    "GrowMask",
    "ImageBatch",
    "ImageCompositeMasked",
    "ImageCrop",
    "ImageInvert",
    "ImagePadForOutpaint",
    "ImageScale",
    "ImageScaleBy",
    "ImageToMask",
    "ImageUpscaleWithModel",
    "InvertMask",
    "LoadImage",
    "LoadImageMask",
    "MaskComposite",
    "MaskToImage",
    "SolidMask",
    // Results, kept in ComfyUI's own folders.
    "PreviewAny",
    "PreviewImage",
    "SaveImage",
];

/// The steps of `graph` that are not on [`KNOWN_LOCAL_STEPS`], sorted, each named once. Names
/// are exact: consent and the check that a workflow did not change compare them, so two steps
/// whose names differ anywhere stay two steps. A node with no readable class is reported as the
/// empty name. Shortening a name for the screen is the screen's job.
pub fn unknown_steps(graph: &Value) -> Vec<String> {
    let mut found: Vec<String> = graph
        .as_object()
        .into_iter()
        .flat_map(|nodes| nodes.values())
        .map(|node| node.get("class_type").and_then(Value::as_str).unwrap_or(""))
        .filter(|class| !KNOWN_LOCAL_STEPS.contains(class))
        .map(str::to_owned)
        .collect();
    found.sort();
    found.dedup();
    found
}
