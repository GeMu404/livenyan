use super::{Event, Vm, resolve_system_sound, volume_gain};
use crate::assets::IMAGE_EXTS;
use crate::lsb::expr::Value;
use crate::lsb::names::Prop;
use crate::scene::{Kind, Motion};
use std::collections::HashMap;

impl Vm {
    /// Nombre del componente que el sistema LiveNovel usa para `{PLAYMOVIE archivo.wmv}`.
    const TEXT_MOVIE: &'static str = "ムービー";

    fn media_file_name(path: &str) -> &str {
        path.rsplit(['\\', '/']).next().unwrap_or(path)
    }

    fn same_media(source: &str, reference: &str) -> bool {
        !reference.is_empty()
            && (source.eq_ignore_ascii_case(reference)
                || Self::media_file_name(source)
                    .eq_ignore_ascii_case(Self::media_file_name(reference)))
    }

    fn resolve_video_ref(&self, reference: &str) -> Option<String> {
        if reference.is_empty() {
            return None;
        }
        let data = self.assets.data();
        if let Some(found) = data.resolve(reference, super::VIDEO_EXTS) {
            return Some(found);
        }
        if !reference.contains(['\\', '/']) {
            data.resolve(&format!("movie\\{reference}"), super::VIDEO_EXTS)
        } else {
            None
        }
    }

    /// Objeto Movie ya existente para el argumento de PLAYMOVIE (nombre o fuente).
    /// `Some(None)` si hay más de un objeto con esa fuente.
    fn existing_movie(&self, reference: &str) -> Option<Option<String>> {
        if reference.is_empty() {
            let videos = self.video_names();
            return Some(
                (videos.len() == 1)
                    .then(|| videos.into_iter().next())
                    .flatten(),
            );
        }
        if self
            .scene
            .get(reference)
            .is_some_and(|obj| obj.kind == Kind::Movie)
        {
            return Some(Some(reference.to_string()));
        }
        let matches: Vec<String> = self
            .scene
            .live()
            .filter(|obj| {
                obj.kind == Kind::Movie && Self::same_media(&obj.str(Prop::SOURCE), reference)
            })
            .map(|obj| obj.name.clone())
            .collect();
        match matches.as_slice() {
            [] => None,
            [name] => Some(Some(name.clone())),
            _ => Some(None),
        }
    }

    /// Resuelve PLAYMOVIE a un objeto Movie, creándolo si el argumento es un clip.
    /// Args observados: [fuente, zoom(256=1×), X(L/C/R|n), Y(T/C/B|n), MODE, …]
    /// Alineación/zoom: `docs/livenovel/script.html` `<MOVIE>` (ZOOM 100=1× allí;
    /// el tag de sistema trae 256=1× como `PR_ZOOM`, **fuente primaria** media.html).
    fn prepare_playmovie(&mut self, args: &[String]) -> Option<String> {
        let reference = args.first().map(String::as_str).unwrap_or("");
        if let Some(found) = self.existing_movie(reference) {
            let name = found?;
            self.apply_playmovie_repeat(&name, args);
            return Some(name);
        }
        let source = self.resolve_video_ref(reference)?;
        let repeat = args.get(4).is_some_and(|mode| mode == "REPEAT");
        // PR_ZOOM: 256 = 1× (media.html). Default 256 si falta.
        let zoom = args
            .get(1)
            .and_then(|s| s.parse::<i64>().ok())
            .filter(|&z| z > 0)
            .unwrap_or(256);
        let mut props = HashMap::new();
        props.insert(Prop::NAME, Value::Str(Self::TEXT_MOVIE.to_string()));
        props.insert(Prop::SOURCE, Value::Str(source.clone()));
        props.insert(Prop::ZOOMX, Value::Int(zoom));
        props.insert(Prop::ZOOMY, Value::Int(zoom));
        props.insert(Prop::REPEAT, Value::Bool(repeat));
        props.insert(Prop::VISIBLE, Value::Bool(true));
        props.insert(Prop::PLAYING, Value::Bool(false));
        // Sin probe aquí: `restart_video` hace un solo temp+ffprobe; la
        // alineación L/C/R·T/C/B se aplica después (`script.html` <MOVIE>).
        self.clear_clock(Self::TEXT_MOVIE);
        self.scene.create(Self::TEXT_MOVIE, Kind::Movie, props);
        Some(Self::TEXT_MOVIE.to_string())
    }

    /// Coloca el Movie de PLAYMOVIE según args X/Y una vez conocidos ORIG×ZOOM.
    fn apply_playmovie_placement(&mut self, name: &str, args: &[String]) {
        let Some(obj) = self.scene.get(name) else {
            return;
        };
        if obj.props.contains_key(&Prop::LEFT) && obj.props.contains_key(&Prop::TOP) {
            return;
        }
        let native_w = obj.int(Prop::ORIGWIDTH, 0);
        let native_h = obj.int(Prop::ORIGHEIGHT, 0);
        if native_w <= 0 || native_h <= 0 {
            return;
        }
        let zoom = obj.int(Prop::ZOOMX, 256).max(1);
        let z = zoom as f64 / 256.0;
        let dw = ((native_w as f64) * z).round().max(1.0) as i64;
        let dh = ((native_h as f64) * z).round().max(1.0) as i64;
        let (sw, sh) = (self.opts.screen.0 as i64, self.opts.screen.1 as i64);
        let x = match args.get(2).map(String::as_str).unwrap_or("C") {
            "C" => (sw - dw) / 2,
            "R" => sw - dw,
            "L" => 0,
            other => other.parse().unwrap_or(0),
        };
        let y = match args.get(3).map(String::as_str).unwrap_or("C") {
            "C" => (sh - dh) / 2,
            "B" => sh - dh,
            "T" => 0,
            other => other.parse().unwrap_or(0),
        };
        if let Some(obj) = self.scene.get_mut(name) {
            obj.props.insert(Prop::LEFT, Value::Int(x));
            obj.props.insert(Prop::TOP, Value::Int(y));
        }
    }

    fn apply_playmovie_repeat(&mut self, name: &str, args: &[String]) {
        let Some(mode) = args.get(4) else {
            return;
        };
        if let Some(obj) = self.scene.get_mut(name) {
            obj.props
                .insert(Prop::REPEAT, Value::Bool(mode == "REPEAT"));
        }
    }

    /// Eventos de sistema de LiveNovel incrustados en el texto ({CREATECG ...} etc.).
    /// CREATECG/CHANGECG crean imágenes y películas LiveCinema; otros eventos se registran.
    /// Argumentos observados de CREATECG: [nombre, fuente ("$RRGGBB" = color sólido), efecto,
    /// "#n", alineación H (L/C/R), alineación V (T/C/B), ?, ?, tiempo ms, ?, ?, "", ?, ?, ?, alfa].
    /// Solo nombre, fuente, alineación y alfa se usan; el resto **sin verificar** (H7).
    pub(crate) fn system_text_event(&mut self, boxname: &str, name: &str, args: &[String]) {
        log::debug!("system text event on {boxname}: {name} args {args:?}");
        match name {
            "PAUSE" => {
                if let Some(playback) = self.play_mut(boxname) {
                    playback.waiting_click = true;
                }
                if let Some(obj) = self.scene.get_mut(boxname) {
                    obj.props.insert(Prop::TEXTPAUSED, Value::Bool(true));
                }
                // ONNOTIFY → イベント.lsb (cursor, TEXTPAUSED script-side).
                self.fire(boxname, Prop::ONNOTIFY, vec![Value::Str("PAUSE".into())]);
            }
            "TEXTCLEAR" => {
                if let Some(obj) = self.scene.get_mut(boxname) {
                    obj.text.clear();
                    obj.shown = 0;
                    obj.text_rev += 1;
                    obj.props.insert(Prop::TEXT, Value::Str(String::new()));
                }
            }
            // MESON/MESOFF: el runtime LiveNovel notifica a ONNOTIFY; イベント.lsb oculta/muestra
            // el 土台 (y labels). No hardcodear VISIBLE aquí.
            "MESON" | "MESOFF" => {
                let mut params = vec![Value::Str(name.to_string())];
                params.extend(args.iter().cloned().map(Value::Str));
                self.fire(boxname, Prop::ONNOTIFY, params);
            }
            "WAITPLAY" => {
                // ノベルシステム\メッセージボックス\イベント.lsb: si el modo no es
                // NORMAL, espera mientras el objetivo sigue y un clic la corta.
                // Si ya terminó (logo tras CREATECG WAIT), no pide otro clic.
                let mode = args.get(1).map(String::as_str).unwrap_or("");
                let click_skips = !mode.eq_ignore_ascii_case("NORMAL");
                let Some(target) = args.first().filter(|target| !target.is_empty()) else {
                    if click_skips && let Some(playback) = self.play_mut(boxname) {
                        playback.waiting_click = true;
                    }
                    return;
                };
                if !self.effect_busy(target) {
                    return;
                }
                let deadline = self.now + 3_600_000.0;
                if let Some(playback) = self.play_mut(boxname) {
                    playback.wait_for = Some((target.clone(), deadline));
                    playback.click_skips_wait = click_skips;
                }
            }
            "PLAYMOVIE" => {
                // El tag trae la ruta (.wmv), no el nombre del componente. El sistema
                // LiveNovel crea un único objeto "ムービー" con esa fuente (bajo movie\).
                let Some(target) = self.prepare_playmovie(args) else {
                    self.warn(
                        "playmovie:missing-target",
                        "PLAYMOVIE has no unambiguous movie target",
                    );
                    return;
                };
                if let Err(error) = self.restart_video(&target) {
                    self.warn(
                        &format!("playmovie:{target}"),
                        &format!("PLAYMOVIE cannot start {target:?}: {error}"),
                    );
                    return;
                }
                // Solo si el objeto es nuevo (sin LEFT/TOP aún).
                self.apply_playmovie_placement(&target, args);
                if args.get(4).is_some_and(|mode| mode == "WAIT") && !self.skip && !self.opts.trace
                {
                    let deadline = self.now + 3_600_000.0;
                    if let Some(playback) = self.play_mut(boxname) {
                        playback.wait_for = Some((target, deadline));
                    }
                }
            }
            "STOPMOVIE" => {
                let target = args.first().map(String::as_str).unwrap_or("");
                let videos: Vec<String> = self
                    .video_names()
                    .into_iter()
                    .filter(|movie| {
                        target.is_empty()
                            || movie.as_str() == target
                            || self
                                .scene
                                .get(movie)
                                .is_some_and(|obj| obj.str(Prop::SOURCE) == target)
                    })
                    .collect();
                for movie in videos {
                    self.stop_video(&movie);
                }
            }
            "CREATECG" | "CHANGECG" => {
                let (Some(obj), Some(src)) = (args.first(), args.get(1)) else {
                    log::debug!("{name}: missing name/source");
                    return;
                };
                let obj = obj.clone();
                let color = src
                    .strip_prefix('$')
                    .and_then(|h| u32::from_str_radix(h, 16).ok());
                // Posiciones: CREATECG [0]=nombre [1]=fuente/$color [2]=efecto (NORMAL/WAIT...)
                // [3]=#capa [4]=alin. H [5]=alin. V [8]=tiempo ms [15]=alfa (solo forma de 16 args).
                // CHANGECG [0]=nombre [1]=fuente/$color [2]=efecto [4]=tiempo ms.
                // El resto **sin verificar** (H7): no se usa.
                let num = |i: usize| args.get(i).and_then(|a| a.trim().parse::<f64>().ok());
                let alpha = if name == "CREATECG" && args.len() >= 16 {
                    num(15)
                        .filter(|a| (0.0..=255.0).contains(a))
                        .map(|a| a as i64)
                } else {
                    None
                };
                let time = if name == "CREATECG" { num(8) } else { num(4) }
                    .unwrap_or(0.0)
                    .max(0.0);
                let time = if self.skip || self.opts.trace {
                    0.0
                } else {
                    time
                };
                let wait = args.get(2).is_some_and(|e| e == "WAIT");
                if wait && !self.opts.trace {
                    // WAIT: el texto de la caja no sigue hasta que terminen la animación del
                    // objeto (su PLAYEND) y el fundido; con imagen estática y tiempo 0, nada.
                    let deadline = self.now + time + 30_000.0;
                    if let Some(p) = self.play_mut(boxname) {
                        p.wait_for = Some((obj.clone(), deadline));
                    }
                }
                log::debug!(
                    "{name} {obj:?}: effect {:?} time {time} ms{} alpha {alpha:?}",
                    args.get(2),
                    if wait { " (WAIT)" } else { "" }
                );
                if name == "CHANGECG"
                    && self.scene.get(&obj).is_some()
                    && (color.is_none()
                        == (self.scene.get(&obj).map(|o| o.kind) == Some(Kind::Image)))
                {
                    match color {
                        Some(c) => {
                            let tc = ((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF);
                            self.scene.set(&obj, Prop::COLOR, Value::Int(tc as i64));
                        }
                        None => {
                            match self.load_lcm_movie(src) {
                                Ok(Some(movie)) => {
                                    self.scene.set(&obj, Prop::SOURCE, Value::Str(src.clone()));
                                    self.scene.set(
                                        &obj,
                                        Prop::WIDTH,
                                        Value::Int(movie.width as i64),
                                    );
                                    self.scene.set(
                                        &obj,
                                        Prop::HEIGHT,
                                        Value::Int(movie.height as i64),
                                    );
                                    self.start_lcm_movie(&obj, movie);
                                    log::debug!(
                                        "CHANGECG {obj:?}: started LiveCinema image timeline"
                                    );
                                    return;
                                }
                                Err(e) => {
                                    log::warn!("CHANGECG {obj:?}: cannot load movie: {e}");
                                    return;
                                }
                                Ok(None) => {}
                            }
                            self.clear_clock(&obj);
                            if let Some(r) = self.assets.data().resolve(src, IMAGE_EXTS) {
                                self.assets.request(&r);
                            }
                            self.scene.set(&obj, Prop::SOURCE, Value::Str(src.clone()));
                            self.watch_anim(&obj, src);
                        }
                    }
                    log::debug!("CHANGECG {obj:?} -> {src:?}");
                    return;
                }
                // CHANGECG que cambia de imagen a color (o al revés): se recrea el objeto y se
                // conserva su posición/prioridad/padre; el fundido va sobre el nuevo.
                let keep: Vec<(u8, Value)> = self
                    .scene
                    .get(&obj)
                    .map(|o| {
                        [Prop::PRIORITY, Prop::PARENT]
                            .iter()
                            .filter_map(|p| o.get(*p).map(|v| (*p, v.clone())))
                            .collect()
                    })
                    .unwrap_or_default();
                let (sw, sh) = (self.opts.screen.0 as i64, self.opts.screen.1 as i64);
                let mut props: HashMap<u8, Value> = HashMap::new();
                props.insert(Prop::NAME, Value::Str(obj.clone()));
                props.extend(keep);
                let target_alpha = alpha.unwrap_or(255);
                props.insert(
                    Prop::ALPHA,
                    Value::Int(if time > 0.0 { 0 } else { target_alpha }),
                );
                let mut movie = None;
                let kind = match color {
                    Some(c) => {
                        let tc = ((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF);
                        props.insert(Prop::COLOR, Value::Int(tc as i64));
                        props.insert(Prop::WIDTH, Value::Int(sw));
                        props.insert(Prop::HEIGHT, Value::Int(sh));
                        Kind::Box
                    }
                    None => {
                        movie = match self.load_lcm_movie(src) {
                            Ok(movie) => movie,
                            Err(e) => {
                                log::warn!("{name} {obj:?}: cannot load movie: {e}");
                                None
                            }
                        };
                        let resolved = self.assets.data().resolve(src, IMAGE_EXTS);
                        let size = movie.as_ref().map(|m| (m.width, m.height)).or_else(|| {
                            resolved
                                .as_ref()
                                .and_then(|r| self.assets.data().read_prefix(r, 4096))
                                .and_then(|d| crate::gal::peek_size(&d))
                        });
                        if movie.is_none()
                            && let Some(r) = &resolved
                        {
                            self.assets.request(r);
                        } else if movie.is_none() && !src.to_lowercase().ends_with(".lcm") {
                            log::warn!("{name}: image not found: {src:?}");
                        }
                        let (iw, ih) = size.map(|(w, h)| (w as i64, h as i64)).unwrap_or((sw, sh));
                        let x = match args.get(4).map(|s| s.as_str()) {
                            Some("C") => (sw - iw) / 2,
                            Some("R") => sw - iw,
                            _ => 0,
                        };
                        let y = match args.get(5).map(|s| s.as_str()) {
                            Some("C") => (sh - ih) / 2,
                            Some("B") => sh - ih,
                            _ => 0,
                        };
                        props.insert(Prop::SOURCE, Value::Str(src.clone()));
                        props.insert(Prop::LEFT, Value::Int(x));
                        props.insert(Prop::TOP, Value::Int(y));
                        props.insert(Prop::WIDTH, Value::Int(iw));
                        props.insert(Prop::HEIGHT, Value::Int(ih));
                        Kind::Image
                    }
                };
                log::debug!(
                    "{name} creates {kind:?} {obj:?} src={src:?} props {:?}",
                    props
                        .keys()
                        .map(|p| crate::lsb::prop_name(*p))
                        .collect::<Vec<_>>()
                );
                if args.get(2).is_some_and(|e| e == "REPEAT") {
                    props.insert(Prop::REPEAT, Value::Bool(true));
                }
                self.clear_clock(&obj);
                self.scene.create(&obj, kind, props);
                if time > 0.0 {
                    self.scene.add_motion(Motion {
                        obj: obj.clone(),
                        prop: Prop::ALPHA,
                        from: 0.0,
                        to: target_alpha as f64,
                        start: self.now,
                        dur: time,
                        soft: false,
                    });
                }
                if let Some(movie) = movie {
                    log::debug!(
                        "{name} {obj:?}: started LiveCinema {}x{} with {} image tracks",
                        movie.width,
                        movie.height,
                        movie.clips.len()
                    );
                    self.start_lcm_movie(&obj, movie);
                } else if kind == Kind::Image {
                    self.watch_anim(&obj, src);
                }
                self.object_defined(&obj);
            }
            "DELETECG" => {
                // [0]=nombres separados por comas, [2]=tiempo (fundido; aquí se borra ya)
                for obj in args
                    .first()
                    .map(|s| s.as_str())
                    .unwrap_or("")
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    let gone = self.scene.delete_names(obj);
                    log::debug!("DELETECG {obj:?}: removed {} objects", gone.len());
                    for o in gone {
                        self.prevmenus.remove(&o.name);
                        self.clear_clock(&o.name);
                        self.forget_deleted_component(&o.name);
                        self.scene.retain_deleted(o);
                    }
                }
            }
            "PLAYSND" => {
                let (Some(src), Some(channel), Some(mode), Some(volume)) =
                    (args.first(), args.get(1), args.get(2), args.get(3))
                else {
                    log::warn!("PLAYSND on {boxname}: missing source, channel, mode, or volume");
                    return;
                };
                let volume = match volume.parse::<f64>() {
                    Ok(volume) => volume,
                    Err(error) => {
                        log::warn!("PLAYSND on {boxname}: invalid volume {volume:?}: {error}");
                        return;
                    }
                };
                let repeat = if mode.eq_ignore_ascii_case("REPEAT") {
                    true
                } else if mode.eq_ignore_ascii_case("NORMAL") {
                    false
                } else {
                    log::warn!("PLAYSND on {boxname}: unsupported mode {mode:?}");
                    return;
                };
                match resolve_system_sound(self.assets.data(), src) {
                    Some(file) => {
                        self.assets.request(&file);
                        self.events.push(Event::PlaySound {
                            channel: channel.clone(),
                            file,
                            volume: volume_gain(volume),
                            repeat,
                        });
                    }
                    None => self.warn(
                        &format!("snd:{src}"),
                        &format!("PLAYSND audio not found: {src}"),
                    ),
                }
            }
            "STOPSND" => {
                let Some(channel) = args.first() else {
                    log::warn!("STOPSND on {boxname}: missing channel");
                    return;
                };
                let duration_ms = match args.get(1) {
                    Some(duration) => match duration.parse::<u64>() {
                        Ok(duration_ms) => duration_ms,
                        Err(error) => {
                            log::warn!(
                                "STOPSND on {boxname}: invalid duration {duration:?}: {error}"
                            );
                            return;
                        }
                    },
                    None => 0,
                };
                if duration_ms == 0 {
                    self.events.push(Event::StopSound {
                        channel: channel.clone(),
                    });
                } else {
                    self.events.push(Event::FadeSound {
                        channel: channel.clone(),
                        duration_ms,
                    });
                }
            }
            // script.html: ms/carácter; ABSOLUTE ON ignora la opción del jugador.
            // Args observados como otros tags: [TIME, ABSOLUTE?] (**no confirmado** orden).
            "TEXTSPD" => {
                let ms = args
                    .first()
                    .and_then(|s| s.parse::<f64>().ok())
                    .unwrap_or(0.0);
                let absolute = args
                    .get(1)
                    .is_some_and(|s| s.eq_ignore_ascii_case("ON") || s == "1");
                self.text_char_ms.insert(
                    boxname.to_string(),
                    super::TextSpeedOverride { ms, absolute },
                );
            }
            "TXSPF" => {
                // ノーウエイト
                self.text_char_ms.insert(
                    boxname.to_string(),
                    super::TextSpeedOverride {
                        ms: 0.0,
                        absolute: true,
                    },
                );
            }
            "TXSPN" => {
                // 標準値
                self.text_char_ms.remove(boxname);
                if let Some(obj) = self.scene.get_mut(boxname) {
                    obj.props.insert(Prop::TEXTDELAY, Value::Int(256));
                }
            }
            "TXSPS" => {
                // スロー — **interpretación:** periodo baseline × TEXT_SPEED_SLOW_FACTOR.
                let ms = 1000.0 / super::TEXT_SPEED_BASELINE_CPS * super::TEXT_SPEED_SLOW_FACTOR;
                self.text_char_ms.insert(
                    boxname.to_string(),
                    super::TextSpeedOverride {
                        ms,
                        absolute: false,
                    },
                );
            }
            // Permisos de menú/save: el novel system reacciona vía ONNOTIFY (como MESON).
            "SYSMENUON" | "SYSMENUOFF" | "SAVELOADON" | "SAVELOADOFF" => {
                self.fire(boxname, Prop::ONNOTIFY, vec![Value::Str(name.to_string())]);
            }
            "OPENURL" => {
                let url = args.first().map(String::as_str).unwrap_or("");
                if !super::open_external_url(url) {
                    self.warn("openurl", &format!("OPENURL failed for {url:?}"));
                }
            }
            "WALLPAPER" => {
                // script.html / calc SetWallPaper: sin API host → no-op (como Func false).
                self.warn(
                    "wallpaper:unsupported",
                    "WALLPAPER: desktop wallpaper not supported (ignored)",
                );
            }
            // script.html WAITFOR: esperar a que termine el componente.
            "WAITFOR" => {
                let click_abort = args
                    .get(1)
                    .is_some_and(|s| s.eq_ignore_ascii_case("ON") || s == "1");
                let Some(target) = args.first().filter(|t| !t.is_empty()) else {
                    return;
                };
                if !self.effect_busy(target) {
                    return;
                }
                let deadline = self.now + 3_600_000.0;
                if let Some(playback) = self.play_mut(boxname) {
                    playback.wait_for = Some((target.clone(), deadline));
                    playback.click_skips_wait = click_abort;
                }
            }
            "QUAKE" => {
                // Args: NAME, TYPE, RANDOM, X, Y, TIME, CYCLE (**no confirmado** orden exacto;
                // alineado al orden del tag en script.html).
                let names: Vec<String> = args
                    .first()
                    .map(|s| {
                        s.split(',')
                            .map(str::trim)
                            .filter(|n| !n.is_empty())
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                let kind = args
                    .get(1)
                    .and_then(|s| s.chars().next())
                    .unwrap_or('Q')
                    .to_ascii_uppercase();
                let random_phase = args
                    .get(2)
                    .is_some_and(|s| s.eq_ignore_ascii_case("ON") || s == "1");
                let amp_x: f64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let amp_y: f64 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let time: f64 = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let cycle: f64 = args.get(6).and_then(|s| s.parse().ok()).unwrap_or(100.0);
                self.quake = Some(super::QuakeState {
                    names,
                    kind,
                    random_phase,
                    amp_x,
                    amp_y,
                    started_at: self.now,
                    until: self.now + time.max(0.0),
                    cycle_ms: cycle.max(1.0),
                    stop_until: None,
                });
            }
            "STOPQUAKE" => {
                let time = args
                    .first()
                    .and_then(|s| s.parse::<f64>().ok())
                    .unwrap_or(0.0);
                if let Some(q) = self.quake.as_mut() {
                    if time <= 0.0 {
                        self.quake = None;
                    } else {
                        q.stop_until = Some(self.now + time);
                        q.until = q.until.max(self.now + time);
                    }
                }
            }
            "" => {}
            _ => {
                self.warn(
                    &format!("unimpl:text sysevent {name}"),
                    &format!("system text event {name} not implemented (ignored)"),
                );
            }
        }
    }
}
