//! Instructions for image editors that accept a mask as a reference picture.
/// Describes white-means-edit mask guidance; callers still enforce pixel boundaries.
pub fn fill_instruction(prompt: &str) -> String {
    let prompt = prompt.trim().trim_end_matches('.').trim();
    let what = if prompt.is_empty() {
        "Replace the placeholder with what would be there if whatever stood in it had never existed: continue the \
         background, surfaces, textures and lighting from around it, and carry on anything that crosses its edge (lines, \
         edges, horizon, patterns, objects partly outside it) the way it would plausibly continue. Do not add a new \
         object, light or shape that nothing around it suggests."
            .to_owned()
    } else {
        format!(
            "Replace the placeholder with: {prompt}. Make it belong to the scene: the right size for its place in the \
             perspective, lit from the same direction, casting and receiving shadows and reflections, and interacting \
             with whatever touches it. Around the new content, continue the background and anything that crosses the \
             placeholder's edge."
        )
    };
    format!(
        "Edit the first image. The second image is a mask the same size: white marks the area of the first image to \
         fill, black the area to keep. Whatever the white area shows now (it may have been blurred or dimmed out) is \
         only a placeholder and not part of the scene: replace all of it with sharp, detailed content that has the same \
         focus, noise and grain as its surroundings. Before editing, study the whole first image: what the scene is, \
         where the light comes from, the perspective and depth, and what runs up to the placeholder. {what} Keep \
         everything in the black area, the framing, the scale and the position of every object exactly as they are. \
         Reply with the edited image only."
    )
}
