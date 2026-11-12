"""Gemini jobs retain unread terminal states under bounded tracking pressure."""

PATH = 'crates/ai-gemini/src/lib.rs'
# The eviction rule moved into the registry module.
REGISTRY = 'crates/ai-gemini/src/registry.rs'
PACKAGE = 'craft-ai-gemini'
GI = [
    ('gemini-unread-terminal-evicted', REGISTRY,
     'jobs.retain(|_, s| !s.terminal_delivered);',
     'jobs.retain(|_, s| !s.state.is_terminal() || !s.events.is_empty());',
     PACKAGE, 'lib', 'registry_tests::unpolled_terminal_jobs_survive_tracking_pressure'),
    ('gemini-running-poll-counts-as-delivery', PATH,
     'slot.terminal_delivered |= slot.state.is_terminal();',
     'slot.terminal_delivered = true;',
     PACKAGE, 'lib', 'registry_tests::only_a_delivered_terminal_state_can_be_evicted'),
]
