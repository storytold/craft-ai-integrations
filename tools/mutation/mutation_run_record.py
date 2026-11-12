"""Deliberate-bug cases for group `rr`: the run record a real run keeps as evidence
(`ai-client/src/run_record.rs`).

Loaded by mutation_check.py. Each case breaks one rule and names the test that should notice.
Format: (name, file, old, new, package, test target, test filter).
"""
R = 'crates/ai-client/src/run_record.rs'
CL = 'craft-ai-client'
T = 'run_record'
P = 'crates/ai-client/examples/live_openai/output_paths.rs'
PT = 'live_output_paths'

RR = [
    # Run-record and image destinations cannot corrupt the real shared store, even through aliases.
    ('record-can-name-store', P, '&& same_file(record, store)?', '&& false && same_file(record, store)?', CL, PT, 'a_record_cannot_append'),
    ('image-can-name-store', P, '&& same_file(image, store)?', '&& false && same_file(image, store)?', CL, PT, 'an_image_cannot_replace'),
    ('hardlink-identity-ignored', P, 'Ok(matches!((a, b), (Some(a), Some(b)) if a == b))', 'Ok(false)', CL, PT, 'a_record_hardlink_to_the_shared_store'),
    ('record-image-alias-allowed', P, '&& same_file(record, image)?', '&& false && same_file(record, image)?', CL, PT, 'record_and_image_hardlinks'),
    ('identity-error-treated-as-different', P, 'Err(error) => Err(error),', 'Err(_) => Ok(None),', CL, PT, 'an_unreadable_output_identity'),
    ('key-check-off', R, '.any(|s| !s.is_empty() && (line.contains(s) || line.contains(&json_escaped(s))))', '.any(|_: &&str| false)', CL, T, 'refused_even_when_escaped'),
    ('empty-secret-refuses-everything', R, '!s.is_empty() && (line.contains(s)', '(line.contains(s)', CL, T, 'refused_even_when_escaped'),
    ('link-address-kept', R, 'media_type: h.media_type.as_str().to_owned(),\n                size_bytes', 'media_type: h.uri.clone(),\n                size_bytes', CL, T, 'never_a_link'),
    ('hash-of-nothing', R, 'sha256: hex(&Sha256::digest(data)),', 'sha256: hex(&Sha256::digest(b"")),', CL, T, 'type_size_and_hash'),
    ('failure-reads-as-success', R, '                RunOutcome::Failed,\n', '                RunOutcome::Succeeded,\n', CL, T, 'a_failed_run_keeps_the_reason'),
    ('unended-job-reads-as-cancelled', R, 'JobState::Queued | JobState::Running { .. } => (RunOutcome::Unfinished, None),', 'JobState::Queued | JobState::Running { .. } => (RunOutcome::Cancelled, None),', CL, T, 'a_job_not_yet_ended'),
    ('notices-dropped', R, 'Event::Notice { message } => notices.push(clean(message)),', 'Event::Notice { .. } => {}', CL, T, 'a_job_not_yet_ended'),
    ('streamed-text-dropped', R, 'Event::TextDelta { text } => streamed.push_str(text),', 'Event::TextDelta { .. } => {}', CL, T, 'never_a_link'),
    ('start-time-to-the-second', R, 'SecondsFormat::Millis, true', 'SecondsFormat::Secs, true', CL, T, 'recorded_without_its_words'),
    ('key-check-escaped-form-skipped', R, ' || line.contains(&json_escaped(s))', '', CL, T, 'refused_even_when_escaped'),
    ('key-kept-in-messages', R, '.map(|s| (*s, Removed::Key))', '.filter(|_| false).map(|s| (*s, Removed::Key))', CL, T, 'without_the_key_the_prompt_or_a_link'),
    ('prompt-kept-in-messages', R, '.chain(prompts.iter().map(|p| (*p, Removed::RequestText)))', '.chain(prompts.iter().filter(|_| false).map(|p| (*p, Removed::RequestText)))', CL, T, 'without_the_key_the_prompt_or_a_link'),
    ('overlaps-not-merged', R, 'Some(last) if start < last.1 => {', 'Some(last) if false => {', CL, T, 'a_key_inside_the_request_text'),
    ('merged-key-reads-as-text', R, '                last.2 = last.2.max(kind);\n', '', CL, T, 'a_key_inside_the_request_text'),
    ('links-kept-in-messages', R, r'if word.contains("://") || word.contains(r":\/\/") || word.contains("file:") {', 'if false {', CL, T, 'without_the_key_the_prompt_or_a_link'),
    ('notice-not-cleaned', R, 'Event::Notice { message } => notices.push(clean(message)),', 'Event::Notice { message } => notices.push(message.clone()),', CL, T, 'a_job_not_yet_ended'),
    ('error-not-cleaned', R, 'message: clean(&error.message),', 'message: error.message.clone(),', CL, T, 'without_the_key_the_prompt_or_a_link'),
    ('spacing-lost', R, 'spans.push((at, at + word.len(), Removed::Link));', 'spans.push((at, at + piece.len(), Removed::Link));', CL, T, 'without_the_key_the_prompt_or_a_link'),
    ('overlapping-repeats-skipped', R, 'from = at + step;', 'from = at + needle.len();', CL, T, 'repeated_with_overlap'),
    ('usage-dropped', R, 'usage: finished.usage.clone(),', 'usage: None,', CL, T, 'a_job_not_yet_ended'),
    ('param-values-kept', R, 'prompts.extend(text_values(plan.request.params.values()));', '', CL, T, 'text_parameter_values'),
    ('nested-param-values-skipped', R, 'Value::Object(map) => pending.extend(map.values()),', 'Value::Object(_) => {}', CL, T, 'text_parameter_values'),
    ('nested-list-values-skipped', R, 'Value::Array(items) => pending.extend(items),', 'Value::Array(_) => {}', CL, T, 'text_parameter_values'),
    ('escaped-form-skipped', R, 'let differs = escaped != s;', 'let differs = false;', CL, T, 'json_escaped_form'),
    ('result-text-kept', R, '.chain(results.iter().map(|r| (*r, Removed::ResultText)))', '.chain(results.iter().filter(|_| false).map(|r| (*r, Removed::ResultText)))', CL, T, 'returned_text_is_removed'),
    ('streamed-result-kept', R, 'results.push(&streamed);', '', CL, T, 'returned_text_is_removed'),
    ('escaped-link-kept', R, r' || word.contains(r":\/\/")', '', CL, T, 'a_json_escaped_link'),
]
