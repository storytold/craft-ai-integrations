"""Deliberate-bug cases for group `mg`: the MCP bridge's gateway to the Craft apps.

Loaded by mutation_check.py. Each case breaks one rule and names the test that should notice.
Format: (name, file, old, new, package, test target, test filter).
"""
A = 'crates/ai-mcp/src/apps/'
P = 'craft-ai-mcp'

MG = [
    ('relative-command-started', A + 'registry.rs', 'if !raw.command.is_absolute() {', 'if false {', P, 'lib', 'bad_rows_are_left_out'),
    ('any-app-id-accepted', A + 'registry.rs', 'if !valid_id(&raw.id) {', 'if false {', P, 'lib', 'bad_rows_are_left_out'),
    ('app-list-not-capped', A + 'registry.rs', 'if apps.len() >= MAX_APPS {', 'if false {', P, 'lib', 'the_list_is_capped'),
    ('app-line-not-capped', A + 'child.rs', 'if line.len().saturating_add(chunk.len()) > cap {', 'if false {', P, 'lib', 'capped_line_reader'),
    ('timed-out-connection-kept', A + 'child.rs', 'timeout.as_secs()\n                    );\n                    self.closed = Some(why.clone());', 'timeout.as_secs()\n                    );', P, 'lib', 'a_silent_app_times_out'),
    ('dead-connection-reused', A + 'mod.rs', 'if slot.conn.as_ref().is_none_or(|c| c.closed().is_some()) {', 'if slot.conn.is_none() {', P, 'lib', 'a_dead_connection_is_replaced'),
    ('unknown-tool-forwarded', A + 'mod.rs', '.any(|t| t.get("name").and_then(Value::as_str) == Some(tool))\n        {', '.any(|_| true)\n        {', P, 'lib', 'bad_arguments_are_tool_errors'),
    ('stdio-line-not-capped', 'crates/ai-mcp/src/stdio.rs', r"if line.last() != Some(&b'\n') && n > MAX_LINE {", 'if false {', P, 'lib', 'an_overlong_line_is_refused'),
    ('bridge-grants-consent', 'crates/ai-mcp/src/mcp.rs', 'auto_consent: false,', 'auto_consent: true,', P, 'lib', 'new_never_grants_consent_itself'),
    ('partial-tool-list-accepted', A + 'child.rs', '        // A partial list would be cached as complete, and calls to the rest refused.\n        Err(', '        return Ok(tools);\n        #[allow(unreachable_code)]\n        Err(', P, 'lib', 'a_tool_list_that_never_ends'),
    ('changed-tools-kept', A + 'mod.rs', 'let changed = slot.conn.as_mut().is_some_and(Conn::take_tools_changed);', 'let changed = slot.conn.as_mut().is_some_and(Conn::take_tools_changed) && false;', P, 'lib', 'read_again_after_the_app_says'),
    ('unknown-bytes-labelled-png', 'crates/ai-mcp/src/router.rs', '.ok_or_else(|| "the input is not a recognised image".to_string())', '.or(Some("image/png")).ok_or_else(String::new)', P, 'lib', 'unrecognised_bytes_are_refused'),
    ('base64-decoded-before-size-check', 'crates/ai-mcp/src/router.rs', 'if encoded / 4 * 3 > max {', 'if false {', P, 'lib', 'oversized_base64_is_refused'),
    ('failed-write-not-reported', A + 'child.rs', '                    let _ = write_failed\n                        .try_send(Incoming::Closed(format!("writing to the app failed: {e}")));\n', '                    let _ = &write_failed;\n', P, 'lib', 'a_failed_write_ends_the_waiting_request'),
    ('stopped-app-keeps-cached-tools', A + 'mod.rs', 'if changed || slot.conn.as_ref().is_some_and(|c| c.closed().is_some()) {', 'if changed {', P, 'lib', 'an_app_that_stopped_has_its_tools_read_again'),
    ('nested-skills-folder-allowed', 'crates/ai-mcp/src/installer.rs', '.any(|w| matches!(w, [a, b] if a == ".lmstudio" && b == "skills"))', '.last().is_some_and(|w| matches!(w, [a, b] if a == ".lmstudio" && b == "skills"))', P, 'lib', 'bionic_skills_and_anything_inside'),
]
