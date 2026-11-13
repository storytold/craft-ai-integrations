//! Which Craft apps the gateway can reach: the ArtCraft Launcher's app list, read through
//! `craft-ai-agents`, which the launcher writes it with and every app checks it with. The list is
//! also the person's consent: without it the gateway offers no app, and it asks again before every
//! tool call ([`switched_on`]), so turning the switch off stops a gateway a client already runs.

pub use craft_ai_agents::{
    APPS_FILE, AppEntry, DIR_ENV, Found, KNOWN_APPS, MAX_APPS, agents_dir as mcp_dir, known_name,
    load, parse, switched_on, valid_id,
};

/// The apps to offer: the launcher's list, or none while agents are turned off. The second value
/// says what was left out and why.
pub fn discover() -> Found {
    let Some(dir) = mcp_dir() else {
        return (Vec::new(), vec!["no per-user configuration folder".into()]);
    };
    match load(&dir) {
        Ok(Some(found)) => found,
        Ok(None) => (Vec::new(), vec![super::SWITCHED_OFF.into()]),
        Err(e) => (Vec::new(), vec![e]),
    }
}
