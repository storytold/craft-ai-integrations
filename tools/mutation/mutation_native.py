"""Deliberate-bug cases for group `nf`: planning and offering a built-in runtime's own files.

Loaded by mutation_check.py. Each case breaks one rule and names the test that should notice.
Format: (name, file, old, new, package, test target, test filter).
"""
F = 'crates/ai-features/src/'
FE = 'craft-ai-features'

NF = [
    ('native-plan-ignores-usable', F + 'feature.rs', '.find(|r| r.usable && r.native.iter().any(|n| n.provider == provider))', '.find(|r| r.native.iter().any(|n| n.provider == provider))', FE, 'native_setup', 'cannot_run_yet'),
    ('engine-recipe-ignores-the-engine', F + 'feature.rs', '.find(|r| r.usable && r.native.iter().any(|n| n.provider == provider))', '.find(|r| r.usable)', FE, 'engine_recipe', 'each_engine_plans_from'),
    ('native-plan-takes-the-first-recipe', F + 'plan.rs', 'let recipe = feature.engine_recipe(provider);', 'let recipe = feature.first_usable_recipe();', FE, 'engine_recipe', 'each_engine_plans_from'),
    ('first-usable-recipe-takes-the-first', F + 'feature.rs', 'self.locals.iter().find(|r| r.usable)', 'self.locals.first()', FE, 'engine_recipe', 'plain_built_in_offer_skips'),
    ('native-offer-takes-the-first-recipe', F + 'setup.rs', 'Some(provider) => feature.engine_recipe(provider)?,', 'Some(_) => feature.locals.first()?,', FE, 'setup', 'unusable_first_recipe'),
    ('plain-built-in-offer-takes-the-first-recipe', F + 'setup.rs', 'None => feature.first_usable_recipe()?,', 'None => feature.locals.first()?,', FE, 'engine_recipe', 'plain_built_in_offer_skips'),
    ('native-option-from-every-recipe', F + 'feature.rs', 'if !out.contains(&route.provider.as_str()) {', 'if true {', FE, 'native_setup', 'first_usable_recipe'),
    ('native-option-from-the-first-recipe-only', F + 'options.rs', '.engine_recipe(provider)\n                .and_then(|r| r.native.iter().find(|n| n.provider == provider))', '.first_usable_recipe()\n                .and_then(|r| r.native.iter().find(|n| n.provider == provider))', FE, 'engine_recipe', 'each_engine_plans_from'),
    ('native-plan-takes-a-route-without-files', F + 'plan.rs', '.find(|n| n.provider == provider && !n.models.is_empty())\n    }) else {\n        return out;\n    };\n    fill_files(&mut out, &route.models, inventory);\n    check_disk', '.find(|n| n.provider == provider)\n    }) else {\n        return out;\n    };\n    fill_files(&mut out, &route.models, inventory);\n    check_disk', FE, 'native_setup', 'unfinished_or_unknown'),
    ('native-plan-takes-any-provider', F + 'plan.rs', '.find(|n| n.provider == provider && !n.models.is_empty())\n    }) else {\n        return out;\n    };\n    fill_files(&mut out, &route.models, inventory);\n    check_disk', '.find(|n| !n.models.is_empty())\n    }) else {\n        return out;\n    };\n    fill_files(&mut out, &route.models, inventory);\n    check_disk', FE, 'native_setup', 'unfinished_or_unknown'),
    ('native-plan-skips-the-disk-check', F + 'plan.rs', '    fill_files(&mut out, &route.models, inventory);\n    check_disk(&mut out, inventory);', '    fill_files(&mut out, &route.models, inventory);', FE, 'native_setup', 'free_space'),
    ('native-offer-ignores-the-provider', F + 'setup.rs', 'selected_provider.is_none_or(|provider| n.provider == provider)', 'true', FE, 'setup', 'selected_provider'),
    ('built-in-offer-compares-comfyui-files', F + 'setup.rs', 'let compared = if let Some(native) = native {', 'let compared = if let Some(native) = native.filter(|_| false) {', FE, 'setup', 'needs_only_the_model'),
    ('option-offered-for-a-route-without-files', F + 'options.rs', '                .filter(|n| !n.models.is_empty())\n            else {', '            else {', FE, 'native_setup', 'not_unbuilt_burn'),
    ('native-options-before-later-locals', F + 'options.rs', 'locals.extend(natives);', 'locals.splice(0..0, natives);', FE, 'native_setup', 'follow_every_local'),
    ('built-in-offer-uses-the-comfyui-terms', F + 'setup.rs', 'agreement: agreement_for(route),', 'agreement: agreement(),', FE, 'setup', 'agrees_to_its_own'),
    ('native-option-loses-its-model-id', F + 'options.rs', 'model: native.model.clone(),', 'model: String::new(),', FE, 'native_setup', 'candle_with_its_own_size'),
    ('onnx-is-not-weights', F + 'recipe.rs', '            name.ends_with(".safetensors") || name.ends_with(".onnx")', '            name.ends_with(".safetensors")', FE, 'native_onnx', 'an_onnx_file'),
    ('onnx-name-is-case-sensitive', F + 'recipe.rs', 'let name = m.filename.to_ascii_lowercase();', 'let name = m.filename.clone();', FE, 'native_onnx', 'an_onnx_file'),
    ('onnx-path-refused-when-binding', 'crates/ai-client/src/local.rs', '["safetensors", "onnx", "bpk"]', '["safetensors", "bpk"]', 'craft-ai-client', 'local_native', 'the_native_route_is_registered'),
]
