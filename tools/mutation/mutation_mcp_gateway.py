"""Deliberate-bug cases for group `mg`: the MCP bridge's gateway to the Craft apps.

Loaded by mutation_check.py. Each case breaks one rule and names the test that should notice.
Format: (name, file, old, new, package, test target, test filter).
"""
A = 'crates/ai-mcp/src/apps/'
P = 'craft-ai-mcp'
L = 'crates/ai-agents/src/lib.rs'
Q = 'craft-ai-agents'

MG = [
    ('relative-command-started', L, 'if !raw.command.is_absolute() {', 'if false {', Q, 'lib', 'bad_rows_are_left_out'),
    ('any-app-id-accepted', L, 'if !valid_id(&raw.id) {', 'if false {', Q, 'lib', 'bad_rows_are_left_out'),
    ('app-list-not-capped', L, 'if apps.len() >= MAX_APPS {', 'if false {', Q, 'lib', 'the_list_is_capped'),
    ('app-line-not-capped', A + 'child.rs', 'if line.len().saturating_add(chunk.len()) > cap {', 'if false {', P, 'lib', 'capped_line_reader'),
    ('timed-out-connection-kept', A + 'child.rs', 'timeout.as_secs()\n                    );\n                    self.closed = Some(why.clone());', 'timeout.as_secs()\n                    );', P, 'lib', 'a_silent_app_times_out'),
    ('dead-connection-reused', A + 'mod.rs', 'if conn.as_ref().is_none_or(|c| c.closed().is_some()) {', 'if conn.is_none() {', P, 'lib', 'a_dead_connection_is_replaced'),
    ('unknown-tool-forwarded', A + 'mod.rs', '.any(|t| t.get("name").and_then(Value::as_str) == Some(tool))\n        {', '.any(|_| true)\n        {', P, 'lib', 'bad_arguments_are_tool_errors'),
    ('stdio-line-not-capped', 'crates/ai-mcp/src/stdio.rs', r"if line.last() != Some(&b'\n') && n > MAX_LINE {", 'if false {', P, 'lib', 'an_overlong_line_is_refused'),
    ('partial-tool-list-accepted', A + 'child.rs', '        // A partial list would be cached as complete, and calls to the rest refused.\n        Err(', '        return Ok(tools);\n        #[allow(unreachable_code)]\n        Err(', P, 'lib', 'a_tool_list_that_never_ends'),
    ('changed-tools-kept', A + 'mod.rs', 'let changed = conn.as_mut().is_some_and(Conn::take_tools_changed);', 'let changed = conn.as_mut().is_some_and(Conn::take_tools_changed) && false;', P, 'lib', 'read_again_after_the_app_says'),
    ('failed-write-not-reported', A + 'child.rs', '                    let _ = write_failed\n                        .send(Incoming::Closed(format!("writing to the app failed: {e}")));\n', '                    let _ = &write_failed;\n', P, 'lib', 'a_failed_write_ends_the_waiting_request'),
    ('stopped-app-keeps-cached-tools', A + 'mod.rs', 'if changed || conn.as_ref().is_some_and(|c| c.closed().is_some()) {', 'if changed {', P, 'lib', 'an_app_that_stopped_has_its_tools_read_again'),
    ('switch-not-asked', A + 'mod.rs', '        if (self.switch)() {', '        if true {', P, 'lib', 'the_launchers_switch_is_asked'),
    ('any-app-allowed', L, 'apps.iter().any(|a| a.id == app_id)', '!apps.is_empty()', Q, 'lib', 'allowed_only_while_the_list_names_it'),
    ('queued-call-runs-after-switch-off', A + 'mod.rs', '        let mut conn = lock(&slot.conn);\n        if !(self.switch)() {', '        let mut conn = lock(&slot.conn);\n        if false {', P, 'lib', 'a_call_waiting_for_the_app_never_starts'),
    ('list-waits-on-a-call', A + 'mod.rs', 'let started = match try_lock(&s.conn) {', 'let started = match Some(lock(&s.conn)) {', P, 'lib', 'a_long_call_does_not_hold_up'),
    ('invalid-id-dispatched', 'crates/ai-mcp/src/mcp.rs', '|| id.as_ref().is_some_and(|v| !valid_id(v))', '|| false', P, 'lib', 'malformed_envelopes_are_invalid_requests'),
]
