use super::{
    CinemaPlayback, Pending, State, TimerState, VIDEO_EXTS, VideoFrame, VideoFramePair,
    VideoPlayback, Vm, video_pair_progress,
};
use crate::assets::{Asset, IMAGE_EXTS};
use crate::lsb::expr::Value;
use crate::lsb::names::Prop;
use crate::scene::{Kind, TextItem};
use std::collections::{HashMap, VecDeque};

/// GetProp(..., TEXT) lo usan 終了.lsb / PAUSE; debe reflejar el buffer de reproducción.
/// CARETX/CARETY/CARETHEIGHT los lee el ImgNew del メッセージカーソル en ONNOTIFY(PAUSE).
fn parent_chain_visible(scene: &crate::scene::Scene, parent: &str) -> bool {
    let mut parent = parent.to_string();
    let mut guard = 0;
    while !parent.is_empty() && guard < 32 {
        let next = match scene.get(&parent) {
            Some(object) if !object.visible() => return false,
            Some(object) => object.str(Prop::PARENT),
            None => break,
        };
        parent = next;
        guard += 1;
    }
    true
}

fn message_text_string(items: &[TextItem]) -> String {
    let mut s = String::new();
    for item in items {
        match item {
            TextItem::Char(ch) => s.push_str(ch),
            TextItem::Newline => s.push('\n'),
            TextItem::Image { .. } => {}
        }
    }
    s
}

fn estimate_message_pen(text: &str, draw_h: f64, line_step: f64, max_w: f64) -> (f64, f64) {
    let mut line_w = 0.0f64;
    let mut caret_y = 0.0f64;
    for ch in text.chars() {
        if ch == '\n' {
            caret_y += line_step;
            line_w = 0.0;
            continue;
        }
        let cw = draw_h * super::caption_char_width(ch);
        if max_w > 0.0 && line_w + cw > max_w && line_w > 0.0 {
            caret_y += line_step;
            line_w = 0.0;
        }
        line_w += cw;
    }
    (line_w, caret_y)
}

impl Vm {
    /// La flecha del mensaje usa CARETX/CARETY. Tiene que coincidir con la pluma
    /// del raster, incluido el corte de línea; si no, queda fuera del cuadro.
    fn sync_message_text_prop(&mut self, name: &str) {
        let Some(object) = self.scene.get(name) else {
            return;
        };
        let items = object.text.clone();
        let text = message_text_string(&items);
        let family = object.str(Prop::FONTNAME);
        let font_h = object.int(Prop::FONTHEIGHT, 22).max(1) as f64;
        let linespace = object.int(Prop::LINESPACE, 0) as f64;
        let obj_w = object.int(Prop::WIDTH, 0);
        let (abs_x, _) = self.scene.abs_pos(object);
        let max_w = if obj_w > 0 {
            obj_w as f32
        } else {
            (self.opts.screen.0 as f64 - abs_x).max(1.0) as f32
        };
        let mut images = HashMap::new();
        for item in &items {
            if let TextItem::Image { source, .. } = item
                && let Some(Asset::Image(image)) = self.assets.try_get(source)
            {
                images.insert(source.clone(), image);
            }
        }
        let pen = self
            .message_font(&family)
            .and_then(|renderer| {
                let px = renderer.size_for_cell_height(font_h as f32);
                Some(renderer.pen_after(&items, max_w, px, &images))
            })
            .map(|(x, y)| (f64::from(x), f64::from(y)))
            .unwrap_or_else(|| {
                let draw_h =
                    font_h * f64::from(crate::text::outline_size_scale_for_fallback(&family));
                estimate_message_pen(&text, draw_h, font_h + linespace, f64::from(max_w))
            });
        let Some(object) = self.scene.get_mut(name) else {
            return;
        };
        object.props.insert(Prop::TEXT, Value::Str(text));
        object
            .props
            .insert(Prop::CARETX, Value::Int(pen.0.round() as i64));
        object
            .props
            .insert(Prop::CARETY, Value::Int(pen.1.round() as i64));
        object
            .props
            .insert(Prop::CARETHEIGHT, Value::Int(font_h.round() as i64));
    }

    fn message_font(&mut self, family: &str) -> Option<&mut crate::text::TextRenderer> {
        if !self.menu_fonts.contains_key(family) {
            let loaded = crate::text::TextRenderer::load_family(
                family,
                &self.pixel_fonts,
                self.menu_font_override.clone(),
            );
            self.menu_fonts.insert(family.to_string(), loaded);
        }
        self.menu_fonts.get_mut(family)?.as_mut()
    }

    pub fn video_frame(&self, name: &str) -> Option<(u64, u64, &[u8], u32, u32)> {
        let playback = self.videos.get(name)?;
        let frame = playback.frames.back()?;
        Some((
            playback.generation,
            playback.frame_seq,
            frame.pixels.as_ref(),
            playback.width,
            playback.height,
        ))
    }

    pub fn video_frame_pair(&self, name: &str, now_ms: f64) -> Option<VideoFramePair<'_>> {
        let playback = self.videos.get(name)?;
        let first = playback.frames.front()?;
        let playback_now_ms = playback.paused_at_ms.unwrap_or(now_ms);
        let playhead = playback
            .first_pts_seconds
            .map_or(first.pts_seconds, |first_pts| {
                first_pts + ((playback_now_ms - playback.started_at_ms).max(0.0) / 1000.0)
            });
        let (previous, current) = playback
            .frames
            .iter()
            .zip(playback.frames.iter().skip(1))
            .find(|(_, next)| playhead < next.pts_seconds)
            .map_or_else(
                || {
                    let current = playback.frames.back().expect("front was present");
                    let previous = playback.frames.iter().rev().nth(1);
                    (previous, current)
                },
                |(previous, current)| (Some(previous), current),
            );
        let previous = previous.map(|frame| {
            (
                frame.pixels.as_ref(),
                frame.pts_seconds,
                frame.duplicate,
                frame.sequence,
            )
        });
        let progress = previous.map_or(1.0, |(_, previous_pts, _, _)| {
            playback.first_pts_seconds.map_or(1.0, |first_pts| {
                video_pair_progress(
                    playback_now_ms,
                    playback.started_at_ms,
                    first_pts,
                    previous_pts,
                    current.pts_seconds,
                )
            })
        });
        Some(VideoFramePair {
            generation: playback.generation,
            sequence: current.sequence,
            previous,
            current: current.pixels.as_ref(),
            current_pts_seconds: current.pts_seconds,
            current_duplicate: current.duplicate,
            progress,
            width: playback.width,
            height: playback.height,
        })
    }
    pub(crate) fn start_play(&mut self, target: &str, plan: Vec<Pending>) {
        let p = self.plays.entry(target.to_string()).or_default();
        p.plan.extend(plan);
        if p.next < self.now {
            p.next = self.now;
        }
        if let Some(o) = self.scene.get_mut(target) {
            o.props.insert(Prop::PLAYING, Value::Bool(true));
            o.props.insert(Prop::TEXTPAUSED, Value::Bool(false));
        }
    }

    pub(crate) fn restart_video(&mut self, name: &str) -> Result<(), String> {
        let obj = self
            .scene
            .get(name)
            .filter(|obj| obj.kind == Kind::Movie)
            .cloned()
            .ok_or_else(|| format!("movie object {name:?} does not exist"))?;
        let source = obj.str(Prop::SOURCE);
        let file = self
            .assets
            .data()
            .resolve(&source, VIDEO_EXTS)
            .ok_or_else(|| format!("video asset not found: {source:?}"))?;
        let data = self
            .assets
            .data()
            .read(&file)
            .map_err(|error| format!("{file}: {error}"))?;
        let (native_w, native_h) = crate::video::probe_size(&data)?;
        let decoder = crate::video::Decoder::start(
            &data,
            native_w,
            native_h,
            obj.get(Prop::REPEAT).is_some_and(Value::truthy),
        )?;
        self.video_generation = self.video_generation.wrapping_add(1);
        self.videos.insert(
            name.to_string(),
            VideoPlayback {
                decoder: Some(decoder),
                frames: VecDeque::with_capacity(2),
                frame_seq: 0,
                generation: self.video_generation,
                width: native_w,
                height: native_h,
                started_at_ms: self.now,
                first_pts_seconds: None,
                paused_at_ms: None,
            },
        );
        if let Some(obj) = self.scene.get_mut(name) {
            obj.props
                .insert(Prop::ORIGWIDTH, Value::Int(native_w as i64));
            obj.props
                .insert(Prop::ORIGHEIGHT, Value::Int(native_h as i64));
            obj.props.insert(Prop::WIDTH, Value::Int(native_w as i64));
            obj.props
                .insert(Prop::HEIGHT, Value::Int(native_h as i64));
            obj.props.insert(Prop::PLAYING, Value::Bool(true));
        }
        Ok(())
    }

    pub(crate) fn stop_video(&mut self, name: &str) {
        if let Some(playback) = self.videos.get_mut(name) {
            playback.decoder = None;
        }
        if let Some(obj) = self.scene.get_mut(name) {
            obj.props.insert(Prop::PLAYING, Value::Bool(false));
        }
        self.fire(name, Prop::ONPLAYEND, vec![]);
    }
    /// Animación, fundido o vídeo de una sola pasada todavía en curso.
    /// Misma condición que la espera de `CREATECG WAIT` / `WAITPLAY`.
    pub(super) fn effect_busy(&self, name: &str) -> bool {
        let Some(object) = self.scene.get(name) else {
            return false;
        };
        let repeat = object.get(Prop::REPEAT).is_some_and(|value| value.truthy());
        (!repeat && self.anims.contains_key(name))
            || self.scene.motions.iter().any(|motion| motion.obj == name)
            || self
                .cinema_movies
                .get(name)
                .is_some_and(|movie| self.now < movie.started_at + movie.duration_ms)
            || (!repeat
                && self
                    .videos
                    .get(name)
                    .is_some_and(|video| video.decoder.is_some()))
    }

    /// Avanza la reproducción de texto de todos los objetos de mensaje.
    pub(crate) fn update_plays(&mut self) {
        let cps_of = |vm: &Self, name: &str| -> f64 {
            if vm.opts.text_cps <= 0.0 || vm.skip {
                return f64::INFINITY;
            }
            // docs/livenovel/text.html PR_TEXTDELAY: 256 equis, 0 y -1 sin espera.
            match vm
                .scene
                .get(name)
                .and_then(|object| object.get(Prop::TEXTDELAY))
            {
                Some(delay) if delay.as_int() <= 0 => f64::INFINITY,
                Some(delay) => vm.opts.text_cps as f64 * (256.0 / delay.as_int() as f64),
                None => vm.opts.text_cps as f64,
            }
        };
        let names: Vec<String> = self.plays.keys().cloned().collect();
        for n in names {
            if self.placeholders.contains(&n) {
                continue; // aún no existe el objeto real
            }
            let mut finished = false;
            let mut notifies = vec![];
            // efecto WAIT pendiente: animación o fundido del objeto aún en curso
            let waiting_obj = self.plays.get(&n).and_then(|p| p.wait_for.clone());
            if let Some((o, deadline)) = waiting_obj {
                let busy = self.effect_busy(&o);
                if busy && self.now < deadline && !self.skip {
                    self.play_debug(&n, &format!("waiting-for-effect on {o:?}"));
                    continue;
                }
                log::debug!(
                    "text on {n}: WAIT on {o:?} done{}",
                    if busy { " (deadline/skip)" } else { "" }
                );
                if let Some(p) = self.plays.get_mut(&n) {
                    p.wait_for = None;
                    p.click_skips_wait = false;
                }
            }
            if let Some(p) = self.plays.get(&n) {
                let st = if p.waiting_click {
                    "waiting-for-click".to_string()
                } else if p.next > self.now + 1.0 {
                    format!("timed wait {:.0} ms left", p.next - self.now)
                } else if p.pos < p.cur.len() {
                    format!(
                        "playing ({}/{} items of chunk, {} queued)",
                        p.pos,
                        p.cur.len(),
                        p.plan.len()
                    )
                } else {
                    format!(
                        "between chunks ({} queued, last tag {:?})",
                        p.plan.len(),
                        p.last_tag
                    )
                };
                self.play_debug(&n, &st);
            }
            loop {
                let (shown, instant, parent) = match self.scene.get(&n) {
                    Some(object) => (
                        object.visible(),
                        object
                            .get(Prop::TEXTDELAY)
                            .is_some_and(|delay| delay.as_int() <= 0),
                        object.str(Prop::PARENT),
                    ),
                    None => (false, false, String::new()),
                };
                // Caja oculta (ella o un padre, p.ej. 土台): no saltar el clic.
                // MESOFF + WAITPLAY/CLICK antes de MESEND avanzaba la escena sola.
                let shown = shown && parent_chain_visible(&self.scene, &parent);
                let cps = cps_of(self, &n);
                let Some(p) = self.plays.get_mut(&n) else {
                    break;
                };
                if p.next > self.now + 1.0 && p.pos >= p.cur.len() {
                    break; // espera temporizada (WAIT de efecto con tiempo)
                }
                if p.waiting_click {
                    if shown && (self.skip || instant) {
                        p.waiting_click = false;
                    } else {
                        break;
                    }
                }
                if p.pos < p.cur.len() {
                    let start = p.pos;
                    if cps.is_infinite() {
                        p.pos = p.cur.len();
                    } else {
                        while p.pos < p.cur.len() && p.next <= self.now {
                            p.pos += 1;
                            p.next += 1000.0 / cps;
                        }
                    }
                    let add: Vec<TextItem> = p.cur[start..p.pos].to_vec();
                    let more = p.pos < p.cur.len();
                    if !add.is_empty() {
                        if let Some(o) = self.scene.get_mut(&n) {
                            o.text.extend(add);
                            o.shown = o.text.len();
                            o.text_rev += 1;
                        }
                        self.sync_message_text_prop(&n);
                        // Por lote de glifos: SE de carácter + mostrar 土台 (再生中.lsb).
                        self.fire(&n, Prop::ONPLAYING, vec![]);
                    }
                    if more {
                        break;
                    }
                    continue;
                }
                let mut started_read = false;
                match p.plan.pop_front() {
                    Some(Pending::Text {
                        items,
                        already_read,
                    }) => {
                        p.cur = items;
                        p.pos = 0;
                        p.already_read = already_read;
                        if p.next < self.now {
                            p.next = self.now;
                        }
                        started_read = true;
                    }
                    Some(Pending::Click) => {
                        p.last_tag = "click-wait".into();
                        p.waiting_click = true
                    }
                    Some(Pending::Notify(e)) => {
                        p.last_tag = e
                            .trim_start_matches('\x01')
                            .split("\r\n")
                            .next()
                            .unwrap_or("")
                            .to_string();
                        notifies.push(e);
                        // un evento por vez: puede fijar una espera (WAIT) antes de seguir
                        break;
                    }
                    Some(Pending::Clear) => {
                        if let Some(o) = self.scene.get_mut(&n) {
                            o.text.clear();
                            o.shown = 0;
                            o.text_rev += 1;
                        }
                        self.sync_message_text_prop(&n);
                    }
                    None => {
                        finished = true;
                        break;
                    }
                }
                if started_read {
                    // docs/livenovel/text.html: 再生開始, al empezar a mostrar texto.
                    // 既読なら @ParamStr[0] = 1. 再生開始.lsb usa eso para TEXTDELAY.
                    let read = self.plays.get(&n).is_some_and(|play| play.already_read);
                    self.fire(&n, Prop::ONPLAYSTART, vec![Value::Int(i64::from(read))]);
                }
            }
            for raw in notifies {
                // <EVENT VALUE="a\nb"> -> ONNOTIFY de la caja con @ParamStr = [a, b] (manual de
                // LiveMaker: "メッセージ中の<EVENT>タグで呼び出される（引数あり）").
                // Los eventos de sistema (\x01: PLAYSND, WAITPLAY, CHANGECG...) son del motor.
                if let Some(sys) = raw.strip_prefix('\x01') {
                    let mut it = sys.split("\r\n");
                    let name = it.next().unwrap_or("").to_string();
                    let args: Vec<String> = it.map(|x| x.to_string()).collect();
                    self.system_text_event(&n, &name, &args);
                    continue;
                }
                let parts: Vec<Value> = raw
                    .split("\r\n")
                    .map(|x| Value::Str(x.to_string()))
                    .collect();
                log::debug!(
                    "text event on {n}: {:?} ({} params) -> ONNOTIFY",
                    parts.first().map(|v| v.as_str()),
                    parts.len()
                );
                if !self.fire(&n, Prop::ONNOTIFY, parts) {
                    self.warn(
                        "notify-nohandler",
                        "text <EVENT> but the message box has no ONNOTIFY handler",
                    );
                }
            }
            if finished {
                self.plays.remove(&n);
                log::debug!("playback end on {n}");
                if let Some(o) = self.scene.get_mut(&n) {
                    o.props.insert(Prop::PLAYING, Value::Bool(false));
                    o.props.insert(Prop::TEXTPAUSED, Value::Bool(false));
                }
                if !self.fire(&n, Prop::ONPLAYEND, vec![]) {
                    self.ended_unnotified.insert(n.clone());
                }
                if matches!(&self.state, State::WaitText { target } if *target == n) {
                    self.state = State::Running;
                }
            }
        }
        if let State::WaitText { target } = &self.state {
            if !self.plays.contains_key(target) {
                self.state = State::Running;
            }
        }
    }

    /// Timer: al vencer PR_DELAY (una vez salvo PR_REPEAT) dispara el callback.
    ///
    /// LiveMaker clásico / fixtures usan `ONPLAYEND`; scripts LiveNovel del juego de
    /// prueba cuelgan el tick en `ONNOTIFY` (p. ej. 男台詞呼び出し → TextIns). Si
    /// ambos existen se prefiere `ONPLAYEND`.
    pub(crate) fn update_timers(&mut self) {
        let names: Vec<String> = self.timers.keys().cloned().collect();
        let mut due = Vec::new();
        for name in names {
            let Some(obj) = self.scene.get(&name) else {
                self.timers.remove(&name);
                continue;
            };
            let paused = obj.get(Prop::PAUSED).is_some_and(|v| v.truthy());
            let Some(timer) = self.timers.get_mut(&name) else {
                continue;
            };
            if paused {
                timer.paused_at.get_or_insert(self.now);
                continue;
            }
            if let Some(paused_at) = timer.paused_at.take() {
                timer.due_at += (self.now - paused_at).max(0.0);
            }
            if timer.due_at <= self.now {
                due.push(name);
            }
        }
        for n in due {
            let Some(o) = self.scene.get(&n) else {
                continue;
            };
            let delay = o
                .get(Prop::DELAY)
                .map(|v| v.as_f64())
                .unwrap_or(0.0)
                .max(1.0);
            let repeat = o.get(Prop::REPEAT).is_some_and(|v| v.truthy());
            if !self.fire(&n, Prop::ONPLAYEND, vec![]) {
                let _ = self.fire(&n, Prop::ONNOTIFY, vec![]);
            }
            if repeat {
                self.timers.insert(
                    n,
                    TimerState {
                        due_at: self.now + delay,
                        paused_at: None,
                    },
                );
            } else {
                self.timers.remove(&n);
            }
        }
    }

    /// Registra un objeto de imagen para animar si su GAL tiene varios frames.
    pub(crate) fn watch_anim(&mut self, obj: &str, src: &str) {
        match self.assets.data().resolve(src, IMAGE_EXTS) {
            Some(n) => {
                self.anims.insert(obj.to_string(), (n, None));
            }
            None => {
                self.anims.remove(obj);
            }
        }
    }

    /// Avanza animaciones GAL: frame por tiempo (duraciones del archivo), bucle con
    /// PR_REPEAT, pausa con PR_PAUSED. Mientras anima PR_PLAYING = true; al terminar
    /// (sin repetición) queda en el último frame, PLAYING = false y se dispara ONPLAYEND.
    pub(crate) fn update_anims(&mut self) {
        let dt = (self.now - self.anim_clock).max(0.0);
        self.anim_clock = self.now;
        let names: Vec<String> = self.anims.keys().cloned().collect();
        for n in names {
            let Some(o) = self.scene.get(&n) else {
                self.anims.remove(&n);
                continue;
            };
            let paused = o.get(Prop::PAUSED).is_some_and(|v| v.truthy());
            let repeat = o.get(Prop::REPEAT).is_some_and(|v| v.truthy());
            let (file, start) = self.anims[&n].clone();
            let delays = match self.assets.try_get(&file) {
                Some(crate::assets::Asset::Image(img)) => img.delays.clone(),
                Some(_) => vec![],
                None => continue, // aún cargando
            };
            if delays.len() < 2 {
                self.anims.remove(&n);
                continue;
            }
            let total: f64 = delays.iter().map(|d| *d as f64).sum::<f64>().max(1.0);
            let start = match start {
                Some(s) => s,
                None => {
                    log::debug!("GAL animation {n}: {} frames, {total} ms", delays.len());
                    self.now
                }
            };
            let mut start = start;
            if paused {
                // congelar: el inicio avanza con el reloj
                start += dt;
            }
            let mut el = self.now - start;
            if self.skip && !repeat {
                el = total;
            }
            let done = !repeat && el >= total;
            let t = if repeat {
                el.rem_euclid(total)
            } else {
                el.min(total - 0.001)
            };
            let mut acc = 0.0;
            let mut idx = delays.len() - 1;
            for (i, d) in delays.iter().enumerate() {
                acc += *d as f64;
                if t < acc {
                    idx = i;
                    break;
                }
            }
            if let Some(o) = self.scene.get_mut(&n) {
                o.frame = idx;
                o.props.insert(Prop::PLAYING, Value::Bool(!done));
            }
            if done {
                self.anims.remove(&n);
                self.fire(&n, Prop::ONPLAYEND, vec![]);
            } else {
                self.anims.insert(n, (file, Some(start)));
            }
        }
    }

    pub(crate) fn update_cinemas(&mut self) {
        let done: Vec<String> = self
            .cinemas
            .iter()
            .filter(|(_, t)| **t <= self.now || self.skip)
            .map(|(n, _)| n.clone())
            .collect();
        for n in done {
            self.cinemas.remove(&n);
            if let Some(o) = self.scene.get_mut(&n) {
                o.props.insert(Prop::PLAYING, Value::Bool(false));
            }
            self.fire(&n, Prop::ONPLAYEND, vec![]);
        }
    }

    pub(crate) fn update_videos(&mut self) {
        let names: Vec<String> = self.videos.keys().cloned().collect();
        for name in names {
            let paused = self
                .scene
                .get(&name)
                .is_some_and(|o| o.get(Prop::PAUSED).is_some_and(Value::truthy));
            let Some(playback) = self.videos.get_mut(&name) else {
                continue;
            };
            if paused {
                playback.paused_at_ms.get_or_insert(self.now);
                continue;
            }
            if let Some(paused_at_ms) = playback.paused_at_ms.take() {
                playback.started_at_ms += (self.now - paused_at_ms).max(0.0);
            }
            let started_at_ms = playback.started_at_ms;
            let mut finished = None;
            if let Some(decoder) = playback.decoder.as_mut() {
                loop {
                    if playback.frames.len() >= 4 {
                        break;
                    }
                    match decoder.try_recv() {
                        Ok(crate::video::Message::Frame(frame)) => {
                            if playback.first_pts_seconds.is_none() {
                                playback.first_pts_seconds = Some(frame.pts_seconds);
                            }
                            playback.frame_seq = playback.frame_seq.saturating_add(1);
                            playback.frames.push_back(VideoFrame {
                                pixels: frame.pixels,
                                pts_seconds: frame.pts_seconds,
                                duplicate: frame.duplicate,
                                sequence: playback.frame_seq,
                            });
                        }
                        Ok(crate::video::Message::Finished(result)) => {
                            finished = Some(result);
                            break;
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            finished = Some(Err("video decoder stopped unexpectedly".into()));
                            break;
                        }
                    }
                }
            }
            if let Some(playback) = self.videos.get_mut(&name)
                && let Some(first_pts) = playback.first_pts_seconds
            {
                let playhead = first_pts + ((self.now - started_at_ms).max(0.0) / 1000.0);
                while playback.frames.len() > 2
                    && playback
                        .frames
                        .get(1)
                        .is_some_and(|frame| frame.pts_seconds <= playhead)
                {
                    playback.frames.pop_front();
                }
            }
            if let Some(result) = finished {
                if let Err(e) = result {
                    log::error!("Movie {name:?}: {e}");
                }
                if let Some(playback) = self.videos.get_mut(&name) {
                    playback.decoder = None;
                }
                if let Some(o) = self.scene.get_mut(&name) {
                    o.props.insert(Prop::PLAYING, Value::Bool(false));
                }
                self.fire(&name, Prop::ONPLAYEND, vec![]);
            }
        }
    }
    /// Estado de reproducción de una caja, a debug, como mucho una vez por segundo y caja.
    fn play_debug(&mut self, name: &str, st: &str) {
        if !log::log_enabled!(log::Level::Debug) {
            return;
        }
        let last = self
            .play_dbg
            .get(name)
            .copied()
            .unwrap_or(f64::NEG_INFINITY);
        if self.now - last >= 1000.0 {
            self.play_dbg.insert(name.to_string(), self.now);
            log::debug!("text playback {name}: {st}");
        }
    }

    pub(crate) fn load_lcm_movie(&self, src: &str) -> Result<Option<crate::lcm::Movie>, String> {
        let Some(name) = self.assets.data().resolve(src, &[".lcm"]).filter(|name| {
            name.rsplit('.')
                .next()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("lcm"))
        }) else {
            return Ok(None);
        };
        let bytes = self
            .assets
            .data()
            .read(&name)
            .map_err(|e| format!("{name}: {e}"))?;
        let mut movie = crate::lcm::parse(&bytes).map_err(|e| format!("{name}: {e}"))?;
        for (clip_index, clip) in movie.clips.iter_mut().enumerate() {
            if let crate::lcm::TrackContent::Text { value, .. } = &mut clip.content
                && let Some(translated) =
                    self.translations.get(&format!("{name}:clip={clip_index}"))
            {
                *value = translated.clone();
            }
            if let crate::lcm::TrackContent::Image(media) = &mut clip.content {
                let resource = self
                    .assets
                    .data()
                    .resolve_relative(&name, media, IMAGE_EXTS)
                    .ok_or_else(|| format!("{name}: LCM image not found: {media:?}"))?;
                self.assets.request(&resource);
                *media = resource;
            }
        }
        Ok(Some(movie))
    }

    pub(crate) fn start_lcm_movie(&mut self, object: &str, movie: crate::lcm::Movie) {
        let duration_ms = movie.duration_frames as f64 * (1000.0 / 60.0);
        let repeat = self
            .scene
            .get(object)
            .is_some_and(|o| o.get(Prop::REPEAT).is_some_and(Value::truthy));
        // REPEAT no entra en el mapa de “una pasada”: no bloquea el clic ni dispara
        // ONPLAYEND al terminar cada vuelta. El playhead hace rollover al muestrear.
        if repeat {
            self.cinemas.remove(object);
        } else {
            self.cinemas
                .insert(object.to_string(), self.now + duration_ms);
        }
        let (mw, mh) = (movie.width, movie.height);
        self.cinema_movies.insert(
            object.to_string(),
            CinemaPlayback {
                movie,
                started_at: self.now,
                duration_ms,
                repeat,
            },
        );
        if let Some(o) = self.scene.get_mut(object) {
            // Cinema no tiene PR_WIDTH/HEIGHT en el manual; el tamaño es el del LCM.
            o.props
                .entry(Prop::ORIGWIDTH)
                .or_insert(Value::Int(mw as i64));
            o.props
                .entry(Prop::ORIGHEIGHT)
                .or_insert(Value::Int(mh as i64));
            o.props.entry(Prop::WIDTH).or_insert(Value::Int(mw as i64));
            o.props.entry(Prop::HEIGHT).or_insert(Value::Int(mh as i64));
            o.props.insert(Prop::PLAYING, Value::Bool(true));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{sample_animation_frames, video_pair_progress};

    #[test]
    fn video_frame_pair_progress_tracks_presentation_timestamps() {
        assert_eq!(video_pair_progress(1_100.0, 1_000.0, 2.0, 2.1, 2.2), 0.0);
        assert!((video_pair_progress(1_150.0, 1_000.0, 2.0, 2.1, 2.2) - 0.5).abs() < 1e-9);
        assert_eq!(video_pair_progress(1_300.0, 1_000.0, 2.0, 2.1, 2.2), 1.0);
        assert_eq!(video_pair_progress(900.0, 1_000.0, 2.0, 2.1, 2.2), 0.0);
        assert_eq!(video_pair_progress(1_150.0, 1_000.0, 2.0, 2.1, 2.1), 1.0);
    }

    #[test]
    fn animated_gal_blend_tracks_elapsed_time_and_looping() {
        let delays = [100, 200, 100];
        assert_eq!(
            sample_animation_frames(&delays, 50.0, 400.0, false),
            Some((0, 1, 0.5))
        );
        assert_eq!(
            sample_animation_frames(&delays, 250.0, 400.0, false),
            Some((1, 2, 0.75))
        );
        assert_eq!(sample_animation_frames(&delays, 350.0, 400.0, false), None);
        assert_eq!(
            sample_animation_frames(&delays, 350.0, 400.0, true),
            Some((2, 0, 0.5))
        );
        assert_eq!(
            sample_animation_frames(&[100, 0, 100], 50.0, 200.0, false),
            Some((0, 2, 0.5))
        );
    }

    fn digit_movie() -> crate::lcm::Movie {
        let clip = |start: u32, value: &str| crate::lcm::Track {
            previous: None,
            start,
            duration: 4,
            order: 0,
            pivot: [0, 0],
            position_start: [16, 0],
            position_end: [16, 0],
            position_curve: [0, 0],
            scale_x: [1.0, 1.0],
            scale_y: [1.0, 1.0],
            scale_curve: [0, 0],
            opacity_start: 0,
            opacity_end: 0,
            content: crate::lcm::TrackContent::Text {
                value: value.to_string(),
                font_height: 24,
                font_style: 0,
                color: 0x00ff_ffff,
            },
        };
        crate::lcm::Movie {
            width: 80,
            height: 24,
            duration_frames: 8,
            clips: vec![clip(0, "10:87"), clip(4, "10:42")],
        }
    }

    #[test]
    fn repeating_cinema_rolls_over_to_the_first_digit() {
        use super::super::{Options, Vm};
        use crate::archive::GameData;
        use crate::assets::Assets;
        use crate::lsb::expr::Value;
        use crate::lsb::names::Prop;
        use crate::scene::Kind;
        use std::collections::HashMap;
        use std::sync::Arc;

        let path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
        let data = GameData::open(&path).unwrap();
        let assets = Arc::new(Assets::new(data, 2, 64));
        let mut vm = Vm::new(
            assets,
            "ゲームメイン.lsb",
            Options {
                pick: None,
                trace_conds: false,
                screen: (800, 600),
                cinema_ms: 0.0,
                text_cps: 0.0,
                lookahead: 50,
                trace: false,
                show_text: false,
            },
        )
        .unwrap();
        let frame = 1000.0 / 60.0;
        let duration = 8.0 * frame;
        vm.scene.create(
            "count",
            Kind::Image,
            HashMap::from([
                (Prop::REPEAT, Value::Bool(true)),
                (Prop::ONPLAYEND, Value::Str("should-not-run.lsb".into())),
            ]),
        );
        vm.now = 1_000.0;
        vm.start_lcm_movie("count", digit_movie());
        assert!(!vm.cinemas.contains_key("count"));
        assert!(
            vm.scene
                .get("count")
                .unwrap()
                .get(Prop::PLAYING)
                .unwrap()
                .truthy()
        );

        vm.now = 1_000.0 + duration * 2.0;
        vm.update_cinemas();
        assert!(vm.queue.is_empty());
        assert_eq!(vm.cinema_movies["count"].texts_at(vm.now)[0].value, "10:87");

        vm.now = 1_000.0 + duration - frame * 0.5;
        assert_eq!(vm.cinema_movies["count"].texts_at(vm.now)[0].value, "10:42");

        vm.scene.create(
            "once",
            Kind::Image,
            HashMap::from([
                (Prop::REPEAT, Value::Bool(false)),
                (Prop::ONPLAYEND, Value::Str("ended.lsb".into())),
            ]),
        );
        vm.now = 5_000.0;
        vm.start_lcm_movie("once", digit_movie());
        assert!(vm.cinemas.contains_key("once"));
        vm.now = 5_000.0 + duration + frame;
        vm.update_cinemas();
        assert!(!vm.cinemas.contains_key("once"));
        assert!(
            !vm.scene
                .get("once")
                .unwrap()
                .get(Prop::PLAYING)
                .unwrap()
                .truthy()
        );
        assert_eq!(vm.queue.len(), 1);
        assert_eq!(vm.queue[0].script, "ended.lsb");
        assert_eq!(vm.cinema_movies["once"].texts_at(vm.now)[0].value, "10:42");
    }
}
