//! MCP over stdin and stdout, one JSON-RPC message per line: how desktop MCP clients (Claude
//! Desktop, Claude Code, Cursor, LM Studio) start a local server. The client owns the process, so
//! there is no port to reach and nothing for another local program to connect to.

use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

use serde_json::{Value, json};

use crate::mcp::McpState;

/// Largest request line read. Inline images arrive base64-encoded inside one line.
pub const MAX_LINE: usize = 96 << 20;
/// Requests answered at once. A long tool call (a filter, a generation) runs beside the others,
/// so `ping`, `tools/list` and other calls still get their answers; past this many the client is
/// told to wait.
pub const MAX_IN_FLIGHT: usize = 16;

/// Answer every request on `input` until it ends. Each reply is one line on `output`, written
/// whole; requests are answered in parallel, so replies can come in another order (each carries
/// its request's id). Returns once every started request has been answered.
pub fn serve(
    state: &McpState,
    input: impl BufRead,
    output: impl Write + Send,
) -> std::io::Result<()> {
    let output = Mutex::new(output);
    let in_flight = AtomicUsize::new(0);
    std::thread::scope(|scope| read_requests(state, input, &output, &in_flight, scope))
}

fn read_requests<'s, W: Write + Send>(
    state: &'s McpState,
    input: impl BufRead,
    output: &'s Mutex<W>,
    in_flight: &'s AtomicUsize,
    scope: &'s std::thread::Scope<'s, '_>,
) -> std::io::Result<()> {
    let reply = |v: &Value| {
        let mut out = output.lock().unwrap_or_else(PoisonError::into_inner);
        write_line(&mut *out, v)
    };
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
            // Too long: let the line go, skip the rest of it in the reader's own buffer, then
            // say so.
            drop(line);
            input.set_limit(u64::MAX);
            loop {
                let buf = input.fill_buf()?;
                if buf.is_empty() {
                    break;
                }
                if let Some(i) = buf.iter().position(|b| *b == b'\n') {
                    input.consume(i + 1);
                    break;
                }
                let len = buf.len();
                input.consume(len);
            }
            reply(&parse_error(format!(
                "request larger than {MAX_LINE} bytes"
            )))?;
            continue;
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let req = match serde_json::from_slice::<Value>(&line) {
            Ok(req @ Value::Object(_)) => req,
            Ok(_) => {
                reply(&json!({"jsonrpc": "2.0", "id": null,
                    "error": {"code": -32600, "message": "a request must be one JSON object"}}))?;
                continue;
            }
            Err(e) => {
                reply(&parse_error(e.to_string()))?;
                continue;
            }
        };
        if in_flight.fetch_add(1, Ordering::SeqCst) >= MAX_IN_FLIGHT {
            in_flight.fetch_sub(1, Ordering::SeqCst);
            if let Some(id) = req.get("id") {
                reply(
                    &json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000,
                    "message": format!("{MAX_IN_FLIGHT} requests are already running; try again when one finishes")}}),
                )?;
            }
            continue;
        }
        scope.spawn(move || {
            if let Some(v) = state.handle(&req) {
                // A client that stopped reading ends the session when its input closes.
                let _ = reply(&v);
            }
            in_flight.fetch_sub(1, Ordering::SeqCst);
        });
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
    use std::sync::Arc;

    /// The state and a second value kept for the tests' `let (s, _d) = state()` shape.
    fn state() -> (McpState, ()) {
        (McpState::new(), ())
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
        let mut ids: Vec<i64> = out.iter().filter_map(|v| v["id"].as_i64()).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec![1, 2]);
    }

    #[test]
    fn a_long_call_does_not_hold_up_a_ping() {
        use crate::apps::child::tests::fake;
        use crate::apps::{Hub, registry::AppEntry};
        let app = AppEntry {
            id: "slowcraft".into(),
            name: "SlowCraft".into(),
            command: std::path::PathBuf::from("/fake"),
            args: vec![],
        };
        let hub = Hub::with_connector(
            vec![app],
            vec![],
            Box::new(|_| {
                Ok(fake(|m, req| {
                    let r = match m {
                        "tools/list" => json!({"tools": [{"name": "slow"}]}),
                        "tools/call" => {
                            std::thread::sleep(std::time::Duration::from_millis(800));
                            json!({"content": []})
                        }
                        _ => return None,
                    };
                    Some(vec![
                        json!({"jsonrpc": "2.0", "id": req["id"], "result": r}).to_string(),
                    ])
                }))
            }),
        );
        let (s, _d) = state();
        let s = s.with_apps(Arc::new(hub));
        let input = concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"craft_call_tool","arguments":{"app":"slowcraft","tool":"slow"}}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
            "\n",
        );
        let mut out = Vec::new();
        serve(&s, input.as_bytes(), &mut out).unwrap();
        let ids: Vec<i64> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| {
                serde_json::from_str::<Value>(l).unwrap()["id"]
                    .as_i64()
                    .unwrap()
            })
            .collect();
        assert_eq!(ids, vec![2, 1], "the ping is answered while the call runs");
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
