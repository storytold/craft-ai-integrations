//! Video graphs that start from an image: a Wan 2.2 TI2V or I2V graph loads its first frame through
//! a `LoadImage` node, and the request's source image replaces the file saved in the graph.
use super::{Binding, Workflow};
use craft_ai_core::AiError;
use serde_json::Value;

impl Workflow {
    /// Import a `video.generate` graph with its start image mapped: a text prompt and one source
    /// image in, one video out. The source must map to a ComfyUI image loader and feed the output,
    /// like a selection source, and every request must then carry the image.
    pub fn import_video_from_image(
        id: &str,
        graph: Value,
        prompt: Binding,
        output_node: &str,
        source: Binding,
    ) -> Result<Self, AiError> {
        let mut workflow = Self::import(id, "video.generate", graph, prompt, output_node)?;
        workflow.check_media_binding(&source, "source")?;
        for (binding, role) in [(&workflow.prompt, "prompt"), (&source, "source")] {
            if !workflow.feeds_output(&binding.node) {
                return Err(AiError::invalid(format!(
                    "{role} node is not connected to the output"
                )));
            }
        }
        workflow.source = Some(source);
        Ok(workflow)
    }
}
