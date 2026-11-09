//! Keeps what a newer build saved when an older build rewrites the shared file.
//!
//! Every app pins its own revision of this layer and they all share one file, so a build routinely
//! rewrites a file saved by a newer one. Its types have no field for what the newer build added.
//!
//! A field counts as unknown when it is in the file but missing from what this build gets by parsing
//! the file and serialising it again. No list of known fields is kept, so this cannot drift from the
//! types. Three documents are compared:
//!
//! - `file`: the JSON on disk before the change;
//! - `seen`: that same file as this build understood it;
//! - `written`: the store after the change, which receives the unknown fields.
//!
//! Invariants: a field in `seen` but not in `written` was removed on purpose and stays removed. A
//! value in `written` always wins. List items are matched by identity, never by position, so items
//! can be reordered, and a removed item takes its unknown fields with it. The identity comes from
//! the written item, because a newer build may add a field (an `id` on a binding) that this build
//! does not write; file items are indexed under every identity field they hold so either side can
//! be found. An item whose identity appears twice in any of the three lists, or that has none, is
//! left as written, because there is no safe way to choose.

use std::collections::HashMap;

use serde_json::{Map, Value};

/// Fields that identify a list item in the store: connections, models, tools and folders by `id`,
/// bindings by `role`, capability entries by `capability`. The first one present is the identity.
const IDENTITY_FIELDS: [&str; 3] = ["id", "role", "capability"];

/// An item's identity: the field that holds it and its value.
type Identity<'a> = (&'static str, &'a str);

/// Copies into `written` every field of `file` that `seen` shows this build does not understand.
pub(super) fn carry(file: &Value, seen: &Value, written: &mut Value) {
    match (file, seen, written) {
        (Value::Object(file), Value::Object(seen), Value::Object(written)) => {
            carry_fields(file, seen, written);
        }
        (Value::Array(file), Value::Array(seen), Value::Array(written)) => {
            carry_items(file, seen, written);
        }
        _ => {}
    }
}

/// Copies each field of `file` that `seen` lacks into `written`, unless the writer set it, and
/// recurses into fields all three hold.
fn carry_fields(
    file: &Map<String, Value>,
    seen: &Map<String, Value>,
    written: &mut Map<String, Value>,
) {
    for (name, in_file) in file {
        match (seen.get(name), written.get_mut(name)) {
            (None, None) => {
                written.insert(name.clone(), in_file.clone());
            }
            (Some(in_seen), Some(in_written)) => carry(in_file, in_seen, in_written),
            // Unknown here but the writer set it, or known and the writer removed it.
            (None, Some(_)) | (Some(_), None) => {}
        }
    }
}

/// Copies unknown fields into each written item from the one file item with the same identity.
/// Written identities are counted first, so a writer that adds a second item with an identity the
/// file already holds leaves both as written rather than copying one old item's fields onto two.
fn carry_items(file: &[Value], seen: &[Value], written: &mut [Value]) {
    let file = unique_by_identity(file);
    let seen = unique_by_identity(seen);
    let mut written_times: HashMap<(&'static str, String), usize> = HashMap::new();
    for item in written.iter() {
        if let Some((field, value)) = identity(item) {
            *written_times.entry((field, value.to_owned())).or_default() += 1;
        }
    }
    for item in written {
        let Some(key) = identity(item) else { continue };
        // The key borrows `item`, so it is copied out before `item` is changed.
        let key = (key.0, key.1.to_owned());
        if written_times.get(&key) != Some(&1) {
            continue;
        }
        let lookup = (key.0, key.1.as_str());
        if let (Some(Some(in_file)), Some(Some(in_seen))) = (file.get(&lookup), seen.get(&lookup)) {
            carry(in_file, in_seen, item);
        }
    }
}

/// Indexes a list under every identity field each item holds, in one pass. An identity seen twice
/// maps to `None`. Indexing every field, not just the first, lets a written item that has only
/// `role` find a file item a newer build also gave an `id`.
fn unique_by_identity(items: &[Value]) -> HashMap<Identity<'_>, Option<&Value>> {
    let mut found = HashMap::with_capacity(items.len());
    for item in items {
        for key in identities(item) {
            found
                .entry(key)
                .and_modify(|slot| *slot = None)
                .or_insert(Some(item));
        }
    }
    found
}

/// Every identity field an item holds as a string, in [`IDENTITY_FIELDS`] order.
fn identities(item: &Value) -> impl Iterator<Item = Identity<'_>> {
    IDENTITY_FIELDS
        .iter()
        .filter_map(move |field| Some((*field, item.get(*field)?.as_str()?)))
}

/// The identity a written item is matched on: the first identity field it holds.
fn identity(item: &Value) -> Option<Identity<'_>> {
    identities(item).next()
}
