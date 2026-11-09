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

/// [`fill_instruction`] for a request that also sends `references` reference pictures after the
/// source and the mask (the third image onwards). With none it is [`fill_instruction`] unchanged.
/// The references guide what the new content looks like; they are never pasted in whole and never
/// change the area to keep.
pub fn fill_instruction_with_references(prompt: &str, references: usize) -> String {
    let base = fill_instruction(prompt);
    let (which, them) = match references {
        0 => return base,
        1 => ("The third image is a reference picture".to_owned(), "it"),
        n => (
            format!("Images 3 to {} are reference pictures", n + 2),
            "them",
        ),
    };
    format!(
        "{base} {which} from the person: use {them} only as a guide to what the new content in the \
         white area should look like, such as its subject, style, colours or materials. Do not paste \
         {them} in whole, and do not change the black area to match {them}."
    )
}
