"""Deliberate-bug cases for group `rr`: the run record a real run keeps as evidence
(`ai-client/src/run_record.rs`).

Loaded by mutation_check.py. Each case breaks one rule and names the test that should notice.
Format: (name, file, old, new, package, test target, test filter).
"""
R = 'crates/ai-client/src/run_record.rs'
CL = 'craft-ai-client'
T = 'run_record'

RR = [
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
    ('key-kept-in-messages', R, '        out = out.replace(secret, "[key removed]");\n', '', CL, T, 'without_the_key_the_prompt_or_a_link'),
    ('prompt-kept-in-messages', R, '        out = out.replace(prompt, "[request text removed]");\n', '', CL, T, 'without_the_key_the_prompt_or_a_link'),
    ('links-kept-in-messages', R, 'if word.contains("://") || word.contains("file:") {', 'if false {', CL, T, 'without_the_key_the_prompt_or_a_link'),
    ('notice-not-cleaned', R, 'Event::Notice { message } => notices.push(clean(message)),', 'Event::Notice { message } => notices.push(message.clone()),', CL, T, 'a_job_not_yet_ended'),
    ('error-not-cleaned', R, 'message: clean(&error.message),', 'message: error.message.clone(),', CL, T, 'without_the_key_the_prompt_or_a_link'),
    ('spacing-lost', R, 'out.push_str(&piece[word.len()..]);', "out.push(' ');", CL, T, 'without_the_key_the_prompt_or_a_link'),
    ('usage-dropped', R, 'usage: finished.usage.clone(),', 'usage: None,', CL, T, 'a_job_not_yet_ended'),
]
