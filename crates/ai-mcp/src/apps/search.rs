//! Ranking an app's tools against a few words, so an agent asks for what it needs instead of
//! loading every tool definition of every app.

use serde_json::Value;

/// Longest summary returned per hit.
pub const SUMMARY_CHARS: usize = 160;
/// Hits returned when the caller does not say.
pub const DEFAULT_LIMIT: usize = 20;
/// Most hits returned.
pub const MAX_LIMIT: usize = 50;
/// Most query words looked at.
const MAX_WORDS: usize = 16;

/// One match.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub app: String,
    pub tool: String,
    pub summary: String,
    pub score: u32,
}

/// Lower-case words of `s` (letters and digits).
fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// The first sentence of `description`, at most [`SUMMARY_CHARS`] characters.
pub fn summary(description: &str) -> String {
    let first = description
        .split_inclusive(['.', '\n'])
        .next()
        .unwrap_or(description)
        .trim();
    if first.chars().count() <= SUMMARY_CHARS {
        return first.to_string();
    }
    let mut s: String = first
        .chars()
        .take(SUMMARY_CHARS.saturating_sub(1))
        .collect();
    s.push('…');
    s
}

/// How well `tool` (an MCP tool object) matches `query`: a word in the name counts 3, part of the
/// name 2, a word in the description 1. Zero means no match. An empty query matches every tool
/// with 1, so listing an app's tools works too.
pub fn score(query: &[String], tool: &Value) -> u32 {
    let name = tool.get("name").and_then(Value::as_str).unwrap_or("");
    let title = tool.get("title").and_then(Value::as_str).unwrap_or("");
    let desc = tool
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    if query.is_empty() {
        return 1;
    }
    let name_lower = name.to_lowercase();
    let name_words = words(&format!("{name} {title}"));
    let desc_words = words(desc);
    let mut total = 0u32;
    for q in query {
        let s = if name_words.iter().any(|w| w == q) {
            3
        } else if name_lower.contains(q.as_str())
            || name_words.iter().any(|w| w.starts_with(q.as_str()))
        {
            2
        } else if desc_words
            .iter()
            .any(|w| w == q || w.starts_with(q.as_str()))
        {
            1
        } else {
            0
        };
        total = total.saturating_add(s);
    }
    total
}

/// The best `limit` matches of `query` among `(app id, that app's tools)`, best first; ties keep
/// app and tool order.
pub fn find<'a>(
    query: &str,
    apps: impl IntoIterator<Item = (&'a str, &'a [Value])>,
    limit: usize,
) -> Vec<Hit> {
    let mut q = words(query);
    q.truncate(MAX_WORDS);
    let limit = limit.clamp(1, MAX_LIMIT);
    let mut hits: Vec<Hit> = Vec::new();
    for (app, tools) in apps {
        for t in tools {
            let s = score(&q, t);
            if s == 0 {
                continue;
            }
            let desc = t
                .get("description")
                .and_then(Value::as_str)
                .or_else(|| t.get("title").and_then(Value::as_str))
                .unwrap_or("");
            hits.push(Hit {
                app: app.to_string(),
                tool: t
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                summary: summary(desc),
                score: s,
            });
        }
    }
    hits.sort_by_key(|h| std::cmp::Reverse(h.score));
    hits.truncate(limit);
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tools() -> Vec<Value> {
        vec![
            json!({"name": "filter.gaussian_blur", "description": "Blur the layer. Radius in pixels."}),
            json!({"name": "layer.new", "description": "Add an empty layer above the current one."}),
            json!({"name": "image.resize", "description": "Change the image size; can blur when downsampling."}),
        ]
    }

    #[test]
    fn name_beats_description() {
        let t = tools();
        let hits = find("blur", [("photocraft", t.as_slice())], 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].tool, "filter.gaussian_blur");
        assert_eq!(hits[1].tool, "image.resize");
    }

    #[test]
    fn several_words_add_up_and_no_match_is_left_out() {
        let t = tools();
        let hits = find("new layer", [("photocraft", t.as_slice())], 10);
        assert_eq!(hits[0].tool, "layer.new");
        assert!(find("zebra", [("photocraft", t.as_slice())], 10).is_empty());
    }

    #[test]
    fn empty_query_lists_and_limit_caps() {
        let t = tools();
        assert_eq!(find("", [("a", t.as_slice())], 10).len(), 3);
        assert_eq!(find("", [("a", t.as_slice())], 2).len(), 2);
        assert_eq!(
            find("", [("a", t.as_slice())], 0).len(),
            1,
            "limit 0 still returns one"
        );
    }

    #[test]
    fn hits_name_their_app() {
        let a = vec![json!({"name": "doc.new"})];
        let b = vec![json!({"name": "doc.new"})];
        let hits = find(
            "doc",
            [("wordcraft", a.as_slice()), ("gridcraft", b.as_slice())],
            10,
        );
        assert_eq!(
            hits.iter().map(|h| h.app.as_str()).collect::<Vec<_>>(),
            vec!["wordcraft", "gridcraft"]
        );
    }

    #[test]
    fn summary_is_the_first_sentence_and_capped() {
        assert_eq!(summary("Blur it. More text."), "Blur it.");
        let long = "x".repeat(400);
        let s = summary(&long);
        assert_eq!(s.chars().count(), SUMMARY_CHARS);
        assert!(s.ends_with('…'));
        assert_eq!(
            summary("ümläut ".repeat(50).as_str()).chars().count(),
            SUMMARY_CHARS
        );
    }
}
