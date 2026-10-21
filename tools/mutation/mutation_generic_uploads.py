"""Deliberate-bug cases for group `gu`: the picture upload cache of the generic `image.segment` route
on ComfyUI (what keys it, how many it keeps, the resend after a cleared scratch area or a failed
job).

Loaded by mutation_check.py. Each case breaks one rule and names the test that should notice.
Format: (name, file, old, new, package, test target, test filter).

Not covered: the live test (`tests/generic_segment_live.rs`, ignored) is the only check of a real
graph.
"""
SR = 'crates/ai-comfyui/src/segment_route.rs'
PV = 'crates/ai-comfyui/src/provider.rs'
CC = 'craft-ai-comfyui'
FIND = 'if let Some((_, name)) = self.uploads().iter().find(|(k, _)| *k == key) {'
STALE = 'Err(e) if reused && e.kind == ErrorKind::InvalidRequest => {'

GU_CASES = [
    ('cache-key-ignores-the-bytes', SR, 'image.data.hash(&mut hasher);', '', CC, 'lib', 'the_cache_key_follows'),
    ('picture-sent-every-time', PV, FIND, 'if let Some((_, name)) = self.uploads().iter().find(|_| false) {', CC, 'segment_route', 'a_picture_is_sent_once'),
    ('nothing-is-forgotten', PV, 'if uploads.len() > REMEMBERED_UPLOADS {', 'if false {', CC, 'segment_route', 'the_oldest_picture'),
    ('stale-upload-not-retried', PV, STALE, 'Err(e) if false && reused && e.kind == ErrorKind::InvalidRequest => {', CC, 'segment_route', 'a_cleared_scratch'),
    ('fresh-refusal-retried', PV, STALE, 'Err(e) if e.kind == ErrorKind::InvalidRequest => {', CC, 'segment_route', 'a_refusal_for_a_fresh'),
    ('stale-name-kept', PV, '                self.uploads().retain(|(k, _)| *k != key);\n', '', CC, 'segment_route', 'a_cleared_scratch'),
    ('failed-job-keeps-the-upload', PV, '                self.uploads().clear();\n', '', CC, 'segment_route', 'a_failed_job_makes'),
]
