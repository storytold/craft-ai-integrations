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
]
