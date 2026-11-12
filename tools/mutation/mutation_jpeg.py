"""The Extended XMP fragment cap every JPEG decoder checks before `image`/`zune-jpeg` sees a file.

The counter lives in `craft-ai-core` (`jpeg.rs`); the rten and candle engines and the returned-
picture check in `craft-ai-media` each call it first. Each case breaks the counter or drops one
caller's check, and the test that builds a file one fragment over the cap must notice.
"""

CORE = 'crates/ai-core/src/jpeg.rs'
RTEN = 'crates/ai-engine-rten/src/picture.rs'
CANDLE = 'crates/ai-engine-candle/src/jpeg.rs'
MEDIA = 'crates/ai-media/src/result_image.rs'
FILTER = 'a_jpeg_made_of_tiny_xmp_fragments_is_refused_before_decoding'
JX = [
    ('jpeg-cap-off-by-one', CORE,
     'extended_xmp_fragments(data) > MAX_XMP_FRAGMENTS',
     'extended_xmp_fragments(data) > MAX_XMP_FRAGMENTS + 1',
     'craft-ai-core', 'lib', 'the_cap_is_inclusive'),
    ('jpeg-cap-refuses-the-limit', CORE,
     'extended_xmp_fragments(data) > MAX_XMP_FRAGMENTS',
     'extended_xmp_fragments(data) >= MAX_XMP_FRAGMENTS',
     'craft-ai-core', 'lib', 'the_cap_is_inclusive'),
    ('jpeg-fragments-not-counted', CORE,
     "if marker == 0xE1 && body.is_some_and(|b| b.starts_with(EXTENDED_XMP_PREFIX)) {",
     "if marker == 0xE2 && body.is_some_and(|b| b.starts_with(EXTENDED_XMP_PREFIX)) {",
     'craft-ai-core', 'lib', 'counts_each_fragment'),
    ('jpeg-any-app1-counted', CORE,
     "if marker == 0xE1 && body.is_some_and(|b| b.starts_with(EXTENDED_XMP_PREFIX)) {",
     "if marker == 0xE1 {",
     'craft-ai-core', 'lib', 'other_app1_segments'),
    ('jpeg-scan-data-counted', CORE,
     'Some(0xDA) | None => break,',
     'None => break,',
     'craft-ai-core', 'lib', 'counts_each_fragment'),
    ('rten-cap-not-checked', RTEN,
     'if too_many_xmp_fragments(data) {',
     'if false {',
     'craft-ai-engine-rten', 'lib', FILTER),
    ('candle-cap-not-checked', CANDLE,
     'if too_many_xmp_fragments(data) {',
     'if false {',
     'craft-ai-engine-candle', 'lib', FILTER),
    ('result-cap-not-checked', MEDIA,
     '!too_many_xmp_fragments(data)\n                && reaches_eoi(data)',
     'reaches_eoi(data)',
     'craft-ai-media', 'lib', FILTER),
]
