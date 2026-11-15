//! Instructions for image editors that accept a mask as a reference picture.
//!
//! Two kinds of fill, told apart by the prompt:
//!
//! - **No prompt: remove.** The caller may have blurred or dimmed the white area out first. It is
//!   a placeholder, and the model continues the picture through it.
//! - **A prompt: edit in place.** The white area shows the real picture. The model changes only
//!   what the prompt asks for inside it and keeps everything else, so "change the eyebrows to
//!   bananas" over a whole face changes the eyebrows and leaves the face alone.
//!
//! The person's few words are expanded ("upsampled") into a full instruction here, so every
//! service that reads instructions gets the same rules. Callers still enforce the pixel boundary
//! by keeping only the masked part of the result.

/// The person's prompt, tidied: surrounding space and trailing full stops dropped, and runs of
/// white space (line breaks included) made single spaces.
fn tidy(prompt: &str) -> String {
    let joined = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    joined.trim_end_matches('.').trim_end().to_owned()
}

/// The instruction for a fill with no prompt: remove what is in the white area.
fn remove_instruction() -> String {
    "Edit the first image. The second image is a mask the same size: white marks the area of the \
     first image to fill, black the area to keep. Whatever the white area shows now (it may have \
     been blurred or dimmed out) is only a placeholder and not part of the scene: replace all of \
     it with sharp, detailed content that has the same focus, noise and grain as its \
     surroundings. Before editing, study the whole first image: what the scene is, where the \
     light comes from, the perspective and depth, and what runs up to the placeholder. Replace \
     the placeholder with what would be there if whatever stood in it had never existed: continue \
     the background, surfaces, textures and lighting from around it, and carry on anything that \
     crosses its edge (lines, edges, horizon, patterns, objects partly outside it) the way it \
     would plausibly continue. Do not add a new object, light or shape that nothing around it \
     suggests. Keep everything in the black area, the framing, the scale and the position of \
     every object exactly as they are. Reply with the edited image only."
        .to_owned()
}

/// The instruction for a fill with a prompt: change only what `request` asks for, inside the
/// white area, and keep the rest of the picture as it is.
fn edit_instruction(request: &str) -> String {
    format!(
        "Edit the first image. The second image is a mask the same size: white marks the only \
         area you may change, black the area that must stay exactly as it is. The requested \
         change is: \"{request}\". Before editing, study the whole first image: what the scene \
         and its subject are, where the light comes from, the perspective and depth. Make only \
         the requested change, and only inside the white area. Everything the request does not \
         name stays as it is now, inside the white area too: the same subject, identity, face, \
         expression, pose, hair, skin, clothing, objects, colours and background. If the request \
         names a part of the subject (an eye, a sleeve, a sign), change that part alone and keep \
         its place, size and angle. If it adds something, give it the right size for its place \
         in the perspective, light it from the same direction, and give it shadows and \
         reflections that match. The changed pixels must blend into the picture around them with \
         no visible seam or edge, with the same focus, noise, grain and colour balance. Keep \
         everything in the black area, the framing, the scale and the position of every object \
         exactly as they are. Reply with the edited image only."
    )
}

/// Describes white-means-edit mask guidance; callers still enforce pixel boundaries.
///
/// An empty prompt (after tidying) asks for removal, where the white area is a placeholder. A
/// prompt asks for an edit in place, where the white area shows the real picture and only what
/// the prompt names changes.
pub fn fill_instruction(prompt: &str) -> String {
    let request = tidy(prompt);
    if request.is_empty() {
        remove_instruction()
    } else {
        edit_instruction(&request)
    }
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

#[cfg(test)]
#[path = "instruction_tests.rs"]
mod tests;
