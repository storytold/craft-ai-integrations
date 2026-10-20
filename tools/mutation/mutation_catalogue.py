"""Deliberate-bug cases for group `cg`: the SAM 2.1 catalogue entry and its disclosure.

Loaded by mutation_check.py. Each case breaks one rule and names the test that should notice.
Format: (name, file, old, new, package, test target, test filter).
"""
C = 'crates/ai-features/src/catalogue/'
FE = 'craft-ai-features'
CU = 'craft-ai-comfyui'

CG = [
    ('detector-name-drifts', C + 'florence2.rs', 'pub const FLORENCE2_LARGE_DIR: &str = "Florence-2-large";', 'pub const FLORENCE2_LARGE_DIR: &str = "Florence-2-Flux-Large";', CU, 'detector_name', ''),
    ('detector-files-dropped', C + 'sam21.rs', 'models.extend(florence2_large_files());', 'let _ = florence2_large_files();', FE, 'sam21', 'ready_only_with'),
    ('detector-file-unpinned', C + 'florence2.rs', 'revision: Some(REVISION.to_owned()),', 'revision: None,', FE, 'sam21', 'pinned_revision'),
    ('disclosure-gates-download', C + 'sam21.rs', '        url: Some(url.to_owned()),\n        accept_required: false,', '        url: Some(url.to_owned()),\n        accept_required: true,', FE, 'sam21', 'none_of_it_gates'),
    ('add-on-download-not-disclosed', C + 'sam21.rs', 'this add-on downloads it from Hugging Face by itself', 'this add-on may fetch it', FE, 'sam21', 'none_of_it_gates'),
    ('sam21-not-offered', C + 'mod.rs', 'select.locals = vec![sam3(), sam21()];', 'select.locals = vec![sam3()];', FE, 'sam21', 'offers_sam_3_then'),
]
