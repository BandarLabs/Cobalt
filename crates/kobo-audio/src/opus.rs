//! Opus, decoded by the pure Rust `rusty-opus` crate and offered to
//! symphonia as one of its codecs, so an Ogg Opus book goes through the
//! same demuxer, seek and metadata path as a Vorbis one.
//!
//! Opus always decodes at 48 kHz; the demuxer reports that rate and the
//! stream's pre-skip as the track's delay, and symphonia trims the delay
//! off the first packets through `trim_start`, so nothing here has to know
//! about either.

use rusty_opus::OpusDecoder;
use symphonia::core::audio::{
    AsGenericAudioBufferRef, AudioBuffer, AudioSpec, Channels, GenericAudioBufferRef,
};
use symphonia::core::codecs::audio::well_known::CODEC_ID_OPUS;
use symphonia::core::codecs::audio::{
    AudioCodecParameters, AudioDecoder, AudioDecoderOptions, FinalizeResult,
};
use symphonia::core::codecs::registry::{RegisterableAudioDecoder, SupportedAudioCodec};
use symphonia::core::codecs::CodecInfo;
use symphonia::core::errors::{decode_error, unsupported_error, Result};
use symphonia::core::packet::PacketRef;

/// The longest an Opus packet may play: 120 ms at 48 kHz.
const MAX_FRAMES: usize = 5_760;

const INFO: CodecInfo = CodecInfo {
    short_name: "opus",
    long_name: "Opus",
    profiles: &[],
};

pub(crate) struct Adapter {
    decoder: OpusDecoder,
    params: AudioCodecParameters,
    channels: usize,
    pcm: Vec<f32>,
    buf: AudioBuffer<i16>,
}

impl Adapter {
    fn try_new(params: &AudioCodecParameters) -> Result<Self> {
        let channels = params.channels.as_ref().map_or(2, Channels::count);
        if !(1..=2).contains(&channels) {
            return unsupported_error("opus: more than two channels");
        }
        let decoder = OpusDecoder::new(48_000, channels)
            .map_err(|_| symphonia::core::errors::Error::Unsupported("opus: decoder"))?;
        let mut params = params.clone();
        params.sample_rate = Some(48_000);
        let spec = AudioSpec::new(
            48_000,
            params.channels.clone().unwrap_or(Channels::Discrete(2)),
        );
        Ok(Self {
            decoder,
            params,
            channels,
            pcm: vec![0.0; MAX_FRAMES * channels],
            buf: AudioBuffer::new(spec, MAX_FRAMES),
        })
    }

    fn decode_inner(&mut self, packet: &PacketRef<'_>) -> Result<()> {
        self.buf.clear();
        let frames = self
            .decoder
            .decode(packet.data, MAX_FRAMES, &mut self.pcm)
            .map_err(|_| symphonia::core::errors::Error::DecodeError("opus: packet"))?;
        if frames > MAX_FRAMES {
            return decode_error("opus: packet too long");
        }
        let channels = self.channels;
        let pcm = &self.pcm;
        self.buf.render_with(Some(frames), |index, planes| {
            for (channel, plane) in planes.iter_mut().enumerate() {
                plane[index] = to_i16(pcm[index * channels + channel.min(channels - 1)]);
            }
            Ok(())
        })?;
        let trim_start = usize::try_from(packet.trim_start.get()).unwrap_or(0);
        let trim_end = usize::try_from(packet.trim_end.get()).unwrap_or(0);
        if trim_start > 0 || trim_end > 0 {
            self.buf.trim(trim_start, trim_end);
        }
        Ok(())
    }
}

/// A sample in the decoder's nominal range, clipped to sixteen bits. Opus
/// can overshoot the range by design on loud transients; clipping is what
/// every integer Opus output does with that.
#[allow(
    clippy::cast_possible_truncation,
    reason = "the value is clamped to the i16 range before the cast"
)]
fn to_i16(sample: f32) -> i16 {
    (sample * 32_767.0).round().clamp(-32_768.0, 32_767.0) as i16
}

impl AudioDecoder for Adapter {
    fn reset(&mut self) {
        // The decoder has no reset of its own; a fresh one is the same
        // thing, and a seek is rare enough to pay for it.
        if let Ok(fresh) = OpusDecoder::new(48_000, self.channels) {
            self.decoder = fresh;
        }
        self.buf.clear();
    }

    fn codec_info(&self) -> &CodecInfo {
        &INFO
    }

    fn codec_params(&self) -> &AudioCodecParameters {
        &self.params
    }

    fn decode_ref(&mut self, packet: &PacketRef<'_>) -> Result<GenericAudioBufferRef<'_>> {
        if let Err(error) = self.decode_inner(packet) {
            self.buf.clear();
            return Err(error);
        }
        Ok(self.buf.as_generic_audio_buffer_ref())
    }

    fn finalize(&mut self) -> FinalizeResult {
        FinalizeResult::default()
    }

    fn last_decoded(&self) -> GenericAudioBufferRef<'_> {
        self.buf.as_generic_audio_buffer_ref()
    }
}

impl RegisterableAudioDecoder for Adapter {
    fn try_registry_new(
        params: &AudioCodecParameters,
        _opts: &AudioDecoderOptions,
    ) -> Result<Box<dyn AudioDecoder>> {
        Ok(Box::new(Self::try_new(params)?))
    }

    fn supported_codecs() -> &'static [SupportedAudioCodec] {
        &[SupportedAudioCodec {
            id: CODEC_ID_OPUS,
            info: INFO,
        }]
    }
}
