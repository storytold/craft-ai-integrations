"""bo: a recipe only built-in runtimes run (Remove background on InSPyReNet, Burn) is never treated
as a ComfyUI model: not offered, planned, installed, registered or advised as one.

Loaded by mutation_check.py. Each case breaks one rule and names the test that should notice.
Format: (name, file, old, new, package, test target, test filter).
"""
F = 'crates/ai-features/src/'
CLS = 'crates/ai-client/src/'
FE = 'craft-ai-features'
CL = 'craft-ai-client'

BO = [
    ('recipe-run-by-comfyui', F + 'catalogue/inspyrenet.rs', 'comfyui: false,', 'comfyui: true,', FE, 'built_in_only', 'runs_inspyrenet_on_burn'),
    ('comfyui-option-for-built-in-recipe', F + 'options.rs', 'for recipe in feature.comfyui_recipes() {', 'for recipe in &feature.locals {', FE, 'built_in_only', 'only_the_built_in_route'),
    ('comfyui-plan-for-built-in-recipe', F + 'plan.rs', '    match feature.comfyui_recipes().next() {', '    match feature.locals.first() {', FE, 'built_in_only', 'planned_and_installed'),
    ('comfyui-plans-for-built-in-recipe', F + 'plan.rs', '        .comfyui_recipes()\n        .map(|recipe| plan_recipe', '        .locals\n        .iter()\n        .map(|recipe| plan_recipe', FE, 'built_in_only', 'planned_and_installed'),
    ('comfyui-offer-for-built-in-recipe', F + 'setup.rs', 'feature.comfyui_recipes().next().filter(|r| r.usable)?', 'feature.locals.first().filter(|r| r.usable)?', FE, 'built_in_only', 'planned_and_installed'),
    ('comfyui-connection-for-built-in-recipe', CLS + 'local.rs', '            .comfyui_recipes()\n            .find(|r| r.usable)', '            .locals\n            .iter()\n            .find(|r| r.usable)', CL, 'built_in_only', 'comfyui_is_refused'),
    ('named-comfyui-connection-for-built-in-recipe', CLS + 'local.rs', '            .comfyui_recipes()\n            .find(|r| r.workflow == workflow && r.usable)', '            .locals\n            .iter()\n            .find(|r| r.workflow == workflow && r.usable)', CL, 'built_in_only', 'comfyui_is_refused'),
    ('comfyui-advised-without-recipe', CLS + 'engine.rs', 'if feature.comfyui_recipes().any(|r| r.usable) {', 'if true {', CL, 'built_in_only', 'never_comfyui'),
]
