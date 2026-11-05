"""Deliberate-bug cases for group `gs`: the generic `image.segment` route on ComfyUI (the scratch
sinks, the plain refusal of clicks and the stored selection workflow).

Loaded by mutation_check.py. Each case breaks one rule and names the test that should notice.
Format: (name, file, old, new, package, test target, test filter).
"""
PV = 'crates/ai-comfyui/src/provider.rs'
WF = 'crates/ai-comfyui/src/workflow.rs'
LB = 'crates/ai-comfyui/src/library.rs'
CC = 'craft-ai-comfyui'

GS_CASES = [
    ('sink-not-unique', PV, '        crate::scratch::route_sinks(&mut graph)?;\n', '', CC, 'segment_route', 'every_selection_has_its_own_sink'),
    ('clicks-get-the-generic-refusal', WF, 'if self.capability.as_str() == "image.segment" {\n                    "This', 'if false {\n                    "This', CC, 'segment_route', 'clicks_are_refused'),
    ('stored-selection-takes-a-mask', LB, '            if self.mask.is_some() {\n                return Err(AiError::invalid("a selection workflow has no mask binding"));\n            }\n', '', CC, 'segment_route', 'a_stored_workflow_can_be'),
    ('plain-import-of-a-selection', WF, 'if capability.as_str() == "image.segment" {', 'if false {', CC, 'segment_route', 'a_selection_workflow_needs'),
]
