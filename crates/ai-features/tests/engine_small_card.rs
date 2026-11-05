//! An engine measured quick on the main chip is not called slow because the card that matches it
//! is too small for it: the card is simply not needed. Pure data, no machine probed.
use craft_ai_features::{EngineOffer, Gpu, GpuVendor, Os, Pace, SystemProfile, advise};

/// A Windows machine with one small Nvidia card.
fn small_card_machine() -> SystemProfile {
    SystemProfile {
        os: Os::Windows,
        gpus: vec![Gpu {
            vendor: GpuVendor::Nvidia,
            name: "Small".to_owned(),
            vram_mb: Some(2_048),
        }],
        comfyui_found: false,
    }
}

/// A built-in engine whose build can use an Nvidia card.
fn offer(fast_on_processor: bool) -> EngineOffer {
    EngineOffer {
        provider: "rten".to_owned(),
        adapter_present: true,
        accelerators: Some(vec![GpuVendor::Nvidia]),
        fast_on_processor,
    }
}

/// With a card too small for the model, a processor-quick engine runs on the processor and is
/// not called slow; an engine with no such statement is still slow.
#[test]
fn a_card_too_small_does_not_make_a_processor_quick_engine_slow() {
    let machine = small_card_machine();
    let quick = advise(&machine, &[offer(true)], Some(8_192));
    assert_eq!(quick[0].pace, Some(Pace::Processor), "{quick:?}");
    assert!(!quick[0].reason.contains("slow"), "{}", quick[0].reason);
    let other = advise(&machine, &[offer(false)], Some(8_192));
    assert_eq!(other[0].pace, Some(Pace::Cpu), "{other:?}");
}
