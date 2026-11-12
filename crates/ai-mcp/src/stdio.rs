//! MCP over stdin and stdout, one JSON-RPC message per line: how desktop MCP clients (Claude
//! Desktop, Claude Code, Cursor, LM Studio) start a local server. The client owns the process, so
//! there is no port to reach and nothing for another local program to connect to.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

use crate::mcp::McpState;

/// Largest request line read. Inline images arrive base64-encoded inside one line.
pub const MAX_LINE: usize = 96 << 20;

/// Answer every request on `input` until it ends. Each reply is one line on `output`.
pub fn serve(state: &McpState, input: impl BufRead, mut output: impl Write) -> std::io::Result<()> {
    let mut input = input.take(0);
    loop {
        let mut line = Vec::new();
        // Read one line, at most MAX_LINE bytes plus its newline.
        input.set_limit(MAX_LINE as u64 + 1);
        let n = input.read_until(b'\n', &mut line)?;
        if n == 0 {
            return Ok(());
        }
        if line.last() != Some(&b'\n') && n > MAX_LINE {
            // Too long: skip the rest of it, then say so.
            loop {
                input.set_limit(MAX_LINE as u64);
                let mut skip = Vec::new();
                let m = input.read_until(b'\n', &mut skip)?;
                if m == 0 || skip.last() == Some(&b'\n') {
                    break;
                }
            }
            write_line(
                &mut output,
                &parse_error(format!("request larger than {MAX_LINE} bytes")),
            )?;
            continue;
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let reply = match serde_json::from_slice::<Value>(&line) {
            Ok(req @ Value::Object(_)) => state.handle(&req),
            Ok(_) => Some(json!({"jsonrpc": "2.0", "id": null,
                "error": {"code": -32600, "message": "a request must be one JSON object"}})),
            Err(e) => Some(parse_error(e.to_string())),
        };
        if let Some(reply) = reply {
            write_line(&mut output, &reply)?;
        }
    }
}

fn parse_error(message: String) -> Value {
    json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": message}})
}

fn write_line(output: &mut impl Write, v: &Value) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec(v).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    output.write_all(&bytes)?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_ai_client::AiClient;
    use craft_ai_core::AppId;
    use craft_ai_native::secrets::MemoryStore;
    use craft_ai_native::shared::SharedStore;
    use std::sync::Arc;

    fn state() -> (McpState, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let shared = SharedStore::new(dir.path().join("connections.json"));
        let client = AiClient::new(
            AppId::new("mcp").unwrap(),
            shared,
            Arc::new(MemoryStore::new()),
        );
        (McpState::new(client), dir)
    }

    fn run(input: &str) -> Vec<Value> {
        let (s, _d) = state();
        let mut out = Vec::new();
        serve(&s, input.as_bytes(), &mut out).unwrap();
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn answers_requests_and_not_notifications() {
        let out = run(concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            "\n",
            "\n",
            r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
            "\n",
        ));
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["id"], 1);
        assert_eq!(out[1]["id"], 2);
    }

    #[test]
    fn garbage_gets_an_error_and_the_next_request_still_works() {
        let out = run("not json\n[1,2]\n{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"ping\"}");
        assert_eq!(out.len(), 3);
        assert_eq!(out[0]["error"]["code"], -32700);
        assert_eq!(out[1]["error"]["code"], -32600);
        assert_eq!(
            out[2]["id"], 3,
            "a last line without newline is answered too"
        );
    }

    #[test]
    fn an_overlong_line_is_refused_and_skipped() {
        let (s, _d) = state();
        let mut input = vec![b'x'; MAX_LINE + 10];
        input.extend_from_slice(b"\n{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"ping\"}\n");
        let mut out = Vec::new();
        serve(&s, input.as_slice(), &mut out).unwrap();
        let lines: Vec<Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["error"]["code"], -32700);
        assert_eq!(lines[1]["id"], 4);
    }
}
