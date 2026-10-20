"""Deliberate-bug cases for group `sf`: ComfyUI helpers must leave nothing in the user's folders.

Loaded by mutation_check.py. Each case breaks one rule and names the test that should notice.
Format: (name, file, old, new, package, test target, test filter). Target 'lib' runs unit tests.
"""
C = 'crates/ai-comfyui/src/'
CU = 'craft-ai-comfyui'
BS = chr(92)
RN = BS + 'r' + BS + 'n'
HTTP = C + 'http.rs'
OUT = C + 'output.rs'
MASKS = C + 'sam3_masks.rs'
SCRATCH = C + 'scratch.rs'
ROUTE = 'crate::scratch::route_sinks(&mut graph)?;'

SF = [
    ('upload-goes-to-input', HTTP, RN + RN + 'temp' + RN + '--{boundary}', RN + RN + 'input' + RN + '--{boundary}', CU, 'scratch_files', 'scratch_area_not_input'),
    ('reply-in-input-accepted', HTTP, 'reply.get("type").and_then(Value::as_str) != Some("temp")', 'false', CU, 'scratch_files', 'landed_in_input'),
    ('scratch-tag-dropped', HTTP, 'Ok(format!("{name}{}", crate::scratch::TEMP_TAG))', 'Ok(name.to_owned())', CU, 'scratch_files', 'scratch_copy'),
    ('scratch-tag-unsafe', HTTP, 'let name = name.strip_suffix(crate::scratch::TEMP_TAG).unwrap_or(name);', '', CU, 'inpaint', 'uploads_source_and_mask'),
    ('sam21-sink-not-rerouted', C + 'sam21_session.rs', ROUTE, '', CU, 'scratch_files', 'scratch_copy'),
    ('sam3-point-sink-not-rerouted', C + 'sam3_point.rs', ROUTE, '', CU, 'sam3_point', 'a_point_queues'),
    ('sam3-queue-sink-not-rerouted', C + 'sam3_session/queue.rs', 'crate::scratch::route_sinks(&mut graph)?;', '', CU, 'sam3_cached', 'fresh_sink'),
    ('sink-shares-cache-key', SCRATCH, 'NEXT.fetch_add(1, Ordering::Relaxed)', '0', CU, 'lib', 'each_sink_gets_its_own_cache_key'),
    ('sink-prefix-not-added', SCRATCH, 'inputs.insert("filename_prefix".into(), Value::String(unique));', 'let _ = unique;', CU, 'lib', 'a_sink_with_no_prefix'),
    ('sink-class-kept', SCRATCH, 'node["class_type"] = Value::String("PreviewImage".into());', '', CU, 'lib', 'save_becomes_preview'),
    ('temp-result-refused', OUT, '!matches!(file_type, "output" | "temp")', 'file_type != "output"', CU, 'provider', 'left_in_the_scratch_area'),
    ('any-result-folder-accepted', OUT, '!matches!(file_type, "output" | "temp")', 'false', CU, 'provider', 'any_other_folder'),
    ('result-fetched-from-output', OUT, '"/view?filename={}&type={file_type}{}"', '"/view?filename={}&type=output{}"', CU, 'scratch_files', 'scratch_copy'),
    ('mask-temp-refused', MASKS, 'Some("output" | "temp")', 'Some("output")', CU, 'scratch_files', 'scratch_copy'),
    ('mask-any-folder-accepted', MASKS, ') || !safe(name)', ') && false || !safe(name)', CU, 'sam3_mask_failures', ''),
]

SF_EQUIVALENT = {
    'mask-any-folder-accepted': 'every mask is then fetched through output::fetch, which refuses any folder but output and temp; the any-result-folder-accepted case covers that second check on its own',
    # Group s21: route_sinks now also gives every SAM 2.1 sink a unique prefix.
    'selections-share-a-sink-name': 'route_sinks appends its own counter to every sink prefix before queuing, so the SAM 2.1 graph still gets a fresh cache key; sink-shares-cache-key covers that counter on its own',
}
