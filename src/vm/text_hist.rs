use super::{Event, HistoryEntry, Pending, State, Vm, resolve_system_sound, volume_gain};
use crate::assets::IMAGE_EXTS;
use crate::lsb::expr::Value;
use crate::lsb::names::Prop;
use crate::lsb::tpword::{Glyph, brk};
use crate::scene::{Kind, Motion, TextItem};
use std::collections::HashMap;

/// Nombre del evento de sistema LiveNovel en un `Notify` (`\x01NAME\r\n...`).
fn system_text_event_name(event: &str) -> Option<&str> {
    let body = event.strip_prefix('\x01').unwrap_or(event);
    let name = body.split("\r\n").next().unwrap_or("").trim();
    (!name.is_empty()).then_some(name)
}

/// El plan ya exige un clic de lectura justo antes de este punto (p. ej. `<PS>` o
/// `{PAUSE}`), ignorando limpiezas intermedias (`Clear` / `{TEXTCLEAR}`). Así un
/// `<PG>` posterior no pide un segundo clic. **fuente primaria:** `script.html`
/// (`<PS>` clic; `<PG>` clic+改ページ); la fusión evita duplicar esperas en el
/// mismo `TextIns` sin anular un `<PG>` solo.
fn plan_has_trailing_click_wait(plan: &[Pending]) -> bool {
    for pending in plan.iter().rev() {
        match pending {
            Pending::Clear => continue,
            Pending::Notify(event)
                if system_text_event_name(event) == Some("TEXTCLEAR") =>
            {
                continue;
            }
            Pending::Click => return true,
            Pending::Notify(event) if system_text_event_name(event) == Some("PAUSE") => {
                return true;
            }
            _ => return false,
        }
    }
    false
}

impl Vm {
    pub(crate) fn append_history(&mut self, page: String, text: Vec<TextItem>) {
        if text.is_empty() {
            return;
        }
        self.history.push_back(HistoryEntry { page, text });
        if self.history_max > 0 {
            while self.history.len() > self.history_max as usize {
                self.history.pop_front();
            }
        }
    }

    pub(crate) fn run_text(
        &mut self,
        target: &str,
        tw: &crate::lsb::TpWord,
        wait: bool,
        keep_history: bool,
        already_read: bool,
    ) {
        self.last_text_target = Some(target.to_string());
        if self.scene.get(target).is_none() {
            let msgs: Vec<String> = self
                .scene
                .draw_order()
                .into_iter()
                .filter(|o| o.kind == Kind::Message)
                .map(|o| format!("{:?}", o.name))
                .collect();
            log::debug!(
                "TextIns target {target:?} (bytes {:02x?}) missing; existing message objects before stand-in: {msgs:?}",
                target.as_bytes()
            );
            match self.graveyard.get(target).cloned() {
                Some((kind, props)) => {
                    let evs: Vec<&str> = props
                        .keys()
                        .map(|p| crate::lsb::prop_name(*p))
                        .filter(|n| n.starts_with("ON"))
                        .collect();
                    self.warn(&format!("mes:{target}"), &format!("TextIns into deleted message object {target:?}; re-creating it with its last properties"));
                    log::debug!(
                        "stand-in {target:?} inherits {} props of the deleted object (events {evs:?})",
                        props.len()
                    );
                    self.scene.create(target, kind, props);
                }
                None => {
                    self.warn(&format!("mes:{target}"), &format!("TextIns into never-created message object {target:?}; creating an empty stand-in (no event handlers)"));
                    self.scene.create(target, Kind::Message, HashMap::new());
                    // texto retenido hasta que llegue el MesNew real
                    self.placeholders.insert(target.to_string());
                }
            }
        }
        self.ended_unnotified.remove(target);
        let mut plan: Vec<Pending> = vec![];
        let mut cur: Vec<TextItem> = vec![];
        let flush = |plan: &mut Vec<Pending>, cur: &mut Vec<TextItem>| {
            if !cur.is_empty() {
                plan.push(Pending::Text {
                    items: std::mem::take(cur),
                    already_read,
                });
            }
        };
        let has_chars = tw.body.iter().any(|g| matches!(g, Glyph::Char { .. }));
        for g in &tw.body {
            match g {
                Glyph::Char { ch, .. } => cur.push(TextItem::Char(ch.clone())),
                Glyph::Var { name, .. } => {
                    let v = self.lookup(name).cloned().unwrap_or_default().as_str();
                    let displayed = self
                        .translations
                        .get(&crate::text_export::menu_item_id(&v))
                        .map(String::as_str)
                        .unwrap_or(&v);
                    cur.extend(displayed.chars().map(|c| TextItem::Char(c.to_string())));
                }
                Glyph::Return { kind } => match *kind {
                    brk::LINE => cur.push(TextItem::Newline),
                    // Un <PG>/<PS> en un TextIns sin caracteres (logo, cierre del
                    // menú) no es un clic de lectura. El arreglo de test2 los dejaba
                    // esperando igual que una página con diálogo.
                    brk::PAUSE if !has_chars => {
                        flush(&mut plan, &mut cur);
                    }
                    brk::PAUSE => {
                        flush(&mut plan, &mut cur);
                        plan.push(Pending::Click);
                    }
                    // <PG>: docs/livenovel/script.html — show cursor, wait for click,
                    // then page break. If this TextIns already queued a click-wait
                    // (`<PS>` / `{PAUSE}`), only clear — do not demand a second click.
                    brk::PAGE => {
                        flush(&mut plan, &mut cur);
                        if !plan_has_trailing_click_wait(&plan) {
                            plan.push(Pending::Click);
                        }
                        plan.push(Pending::Clear);
                    }
                    // <CLR> / other breaks: clear without an implied click-wait.
                    _ => {
                        flush(&mut plan, &mut cur);
                        plan.push(Pending::Clear);
                    }
                },
                Glyph::Event { event } => {
                    flush(&mut plan, &mut cur);
                    plan.push(Pending::Notify(event.clone()));
                }
                Glyph::Img { src, align } => {
                    if let Some(source) = self.assets.data().resolve(src, IMAGE_EXTS) {
                        self.assets.request(&source);
                        cur.push(TextItem::Image {
                            source,
                            align: *align,
                        });
                    } else {
                        self.warn(
                            &format!("inline-image:{src}"),
                            &format!("TextIns inline image not found: {src}"),
                        );
                    }
                }
                _ => {}
            }
        }
        flush(&mut plan, &mut cur);
        if log::log_enabled!(log::Level::Debug) {
            let tags: Vec<String> = tw
                .body
                .iter()
                .filter_map(|g| match g {
                    Glyph::Return { kind } => Some(format!("Return({kind})")),
                    Glyph::Event { event } => Some(format!(
                        "Event({})",
                        event
                            .trim_start_matches('\x01')
                            .split("\r\n")
                            .next()
                            .unwrap_or("")
                    )),
                    Glyph::Img { .. } => Some("Img".into()),
                    _ => None,
                })
                .collect();
            log::debug!("TextIns {target}: {} glyphs, tags {tags:?}", tw.body.len());
        }
        if keep_history {
            let entry = plan
                .iter()
                .filter_map(|pending| match pending {
                    Pending::Text { items, .. } => Some(items.as_slice()),
                    Pending::Clear | Pending::Click | Pending::Notify(_) => None,
                })
                .flatten()
                .cloned()
                .collect();
            let page = self
                .frames
                .last()
                .map(|frame| frame.script.name.clone())
                .unwrap_or_default();
            self.append_history(page, entry);
        }
        self.start_play(target, plan);
        if wait && !self.placeholders.contains(target) {
            self.state = State::WaitText {
                target: target.to_string(),
            };
        }
    }

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
            return Some(
                (self.videos.len() == 1)
                    .then(|| self.videos.keys().next().cloned())
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
            .objs
            .values()
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
        self.videos.remove(Self::TEXT_MOVIE);
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
    /// Solo nombre, fuente, alineación y alfa se usan; el resto está sin verificar.
    pub(crate) fn system_text_event(&mut self, boxname: &str, name: &str, args: &[String]) {
        log::debug!("system text event on {boxname}: {name} args {args:?}");
        match name {
            "PAUSE" => {
                if let Some(playback) = self.plays.get_mut(boxname) {
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
                    if click_skips && let Some(playback) = self.plays.get_mut(boxname) {
                        playback.waiting_click = true;
                    }
                    return;
                };
                if !self.effect_busy(target) {
                    return;
                }
                let deadline = self.now + 3_600_000.0;
                if let Some(playback) = self.plays.get_mut(boxname) {
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
                    if let Some(playback) = self.plays.get_mut(boxname) {
                        playback.wait_for = Some((target, deadline));
                    }
                }
            }
            "STOPMOVIE" => {
                let target = args.first().map(String::as_str).unwrap_or("");
                let videos: Vec<String> = self
                    .videos
                    .keys()
                    .filter(|movie| {
                        target.is_empty()
                            || movie.as_str() == target
                            || self
                                .scene
                                .get(movie)
                                .is_some_and(|obj| obj.str(Prop::SOURCE) == target)
                    })
                    .cloned()
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
                // El resto sin verificar: no se usa (nunca se deduce alfa por posición dudosa).
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
                    if let Some(p) = self.plays.get_mut(boxname) {
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
                            self.cinema_movies.remove(&obj);
                            self.cinemas.remove(&obj);
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
                self.cinema_movies.remove(&obj);
                self.cinemas.remove(&obj);
                self.scene.create(&obj, kind, props);
                if time > 0.0 {
                    self.scene.add_motion(Motion {
                        obj: obj.clone(),
                        prop: Prop::ALPHA,
                        from: 0.0,
                        to: target_alpha as f64,
                        start: self.now,
                        dur: time,
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
                        self.anims.remove(&o.name);
                        self.prevmenus.remove(&o.name);
                        self.videos.remove(&o.name);
                        self.cinema_movies.remove(&o.name);
                        self.cinemas.remove(&o.name);
                        self.forget_deleted_component(&o.name);
                        self.graveyard.insert(o.name.clone(), (o.kind, o.props));
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
                // スロー — ratio no documentado: 3× el periodo baseline (**inventado**).
                let ms = 1000.0 / super::TEXT_SPEED_BASELINE_CPS * 3.0;
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
                    self.warn(
                        "openurl",
                        &format!("OPENURL failed for {url:?}"),
                    );
                }
            }
            "WALLPAPER" => {
                // Escritorio del SO: sin API → ignorar con aviso (script.html).
                self.warn(
                    "unimpl:text sysevent WALLPAPER",
                    "WALLPAPER ignored (no desktop wallpaper API)",
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
                if let Some(playback) = self.plays.get_mut(boxname) {
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
                let time = args.first().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
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

#[cfg(test)]
mod tests {
    use super::super::{Event, Options, Playback, PmButton, PrevMenu, Vm};
    use crate::archive::GameData;
    use crate::assets::Assets;
    use crate::lsb::expr::{Env, Value};
    use crate::lsb::names::{Func, Prop};
    use crate::lsb::tpword::Glyph;
    use crate::scene::{Kind, TextItem};
    use std::collections::HashMap;
    use std::sync::Arc;

    #[test]
    fn textspd_and_txspf_set_speed_override() {
        // script.html TEXTSPD / TXSPF
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
                text_cps: 60.0,
                lookahead: 0,
                trace: false,
                show_text: false,
            },
        )
        .unwrap();
        vm.system_text_event("msg", "TEXTSPD", &["50".into(), "ON".into()]);
        let over = vm.text_char_ms.get("msg").copied().expect("TEXTSPD");
        assert!((over.ms - 50.0).abs() < f64::EPSILON);
        assert!(over.absolute);
        vm.system_text_event("msg", "TXSPF", &[]);
        assert_eq!(vm.text_char_ms["msg"].ms, 0.0);
        vm.system_text_event("msg", "TXSPN", &[]);
        assert!(!vm.text_char_ms.contains_key("msg"));
    }

    #[test]
    fn history_count_and_group_index_are_available() {
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

        vm.append_history("page".into(), vec![TextItem::Char("A".into())]);
        vm.append_history("page".into(), vec![TextItem::Char("B".into())]);
        assert_eq!(
            vm.call_engine(Func::GETHISTORYCOUNT, &[], &[]),
            Some(Value::Int(2))
        );
        assert!(!vm.unimpl.contains_key("func GetHistoryCount"));

        vm.scene.create(
            "grouped",
            Kind::Button,
            HashMap::from([(Prop::GROUPINDEX, Value::Int(7))]),
        );
        assert_eq!(
            vm.call_engine(Func::GETGROUPINDEX, &[Value::Str("grouped".into())], &[]),
            Some(Value::Int(7))
        );
        vm.prevmenus.insert(
            "menu".into(),
            PrevMenu {
                buttons: vec![PmButton {
                    x: 0,
                    y: 0,
                    w: 0,
                    h: 0,
                    src: String::new(),
                    src_selected: String::new(),
                    src_disabled: String::new(),
                    name: "grouped".into(),
                    group: 0,
                }],
                hover: None,
            },
        );
        assert_eq!(
            vm.call_engine(
                Func::GETGROUPINDEX,
                &[Value::Str("menu".into()), Value::Int(0)],
                &[],
            ),
            Some(Value::Int(7))
        );
    }

    #[test]
    fn text_audio_events_start_and_stop_named_channels() {
        let p =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
        let data = GameData::open(&p).unwrap();
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

        vm.system_text_event(
            "msg",
            "PLAYSND",
            &[
                "se.wav".into(),
                "VOICE".into(),
                "REPEAT".into(),
                "500".into(),
                "0".into(),
            ],
        );
        assert!(matches!(
            vm.events.last(),
            Some(Event::PlaySound {
                channel,
                file,
                volume,
                repeat: true,
            }) if channel == "VOICE" && file == "se.wav" && *volume == 0.5
        ));

        vm.system_text_event(
            "msg",
            "STOPSND",
            &["BGM".into(), "100".into(), "PASS".into()],
        );
        assert!(matches!(
            vm.events.last(),
            Some(Event::FadeSound {
                channel,
                duration_ms: 100,
            }) if channel == "BGM"
        ));
    }

    #[test]
    fn text_pause_and_mesoff_events_update_message_playback() {
        let p =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
        let data = GameData::open(&p).unwrap();
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
        vm.scene.create(
            "msg",
            Kind::Message,
            HashMap::from([(Prop::ONNOTIFY, Value::Str("ev.lsc".into()))]),
        );
        vm.plays.insert("msg".into(), Playback::default());

        vm.system_text_event("msg", "PAUSE", &[]);
        assert!(vm.plays.get("msg").unwrap().waiting_click);
        assert!(vm.get_prop("msg", Prop::TEXTPAUSED).truthy());

        // MESOFF encola ONNOTIFY; el script del juego oculta el 土台 (no hardcode local).
        vm.queue.clear();
        vm.system_text_event("msg", "MESOFF", &["100".into()]);
        assert!(
            vm.queue
                .iter()
                .any(|e| e.obj == "msg" && e.prop == Prop::ONNOTIFY),
            "MESOFF must notify the message box script"
        );
    }

    #[test]
    fn pause_holds_and_idle_waitplay_click_does_not() {
        use super::super::Pending;
        use std::collections::VecDeque;

        let p =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
        let data = GameData::open(&p).unwrap();
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
        vm.scene.create(
            "msg",
            Kind::Message,
            HashMap::from([(Prop::VISIBLE, Value::Bool(true))]),
        );
        // {PAUSE} (system Notify) waits for click. This plan then uses Clear (like
        // <CLR>), not a <PG> glyph — so there is no second click-wait.
        // WAITPLAY CLICK only waits while the target is still busy.
        let mut plan = VecDeque::new();
        plan.push_back(Pending::Text {
            items: vec![TextItem::Char("あ".into())],
            already_read: false,
        });
        plan.push_back(Pending::Notify("\x01PAUSE\r\n".into()));
        plan.push_back(Pending::Notify("\x01TEXTCLEAR".into()));
        plan.push_back(Pending::Clear);
        plan.push_back(Pending::Notify("\x01MESOFF".into()));
        plan.push_back(Pending::Notify("\x01WAITPLAY\r\n背景\r\nCLICK".into()));
        plan.push_back(Pending::Notify("MESEND".into()));
        vm.plays.insert(
            "msg".into(),
            Playback {
                plan,
                ..Playback::default()
            },
        );

        vm.update(0.0, 0);
        assert!(
            vm.plays.get("msg").unwrap().waiting_click,
            "PAUSE must wait"
        );
        assert!(vm.plays.contains_key("msg"), "must not reach MESEND yet");

        vm.click();
        vm.update(16.0, 0);
        // WAITPLAY 背景 CLICK con el objetivo ya quieto no abre otro clic
        // (イベント.lsb: la espera es solo mientras sigue reproduciéndose).
        let mut saw_wait = false;
        for step in 0..6 {
            vm.update(32.0 + step as f64 * 16.0, 0);
            if vm
                .plays
                .get("msg")
                .is_some_and(|playback| playback.waiting_click || playback.wait_for.is_some())
            {
                saw_wait = true;
            }
        }
        assert!(
            !saw_wait,
            "idle WAITPLAY CLICK must not wait for another click"
        );
        assert!(
            !vm.plays.contains_key("msg"),
            "MESEND should finish playback without an extra click"
        );
    }

    #[test]
    fn page_break_glyph_waits_for_click_then_clears() {
        // docs/livenovel/script.html <PG>: wait for click, then page break.
        use crate::lsb::tpword::{TpWord, brk};

        let p =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
        let data = GameData::open(&p).unwrap();
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
        vm.scene.create(
            "msg",
            Kind::Message,
            HashMap::from([(Prop::VISIBLE, Value::Bool(true))]),
        );
        let tw = TpWord {
            version: 105,
            body: vec![
                Glyph::Char {
                    ch: "a".into(),
                    decorator: -1,
                    link: -1,
                    speed: 0,
                    condition: -1,
                },
                Glyph::Return { kind: brk::PAGE },
            ],
            ..TpWord::default()
        };
        vm.run_text("msg", &tw, true, false, false);
        vm.update(0.0, 0);
        assert!(
            vm.wants_click(),
            "<PG> must wait for a click after showing text"
        );
        assert!(vm.plays.contains_key("msg"));
        vm.click();
        vm.update(16.0, 0);
        assert!(
            !vm.wants_click(),
            "click should release the <PG> wait"
        );
        assert!(
            !vm.plays.contains_key("msg"),
            "page break should finish playback after the click"
        );
        let text = vm
            .scene
            .get("msg")
            .map(|obj| obj.str(Prop::TEXT))
            .unwrap_or_default();
        assert!(
            text.is_empty(),
            "page break clears the message after the click, got {text:?}"
        );
    }

    #[test]
    fn page_break_after_pause_glyph_does_not_demand_a_second_click() {
        // `<PS>` then `<PG>` in one TextIns: one click-wait, then clear.
        use crate::lsb::tpword::{TpWord, brk};

        let p =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
        let data = GameData::open(&p).unwrap();
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
        vm.scene.create(
            "msg",
            Kind::Message,
            HashMap::from([(Prop::VISIBLE, Value::Bool(true))]),
        );
        let tw = TpWord {
            version: 105,
            body: vec![
                Glyph::Char {
                    ch: "a".into(),
                    decorator: -1,
                    link: -1,
                    speed: 0,
                    condition: -1,
                },
                Glyph::Return { kind: brk::PAUSE },
                Glyph::Return { kind: brk::PAGE },
            ],
            ..TpWord::default()
        };
        vm.run_text("msg", &tw, true, false, false);
        vm.update(0.0, 0);
        assert!(vm.wants_click(), "first click-wait from <PS>/<PG> coalesce");
        vm.click();
        vm.update(16.0, 0);
        assert!(
            !vm.wants_click(),
            "coalesced <PG> must not require a second click"
        );
        assert!(!vm.plays.contains_key("msg"));
    }

    #[test]
    fn page_break_after_pause_system_event_does_not_demand_a_second_click() {
        // LiveNovel pattern: {PAUSE}, optional {TEXTCLEAR}, then <PG>.
        use crate::lsb::tpword::{TpWord, brk};

        let p =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
        let data = GameData::open(&p).unwrap();
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
        vm.scene.create(
            "msg",
            Kind::Message,
            HashMap::from([(Prop::VISIBLE, Value::Bool(true))]),
        );
        let tw = TpWord {
            version: 105,
            body: vec![
                Glyph::Char {
                    ch: "a".into(),
                    decorator: -1,
                    link: -1,
                    speed: 0,
                    condition: -1,
                },
                Glyph::Event {
                    event: "\x01PAUSE\r\n".into(),
                },
                Glyph::Event {
                    event: "\x01TEXTCLEAR".into(),
                },
                Glyph::Return { kind: brk::PAGE },
            ],
            ..TpWord::default()
        };
        vm.run_text("msg", &tw, true, false, false);
        vm.update(0.0, 0);
        assert!(vm.wants_click(), "{{PAUSE}} must wait");
        vm.click();
        vm.update(16.0, 0);
        assert!(
            !vm.wants_click(),
            "<PG> after {{PAUSE}}/{{TEXTCLEAR}} must not wait again"
        );
        assert!(!vm.plays.contains_key("msg"));
    }

    #[test]
    fn message_caret_advances_with_shown_text() {
        use super::super::Pending;
        use std::collections::VecDeque;

        let p =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
        let data = GameData::open(&p).unwrap();
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
        vm.scene.create(
            "msg",
            Kind::Message,
            HashMap::from([
                (Prop::FONTHEIGHT, Value::Int(22)),
                (Prop::WIDTH, Value::Int(600)),
            ]),
        );
        let mut plan = VecDeque::new();
        plan.push_back(Pending::Text {
            items: vec![TextItem::Char("あ".into()), TextItem::Char("い".into())],
            already_read: false,
        });
        plan.push_back(Pending::Click);
        vm.plays.insert(
            "msg".into(),
            Playback {
                plan,
                ..Playback::default()
            },
        );
        vm.update(0.0, 0);
        let caret_x = vm.get_prop("msg", Prop::CARETX).as_int();
        let caret_h = vm.get_prop("msg", Prop::CARETHEIGHT).as_int();
        assert_eq!(caret_h, 22);
        assert!(
            caret_x > 20,
            "CARETX must advance past the start for メッセージカーソル (got {caret_x})"
        );
    }

    #[test]
    fn playmovie_resolves_wmv_path_to_existing_movie_source() {
        let p =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
        let data = GameData::open(&p).unwrap();
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
        vm.scene.create(
            "bg",
            Kind::Movie,
            HashMap::from([(
                Prop::SOURCE,
                Value::Str("MOVIE\\なつみ家外観（昼のみ）.wmv".into()),
            )]),
        );
        vm.plays.insert("msg".into(), Playback::default());

        vm.system_text_event(
            "msg",
            "PLAYMOVIE",
            &[
                "なつみ家外観（昼のみ）.wmv".into(),
                "256".into(),
                "C".into(),
                "C".into(),
                "WAIT".into(),
                "TRUE".into(),
                "0".into(),
            ],
        );

        assert!(vm.warned.contains("playmovie:bg"));
        assert!(!vm.warned.contains("playmovie:なつみ家外観（昼のみ）.wmv"));
        assert!(!vm.warned.contains("playmovie:missing-target"));
        assert!(vm.plays["msg"].wait_for.is_none());
    }

    #[test]
    fn history_text_is_recorded_only_when_requested_and_obeys_the_limit() {
        let p =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
        let data = GameData::open(&p).unwrap();
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
        vm.scene.create("msg", Kind::Message, HashMap::new());
        let text = crate::lsb::TpWord {
            body: vec![Glyph::Char {
                ch: "A".into(),
                decorator: -1,
                link: -1,
                speed: 0,
                condition: -1,
            }],
            ..crate::lsb::TpWord::default()
        };

        vm.run_text("msg", &text, false, false, false);
        assert_eq!(vm.history.len(), 0);
        vm.plays.clear();
        vm.run_text("msg", &text, false, true, false);
        assert_eq!(vm.history.len(), 1);
        vm.append_history("page".into(), vec![TextItem::Char("B".into())]);
        vm.history_max = 1;
        vm.append_history("page".into(), vec![TextItem::Char("C".into())]);
        assert_eq!(vm.history.len(), 1);
        assert_eq!(
            vm.history.front().map(|entry| entry.text.as_slice()),
            Some([TextItem::Char("C".into())].as_slice())
        );
    }
}
