"""Deliberate-bug cases for group `gm`: the mask the generic `image.segment` route on ComfyUI hands
on (its size, its one channel, the empty-mask answer and the picture header).

Loaded by mutation_check.py. Each case breaks one rule and names the test that should notice.
Format: (name, file, old, new, package, test target, test filter).

Equivalent, so not listed: the pixel limit on the mask in `finish` (a mask must equal the size of
a picture that already passed the same limit).
"""
SR = 'crates/ai-comfyui/src/segment_route.rs'
PV = 'crates/ai-comfyui/src/provider.rs'
CC = 'craft-ai-comfyui'
SIZE = 'if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {'

GM_CASES = [
    ('mask-turn-allowed-for-png', SR, '|| (self.turnable && width == self.height && height == self.width)', '|| (width == self.height && height == self.width)', CC, 'lib', 'a_mask_of_the_wrong_size'),
    ('mask-size-not-checked', SR, 'if !size.accepts(width, height) {', 'if false {', CC, 'lib', 'a_mask_of_the_wrong_size'),
    ('any-pixel-is-selected', SR, 'gray.iter().any(|v| *v > 127)', 'gray.iter().any(|v| *v > 0)', CC, 'lib', 'half_is_the_line'),
    ('over-half-is-needed', SR, 'gray.iter().any(|v| *v > 127)', 'gray.iter().any(|v| *v > 128)', CC, 'lib', 'half_is_the_line'),
    ('empty-mask-not-blanked', SR, '    if !matched {\n        gray.iter_mut().for_each(|v| *v = 0);\n    }\n', '', CC, 'lib', 'a_mask_with_nothing_selected'),
    ('alpha-is-the-mask', SR, '.map(|p| p.first().copied().unwrap_or(0))', '.map(|p| p.last().copied().unwrap_or(0))', CC, 'lib', 'the_first_channel'),
    ('jpeg-answer-accepted', SR, 'if media_type.as_str() != "image/png" {', 'if false {', CC, 'lib', 'something_that_is_not_a_png'),
    ('table-marker-taken-for-frame', SR, '0xC0..=0xCF if !matches!(*marker, 0xC4 | 0xC8 | 0xCC) => {', '0xC0..=0xCF => {', CC, 'lib', 'jpeg_segments_before'),
    ('huge-picture-accepted', SR, SIZE, 'if width == 0 || height == 0 {', CC, 'lib', 'no_readable_or_a_huge'),
    ('empty-picture-accepted', SR, SIZE, 'if u64::from(width) * u64::from(height) > MAX_PIXELS {', CC, 'lib', 'no_readable_or_a_huge'),
    ('mask-handed-on-unchecked', PV, '.and_then(|event| segment_route::answer(event, &size)),', '.map(|event| vec![event]),', CC, 'segment_route', 'a_mask_of_the_wrong_size_fails'),
]
