//! Reading a JSON reply into a [`Value`] without letting the reply decide how much memory the
//! parse takes.
//!
//! A byte cap on a reply does not bound what parsing it costs. In a `serde_json::Value`, every
//! object is a `BTreeMap`, and its first entry allocates a whole tree node of about 630 bytes.
//! A reply made of many tiny objects therefore costs about a hundred times its size once parsed:
//! a 128 MiB reply could take the app past 12 GiB.
//!
//! [`parse`] builds the same `Value` that `serde_json::from_slice` would, but charges an
//! estimate of each allocation against a budget as it goes and stops with
//! [`ReplyError::TooLarge`] the moment the budget runs out. The estimate is deliberately on the
//! high side:
//!
//! - every object: one tree node ([`OBJECT_COST`]) when it opens, then [`ENTRY_COST`] plus the
//!   key's length per entry, which covers the extra nodes a larger object splits into;
//! - every array element: two value slots ([`ELEMENT_COST`]), because a growing `Vec` can hold up
//!   to twice the space it uses;
//! - every string: its length.
//!
//! Invariant: the memory a parsed reply holds stays within about its budget, whatever its shape.
//! A reply that is mostly one long string (a base64 picture) costs about its own size, so a
//! caller sets the budget a little above its byte cap; a reply of many small objects is refused
//! long before it can cost a hundred times its size. serde_json's recursion limit (128 levels)
//! still applies, so depth stays bounded too.

use std::cell::Cell;
use std::fmt;

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

/// The estimated cost of one value slot (`Value` is 32 bytes on a 64-bit target).
const VALUE_COST: usize = std::mem::size_of::<Value>();
/// The estimated cost of opening an object: one `BTreeMap` leaf node of `String` keys and
/// `Value` values, measured at about 632 bytes.
pub const OBJECT_COST: usize = 640;
/// The estimated cost of each object entry, on top of its key's length: the key's `String`
/// header and the value slot, plus a share of the further nodes a larger map splits into
/// (nodes hold up to 11 entries and are often half full after a split).
pub const ENTRY_COST: usize = 128;
/// The estimated cost of each array element: two value slots, because a growing `Vec` keeps up
/// to twice the room it uses.
pub const ELEMENT_COST: usize = 2 * VALUE_COST;

/// Why a reply could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyError {
    /// The bytes are not one JSON document.
    NotJson,
    /// Parsing the reply would cost more than its budget.
    TooLarge,
}

/// Parses `bytes` as one JSON document, refusing it once the estimated memory of the parsed
/// tree would pass `budget` bytes. See the module notes for how the estimate is made.
///
/// # Errors
///
/// [`ReplyError::TooLarge`] when the budget runs out, [`ReplyError::NotJson`] for anything
/// `serde_json::from_slice` would also refuse (bad syntax, trailing characters, nesting past its
/// recursion limit).
pub fn parse(bytes: &[u8], budget: usize) -> Result<Value, ReplyError> {
    let meter = Meter {
        left: Cell::new(budget),
        over: Cell::new(false),
    };
    let mut de = serde_json::Deserializer::from_slice(bytes);
    let parsed = meter
        .charge::<serde_json::Error>(VALUE_COST)
        .and_then(|()| Metered(&meter).deserialize(&mut de))
        .and_then(|value| de.end().map(|()| value));
    match parsed {
        Ok(value) => Ok(value),
        Err(_) if meter.over.get() => Err(ReplyError::TooLarge),
        Err(_) => Err(ReplyError::NotJson),
    }
}

/// What is left of the budget, and whether it ran out.
struct Meter {
    left: Cell<usize>,
    over: Cell<bool>,
}

impl Meter {
    fn charge<E: de::Error>(&self, cost: usize) -> Result<(), E> {
        if let Some(left) = self.left.get().checked_sub(cost) {
            self.left.set(left);
            Ok(())
        } else {
            self.over.set(true);
            Err(E::custom("the reply is too large to read"))
        }
    }
}

/// Builds one [`Value`], charging the meter for what it allocates.
#[derive(Clone, Copy)]
struct Metered<'a>(&'a Meter);

impl<'de> DeserializeSeed<'de> for Metered<'_> {
    type Value = Value;

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Metered<'_> {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Value, E> {
        Ok(Value::Number(v.into()))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Value, E> {
        Ok(Value::Number(v.into()))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Value, E> {
        // The same rule as serde_json's own `Value`: a number with no finite form is null.
        Ok(Number::from_f64(v).map_or(Value::Null, Value::Number))
    }

    // An escaped string reaches here too: serde_json unescapes it into a scratch buffer and
    // passes it as `&str`. (Serde's default `visit_string` also forwards here.)
    fn visit_str<E: de::Error>(self, v: &str) -> Result<Value, E> {
        self.0.charge(v.len())?;
        Ok(Value::String(v.to_owned()))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_none<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_some<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        self.deserialize(deserializer)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element_seed(self)? {
            self.0.charge(ELEMENT_COST)?;
            items.push(item);
        }
        Ok(Value::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Value, A::Error> {
        self.0.charge(OBJECT_COST)?;
        let mut map = Map::new();
        while let Some(key) = access.next_key::<String>()? {
            self.0.charge(ENTRY_COST.saturating_add(key.len()))?;
            let value = access.next_value_seed(self)?;
            map.insert(key, value);
        }
        Ok(Value::Object(map))
    }
}

#[cfg(test)]
#[path = "json_tests.rs"]
mod tests;
