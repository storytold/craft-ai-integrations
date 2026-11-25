//! A built-in engine measured quick on the main chip: it is suggested by the recipe's order,
//! with or without a graphics card, and its sentence is plain language. Pure data, no machine probed.
use craft_ai_features::{
    EngineAdvice, EngineOffer, Gpu, GpuVendor, Os, Pace, SystemProfile, advise,
};

/// Engine and hardware words that must never reach a sentence a person reads.
const JARGON: &[&str] = &[
    "cuda", "candle", "burn", "rten", "wgpu", "vram", "cpu", "gpu",
];

/// A built-in engine offer, with what the app said about its build.
fn offer(provider: &str, accelerators: Vec<GpuVendor>, fast: bool) -> EngineOffer {
    EngineOffer {
        provider: provider.to_owned(),
        adapter_present: true,
        accelerators: Some(accelerators),
        fast_on_processor: fast,
    }
}

/// A Windows machine with the given cards.
fn machine(gpus: Vec<Gpu>) -> SystemProfile {
    SystemProfile {
        os: Os::Windows,
        gpus,
        comfyui_found: false,
    }
}

/// The one engine advice recommends, asserting there is at most one.
fn pick(advice: &[EngineAdvice]) -> Option<&str> {
    let all: Vec<&EngineAdvice> = advice.iter().filter(|a| a.recommended).collect();
    assert!(all.len() <= 1, "at most one engine is recommended");
    all.first().map(|a| a.provider.as_str())
}

/// A built-in engine marked quick on the main chip is not called slow without a card, and it ranks
/// level with a card engine, so the recipe's order (its first engine) decides the suggestion.
#[test]
fn an_engine_that_is_quick_on_the_main_chip_is_suggested_by_recipe_order_and_not_called_slow() {
    let offers = vec![
        offer("rten", Vec::new(), true),
        offer("burn", vec![GpuVendor::Nvidia], false),
    ];
    let card = Gpu {
        vendor: GpuVendor::Nvidia,
        name: "RTX".to_owned(),
        vram_mb: Some(24_576),
    };
    for gpus in [vec![card], Vec::new()] {
        let advice = advise(&machine(gpus), &offers, None);
        assert_eq!(pick(&advice), Some("rten"), "{advice:?}");
        let rten = advice.iter().find(|a| a.provider == "rten").unwrap();
        assert_eq!(rten.pace, Some(Pace::Processor));
        assert!(!rten.reason.contains("slow"), "{}", rten.reason);
        for a in &advice {
            let words = a.reason.split(|c: char| !c.is_ascii_alphanumeric());
            for word in words.map(str::to_ascii_lowercase) {
                assert!(!JARGON.contains(&word.as_str()), "{}", a.reason);
            }
        }
    }
}

/// Without the statement the old wording stands: no card, slow.
#[test]
fn an_engine_without_the_statement_is_still_slow_with_no_card() {
    let advice = advise(
        &machine(Vec::new()),
        &[offer("rten", Vec::new(), false)],
        None,
    );
    assert_eq!(advice[0].pace, Some(Pace::Cpu));
}
