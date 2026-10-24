"""Deliberate-bug checks for the FANTASTICO adapter (`crates/ai-fantastico`).

Do not import the runner here: it executes immediately. Each case breaks one safety rule and names
the focused runtime test that must fail, so a compile error never counts as evidence.
"""
SRC = 'crates/ai-fantastico/src/'
CHAT = SRC + 'chat.rs'
ERRORS = SRC + 'errors.rs'
JOBS = SRC + 'jobs.rs'
MODELS = SRC + 'models.rs'
STREAM = SRC + 'stream.rs'
TRANSPORT = SRC + 'transport.rs'
P = 'craft-ai-fantastico'
FA = [
    # Request params pass through an allow-list: `user` would attribute spending to another account.
    ('user-param-passed-through', CHAT,
     '    "response_format",\n];', '    "response_format",\n    "user",\n];',
     P, 'lib', 'only_listed_params_pass_through_and_user_is_not_one'),
    # The account key is scrubbed from every error message.
    ('account-key-not-redacted', ERRORS,
     'AiError::redacted(kind, &message, &[key])', 'AiError::new(kind, message)',
     P, 'lib', 'the_account_key_never_reaches_the_message'),
    # A server-sent event line longer than 1 MiB is refused, not buffered.
    ('stream-line-cap-dropped', STREAM,
     'if line.len() > MAX_LINE_BYTES {', 'if false {',
     P, 'lib', 'an_error_event_becomes_the_body_and_junk_never_panics'),
    # A job's unpolled event backlog is capped: past it the job fails instead of growing.
    ('event-backlog-cap-dropped', JOBS,
     'if g.events.len() >= MAX_EVENTS {', 'if false {',
     P, 'lib', 'a_backlog_past_the_cap_fails_the_job'),
    # A conversation turn must be a system, user, assistant or tool message.
    ('any-turn-role-accepted', CHAT,
     'if !matches!(role, "system" | "user" | "assistant" | "tool") {', 'if false {',
     P, 'lib', 'bad_turns_are_refused'),
    # A model whose outputs don't include text is not offered for chat.
    ('image-only-model-listed', MODELS,
     'if outputs.is_some_and(|o| !o.contains(&"text")) {', 'if false {',
     P, 'lib', 'entries_that_are_not_chat_models_or_are_malformed_are_left_out'),
    # Text already streamed as deltas is not delivered a second time at the end.
    ('streamed-text-repeated', CHAT,
     '} else if !streamed && !c.text.is_empty() {', '} else if !c.text.is_empty() {',
     P, 'lib', 'events_depend_on_streaming_and_structure'),
    # One job more than the connection's cap is refused, not queued (each job holds a key's reserve).
    ('job-cap-dropped', JOBS,
     '>= self.max_active {', '>= usize::MAX {',
     P, 'conformance', 'too_many_jobs_at_once_is_a_rate_limit_not_a_queue'),
    # The assembled stream's text is checked against the cap before a piece is kept or shown.
    ('stream-text-cap-unchecked', STREAM,
     '        if kept > self.max {\n            self.over = true;\n            return false;\n        }',
     '        if false {\n            self.over = true;\n            return false;\n        }',
     P, 'lib', 'a_piece_past_the_cap_is_never_kept_or_shown'),
    # Tool-call arguments count against the same cap.
    ('stream-arguments-cap-unchecked', STREAM,
     '                if kept > self.max {\n                    self.over = true;\n                    return;\n                }',
     '                if false {\n                    self.over = true;\n                    return;\n                }',
     P, 'lib', 'a_piece_past_the_cap_is_never_kept_or_shown'),
    # The factory's request timeout caps each request's own time.
    ('request-timeout-ignored', TRANSPORT,
     'cap.map_or(own, |c| own.min(c))', '{\n        let _ = cap;\n        own\n    }',
     P, 'lib', 'a_request_takes_the_shorter_of_its_own_time_and_the_cap'),
    # A stream the connection cut short is an error, not a shorter answer.
    ('cut-stream-accepted', STREAM,
     'if a.finish_reason.is_none() {', 'if false {',
     P, 'lib', 'a_stream_cut_short_is_refused_and_a_finished_one_kept'),
]
