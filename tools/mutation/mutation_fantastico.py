"""Deliberate-bug checks for the FANTASTICO adapter (`crates/ai-fantastico`).

Do not import the runner here: it executes immediately. Each case breaks one safety rule and names
the focused runtime test that must fail, so a compile error never counts as evidence.
"""
SRC = 'crates/ai-fantastico/src/'
ACCOUNT = SRC + 'account.rs'
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
     '(before < max).then_some(permit)', 'Some(permit)',
     P, 'conformance', 'too_many_jobs_at_once_is_a_rate_limit_not_a_queue'),
    # A cancelled job's worker keeps its place under the cap until it has ended.
    ('worker-place-freed-at-start', JOBS,
     '                worker_cell.start();\n',
     '                worker_cell.start();\n                let permit = { drop(permit); None::<Permit> };\n',
     P, 'conformance', 'a_cancelled_worker_keeps_its_place_until_it_ends'),
    # A job that has ended starts no further request.
    ('cancelled-job-keeps-sending', TRANSPORT,
     'if jobs::sink_stopped() {', 'if false {',
     P, 'conformance', 'a_cancelled_job_starts_no_further_request'),
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
    # The model's provider gets the runtime key, never the account key.
    ('account-key-sent-to-the-model', ACCOUNT,
     'format!("Bearer {}", secret.expose()),', 'format!("Bearer {}", self.key.expose()),',
     P, 'lib', 'mints_once_and_each_key_goes_only_where_it_belongs'),
    # A key comes with the address it is spent at; there is no address of the adapter's own.
    ('key-address-may-be-empty', ACCOUNT,
     '.filter(|u| !u.is_empty())', '.filter(|_| true)',
     P, 'lib', 'a_key_without_an_address_is_refused_with_no_fallback'),
    # A key that expires within a minute is replaced before it is used.
    ('expiring-key-reused', ACCOUNT,
     '.is_some_and(|k| k.expires_at_ms.saturating_sub(now_ms()) > EXPIRY_SKEW_MS)',
     '.is_some_and(|k| k.expires_at_ms.saturating_sub(now_ms()) > 0)',
     P, 'lib', 'an_expiring_key_is_replaced'),
    # A replacement key is minted in the conversation's activity session.
    ('session-left-off-a-later-mint', ACCOUNT,
     'body.insert("activitySessionId".into(), json!(s));', 'let _ = s;',
     P, 'lib', 'the_session_and_product_go_to_every_mint'),
    # Only a rejected key is replaced: a moderation block or a spent rate limit costs no new key.
    ('any-refusal-reminted', ACCOUNT,
     'if !first.is_remintable() {', 'if false {',
     P, 'lib', 'moderation_is_not_reminted_and_keeps_its_name'),
    # Only a failure where nothing was generated is sent again: a 502 may have been answered.
    ('any-failure-retried', ACCOUNT,
     'if p.is_transient() && retries < RETRIES => p,', 'if retries < RETRIES => p,',
     P, 'lib', 'rate_limits_are_retried_with_backoff_then_named'),
    # The account's own chat fields never override what the request writes.
    ('chat-defaults-win', ACCOUNT,
     'body.extend(request.iter().map(|(k, v)| (k.clone(), v.clone())));',
     'body = request.iter().map(|(k, v)| (k.clone(), v.clone())).chain(body).collect();',
     P, 'lib', 'chat_defaults_are_merged_under_what_the_request_writes'),
    # A `max_tokens` the request sets is never replaced by the cap's.
    ('explicit-max-tokens-replaced', ACCOUNT,
     'if !explicit_max {', 'if true {',
     P, 'lib', 'max_tokens_follows_the_key_limit_unless_the_request_sets_it'),
    # Each request to the provider ends within the budget.
    ('request-outlasts-the-budget', ACCOUNT,
     'request.timeout = left.min(PROVIDER_TIMEOUT);', 'request.timeout = PROVIDER_TIMEOUT;',
     P, 'lib', 'the_budget_bounds_each_request_and_the_waits'),
    # No wait between retries runs past the budget.
    ('sleep-past-the-budget', ACCOUNT,
     'if Instant::now() + delay > deadline {', 'if false {',
     P, 'lib', 'the_budget_bounds_each_request_and_the_waits'),
]
