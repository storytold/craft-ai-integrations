//! Record appends use only temporary files.

use super::append_line;

/// Each append is its own line, in a new file and after earlier records.
#[test]
fn records_collect_one_per_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runs.jsonl");
    append_line(&path, r#"{"a":1}"#).unwrap();
    append_line(&path, r#"{"b":2}"#).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "{\"a\":1}\n{\"b\":2}\n"
    );
}

/// A file whose last record has no newline gains one before the next record.
#[test]
fn a_missing_final_newline_is_added_before_the_next_record() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runs.jsonl");
    std::fs::write(&path, r#"{"a":1}"#).unwrap();
    append_line(&path, r#"{"b":2}"#).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "{\"a\":1}\n{\"b\":2}\n"
    );
}
