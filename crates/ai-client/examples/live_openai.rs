//! One real run of the hosted flow against OpenAI, or against ArtCraft Omni with `omni`, using the
//! key the launcher saved in the OS keychain. Costs real (small) money for `text`, `image` and
//! `omni`; `probe` only lists models.
//!
//! ```text
//! cargo run -p craft-ai-client --example live_openai -- probe
//! cargo run -p craft-ai-client --example live_openai -- text
//! cargo run -p craft-ai-client --example live_openai -- image <out.png>
//! cargo run -p craft-ai-client --example live_openai -- text --record <runs.jsonl>
//! cargo run -p craft-ai-client --example live_openai -- omni --connection <id> --record <runs.jsonl>
//! ```
//!
//! `omni` asks the saved ArtCraft Omni connection for one `nano_banana` image from a fixed prompt.
//! Omni returns the image as a link, which is reported by type only and never kept. `--connection`
//! picks one saved connection by id when several use the same service; without it the first
//! enabled one is used.
//!
//! `--record <file>` appends a [`RunRecord`] line for the run: what was sent (kinds and sizes, not
//! the prompt), where, how long it took, and each result's type. Returned text and inline bytes are
//! kept as their size and SHA-256; a returned link is not downloaded or hashed, so it keeps only its
//! type and any size the service declared. That line is the evidence a real run leaves; it is
//! refused if it would contain the key.
//!
//! It works on a copy of the shared store in a temporary folder, so the person's real connections,
//! bindings and history are never written. The key is read through the same keychain code the apps
//! use and goes only into the adapter. Everything printed passes through `Say`, which replaces the
//! key if it ever appears, and a leak check at the end reads back every file this run wrote.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use craft_ai_client::{AiClient, ProviderFactory, RoleStatus, RunRecord, ToolDesk};
use craft_ai_core::{
    Auth, CapabilityId, ConsentLedger, Event, InputRole, JobState, Payload, Request, Role, RoleId,
    SecretString,
};
use craft_ai_native::HistoryFile;
use craft_ai_native::secrets::{KeychainStore, SecretStore};
use craft_ai_native::shared::SharedStore;

/// Prints, after making sure the key is not in the text.
struct Say {
    key: String,
    seen: std::cell::RefCell<Vec<String>>,
}

impl Say {
    /// Prints `text` with the key replaced, and keeps a copy for the final leak check.
    fn line(&self, text: impl AsRef<str>) {
        let mut text = text.as_ref().to_owned();
        if !self.key.is_empty() && text.contains(&self.key) {
            text = text.replace(&self.key, "[key removed]");
        }
        self.seen.borrow_mut().push(text.clone());
        println!("{text}");
    }
}

/// Runs the chosen mode (`probe`, `text`, `image` or `omni`) on a copy of the store, prints what
/// happened through `Say`, appends a run record when `--record` is given, then proves the real
/// store and every written file are free of the key.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let record_to = take_flag(&mut args, "--record")?.map(PathBuf::from);
    let chosen = take_flag(&mut args, "--connection")?;
    let mode = args.first().map(String::as_str).unwrap_or("probe");
    // Checked before anything is written: the image would otherwise replace the record.
    if let (Some(record), Some(image)) = (&record_to, args.get(1))
        && same_file(record, Path::new(image))?
    {
        return Err("--record and the image output name the same file".into());
    }

    // The real store, read only. Its bytes are compared at the end to prove it was not touched.
    let real = SharedStore::at_default_location()?;
    let before = std::fs::read(real.path())?;
    let store = real.load()?;
    let (service, name) = if mode == "omni" {
        ("artcraft-omni", "ArtCraft Omni")
    } else {
        ("openai", "OpenAI")
    };
    let connection = store
        .connections
        .iter()
        .find(|c| {
            c.provider == service
                && c.enabled
                && chosen.as_deref().is_none_or(|id| c.id.as_str() == id)
        })
        .ok_or(format!(
            "no matching {name} connection is saved in the launcher"
        ))?
        .clone();
    let Auth::ApiKey { secret: secret_ref } = &connection.auth else {
        return Err(format!("the {name} connection does not use a key").into());
    };

    let keychain = KeychainStore::new();
    let Some(key) = keychain.get(secret_ref)? else {
        println!("key in the OS keychain: no (nothing to run)");
        return Ok(());
    };
    let say = Say {
        key: key.expose().to_owned(),
        seen: Default::default(),
    };
    say.line("key in the OS keychain: yes");

    // Step 1, free: ask the service which models this key can use. Omni has no free key test, so
    // its saved list is used as it stands; the run itself is what checks it.
    let models = if mode == "omni" {
        connection.models.clone()
    } else {
        ProviderFactory::probing()
            .build(&connection, Some(SecretString::new(key.expose())))?
            .models()?
    };
    say.line(format!(
        "models this key offers that the layer understands: {}",
        models.len()
    ));
    let wanted: &[&str] = match mode {
        "image" => &["gpt-image-1-mini", "gpt-image-1"],
        "omni" => &["nano_banana"],
        _ => &["gpt-4o-mini", "gpt-4.1-mini", "gpt-4o"],
    };
    let model = wanted
        .iter()
        .find_map(|w| models.iter().find(|m| m.id.as_str() == *w))
        .cloned();
    if mode == "probe" {
        let mut ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        ids.truncate(12);
        say.line(format!("first models: {ids:?}"));
        return finish(&say, &real, &before, &[]);
    }
    let Some(model) = model else {
        say.line(format!("none of {wanted:?} is available to this key"));
        return finish(&say, &real, &before, &[]);
    };
    say.line(format!("using model {}", model.id));

    // A temporary copy of the store, with the models just found and a tool for this run.
    let dir = tempfile::tempdir()?;
    let shared = SharedStore::new(dir.path().join("connections.json"));
    shared.replace(&store)?;
    let connection_id = connection.id.clone();
    let listed = models.clone();
    shared.update(|s| {
        if let Some(c) = s.connections.iter_mut().find(|c| c.id == connection_id) {
            c.models = listed;
        }
    })?;
    let (capability, label, inputs) = if mode == "image" || mode == "omni" {
        ("image.generate", "Smoke: image", vec![InputRole::Prompt])
    } else {
        ("text.complete", "Smoke: text", vec![InputRole::Prompt])
    };
    let role = Role {
        id: RoleId::new("smoke-test")?,
        label: label.into(),
        capability: CapabilityId::parse(capability)?,
        inputs,
    };
    let history_file = HistoryFile::new(dir.path().join("jobs.jsonl"));
    let client = AiClient::new(
        craft_ai_core::AppId::new("smoke")?,
        shared.clone(),
        Arc::new(KeychainStore::new()),
    )
    .with_history(history_file.clone());
    let desk = ToolDesk::new(client, shared.clone(), None);
    desk.register_tool(role.clone())?;
    desk.choose(&role, &connection.id, &model.id)?;

    let client = AiClient::new(
        craft_ai_core::AppId::new("smoke")?,
        shared.clone(),
        Arc::new(KeychainStore::new()),
    )
    .with_history(history_file.clone());
    match client.status(&role) {
        RoleStatus::Ready(r) => say.line(format!(
            "status: ready ({} / {})",
            r.connection_label, r.model_label
        )),
        RoleStatus::Unavailable { reason } => {
            say.line(format!("status: unavailable: {reason}"));
            return finish(&say, &real, &before, &[dir.path().to_owned()]);
        }
    }

    let request = if mode == "image" {
        Request::new(role.capability.clone())
            .with_input(
                InputRole::Prompt,
                Payload::text("a plain red circle on a white background"),
            )
            .with_param("size", serde_json::json!("1024x1024"))
            .with_param("quality", serde_json::json!("low"))
    } else if mode == "omni" {
        // The Omni adapter maps no generation parameters, so none are sent.
        Request::new(role.capability.clone()).with_input(
            InputRole::Prompt,
            Payload::text("a plain red circle on a white background"),
        )
    } else {
        Request::new(role.capability.clone())
            .with_input(
                InputRole::Prompt,
                Payload::text("Reply with exactly one word: pong"),
            )
            .with_param("max_tokens", serde_json::json!(8))
    };
    let plan = client.plan(&role, request)?;
    say.line(format!(
        "consent needed before sending: {} (destination {})",
        plan.needs_consent(&ConsentLedger::default()),
        plan.disclosure().destination
    ));
    // The person said go to this run, so the run grants consent for exactly this plan.
    let mut ledger = ConsentLedger::default();
    ledger.grant(plan.disclosure());
    let started = chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now());
    let clock = std::time::Instant::now();
    let job = client.start(&plan, &ledger)?;
    let done = job.wait(Duration::from_millis(500), Duration::from_secs(300))?;
    if let Some(path) = &record_to {
        let line = RunRecord::new(&plan, &done, started, clock.elapsed(), &[&say.key])
            .to_json_line(&[&say.key])?;
        append_line(path, &line)?;
        say.line(format!("run recorded in {}", path.display()));
    }
    say.line(format!("job ended: {:?}", short(&done.state)));
    let mut wrote: Vec<PathBuf> = vec![dir.path().to_owned()];
    wrote.extend(record_to.iter().cloned());
    for event in &done.events {
        match event {
            Event::TextDelta { text } => say.line(format!("text: {text}")),
            Event::Notice { message } => say.line(format!("notice: {message}")),
            Event::Output(o) => match &o.payload {
                Payload::Text { text } => say.line(format!("output text: {text}")),
                Payload::Inline { media_type, data } => {
                    say.line(format!("output: {} bytes of {media_type}", data.len()));
                    if let Some(out) = args.get(1) {
                        std::fs::write(out, data)?;
                        wrote.push(PathBuf::from(out));
                        say.line(format!("saved to {out}"));
                    }
                }
                Payload::Handle(h) => say.line(format!("output link of type {}", h.media_type)),
            },
            _ => {}
        }
    }
    if let Some(usage) = done.usage {
        say.line(format!("usage: {usage:?}"));
    }
    let entries = client.history().map(|h| h.entries.len()).unwrap_or(0);
    say.line(format!("history entries written: {entries}"));
    finish(&say, &real, &before, &wrote)
}

/// Removes `flag <value>` from the arguments and returns the value.
fn take_flag(
    args: &mut Vec<String>,
    flag: &str,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let Some(at) = args.iter().position(|a| a == flag) else {
        return Ok(None);
    };
    if at + 1 >= args.len() {
        return Err(format!("{flag} needs a value").into());
    }
    let value = args.remove(at + 1);
    args.remove(at);
    Ok(Some(value))
}

/// Appends one line, so several runs collect in one file.
fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{line}")
}

fn short(state: &JobState) -> String {
    match state {
        JobState::Failed { error } => format!("failed ({:?}: {})", error.kind, error.message),
        other => format!("{other:?}"),
    }
}

/// Proves the real store was not written and that no file this run wrote contains the key.
fn finish(
    say: &Say,
    real: &SharedStore,
    before: &[u8],
    written: &[PathBuf],
) -> Result<(), Box<dyn std::error::Error>> {
    let untouched = std::fs::read(real.path())? == before;
    let mut leaked = say.seen.borrow().iter().any(|l| l.contains(&say.key));
    for path in written {
        leaked |= contains(path, say.key.as_bytes())?;
    }
    println!("real store untouched: {untouched}");
    println!(
        "leak check over output and every file written: {}",
        if leaked { "KEY FOUND" } else { "clean" }
    );
    if leaked || !untouched {
        return Err("the run did not keep its promises".into());
    }
    Ok(())
}

/// Whether the file at `path`, or any file under it when it is a folder, holds `needle`. The run
/// record and a saved image are single files, so both kinds are read. A file or folder that cannot
/// be read is an error, never "clean": the check must not pass without reading everything.
fn contains(path: &Path, needle: &[u8]) -> std::io::Result<bool> {
    if !path.is_dir() {
        let bytes = std::fs::read(path)?;
        return Ok(!needle.is_empty() && bytes.windows(needle.len()).any(|w| w == needle));
    }
    for entry in std::fs::read_dir(path)? {
        if contains(&entry?.path(), needle)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether `a` and `b` name one file: the same absolute path, or, when both exist, the same file
/// once links are followed.
fn same_file(a: &Path, b: &Path) -> std::io::Result<bool> {
    if std::path::absolute(a)? == std::path::absolute(b)? {
        return Ok(true);
    }
    Ok(match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    })
}
