# Changing the shared store file

Every Craft app pins its own revision of this layer, and they all share one store file. So a build
often reads and rewrites a file saved by a newer build. This page says what that build does, and when
a change needs a `SCHEMA_VERSION` bump.

## What an older build does

- **It keeps fields it does not know.** `SharedStore` carries them through a rewrite at every depth.
  See `crates/ai-native/src/shared/keep_unknown.rs`.
- **It matches list items by identity:** `id`, then `role`, then `capability`. Items can be reordered.
  An item it removes takes its unknown fields with it.
- **It leaves an item as written when it cannot match it safely:** when the identity appears twice, or
  when a list's items have no identity field.
- **It refuses a file with a higher `schema_version`,** for reading and for writing. A bump therefore
  shuts every older app out of the shared file until that app updates.

Only builds that include this behaviour are protected. A revision pinned before it still drops nested
unknown fields until that app moves its pin.

## Do you need a bump?

| Change | Bump? | Why |
|---|---|---|
| Add an optional field with `#[serde(default)]` | No | Older builds keep it and ignore it |
| Add a value to an `open_enum!` type | No | Older builds read it as an unknown value and keep it |
| Add a new list field | No | Older builds copy a field they do not know whole, with all its items |
| Add a field to the items of an existing list | No, if each item has `id`, `role` or `capability` | Otherwise unknown fields inside those items are not kept |
| Add a required field | Yes | Older builds cannot write a file that newer builds can read |
| Add a value to a closed enum | Yes | Older builds fail to parse the whole file |
| Rename a field, or change its type or meaning | Yes | Older builds would keep the old field or misread the new one |
| Add a serde `alias` for an old name | Avoid | An older file keeps the old name as an unknown field, so the file then holds both names |

If a field's meaning depends on another field the older build can change, it can go stale: an older
build keeps the field but does not update it. Do not store such a pair without a bump. For example, a
hash of `model` beside `model` would be wrong after an older build changes `model`.

A bump is a last resort, because it stops older apps from reading the connections they already use.
Prefer an additive change.

## Reserved model tags

A model's `tags` hold catalogue labels such as `recommended`, and also the shared layer's markers for
what a person declared when they assigned the model to a tool. A host must not show these as labels:

| Tag | Means |
|---|---|
| `declared-model` | The connection did not list this model; a person added it for a tool |
| `declared:<capability>` | A person declared this whole ability; the provider did not report it |
| `declared-input:<capability>:<input>` | A person added this input to an ability the provider reports |

A refresh or a saved form puts these declarations back on top of the provider's new list, and a
withdrawal takes back only what they mark. An older build that rewrites the list drops a marker and
the declaration it marks together, so the two never disagree.
