"""The Extended XMP fragment cap every JPEG decoder checks before `image`/`zune-jpeg` sees a file.

The counter lives in `craft-ai-core` (`jpeg.rs`); the rten and candle engines and the returned-
picture check in `craft-ai-media` each call it first. Each case breaks the counter or drops one
caller's check, and the test that builds a file one fragment over the cap must notice. The
decoder tests also put a stray byte before the fragments, which the decoder skips; the old
header walk stopped there and under-counted.
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
     '.filter(|w| *w == EXTENDED_XMP_PREFIX)',
     '.filter(|w| w.starts_with(b"http://ns.adobe.com/xap/"))',
     'craft-ai-core', 'lib', 'counts_each_fragment'),
    ('jpeg-count-stops-at-a-stray-byte', CORE,
     '.filter(|w| *w == EXTENDED_XMP_PREFIX)',
     '.take_while(|w| w.first() != Some(&0x42))\n        .filter(|w| *w == EXTENDED_XMP_PREFIX)',
     'craft-ai-core', 'lib', 'after_stray_bytes'),
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
