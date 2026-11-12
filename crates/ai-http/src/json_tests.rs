use super::*;

/// A reply of `n` tiny objects in one array: the shape that costs about a hundred times its
/// size in a plain `serde_json::Value`.
fn tiny_objects(n: usize) -> Vec<u8> {
    let mut body = b"[".to_vec();
    for i in 0..n {
        if i > 0 {
            body.push(b',');
        }
        body.extend_from_slice(br#"{"":0}"#);
    }
    body.push(b']');
    body
}

#[test]
fn builds_the_same_value_as_serde_json() {
    let docs: [&[u8]; 8] = [
        br#"{"a":1,"b":[true,false,null],"c":{"d":"e\u00e9\n"},"f":-2,"g":1.5e3}"#,
        br#"[18446744073709551615,-9223372036854775808,0.1,"",[],{}]"#,
        br#"{"dup":1,"dup":2}"#,
        br#""just a string""#,
        b"42",
        b"null",
        b" \n [ 1 , 2 ] \n ",
        br#"{"nested":[{"x":[{"y":{}}]}]}"#,
    ];
    for doc in docs {
        let expected: Value = serde_json::from_slice(doc).unwrap();
        assert_eq!(
            parse(doc, 1 << 20),
            Ok(expected),
            "{}",
            String::from_utf8_lossy(doc)
        );
    }
}

#[test]
fn many_tiny_objects_are_refused_within_the_budget() {
    // 70,001 bytes of tiny objects: about 7 MB as a plain Value. A 1 MB budget refuses it.
    let body = tiny_objects(10_000);
    assert!(serde_json::from_slice::<Value>(&body).is_ok());
    assert_eq!(parse(&body, 1 << 20), Err(ReplyError::TooLarge));
}

#[test]
fn one_long_string_costs_about_its_length() {
    // A base64 picture is the large reply a provider really sends; it fits a budget of its size.
    let picture = "A".repeat(1 << 20);
    let body = format!(r#"{{"data":[{{"b64_json":"{picture}"}}]}}"#);
    let parsed = parse(body.as_bytes(), (1 << 20) + 4096).unwrap();
    assert_eq!(
        parsed.pointer("/data/0/b64_json").and_then(Value::as_str),
        Some(picture.as_str())
    );
    assert_eq!(parse(body.as_bytes(), 1 << 20), Err(ReplyError::TooLarge));
}

#[test]
fn the_budget_is_exact() {
    // `[{"ab":"cd"}]`: the top slot, the object, one entry with a 2-byte key, a 2-byte string
    // and one array element.
    let body = br#"[{"ab":"cd"}]"#;
    let cost = VALUE_COST + OBJECT_COST + ENTRY_COST + 2 + 2 + ELEMENT_COST;
    assert!(parse(body, cost).is_ok());
    assert_eq!(parse(body, cost - 1), Err(ReplyError::TooLarge));
}

#[test]
fn each_part_is_charged() {
    // One less than each part's cost leaves too little; the rest of the document is free.
    let top = VALUE_COST;
    assert_eq!(parse(b"[]", top), Ok(Value::Array(Vec::new())));
    assert_eq!(parse(b"[]", top - 1), Err(ReplyError::TooLarge));
    assert_eq!(
        parse(b"[1]", top + ELEMENT_COST - 1),
        Err(ReplyError::TooLarge)
    );
    assert_eq!(
        parse(b"{}", top + OBJECT_COST - 1),
        Err(ReplyError::TooLarge)
    );
    assert!(parse(b"{}", top + OBJECT_COST).is_ok());
    let entry = top + OBJECT_COST + ENTRY_COST;
    assert_eq!(parse(br#"{"k":1}"#, entry), Err(ReplyError::TooLarge));
    assert!(parse(br#"{"k":1}"#, entry + 1).is_ok());
    assert_eq!(parse(br#""abc""#, top + 2), Err(ReplyError::TooLarge));
    assert!(parse(br#""abc""#, top + 3).is_ok());
    // An escaped string is unescaped first; it is charged the same way.
    assert_eq!(parse(br#""a\nc""#, top + 2), Err(ReplyError::TooLarge));
    assert!(parse(br#""a\nc""#, top + 3).is_ok());
}

#[test]
fn a_tiny_object_costs_at_least_a_tree_node() {
    // Its first entry allocates a whole `BTreeMap` leaf, measured at about 632 bytes, so the
    // estimate may not be lower whatever the constants say.
    assert_eq!(parse(br#"{"":0}"#, 632), Err(ReplyError::TooLarge));
    assert_eq!(parse(br#"[{"":0}]"#, 632), Err(ReplyError::TooLarge));
}

#[test]
fn what_serde_json_refuses_is_not_json() {
    for bad in [&b""[..], b"{", b"[1,]", b"{} {}", b"nul", b"\xff"] {
        assert_eq!(parse(bad, 1 << 20), Err(ReplyError::NotJson));
    }
}

#[test]
fn deep_nesting_is_still_refused() {
    let deep = format!("{}{}", "[".repeat(200), "]".repeat(200));
    assert_eq!(parse(deep.as_bytes(), 1 << 20), Err(ReplyError::NotJson));
}

#[test]
fn a_non_finite_number_is_null_as_in_serde_json() {
    // serde_json reads 1e400 as infinity, which its Value stores as null... or refuses; match it.
    let expected = serde_json::from_slice::<Value>(b"[1e400]").map_err(|_| ReplyError::NotJson);
    assert_eq!(parse(b"[1e400]", 1 << 20), expected);
}
