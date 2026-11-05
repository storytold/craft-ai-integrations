"""Deliberate-bug cases for group `nw`: the native setup widget.

Loaded by mutation_check.py. The case breaks one rule and names the test that should notice.
Format: (name, file, old, new, package, test target, test filter).
"""
NW = [
    ('native-widget-download-without-visible-licence-approval', 'crates/ai-egui/src/native_setup.rs', 'session.blocker().is_none()', 'plan.available()', 'craft-ai-egui', 'native_setup_view', 'it_reviews_the_tool_destination_and_licence_before_any_download'),
    ('native-widget-calls-checked-files-ready', 'crates/ai-egui/src/native_setup.rs', 'ui.label("Files checked. This model has not been tried yet.");', 'ui.label("Ready");', 'craft-ai-egui', 'native_setup_view', 'checked_files_are_offered_for_use'),
]
