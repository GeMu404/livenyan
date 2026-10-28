use super::{ROOT, TimerState, Vm, translate_component_text};
use crate::assets::IMAGE_EXTS;
use crate::lsb::expr::Value;
use crate::lsb::names::Prop;
use crate::lsb::{CmdType, Command};
use crate::scene::Kind;

impl Vm {
    /// ImgNew / MesNew / BoxNew / … / WaveNew: crea el objeto en escena.
    pub(crate) fn step_obj_new(&mut self, kind: CmdType, c: &Command) -> String {
        let mut props = self.props_of(c);
        if let Some(source) = props.get(&Prop::TEXT).map(Value::as_str)
            && let Some(translated) = translate_component_text(kind, &source, &self.translations)
        {
            props.insert(Prop::TEXT, Value::Str(translated));
        }
        let name = props
            .get(&Prop::NAME)
            .map(|v| v.as_str())
            .unwrap_or_default();
        let src = props
            .get(&Prop::SOURCE)
            .map(|v| v.as_str())
            .unwrap_or_default();
        let movie = if kind == CmdType::Cinema {
            match self.load_lcm_movie(&src) {
                Ok(movie) => movie,
                Err(e) => {
                    log::warn!("Cinema {name:?}: cannot load movie: {e}");
                    None
                }
            }
        } else {
            None
        };
        if kind != CmdType::Cinema {
            let mut sources = vec![src.clone()];
            match kind {
                CmdType::SliderNew | CmdType::ScrollbarNew | CmdType::GaugeNew => {
                    sources.extend([
                        props
                            .get(&Prop::THUMBSOURCE)
                            .map(Value::as_str)
                            .unwrap_or_default(),
                        props
                            .get(&Prop::BUTTONSOURCE)
                            .map(Value::as_str)
                            .unwrap_or_default(),
                    ]);
                }
                CmdType::Button => {
                    sources.extend([
                        props
                            .get(&Prop::HOVERSOURCE)
                            .map(Value::as_str)
                            .unwrap_or_default(),
                        props
                            .get(&Prop::PRESSEDSOURCE)
                            .map(Value::as_str)
                            .unwrap_or_default(),
                    ]);
                }
                _ => {}
            }
            for source in sources.into_iter().filter(|source| !source.is_empty()) {
                if let Some(n) = self.assets.data().resolve(&source, IMAGE_EXTS) {
                    self.assets.request(&n);
                }
            }
        }
        let detail = format!(
            "name={name:?}{}",
            if src.is_empty() {
                String::new()
            } else {
                format!(" src={src:?}")
            }
        );
        if log::log_enabled!(log::Level::Debug) {
            let resolved = if let Some(movie) = &movie {
                format!(
                    "LiveCinema {}x{} / {} frames",
                    movie.width, movie.height, movie.duration_frames
                )
            } else if src.is_empty() {
                "-".to_string()
            } else {
                self.assets
                    .data()
                    .resolve(&src, IMAGE_EXTS)
                    .unwrap_or_else(|| "NOT FOUND".into())
            };
            let g = |p: u8| {
                props
                    .get(&p)
                    .map(|v| v.as_int().to_string())
                    .unwrap_or_else(|| "-".into())
            };
            log::debug!(
                "{} {name:?} parent={:?} (exists: {}) src={src:?} -> {resolved} pos=({},{}) size={}x{} prio={} alpha={} visible={}",
                c.name(),
                props
                    .get(&Prop::PARENT)
                    .map(|v| v.as_str())
                    .unwrap_or_default(),
                props
                    .get(&Prop::PARENT)
                    .map(|v| v.as_str())
                    .filter(|p| !p.is_empty())
                    .map(|p| p == ROOT || self.scene.get(&p).is_some())
                    .unwrap_or(true),
                g(Prop::LEFT),
                g(Prop::TOP),
                g(Prop::WIDTH),
                g(Prop::HEIGHT),
                g(Prop::PRIORITY),
                g(Prop::ALPHA),
                g(Prop::VISIBLE)
            );
            if resolved == "NOT FOUND" && movie.is_none() {
                log::warn!("asset not found: {src:?} for {name}");
            }
        }
        if matches!(kind, CmdType::ImgNew | CmdType::Button | CmdType::MapImgNew) && !src.is_empty()
        {
            self.watch_anim(&name, &src);
        }
        if kind == CmdType::Cinema && movie.is_none() {
            // Sin pista de imagen LiveCinema compatible, queda el placeholder temporizado.
            let d = props
                .get(&Prop::DURATION)
                .map(|v| v.as_f64())
                .filter(|d| *d > 0.0)
                .unwrap_or(self.opts.cinema_ms);
            self.cinemas
                .insert(name.clone(), self.now + if self.skip { 0.0 } else { d });
            self.warn(
                "unimpl:cmd Cinema (timed placeholder, no video decode)",
                "Cinema: video not decoded; timed placeholder",
            );
        }
        if kind == CmdType::Timer {
            let d = props
                .get(&Prop::DELAY)
                .map(|v| v.as_f64())
                .unwrap_or(0.0)
                .max(1.0);
            let paused = props.get(&Prop::PAUSED).is_some_and(Value::truthy);
            self.timers.insert(
                name.clone(),
                TimerState {
                    due_at: self.now + d,
                    paused_at: paused.then_some(self.now),
                },
            );
        } else {
            self.timers.remove(&name);
        }
        if matches!(
            kind,
            CmdType::ParticleNew | CmdType::FireNew | CmdType::MemoNew | CmdType::WaveNew
        ) {
            self.warn(
                &format!("unimpl:cmd {} (placeholder)", c.name()),
                &format!("{} only creates a placeholder object", c.name()),
            );
        }
        if log::log_enabled!(log::Level::Debug) {
            for (p, v) in &props {
                if crate::lsb::prop_name(*p).starts_with("ON") {
                    log::debug!(
                        "event prop: {} {name}.{} (id {}) = {:?}",
                        c.name(),
                        crate::lsb::prop_name(*p),
                        p,
                        v.as_str()
                    );
                }
            }
        }
        self.videos.remove(&name);
        if !src.is_empty() {
            if let Some((w, h)) = self.slider_asset_size(&src) {
                props.entry(Prop::ORIGWIDTH).or_insert(Value::Int(w as i64));
                props
                    .entry(Prop::ORIGHEIGHT)
                    .or_insert(Value::Int(h as i64));
                props.entry(Prop::WIDTH).or_insert(Value::Int(w as i64));
                props.entry(Prop::HEIGHT).or_insert(Value::Int(h as i64));
            }
        }
        self.scene.create(&name, Kind::from_cmd(c.kind), props);
        if kind == CmdType::PrevMenuNew {
            self.load_prevmenu(&name, &src);
        }
        if let Some(movie) = movie {
            self.start_lcm_movie(&name, movie);
        }
        self.object_defined(&name);
        detail
    }
}
