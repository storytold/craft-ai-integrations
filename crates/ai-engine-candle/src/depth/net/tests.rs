use super::*;

#[test]
fn crop_resample_maps_a_larger_output_onto_the_input() {
    // A 4x4 output over a 2x2 input: the top-left input pixel reads the top-left 2x2 block.
    let out: Vec<f32> = (0..16).map(|v| v as f32).collect();
    let r = crop_resample(&out, (4, 4), 2, (1, 1));
    assert_eq!(r.len(), 1);
    assert!((r[0] - (0.0 + 1.0 + 4.0 + 5.0) / 4.0).abs() < 1e-5, "{r:?}");
    // Same size is the identity.
    assert_eq!(crop_resample(&out, (4, 4), 4, (4, 4)), out);
}

#[test]
fn padding_repeats_the_last_row_and_column() {
    // One channel's worth of a 2 x 1 picture on a 3 x 3 square, the other two channels equal.
    let data = [1.0, 2.0, 1.0, 2.0, 1.0, 2.0];
    let sq = pad_square(&data, 2, 1, 3);
    assert_eq!(&sq[..9], &[1.0, 2.0, 2.0, 1.0, 2.0, 2.0, 1.0, 2.0, 2.0]);
}

#[test]
fn weights_that_are_not_safetensors_fail_to_load_with_words() {
    let e = Worker::start(b"not safetensors".to_vec(), b"nope".to_vec())
        .err()
        .unwrap();
    assert_eq!(e.kind, craft_ai_core::ErrorKind::Provider);
    assert!(e.message.contains("depth model"), "{}", e.message);
    assert!(Worker::start(Vec::new(), vec![1]).is_err());
}

/// Zero weights build the whole network and run it (slow, so on request): the pipeline's sizes,
/// range and finite values hold whatever the weights say.
#[test]
#[ignore = "builds and runs the full network on zero weights (slow)"]
fn pipeline_with_zero_weights() {
    let dev = Device::Cpu;
    let vb = VarBuilder::zeros(DType::F32, &dev);
    let net = Net::new(vb.clone(), vb).unwrap();
    let rgb = vec![0.5f32; 64 * 48 * 3];
    let map = net.run(&rgb, 64, 48).unwrap();
    assert_eq!((map.width, map.height), (518, 392));
    assert!(
        map.data
            .iter()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
    );
}
