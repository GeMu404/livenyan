//! Intérprete de scripts LSB.
//!
//! El VM es independiente de SDL: el bucle principal (o el modo `--trace`) le da
//! la hora actual y la entrada; el VM devuelve eventos de audio/vídeo.

use crate::assets::{Assets, ALL_EXTS, IMAGE_EXTS, SOUND_EXTS};
use crate::lsb::expr::{eval, Env, Value};
use crate::lsb::names::Func;
use crate::lsb::tpword::{brk, Glyph};
use crate::lsb::{CmdType, Command, LiveParser, Lsb, Param, Prop};
use crate::scene::{prop_of, Kind, Motion, Scene, TextItem};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Eventos hacia el host (audio, vídeo…).
#[derive(Debug, Clone)]
pub enum Event {
    PlaySound { channel: String, file: String, volume: f32, repeat: bool },
    StopSound { channel: String },
    MovieSkipped { file: String },
}

#[derive(Debug, Clone)]
pub enum State {
    Running,
    WaitUntil { until: f64, skippable: bool },
    /// Mostrando texto progresivamente.
    Text { target: String, items: Vec<TextItem>, pos: usize, next: f64, then: Vec<Pending> },
    WaitClick { then: Vec<Pending> },
    Menu { choices: Vec<String>, var: String },
    Ended,
}

/// Acciones que quedan tras un clic dentro de un TextIns.
#[derive(Debug, Clone)]
pub enum Pending {
    Clear(String),
    Text(String, Vec<TextItem>),
    Click,
}

#[derive(Debug, Clone)]
struct Frame {
    script: Arc<Lsb>,
    pc: usize,
    locals: HashMap<String, Value>,
    result_var: String,
}

pub struct Options {
    pub text_cps: f32,
    pub lookahead: usize,
    pub trace: bool,
    pub show_text: bool,
}

pub struct Vm {
    assets: Arc<Assets>,
    frames: Vec<Frame>,
    globals: HashMap<String, Value>,
    pub scene: Scene,
    pub state: State,
    pub now: f64,
    pub opts: Options,
    pub events: Vec<Event>,
    pub stats: HashMap<&'static str, u64>,
    pub steps: u64,
    warned: HashSet<String>,
    rng: u64,
    prefetched_upto: HashMap<String, usize>,
    pub skip: bool,
}

fn redact(s: &str, show: bool) -> String {
    if show { format!("{s:?}") } else { format!("<{} chars>", s.chars().count()) }
}

impl Vm {
    pub fn new(assets: Arc<Assets>, entry: &str, opts: Options) -> Result<Self, String> {
        let script = assets.script(entry).ok_or_else(|| format!("cannot load entry script {entry}"))?;
        let mut vm = Self {
            assets,
            frames: vec![],
            globals: HashMap::new(),
            scene: Scene::default(),
            state: State::Running,
            now: 0.0,
            opts,
            events: vec![],
            stats: HashMap::new(),
            steps: 0,
            warned: HashSet::new(),
            rng: 0x9E37_79B9_7F4A_7C15,
            prefetched_upto: HashMap::new(),
            skip: false,
        };
        vm.prefetch_from(&script, 0);
        vm.frames.push(Frame { script, pc: 0, locals: HashMap::new(), result_var: String::new() });
        Ok(vm)
    }

    pub fn location(&self) -> String {
        match self.frames.last() {
            Some(f) => format!("{}:{}", f.script.name, f.pc),
            None => "<ended>".into(),
        }
    }

    fn warn(&mut self, key: &str, msg: &str) {
        if self.warned.insert(key.to_string()) {
            log::warn!("{msg} (first at {})", self.location());
        }
    }

    // ---------- prefetch ----------

    /// Escanea comandos por delante y encola recursos referenciados.
    fn prefetch_from(&mut self, script: &Arc<Lsb>, pc: usize) {
        let end = (pc + self.opts.lookahead).min(script.commands.len());
        let upto = self.prefetched_upto.get(&script.name).copied().unwrap_or(0);
        if end <= upto && pc < upto {
            return;
        }
        let start = pc.max(if pc < upto { upto } else { pc });
        let data = self.assets.data().clone();
        for c in &script.commands[start.min(end)..end] {
            for s in c.strings() {
                if s.len() > 2 && s.len() < 260 {
                    if let Some(n) = data.resolve(s, ALL_EXTS) {
                        self.assets.prefetch(&n);
                    }
                }
            }
            if let Some(p) = c.page() {
                if let Some(n) = crate::game::resolve_lsb(&data, &p.page) {
                    self.assets.prefetch(&n);
                }
            }
        }
        self.prefetched_upto.insert(script.name.clone(), end.max(upto));
    }

    // ---------- variables ----------

    fn lookup(&self, name: &str) -> Option<&Value> {
        self.frames.last().and_then(|f| f.locals.get(name)).or_else(|| self.globals.get(name))
    }

    pub fn var(&self, name: &str) -> Option<&Value> {
        self.lookup(name)
    }

    fn assign(&mut self, name: &str, v: Value) {
        if let Some(f) = self.frames.last_mut() {
            if let Some(slot) = f.locals.get_mut(name) {
                *slot = v;
                return;
            }
        }
        self.globals.insert(name.to_string(), v);
    }

    fn eval(&mut self, e: Option<&LiveParser>) -> Value {
        match e {
            Some(e) if !e.is_empty() => eval(e, self),
            _ => Value::Empty,
        }
    }

    fn eval_bool(&mut self, e: Option<&LiveParser>, default: bool) -> bool {
        match self.eval(e) {
            Value::Empty => default,
            v => v.truthy(),
        }
    }

    // ---------- control externo ----------

    pub fn ended(&self) -> bool {
        matches!(self.state, State::Ended)
    }

    /// Clic / avance del usuario.
    pub fn click(&mut self) {
        match std::mem::replace(&mut self.state, State::Running) {
            State::Text { target, items, then, .. } => {
                // completa el texto actual de golpe
                if let Some(o) = self.scene.get_mut(&target) {
                    o.text.extend(items);
                    o.shown = o.text.len();
                    o.text_rev += 1;
                }
                self.state = State::Running;
                self.continue_pending(then);
            }
            State::WaitClick { then } => self.continue_pending(then),
            State::WaitUntil { until, skippable } => {
                if !skippable {
                    self.state = State::WaitUntil { until, skippable };
                }
            }
            other => self.state = other,
        }
    }

    /// Elección de menú (índice).
    pub fn choose(&mut self, i: usize) {
        if let State::Menu { choices, var } = &self.state {
            let v = choices.get(i).cloned().unwrap_or_default();
            let var = var.clone();
            if self.opts.trace {
                println!("  -> menu choice #{i}{}", if self.opts.show_text { format!(" {v:?}") } else { String::new() });
            }
            self.assign(&var, Value::Str(v));
            self.state = State::Running;
        }
    }

    fn continue_pending(&mut self, mut then: Vec<Pending>) {
        while !then.is_empty() {
            match then.remove(0) {
                Pending::Clear(t) => {
                    if let Some(o) = self.scene.get_mut(&t) {
                        o.text.clear();
                        o.shown = 0;
                        o.text_rev += 1;
                    }
                }
                Pending::Text(t, items) => {
                    self.start_text(t, items, then);
                    return;
                }
                Pending::Click => {
                    self.state = State::WaitClick { then };
                    return;
                }
            }
        }
    }

    fn start_text(&mut self, target: String, items: Vec<TextItem>, then: Vec<Pending>) {
        if self.opts.text_cps <= 0.0 || self.skip || items.is_empty() {
            if let Some(o) = self.scene.get_mut(&target) {
                o.text.extend(items);
                o.shown = o.text.len();
                o.text_rev += 1;
            }
            self.continue_pending(then);
        } else {
            self.state = State::Text { target, items, pos: 0, next: self.now, then };
        }
    }

    /// Avanza el reloj y ejecuta hasta bloquearse (o `budget` comandos).
    pub fn update(&mut self, now: f64, budget: usize) {
        self.now = now;
        self.scene.update(now);
        // texto progresivo
        if let State::Text { target, items, pos, next, then } = &mut self.state {
            let cps = self.opts.text_cps.max(1.0) as f64;
            while *pos < items.len() && *next <= now {
                *pos += 1;
                *next += 1000.0 / cps;
            }
            if *pos >= items.len() {
                let (t, it, th) = (target.clone(), std::mem::take(items), std::mem::take(then));
                if let Some(o) = self.scene.get_mut(&t) {
                    o.text.extend(it);
                    o.shown = o.text.len();
                    o.text_rev += 1;
                }
                self.state = State::Running;
                self.continue_pending(th);
            }
        }
        if let State::WaitUntil { until, .. } = self.state {
            if now >= until || self.skip {
                self.state = State::Running;
            }
        }
        if self.skip {
            if let State::WaitClick { .. } = self.state {
                self.click();
            }
        }
        let mut n = 0;
        while matches!(self.state, State::Running) && n < budget {
            self.step();
            n += 1;
        }
    }

    /// Texto parcialmente visible del TextIns en curso (para el renderizador).
    pub fn partial_text(&self) -> Option<(&str, &[TextItem])> {
        match &self.state {
            State::Text { target, items, pos, .. } => Some((target.as_str(), &items[..*pos])),
            _ => None,
        }
    }

    // ---------- ejecución ----------

    fn goto_script(&mut self, page: &str, label: u32, push: bool, result_var: String, params: Vec<Value>) -> bool {
        let data = self.assets.data().clone();
        let cur = self.frames.last().map(|f| f.script.clone());
        let script = match (&cur, page.is_empty()) {
            (Some(c), true) => c.clone(),
            _ => {
                let Some(name) = crate::game::resolve_lsb(&data, page) else {
                    self.warn(&format!("missing:{page}"), &format!("script not found: {page}"));
                    return false;
                };
                if cur.as_ref().is_some_and(|c| c.name == name) {
                    cur.clone().unwrap()
                } else {
                    match self.assets.script(&name) {
                        Some(s) => s,
                        None => return false,
                    }
                }
            }
        };
        let pc = (label as usize).min(script.commands.len());
        self.prefetch_from(&script, pc);
        if push {
            let mut locals = HashMap::new();
            // Convención provisional: parámetros como Param1..N (no verificado con juegos reales).
            for (i, p) in params.into_iter().enumerate() {
                locals.insert(format!("Param{}", i + 1), p);
            }
            self.frames.push(Frame { script, pc, locals, result_var });
        } else if let Some(f) = self.frames.last_mut() {
            f.script = script;
            f.pc = pc;
        }
        true
    }

    /// Resuelve un destino de bucle: índice de comando (como en pylivemaker) y,
    /// si no cuadra con el tipo esperado, se intenta como LineNo.
    fn loop_target(script: &Lsb, v: u32, want: CmdType) -> usize {
        let i = v as usize;
        if script.commands.get(i).is_some_and(|c| CmdType::from(c.kind) == want) {
            return i;
        }
        script.index_of_line(v).unwrap_or(i)
    }

    fn props_of(&mut self, c: &Command) -> HashMap<u8, Value> {
        let mut m = HashMap::new();
        for (p, e) in c.props() {
            if !e.is_empty() {
                let v = eval(e, self);
                m.insert(*p, v);
            }
        }
        m
    }

    fn trace(&self, c: &Command, detail: String) {
        if self.opts.trace {
            let f = self.frames.last().unwrap();
            println!(
                "[{:>8.0}ms] {}:{:<4} L{:<5} {}{}{}",
                self.now,
                f.script.name,
                f.pc,
                c.line_no,
                "  ".repeat(c.indent.min(12) as usize),
                c.name(),
                if detail.is_empty() { String::new() } else { format!(" {detail}") }
            );
        }
    }

    fn step(&mut self) {
        let Some(f) = self.frames.last() else {
            self.state = State::Ended;
            return;
        };
        let script = f.script.clone();
        let pc = f.pc;
        if pc >= script.commands.len() {
            // fin de script sin Exit: retorna
            self.do_return(Value::Empty);
            return;
        }
        if pc % 32 == 0 {
            self.prefetch_from(&script, pc);
        }
        let c = &script.commands[pc];
        self.steps += 1;
        *self.stats.entry(c.name()).or_default() += 1;
        let mut next = pc + 1;
        let kind = CmdType::from(c.kind);
        let mut detail = String::new();
        let show = self.opts.show_text;
        match kind {
            CmdType::Calc => {
                if self.opts.trace {
                    detail = c.expr("Calc").map(|e| e.render(!show)).unwrap_or_default();
                }
                self.eval(c.expr("Calc"));
            }
            CmdType::If => {
                let ok = self.eval_bool(c.expr("Calc"), false);
                detail = format!("-> {ok}");
                if !ok {
                    next = self.next_branch(&script, pc);
                }
            }
            CmdType::Elseif | CmdType::Else => {
                // Llegar aquí en flujo normal = la rama anterior se ejecutó: saltar el resto.
                next = self.skip_chain(&script, pc);
            }
            CmdType::Label | CmdType::Comment => {
                if self.opts.trace && kind == CmdType::Label {
                    detail = c.str("Name").unwrap_or("").to_string();
                }
            }
            CmdType::Jump => {
                if self.eval_bool(c.expr("Calc"), true) {
                    let p = c.page().cloned().unwrap_or_default();
                    detail = format!("-> {}:{}", p.page, p.label);
                    self.trace(c, detail);
                    if !self.goto_script(&p.page, p.label, false, String::new(), vec![]) {
                        if let Some(f) = self.frames.last_mut() {
                            f.pc = next;
                        }
                    }
                    return;
                }
            }
            CmdType::Call => {
                if self.eval_bool(c.expr("Calc"), true) {
                    let p = c.page().cloned().unwrap_or_default();
                    let params: Vec<Value> = c.exprs("Params").to_vec().iter().map(|e| eval(e, self)).collect();
                    detail = format!("-> {}:{} ({} params)", p.page, p.label, params.len());
                    self.trace(c, detail);
                    if let Some(f) = self.frames.last_mut() {
                        f.pc = next;
                    }
                    if self.frames.len() > 256 {
                        self.warn("stack", "call stack overflow (>256), ignoring Call");
                        return;
                    }
                    self.goto_script(&p.page, p.label, true, c.str("Result").unwrap_or("").to_string(), params);
                    return;
                }
            }
            CmdType::Exit => {
                let v = self.eval(c.expr("Calc"));
                self.trace(c, String::new());
                self.do_return(v);
                return;
            }
            CmdType::Terminate => {
                self.trace(c, String::new());
                self.state = State::Ended;
                return;
            }
            CmdType::Wait => {
                if self.eval_bool(c.expr("Calc"), true) {
                    let t = self.eval(c.expr("Time")).as_f64();
                    let skippable = self.eval_bool(c.expr("StopEvent"), true);
                    detail = format!("{t}ms");
                    if t > 0.0 {
                        self.state = State::WaitUntil { until: self.now + t, skippable };
                    }
                }
            }
            CmdType::VarNew => {
                let name = c.str("Name").unwrap_or("").to_string();
                let ty = c.u32("Type").unwrap_or(0) as u8;
                let scope = c.u32("Scope").unwrap_or(0);
                let mut v = self.eval(c.expr("InitVal"));
                if v == Value::Empty {
                    v = Value::default_for(ty);
                }
                detail = format!("{name} scope={scope}");
                if scope != 0 {
                    if let Some(f) = self.frames.last_mut() {
                        f.locals.insert(name, v);
                    }
                } else {
                    self.globals.insert(name, v);
                }
            }
            CmdType::VarDel => {
                let name = c.str("Name").unwrap_or("");
                detail = name.to_string();
                if let Some(f) = self.frames.last_mut() {
                    if f.locals.remove(name).is_none() {
                        self.globals.remove(name);
                    }
                }
            }
            CmdType::While => {
                let ok = self.eval_bool(c.expr("Calc"), true);
                if !ok {
                    next = Self::loop_target(&script, c.u32("End").unwrap_or(0), CmdType::WhileLoop) + 1;
                }
                detail = format!("-> {ok}");
            }
            CmdType::WhileInit => {
                self.eval(c.expr("Calc"));
            }
            CmdType::WhileLoop => {
                self.eval(c.expr("Calc"));
                next = Self::loop_target(&script, c.u32("Start").unwrap_or(0), CmdType::While);
            }
            CmdType::Break => {
                if self.eval_bool(c.expr("Calc"), true) {
                    next = Self::loop_target(&script, c.u32("End").unwrap_or(0), CmdType::WhileLoop) + 1;
                }
            }
            CmdType::Continue => {
                if self.eval_bool(c.expr("Calc"), true) {
                    next = Self::loop_target(&script, c.u32("Start").unwrap_or(0), CmdType::WhileLoop);
                }
            }
            CmdType::ImgNew
            | CmdType::MesNew
            | CmdType::BoxNew
            | CmdType::TileNew
            | CmdType::Button
            | CmdType::SliderNew
            | CmdType::ScrollbarNew
            | CmdType::GaugeNew
            | CmdType::PrevMenuNew
            | CmdType::MapImgNew
            | CmdType::Timer
            | CmdType::Caption
            | CmdType::CGCaption
            | CmdType::Cinema
            | CmdType::EditNew
            | CmdType::MemoNew
            | CmdType::ParticleNew
            | CmdType::FireNew
            | CmdType::WaveNew => {
                let props = self.props_of(c);
                let name = props.get(&Prop::NAME).map(|v| v.as_str()).unwrap_or_default();
                let src = props.get(&Prop::SOURCE).map(|v| v.as_str()).unwrap_or_default();
                if !src.is_empty() {
                    if let Some(n) = self.assets.data().resolve(&src, IMAGE_EXTS) {
                        self.assets.request(&n);
                    }
                }
                detail = format!("name={name:?}{}", if src.is_empty() { String::new() } else { format!(" src={src:?}") });
                if matches!(kind, CmdType::Timer | CmdType::Cinema | CmdType::ParticleNew | CmdType::FireNew | CmdType::EditNew | CmdType::MemoNew) {
                    self.warn(c.name(), &format!("{} only creates a placeholder object", c.name()));
                }
                self.scene.create(&name, Kind::from_cmd(c.kind), props);
            }
            CmdType::Menu => {
                let props = self.props_of(c);
                let name = props.get(&Prop::NAME).map(|v| v.as_str()).unwrap_or_default();
                let mut choices: Vec<String> = props
                    .get(&Prop::TEXT)
                    .map(|v| match v {
                        Value::Array(a) => a.iter().map(|x| x.as_str()).collect(),
                        v => v.as_str().split(['\n']).map(|s| s.trim_end_matches('\r').to_string()).filter(|s| !s.is_empty()).collect(),
                    })
                    .unwrap_or_default();
                if choices.is_empty() {
                    if let Some(Value::Array(a)) = self.lookup("_tmp") {
                        choices = a.iter().map(|x| x.as_str()).filter(|s| !s.trim().is_empty()).collect();
                    }
                }
                self.warn("Menu", "Menu: choices taken from PR_TEXT or _tmp; result stored in 選択値 (heuristic)");
                detail = format!("name={name:?} choices={}", choices.len());
                self.scene.create(&name, Kind::Menu, props);
                if !choices.is_empty() {
                    self.state = State::Menu { choices, var: "選択値".into() };
                }
            }
            CmdType::Sound => {
                let props = self.props_of(c);
                let ch = props.get(&Prop::NAME).map(|v| v.as_str()).unwrap_or_default();
                let src = props.get(&Prop::SOURCE).map(|v| v.as_str()).unwrap_or_default();
                let vol = props.get(&Prop::VOLUME).map(|v| v.as_f64()).unwrap_or(100.0);
                let repeat = props.get(&Prop::REPEAT).map(|v| v.truthy()).unwrap_or(false);
                detail = format!("ch={ch:?} src={src:?} vol={vol} repeat={repeat}");
                match self.assets.data().resolve(&src, SOUND_EXTS) {
                    Some(file) => {
                        self.assets.request(&file);
                        self.events.push(Event::PlaySound { channel: ch.clone(), file, volume: (vol / 100.0).clamp(0.0, 1.0) as f32, repeat })
                    }
                    None if src.is_empty() => self.events.push(Event::StopSound { channel: ch.clone() }),
                    None => self.warn(&format!("snd:{src}"), &format!("sound not found: {src}")),
                }
                self.scene.create(&ch, Kind::Other(c.kind), props);
            }
            CmdType::MediaPlay => {
                let t = self.eval(c.expr("Target")).as_str();
                detail = format!("target={t:?}");
                if let Some(o) = self.scene.get(&t) {
                    let src = o.str(Prop::SOURCE);
                    if let Some(file) = self.assets.data().resolve(&src, SOUND_EXTS) {
                        let vol = o.int(Prop::VOLUME, 100) as f32 / 100.0;
                        let repeat = o.get(Prop::REPEAT).is_some_and(|v| v.truthy());
                        self.events.push(Event::PlaySound { channel: t.clone(), file, volume: vol, repeat });
                    }
                }
            }
            CmdType::Movie => {
                let props = self.props_of(c);
                let src = props.get(&Prop::SOURCE).map(|v| v.as_str()).unwrap_or_default();
                detail = format!("src={src:?} (skipped)");
                self.warn("Movie", "Movie playback not implemented: skipping");
                self.events.push(Event::MovieSkipped { file: src });
            }
            CmdType::MovieStop => {
                self.warn("MovieStop", "MovieStop: no-op (movies are skipped)");
            }
            CmdType::SetProp => {
                let obj = self.eval(c.expr("ObjName")).as_str();
                let pv = self.eval(c.expr("ObjProp"));
                let v = self.eval(c.expr("Value"));
                let p = prop_of(&pv);
                detail = format!("{obj:?}.{}", p.map(crate::lsb::prop_name).unwrap_or("?"));
                match p {
                    Some(p) => {
                        if p == Prop::SOURCE {
                            if let Some(n) = self.assets.data().resolve(&v.as_str(), IMAGE_EXTS) {
                                self.assets.request(&n);
                            }
                        }
                        if !self.scene.set(&obj, p, v) {
                            self.warn(&format!("noobj:{obj}"), &format!("SetProp on missing object {obj:?}"));
                        }
                    }
                    None => self.warn("setprop-bad", "SetProp with unknown property"),
                }
            }
            CmdType::GetProp => {
                let obj = self.eval(c.expr("ObjName")).as_str();
                let pv = self.eval(c.expr("ObjProp"));
                let var = c.str("VarName").unwrap_or("").to_string();
                let v = prop_of(&pv).and_then(|p| self.scene.get(&obj).and_then(|o| o.get(p).cloned())).unwrap_or(Value::Int(0));
                detail = format!("{obj:?} -> {var}");
                self.assign(&var, v);
            }
            CmdType::ObjDel => {
                let name = self.eval(c.expr("Name")).as_str();
                let n = self.scene.delete(&name);
                detail = format!("{name:?} ({n} objs)");
            }
            CmdType::PropMotion => {
                let obj = self.eval(c.expr("ObjName")).as_str();
                let pv = self.eval(c.expr("ObjProp"));
                let to = self.eval(c.expr("Value")).as_f64();
                let dur = self.eval(c.expr("Time")).as_f64();
                if let Some(p) = prop_of(&pv) {
                    let from = self.scene.get(&obj).map(|o| o.int(p, 0) as f64).unwrap_or(0.0);
                    detail = format!("{obj:?}.{} {from}->{to} in {dur}ms", crate::lsb::prop_name(p));
                    self.scene.add_motion(Motion { obj, prop: p, from, to, start: self.now, dur });
                }
            }
            CmdType::TextIns => {
                let target = self.eval(c.expr("Target")).as_str();
                let tw = c.text().cloned().unwrap_or_default();
                detail = format!("target={target:?} chars={}", tw.char_count());
                if show {
                    detail += &format!(" text={}", redact(&tw.plain_text(), true));
                }
                self.trace(c, detail);
                if let Some(f) = self.frames.last_mut() {
                    f.pc = next;
                }
                self.run_text(&target, &tw);
                return;
            }
            CmdType::TextClr => {
                let t = self.eval(c.expr("Target")).as_str();
                if let Some(o) = self.scene.get_mut(&t) {
                    o.text.clear();
                    o.shown = 0;
                    o.text_rev += 1;
                }
            }
            CmdType::Flip => {
                // Transiciones: se aplican de forma instantánea por ahora.
                let t = self.eval(c.expr("Time")).as_f64();
                detail = format!("{t}ms (instant)");
                self.warn("Flip", "Flip transitions are applied instantly");
            }
            CmdType::DoEvent => {}
            CmdType::IFDEF | CmdType::IFNDEF | CmdType::ENDIF => {}
            _ => {
                self.warn(c.name(), &format!("unimplemented command {}", c.name()));
            }
        }
        self.trace(c, detail);
        if let Some(f) = self.frames.last_mut() {
            if Arc::ptr_eq(&f.script, &script) && f.pc == pc {
                f.pc = next;
            }
        }
    }

    fn do_return(&mut self, v: Value) {
        let f = self.frames.pop();
        if self.frames.is_empty() {
            self.state = State::Ended;
            return;
        }
        if let Some(f) = f {
            if !f.result_var.is_empty() {
                self.assign(&f.result_var, v);
            }
        }
    }

    /// If falso: busca la siguiente rama Elseif/Else del mismo nivel, o el final del bloque.
    fn next_branch(&mut self, s: &Lsb, pc: usize) -> usize {
        let ind = s.commands[pc].indent;
        let mut j = pc + 1;
        while j < s.commands.len() {
            let c = &s.commands[j];
            if c.indent <= ind {
                if c.indent == ind {
                    match CmdType::from(c.kind) {
                        CmdType::Elseif => {
                            *self.stats.entry("Elseif").or_default() += 1;
                            if self.eval_bool(c.expr("Calc"), false) {
                                return j + 1;
                            }
                            j += 1;
                            continue;
                        }
                        CmdType::Else => return j + 1,
                        _ => {}
                    }
                }
                return j;
            }
            j += 1;
        }
        j
    }

    /// Salta el resto de una cadena If/Elseif/Else ya resuelta.
    fn skip_chain(&self, s: &Lsb, pc: usize) -> usize {
        let ind = s.commands[pc].indent;
        let mut j = pc + 1;
        while j < s.commands.len() {
            let c = &s.commands[j];
            if c.indent < ind || (c.indent == ind && !matches!(CmdType::from(c.kind), CmdType::Elseif | CmdType::Else)) {
                return j;
            }
            j += 1;
        }
        j
    }

    fn run_text(&mut self, target: &str, tw: &crate::lsb::TpWord) {
        if self.scene.get(target).is_none() {
            self.warn(&format!("mes:{target}"), &format!("TextIns into missing message object {target:?}; creating one"));
            self.scene.create(target, Kind::Message, HashMap::new());
        }
        let mut plan: Vec<Pending> = vec![];
        let mut cur: Vec<TextItem> = vec![];
        let flush = |plan: &mut Vec<Pending>, cur: &mut Vec<TextItem>| {
            if !cur.is_empty() {
                plan.push(Pending::Text(target.to_string(), std::mem::take(cur)));
            }
        };
        for g in &tw.body {
            match g {
                Glyph::Char { ch, .. } => cur.push(TextItem::Char(ch.clone())),
                Glyph::Var { name, .. } => {
                    let v = self.lookup(name).cloned().unwrap_or_default().as_str();
                    cur.extend(v.chars().map(|c| TextItem::Char(c.to_string())));
                }
                Glyph::Return { kind } => match *kind {
                    brk::LINE => cur.push(TextItem::Newline),
                    brk::PAUSE => {
                        flush(&mut plan, &mut cur);
                        plan.push(Pending::Click);
                    }
                    brk::PAGE => {
                        flush(&mut plan, &mut cur);
                        plan.push(Pending::Click);
                        plan.push(Pending::Clear(target.to_string()));
                    }
                    _ => {
                        flush(&mut plan, &mut cur);
                        plan.push(Pending::Clear(target.to_string()));
                    }
                },
                Glyph::Event { .. } => self.warn("twd-event", "TextIns inline events are ignored"),
                Glyph::Img { .. } => self.warn("twd-img", "TextIns inline images are ignored"),
                _ => {}
            }
        }
        flush(&mut plan, &mut cur);
        self.state = State::Running;
        self.continue_pending(plan);
    }

    /// Clic en coordenadas lógicas: botones o avance.
    pub fn click_at(&mut self, x: f64, y: f64) {
        if let Some(b) = self.scene.hit_button(x, y) {
            let handler = b
                .get(Prop::ONLBTNUP)
                .or_else(|| b.get(Prop::ONLBTNDOWN))
                .or_else(|| b.get(Prop::ONSELECT))
                .map(|v| v.as_str())
                .unwrap_or_default();
            if !handler.is_empty() && matches!(self.state, State::Running | State::WaitClick { .. } | State::WaitUntil { .. }) {
                self.goto_script(&handler, 0, true, String::new(), vec![]);
                if !matches!(self.state, State::WaitClick { .. }) {
                    self.state = State::Running;
                }
                return;
            }
        }
        self.click();
    }

    pub fn counts_sorted(&self) -> Vec<(&'static str, u64)> {
        let mut v: Vec<_> = self.stats.iter().map(|(k, v)| (*k, *v)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        v
    }
}

impl Env for Vm {
    fn get_var(&mut self, name: &str) -> Value {
        match self.lookup(name) {
            Some(v) => v.clone(),
            None => {
                self.warn(&format!("undef:{name}"), &format!("read of undefined variable {name}"));
                Value::Empty
            }
        }
    }
    fn set_var(&mut self, name: &str, v: Value) {
        self.assign(name, v);
    }
    fn call_engine(&mut self, f: u8, a: &[Value], raw: &[Param]) -> Option<Value> {
        let s = |i: usize| a.get(i).map(|v| v.as_str()).unwrap_or_default();
        Some(match f {
            Func::GETPROP => {
                let p = prop_of(a.get(1)?)?;
                self.scene.get(&s(0)).and_then(|o| o.get(p).cloned()).unwrap_or(Value::Int(0))
            }
            Func::SETPROP => {
                let p = prop_of(a.get(1)?)?;
                Value::Bool(self.scene.set(&s(0), p, a.get(2).cloned().unwrap_or_default()))
            }
            Func::EXISTS => Value::Bool(self.scene.get(&s(0)).is_some()),
            Func::VAREXISTS => {
                let n = match raw.first() {
                    Some(Param::Var(n)) if !n.starts_with("____") => n.clone(),
                    _ => s(0),
                };
                Value::Bool(self.lookup(&n).is_some())
            }
            Func::GETVISIBLE => Value::Bool(self.scene.get(&s(0)).is_some_and(|o| o.visible())),
            Func::SETVISIBLE => Value::Bool(self.scene.set(&s(0), Prop::VISIBLE, Value::Bool(a.get(1).is_some_and(|v| v.truthy())))),
            Func::GETENABLED => Value::Bool(true),
            Func::INCABINET | Func::FILEEXISTS => Value::Bool(self.assets.data().find(&s(0)).is_some()),
            Func::GETHISTORYCOUNT => Value::Int(0),
            Func::GETDISPLAYMODE => Value::Int(0),
            _ => return None,
        })
    }
    fn random(&mut self) -> f64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 11) as f64 / (1u64 << 53) as f64
    }
    fn warn_once(&mut self, key: &str, msg: &str) {
        self.warn(key, msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::GameData;
    #[test]
    fn runs_fixture_to_end() {
        let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
        let data = GameData::open(&p).unwrap();
        let assets = Arc::new(Assets::new(data, 2, 64));
        let mut vm = Vm::new(assets, "ゲームメイン.lsb", Options { text_cps: 0.0, lookahead: 50, trace: false, show_text: false }).unwrap();
        let mut t = 0.0;
        for _ in 0..1000 {
            vm.update(t, 1000);
            if vm.ended() {
                break;
            }
            if matches!(vm.state, State::WaitClick { .. }) {
                vm.click();
            }
            t += 50.0;
        }
        assert!(vm.ended(), "state {:?}", vm.state);
        assert_eq!(vm.globals.get("count"), Some(&Value::Int(2)));
        assert_eq!(vm.globals.get("name"), Some(&Value::Str("3".into())));
        assert_eq!(vm.globals.get("ret"), Some(&Value::Int(42)));
        assert_eq!(vm.globals.get("subran"), Some(&Value::Int(1)));
        assert!(vm.scene.get("bg").is_none());
        assert!(vm.scene.get("msg").is_some());
        assert_eq!(vm.stats.get("Jump"), Some(&2));
        assert!(vm.events.iter().any(|e| matches!(e, Event::PlaySound { file, .. } if file == "se.wav")));
    }
}
