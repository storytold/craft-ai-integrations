"""Deliberate-bug cases for group `at`: AppTools lists, plans and runs tools the same way for every app.

Loaded by mutation_check.py. Each case breaks one rule and names the test that should notice.
Format: (name, file, old, new, package, test target, test filter). Target 'lib' runs unit tests.

Not mutation-covered, with the reason: "reading writes nothing" and "a tool with no choice is
greyed, not an error" are delegated to AiClient::status and SharedStore::load, whose own groups
(`ca`, `ct`) cover them; app_tools.rs adds no code of its own to break there. The consent ledger is
passed straight to AiClient::start, which refuses a missing agreement (group `ct`,
`consent-not-enforced`); ignoring a granted ledger would refuse a job, not send one.
"""
A = 'crates/ai-client/src/app_tools.rs'
CL = 'craft-ai-client'
T = 'app_tools'

AT = [
    ('unknown-tool-falls-back-to-the-first-tool', A, 'self.custom_tools()\n            .into_iter()\n            .find(|role| role.id.as_str() == tool)', 'self.custom_tools()\n            .into_iter()\n            .find(|role| role.id.as_str() == tool)\n            .or_else(|| self.roles.first().map(|(_, role)| role.clone()))', CL, T, 'an_unknown_tool'),
    ('custom-tools-left-out-of-the-report', A, 'custom.iter().map(|role| (role.label.as_str(), role))', 'custom.iter().take(0).map(|role| (role.label.as_str(), role))', CL, T, 'a_custom_tool_is_listed'),
    ('custom-tool-has-no-role', A, 'self.custom_tools()\n            .into_iter()\n            .find(|role| role.id.as_str() == tool)', 'None', CL, T, 'a_custom_tool_is_listed'),
    ('request-for-the-wrong-capability', A, 'Request::new(role.capability)', 'Request::new(craft_ai_core::CapabilityId::parse("text.complete").unwrap())', CL, T, 'a_request_carries'),
    ('run-drops-the-outputs', A, 'Event::Output(output) => Some(output),', 'Event::Output(_) => None,', CL, T, 'plan_then_run'),
    ('cancelled-job-reported-as-success', A, 'JobState::Cancelled => Err("Cancelled.".to_owned()),', 'JobState::Cancelled => Ok(Vec::new()),', CL, T, 'cancel_reaches_the_running_job'),
    ('failed-job-reported-as-success', A, 'JobState::Failed { error } => Err(error.message),', 'JobState::Failed { error } => {\n                let _ = error;\n                Ok(Vec::new())\n            }', CL, T, 'a_failed_job'),
    ('running-job-not-recorded', A, 'lock(&self.running).push((number, job.clone()));', 'let _ = number;', CL, T, 'cancel_reaches_the_running_job'),
    ('cancel-reaches-only-the-first-job', A, 'for job in jobs {', 'for job in jobs.into_iter().take(1) {', CL, T, 'one_cancel_reaches'),
    ('finished-job-forgets-every-job', A, 'lock(&self.running).retain(|(n, _)| *n != number);', 'lock(&self.running).clear();', CL, T, 'a_finished_job_removes'),
    ('own-limit-ignored', A, 'let done = job.wait(STEP, limit);', 'let done = job.wait(STEP, LIMIT);', CL, T, 'outlasts_the_wait'),
]

AT_EQUIVALENT = {}

# Reviewed and dropped: removing a cancel from run_for after a timeout. Job::wait already cancels the
# job when its limit passes, so that mutant is equivalent; own-limit-ignored and the outlasts test
# cover the limit instead.
