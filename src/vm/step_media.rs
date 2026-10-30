use super::{Clock, Event, VIDEO_EXTS, VideoPlayback, Vm, volume_gain};
use crate::assets::SOUND_EXTS;
use crate::lsb::Command;
use crate::lsb::expr::Value;
use crate::lsb::names::Prop;
use crate::scene::Kind;
use std::collections::VecDeque;

impl Vm {
    /// Sound: reproduce o detiene un canal y crea el objeto de sonido en escena.
    pub(crate) fn step_sound(&mut self, c: &Command) -> String {
        let props = self.props_of(c);
        let ch = props
            .get(&Prop::NAME)
            .map(|v| v.as_str())
            .unwrap_or_default();
        let src = props
            .get(&Prop::SOURCE)
            .map(|v| v.as_str())
            .unwrap_or_default();
        let vol = props
            .get(&Prop::VOLUME)
            .map(|v| v.as_f64())
            .unwrap_or(1000.0);
        let repeat = props
            .get(&Prop::REPEAT)
            .map(|v| v.truthy())
            .unwrap_or(false);
        let detail = format!("ch={ch:?} src={src:?} vol={vol} repeat={repeat}");
        match self.assets.data().resolve(&src, SOUND_EXTS) {
            Some(file) => {
                self.assets.request(&file);
                self.events.push(Event::PlaySound {
                    channel: ch.clone(),
                    file,
                    volume: volume_gain(vol),
                    repeat,
                })
            }
            None if src.is_empty() => self.events.push(Event::StopSound {
                channel: ch.clone(),
            }),
            None => self.warn(&format!("snd:{src}"), &format!("sound not found: {src}")),
        }
        self.scene.create(&ch, Kind::Other(c.kind), props);
        detail
    }

    /// MediaPlay: arranca el audio del objeto indicado por Target.
    pub(crate) fn step_media_play(&mut self, c: &Command) -> String {
        let t = self.eval(c.expr("Target")).as_str();
        let detail = format!("target={t:?}");
        if let Some(o) = self.scene.get(&t) {
            let src = o.str(Prop::SOURCE);
            if let Some(file) = self.assets.data().resolve(&src, SOUND_EXTS) {
                let vol = volume_gain(o.int(Prop::VOLUME, 1000) as f64);
                let repeat = o.get(Prop::REPEAT).is_some_and(|v| v.truthy());
                self.events.push(Event::PlaySound {
                    channel: t.clone(),
                    file,
                    volume: vol,
                    repeat,
                });
            }
        }
        detail
    }

    /// Movie: crea el objeto de vídeo y arranca el decoder si el asset existe.
    /// Tamaño = fuente × zoom (`docs/livenovel/media.html`; no hay PR_WIDTH/HEIGHT).
    pub(crate) fn step_movie(&mut self, c: &Command) -> String {
        let mut props = self.props_of(c);
        let name = props
            .get(&Prop::NAME)
            .map(|v| v.as_str())
            .unwrap_or_default();
        let src = props
            .get(&Prop::SOURCE)
            .map(|v| v.as_str())
            .unwrap_or_default();
        let detail = format!("name={name:?} src={src:?}");
        // Manual: Movie no documenta WIDTH/HEIGHT. Decode a tamaño nativo
        // (**interpretación livenyan** I1: menos latencia que scale en ffmpeg;
        // el present/SDL escala). Ver COMPATIBILITY.md.
        props.remove(&Prop::WIDTH);
        props.remove(&Prop::HEIGHT);
        props.insert(Prop::PLAYING, Value::Bool(false));

        self.clear_clock(&name);
        if let Some(object) = self.scene.get_mut(&name) {
            object.anim = None;
        }
        self.video_generation = self.video_generation.wrapping_add(1);
        let generation = self.video_generation;
        let decoder = self
            .assets
            .data()
            .resolve(&src, VIDEO_EXTS)
            .ok_or_else(|| format!("video asset not found: {src:?}"))
            .and_then(|file| {
                let started = std::time::Instant::now();
                let data = self
                    .assets
                    .data()
                    .read(&file)
                    .map_err(|e| format!("{file}: {e}"))?;
                let repeat = props.get(&Prop::REPEAT).is_some_and(Value::truthy);
                // Un solo temp: probe nativo + decode sin scale.
                let (decoder, native_w, native_h) =
                    crate::video::Decoder::start(&data, crate::video::OutputSize::Source, repeat)?;
                log::info!(
                    "load video {file}: {native_w}x{native_h} in {:.1} ms",
                    started.elapsed().as_secs_f64() * 1000.0
                );
                // Igual que Image: WIDTH/ORIG = fuente; zoom en `placed_rect`.
                props.insert(Prop::ORIGWIDTH, Value::Int(native_w as i64));
                props.insert(Prop::ORIGHEIGHT, Value::Int(native_h as i64));
                props.insert(Prop::WIDTH, Value::Int(native_w as i64));
                props.insert(Prop::HEIGHT, Value::Int(native_h as i64));
                Ok((decoder, native_w, native_h))
            });
        let playback = match decoder {
            Ok((decoder, width, height)) => {
                props.insert(Prop::PLAYING, Value::Bool(true));
                Some(VideoPlayback {
                    decoder: Some(decoder),
                    frames: VecDeque::with_capacity(2),
                    frame_seq: 0,
                    generation,
                    width,
                    height,
                    started_at_ms: self.now,
                    first_pts_seconds: None,
                    paused_at_ms: None,
                })
            }
            Err(e) => {
                log::error!("Movie {name:?}: cannot play {src:?}: {e}");
                None
            }
        };
        self.scene.create(&name, Kind::Movie, props);
        if let Some(playback) = playback {
            self.clocks.insert(name.clone(), Clock::Video(playback));
        }
        self.object_defined(&name);
        detail
    }

    /// MovieStop: para los vídeos y cinemas cuyo nombre coincide con Target.
    pub(crate) fn step_movie_stop(&mut self, c: &Command) {
        let target = self.eval(c.expr("Target")).as_str();
        let matches_target = |name: &str| target.is_empty() || name == target;
        let videos: Vec<String> = self
            .video_names()
            .into_iter()
            .filter(|name| matches_target(name))
            .collect();
        for name in videos {
            self.stop_video(&name);
        }
        let names = self.cinema_names();
        for name in names {
            if !matches_target(&name) {
                continue;
            }
            let now = self.now;
            if let Some(movie) = self.cinema_mut(&name) {
                movie.repeat = false;
                movie.started_at = now - movie.duration_ms;
            }
            self.set_cinema_deadline(&name, self.now);
        }
        for name in self.one_shot_cinemas() {
            if matches_target(&name) {
                self.set_cinema_deadline(&name, self.now);
            }
        }
    }
}
