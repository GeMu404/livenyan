//! Audio vía SDL3: un AudioStream por canal (nombre de objeto Sound), mezclado por SDL.

use crate::assets::{Asset, Assets, Pcm};
use sdl3::audio::{AudioDevice, AudioFormat, AudioSpec, AudioStreamOwner};
use sdl3::AudioSubsystem;
use std::collections::HashMap;
use std::sync::Arc;

struct Chan {
    stream: AudioStreamOwner,
    pcm: Arc<Pcm>,
    repeat: bool,
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
}

impl Audio {
    pub fn new(sub: AudioSubsystem, master: f32) -> Result<Self, String> {
        let device = sub
            .open_playback_device(&AudioSpec { freq: None, channels: None, format: None })
            .map_err(|e| e.to_string())?;
        Ok(Self { sub, device, chans: HashMap::new(), pending: vec![], master })
    }

    pub fn play(&mut self, channel: String, file: String, volume: f32, repeat: bool) {
        self.pending.retain(|p| p.channel != channel);
        self.chans.remove(&channel);
        self.pending.push(PendingPlay { channel, file, volume, repeat });
    }

    pub fn stop(&mut self, channel: &str) {
        self.pending.retain(|p| p.channel != channel);
        self.chans.remove(channel);
    }

    /// Arranca reproducciones cuyo audio ya está decodificado y rellena bucles.
    pub fn update(&mut self, assets: &Assets) {
        let mut still = vec![];
        for p in std::mem::take(&mut self.pending) {
            match assets.try_get(&p.file) {
                Some(Asset::Sound(pcm)) => {
                    if let Err(e) = self.start(&p, pcm) {
                        log::warn!("audio {}: {e}", p.file);
                    }
                }
                Some(_) => {}
                None => still.push(p),
            }
        }
        self.pending = still;
        self.chans.retain(|_, c| {
            let q = c.stream.queued_bytes().unwrap_or(0);
            if c.repeat && (q as usize) < c.pcm.samples.len() {
                let _ = c.stream.put_data_i16(&c.pcm.samples);
            }
            c.repeat || q > 0
        });
    }

    fn start(&mut self, p: &PendingPlay, pcm: Arc<Pcm>) -> Result<(), String> {
        let spec = AudioSpec { freq: Some(pcm.rate as i32), channels: Some(pcm.channels as i32), format: Some(AudioFormat::S16LE) };
        let stream = self.sub.new_stream(Some(&spec), None).map_err(|e| e.to_string())?;
        // enlazar primero: el formato de destino lo fija el dispositivo
        self.device.bind_stream(&stream).map_err(|e| e.to_string())?;
        stream.set_gain(p.volume * self.master).map_err(|e| e.to_string())?;
        stream.put_data_i16(&pcm.samples).map_err(|e| e.to_string())?;
        self.chans.insert(p.channel.clone(), Chan { stream, pcm, repeat: p.repeat });
        Ok(())
    }
}
