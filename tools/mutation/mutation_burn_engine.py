"""be: the Burn engine's own rules (saved-weights loading, the panic guard, the answer checks, the
file names it accepts), the catalogue route it is listed by, the binding's `.bpk` rule and the
shared revision.

Not reached here, because they need the real model, a graphics card or a graph the converter
panics on (the ignored live tests in `crates/ai-engine-burn/tests/live.rs` and
`crates/ai-client/tests/burn_engine.rs` are not run by this checker): the picture-size check in
`encode`, the click-count check in `segment`, the choice between the graphics card and the
processor, the converter's panic guard in `runner/prepare.rs`, and the cargo feature wiring in
`ai-client`.
Case shape: name, file, old, new, package, target, test filter.
"""
B = 'crates/ai-engine-burn/src/'
PKG = 'craft-ai-engine-burn'


def lib(name, file, old, new, test):
    """A case run against the engine crate's own unit tests."""
    return name, B + file, old, new, PKG, 'lib', test


BE = [
    lib('burn-magic-unchecked', 'backend.rs', '        && &head == MAGIC', '        && true',
        'only_a_file_that_starts'),
    lib('burn-missing-weights-allowed', 'backend.rs',
        '&& result.missing.is_empty()', '', 'a_load_that_left_a_weight'),
    lib('burn-weight-errors-allowed', 'backend.rs',
        'result.is_success() &&', '', 'a_load_that_left_a_weight'),
    lib('burn-panic-not-guarded', 'backend.rs',
        'catch_unwind(AssertUnwindSafe(work)).ok()', 'Some(work())', 'a_panic_in_guarded'),
    lib('burn-wrong-length-answer-accepted', 'backend.rs',
        'if masks.len() != PROPOSALS * side || scores.len() != PROPOSALS {', 'if false {',
        'an_answer_of_the_wrong_size'),
    lib('burn-best-proposal-ignored', 'backend.rs',
        'let best = best_proposal(scores);', 'let best = 0;', 'best_rated'),
    lib('burn-onnx-file-guessed', 'runner.rs',
        'if weights.file_name() == Some(OsStr::new("encoder.onnx")) {', 'if false {',
        'not_the_supported_export'),
    lib('burn-prepare-decoder-not-required', 'runner/prepare.rs',
        'filter(|d| d.join("decoder.onnx").is_file())', 'filter(|_| true)',
        'no_decoder_says_what_is_missing'),
    lib('burn-prepared-copy-inside-the-model-folder', 'runner/prepare.rs',
        'source.parent().map(|p| p.join(PREPARED_DIR))', 'Some(source.join(PREPARED_DIR))',
        'not_the_supported_export'),
    lib('burn-decoder-not-required', 'runner.rs',
        'd.join(DECODER_FILE).is_file() && weights.is_file()', 'weights.is_file()',
        'not_both_files'),
    lib('burn-encoder-name-not-checked', 'runner.rs',
        '.filter(|_| weights.file_name() == Some(OsStr::new(ENCODER_FILE)));',
        '.filter(|_| true);', 'not_both_files'),
    lib('burn-nvidia-not-declared', 'runner.rs',
        '    GpuVendor::Nvidia,\n    GpuVendor::Amd,', '    GpuVendor::Amd,\n    GpuVendor::Amd,',
        'is_the_burn_sam21'),
    ('burn-revision-bumped-in-the-architecture', B + 'model/mod.rs',
     'onnx-0.22.0-v1";', 'onnx-0.22.0-v2";', PKG, 'revision', 'the_architecture_and_the_saved'),
    ('burn-revision-bumped-in-the-cache-key', 'crates/ai-burn-setup/src/sam21.rs',
     'onnx-0.22.0-v1";', 'onnx-0.22.0-v2";', PKG, 'revision', 'the_architecture_and_the_saved'),
    ('burn-route-lists-no-files', 'crates/ai-features/src/catalogue/sam21.rs',
     '                models: sam21_onnx_files(),\n            },\n        ],',
     '                models: Vec::new(),\n            },\n        ],',
     'craft-ai-features', 'native_files', 'burns_sam_2_1_route'),
    ('burn-route-named-rten', 'crates/ai-features/src/catalogue/sam21.rs',
     'provider: "burn".to_owned(),', 'provider: "rten".to_owned(),',
     'craft-ai-features', 'native_files', 'burns_sam_2_1_route'),
    ('burn-bpk-refused-when-binding', 'crates/ai-client/src/local.rs',
     '["safetensors", "onnx", "bpk"]', '["safetensors", "onnx"]',
     'craft-ai-client', 'local_native', 'burn_route_is_registered'),
]
