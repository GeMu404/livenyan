//! Audio vía SDL3: un AudioStream por canal (nombre de objeto Sound), mezclado por SDL.

use crate::assets::{Asset, Assets, Pcm};
use sdl3::AudioSubsystem;
use sdl3::audio::{AudioDevice, AudioFormat, AudioSpec, AudioStreamOwner};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Chan {
    stream: AudioStreamOwner,
    pcm: Arc<Pcm>,
    repeat: bool,
    repeat_cursor: usize,
    base_gain: f32,
    gain: f32,
    fade_out: Option<FadeOut>,
}

#[derive(Clone, Copy)]
struct FadeOut {
    started_at: Instant,
    duration: Duration,
    start_gain: f32,
}

struct PendingPlay {
    channel: String,
    file: String,
    volume: f32,
    repeat: bool,
}

pub struct Audio {
    sub: AudioSubsystem,
    device: AudioDevice,
    chans: HashMap<String, Chan>,
    pending: Vec<PendingPlay>,
    master: f32,
    last_gain_update: Instant,
}

const GAIN_SMOOTHING: Duration = Duration::from_millis(25);

fn faded_gain(start_gain: f32, elapsed: Duration, duration: Duration) -> Option<f32> {
    if duration.is_zero() || elapsed >= duration {
        return None;
    }
    let progress = elapsed.as_secs_f64() / duration.as_secs_f64();
    Some(start_gain * (1.0 - progress as f32))
}

fn bounded_gain(gain: f32) -> f32 {
    if gain.is_finite() {
        gain.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn mixed_gain(base_gain: f32, active_channels: usize) -> f32 {
    bounded_gain(base_gain / active_channels.max(1) as f32)
}

fn smooth_gain(current: f32, target: f32, elapsed: Duration) -> f32 {
    if elapsed.is_zero() {
        return current;
    }
    let response = 1.0 - (-(elapsed.as_secs_f64() / GAIN_SMOOTHING.as_secs_f64())).exp();
    current + (target - current) * response as f32
}

fn repeat_needs_refill(queued_bytes: usize, pcm: &Pcm) -> bool {
    let low_watermark_bytes = (pcm.rate as usize)
        .saturating_mul(usize::from(pcm.channels))
        .saturating_mul(std::mem::size_of::<i16>())
        / 2;
    queued_bytes < low_watermark_bytes
}

fn next_loop_chunk(samples: &[i16], cursor: &mut usize, sample_count: usize) -> Vec<i16> {
    if samples.is_empty() || sample_count == 0 {
        return Vec::new();
    }

    let mut chunk = Vec::with_capacity(sample_count);
    let mut offset = *cursor % samples.len();
    while chunk.len() < sample_count {
        let count = (sample_count - chunk.len()).min(samples.len() - offset);
        chunk.extend_from_slice(&samples[offset..offset + count]);
        offset = 0;
    }
    *cursor = (*cursor % samples.len() + sample_count) % samples.len();
    chunk
}

impl Audio {
    pub fn new(sub: AudioSubsystem, master: f32) -> Result<Self, String> {
        let device = sub
            .open_playback_device(&AudioSpec {
                freq: None,
                channels: None,
                format: None,
            })
            .map_err(|e| e.to_string())?;
        if !master.is_finite() || !(0.0..=1.0).contains(&master) {
            log::warn!("audio master volume {master} is outside 0..=1; clamping it");
        }
        log::info!("audio master gain: {:.3}", bounded_gain(master));
        match device.name() {
            Ok(name) => log::info!("audio playback device: {name}"),
            Err(error) => log::warn!("cannot query audio playback device name: {error}"),
        }
        Ok(Self {
            sub,
            device,
            chans: HashMap::new(),
            pending: vec![],
            master: bounded_gain(master),
            last_gain_update: Instant::now(),
        })
    }

    pub fn play(&mut self, channel: String, file: String, volume: f32, repeat: bool) {
        self.pending.retain(|p| p.channel != channel);
        self.chans.remove(&channel);
        if !volume.is_finite() || !(0.0..=1.0).contains(&volume) {
            log::warn!("audio channel {channel:?} volume {volume} is outside 0..=1; clamping it");
        }
        self.pending.push(PendingPlay {
            channel,
            file,
            volume: bounded_gain(volume),
            repeat,
        });
    }

    pub fn stop(&mut self, channel: &str) {
        self.pending.retain(|p| p.channel != channel);
        self.chans.remove(channel);
    }

    pub fn fade_out(&mut self, channel: &str, duration_ms: u64) {
        self.pending.retain(|p| p.channel != channel);
        if duration_ms == 0 {
            self.chans.remove(channel);
            return;
        }
        if let Some(chan) = self.chans.get_mut(channel) {
            chan.fade_out = Some(FadeOut {
                started_at: Instant::now(),
                duration: Duration::from_millis(duration_ms),
                start_gain: chan.base_gain,
            });
        }
    }

    /// Arranca reproducciones decodificadas y mantiene al menos un clip en los bucles.
    pub fn update(&mut self, assets: &Assets) {
        let mut still = vec![];
        for p in std::mem::take(&mut self.pending) {
            match assets.try_get(&p.file) {
                Some(Asset::Sound(pcm)) => {
                    if let Err(e) = self.start(&p, pcm) {
                        log::warn!("audio {}: {e}", p.file);
                    }
                }
                Some(Asset::Failed(error)) => {
                    log::warn!("cannot decode audio {}: {error}", p.file);
                }
                Some(_) => {
                    log::warn!("audio resource {} resolved to a non-audio asset", p.file);
                }
                None => still.push(p),
            }
        }
        self.pending = still;
        self.chans.retain(|channel, c| {
            if let Some(fade_out) = c.fade_out
                && faded_gain(
                    fade_out.start_gain,
                    fade_out.started_at.elapsed(),
                    fade_out.duration,
                )
                .is_none()
            {
                return false;
            }
            let queued_bytes = match c.stream.queued_bytes() {
                Ok(queued_bytes) if queued_bytes >= 0 => queued_bytes as usize,
                Ok(queued_bytes) => {
                    log::warn!(
                        "audio channel {channel:?} reported invalid queue size {queued_bytes}"
                    );
                    return false;
                }
                Err(error) => {
                    log::warn!("cannot query audio channel {channel:?} queue: {error}");
                    return false;
                }
            };
            if c.repeat && repeat_needs_refill(queued_bytes, &c.pcm) {
                let channels = usize::from(c.pcm.channels);
                let chunk_samples = (c.pcm.rate as usize).saturating_mul(channels);
                let chunk = next_loop_chunk(&c.pcm.samples, &mut c.repeat_cursor, chunk_samples);
                if let Err(error) = c.stream.put_data_i16(&chunk) {
                    log::warn!("cannot refill audio channel {channel:?}: {error}");
                    return false;
                }
            }
            c.repeat || queued_bytes > 0
        });
        let active_channels = self.chans.len().max(1);
        let elapsed = self.last_gain_update.elapsed();
        self.last_gain_update = Instant::now();
        self.chans.retain(|channel, c| {
            let base_gain = c.fade_out.map_or(c.base_gain, |fade_out| {
                faded_gain(
                    fade_out.start_gain,
                    fade_out.started_at.elapsed(),
                    fade_out.duration,
                )
                .unwrap_or(0.0)
            });
            let target_gain = mixed_gain(base_gain, active_channels);
            let gain = smooth_gain(c.gain, target_gain, elapsed);
            if (gain - c.gain).abs() > 0.001 {
                if let Err(error) = c.stream.set_gain(gain) {
                    log::warn!("cannot update mixed gain for audio channel {channel:?}: {error}");
                    return false;
                }
                c.gain = gain;
            }
            true
        });
    }

    fn start(&mut self, p: &PendingPlay, pcm: Arc<Pcm>) -> Result<(), String> {
        if pcm.rate == 0
            || pcm.channels == 0
            || pcm.samples.is_empty()
            || !pcm.samples.len().is_multiple_of(usize::from(pcm.channels))
        {
            return Err(format!(
                "invalid PCM format: {} Hz, {} channel(s), {} samples",
                pcm.rate,
                pcm.channels,
                pcm.samples.len()
            ));
        }
        let spec = AudioSpec {
            freq: Some(pcm.rate as i32),
            channels: Some(pcm.channels as i32),
            format: Some(AudioFormat::S16LE),
        };
        let stream = self
            .sub
            .new_stream(Some(&spec), None)
            .map_err(|e| e.to_string())?;
        // Enlazar primero: el formato de destino lo fija el dispositivo
        self.device
            .bind_stream(&stream)
            .map_err(|e| e.to_string())?;
        let base_gain = bounded_gain(p.volume * self.master);
        let gain = if self.chans.is_empty() {
            base_gain
        } else {
            0.0
        };
        stream.set_gain(gain).map_err(|e| e.to_string())?;
        stream
            .put_data_i16(&pcm.samples)
            .map_err(|e| e.to_string())?;
        stream.resume().map_err(|e| e.to_string())?;
        log::debug!(
            "started audio channel {:?} from {:?}: {} Hz, {} channel(s), {} samples, channel gain {:.3}, master gain {:.3}, effective gain {:.3}, repeat={}",
            p.channel,
            p.file,
            pcm.rate,
            pcm.channels,
            pcm.samples.len(),
            p.volume,
            self.master,
            gain,
            p.repeat
        );
        self.chans.insert(
            p.channel.clone(),
            Chan {
                stream,
                pcm,
                repeat: p.repeat,
                repeat_cursor: 0,
                base_gain,
                gain,
                fade_out: None,
            },
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        bounded_gain, faded_gain, mixed_gain, next_loop_chunk, repeat_needs_refill, smooth_gain,
    };
    use crate::assets::Pcm;
    use std::time::Duration;

    #[test]
    fn gain_is_finite_and_clamped_to_sdl_range() {
        assert_eq!(bounded_gain(0.5), 0.5);
        assert_eq!(bounded_gain(-1.0), 0.0);
        assert_eq!(bounded_gain(2.0), 1.0);
        assert_eq!(bounded_gain(f32::NAN), 0.0);
        assert_eq!(bounded_gain(f32::INFINITY), 0.0);
    }

    #[test]
    fn mixed_gain_keeps_simultaneous_full_scale_channels_below_clipping() {
        let gains: Vec<f32> = (0..5).map(|_| mixed_gain(1.0, 5)).collect();
        assert!(gains.iter().all(|gain| *gain == 0.2));
        assert!(gains.iter().sum::<f32>() <= 1.0);
        assert_eq!(mixed_gain(0.5, 2), 0.25);
        assert_eq!(mixed_gain(1.0, 0), 1.0);
    }

    #[test]
    fn gain_changes_are_smoothed_without_overshooting() {
        let current = 0.5;
        let target = 0.25;
        let next = smooth_gain(current, target, Duration::from_millis(10));
        assert!(next < current && next > target);
        assert_eq!(smooth_gain(current, target, Duration::ZERO), current);
        assert_eq!(smooth_gain(target, target, Duration::from_secs(1)), target);
    }

    #[test]
    fn repeat_refill_uses_a_half_second_low_watermark() {
        let pcm = Pcm {
            rate: 100,
            channels: 2,
            samples: vec![0; 200],
        };
        assert!(repeat_needs_refill(199, &pcm));
        assert!(!repeat_needs_refill(200, &pcm));
        assert!(!repeat_needs_refill(usize::MAX, &pcm));
    }

    #[test]
    fn repeated_audio_chunks_wrap_at_the_sample_boundary() {
        let samples = [0, 1, 2, 3];
        let mut cursor = 0;
        assert_eq!(next_loop_chunk(&samples, &mut cursor, 3), [0, 1, 2]);
        assert_eq!(cursor, 3);
        assert_eq!(next_loop_chunk(&samples, &mut cursor, 3), [3, 0, 1]);
        assert_eq!(cursor, 2);
    }

    #[test]
    fn fade_reaches_zero_then_ends_channel() {
        let duration = Duration::from_millis(100);
        assert_eq!(
            faded_gain(0.8, Duration::from_millis(50), duration),
            Some(0.4)
        );
        assert_eq!(faded_gain(0.8, duration, duration), None);
    }
}
