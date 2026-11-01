//! WAV in and out for the `audio.enhance` runner: any PCM or float WAV in, 32-bit float WAV out at
//! the same rate and channel count.

use std::io::Cursor;

use craft_ai_core::AiError;

/// Samples per channel and the format they came in.
pub(crate) struct Audio {
    pub sample_rate: u32,
    /// One vector per channel, all the same length.
    pub channels: Vec<Vec<f32>>,
}

/// The most channels the runner takes: each is processed on its own, so this bounds the work.
pub(crate) const MAX_CHANNELS: usize = 8;

/// Reads a WAV file. Refuses anything else in words a person can act on.
pub(crate) fn decode(data: &[u8]) -> Result<Audio, AiError> {
    let reader = hound::WavReader::new(Cursor::new(data))
        .map_err(|_| AiError::invalid("The audio must be a WAV file."))?;
    let spec = reader.spec();
    let count = usize::from(spec.channels);
    if count == 0 || count > MAX_CHANNELS {
        return Err(AiError::invalid(format!(
            "The audio has {count} channels; up to {MAX_CHANNELS} are supported."
        )));
    }
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float if spec.bits_per_sample == 32 => reader
            .into_samples::<f32>()
            .collect::<Result<_, _>>()
            .map_err(|_| unreadable())?,
        hound::SampleFormat::Int if (8..=32).contains(&spec.bits_per_sample) => {
            let scale = (1u64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .into_samples::<i32>()
                .map(|s| s.map(|v| v as f32 / scale))
                .collect::<Result<_, _>>()
                .map_err(|_| unreadable())?
        }
        _ => {
            return Err(AiError::invalid(
                "This WAV sample format is not supported. Use 16, 24 or 32-bit PCM, or 32-bit float.",
            ));
        }
    };
    let frames = interleaved.len() / count;
    let mut channels = vec![Vec::with_capacity(frames); count];
    for frame in interleaved.chunks_exact(count) {
        for (ch, s) in channels.iter_mut().zip(frame) {
            ch.push(if s.is_finite() { *s } else { 0. });
        }
    }
    Ok(Audio {
        sample_rate: spec.sample_rate,
        channels,
    })
}

fn unreadable() -> AiError {
    AiError::invalid("The WAV file is damaged or cut short.")
}

/// Writes `channels` (all the same length) as a 32-bit float WAV.
pub(crate) fn encode(sample_rate: u32, channels: &[Vec<f32>]) -> Result<Vec<u8>, AiError> {
    let spec = hound::WavSpec {
        channels: u16::try_from(channels.len())
            .map_err(|_| AiError::invalid("Too many channels."))?,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let frames = channels.first().map_or(0, Vec::len);
    let mut out = Cursor::new(Vec::with_capacity(44 + frames * channels.len() * 4));
    let write = |out: &mut Cursor<Vec<u8>>| -> Result<(), hound::Error> {
        let mut w = hound::WavWriter::new(out, spec)?;
        for i in 0..frames {
            for ch in channels {
                w.write_sample(ch.get(i).copied().unwrap_or(0.))?;
            }
        }
        w.finalize()
    };
    write(&mut out)
        .map_err(|e| AiError::invalid(format!("The result could not be written: {e}")))?;
    Ok(out.into_inner())
}
