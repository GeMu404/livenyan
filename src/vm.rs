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

/// Nombre de la capa raíz (pantalla) en LiveMaker: el carácter 0x01. Siempre existe;
/// sus hijos son de nivel superior.
pub const ROOT: &str = "\u{1}";

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
    /// TextIns con Wait: espera a que el objeto de mensaje termine de reproducir.
    WaitText { target: String },
    Menu { choices: Vec<String>, var: String },
    Ended,
}

/// Elementos pendientes de la reproducción de texto de un objeto de mensaje.
#[derive(Debug, Clone)]
pub enum Pending {
    Clear,
    Text(Vec<TextItem>),
    Click,
    /// Evento en línea (TWdOpeEvent) -> ONNOTIFY.
    Notify(String),
}

/// Reproducción de texto en curso de un objeto de mensaje (no bloquea al VM salvo Wait).
#[derive(Debug, Clone, Default)]
pub struct Playback {
    plan: std::collections::VecDeque<Pending>,
    cur: Vec<TextItem>,
    pos: usize,
    next: f64,
    waiting_click: bool,
    /// Efecto WAIT: el texto espera a que termine la animación/fundido de este objeto
    /// (con plazo máximo de seguridad).
    wait_for: Option<(String, f64)>,
    /// Última etiqueta/evento procesado (solo nombre, para depurar).
    last_tag: String,
}

#[derive(Debug, Clone)]
struct Frame {
    script: Arc<Lsb>,
    pc: usize,
    locals: HashMap<String, Value>,
    result_var: String,
    /// Parámetros de la llamada (Call Params / evento), expuestos como @ParamStr[i] y @ParamCount.
    params: Vec<Value>,
    /// Para manejadores de evento: estado a restaurar al volver.
    saved_state: Option<State>,
    /// Manejadores de evento: objeto que lo disparó (@Sender).
    sender: Option<String>,
}

/// Botón de un PrevMenu, en coordenadas relativas al objeto.
#[derive(Debug, Clone)]
pub struct PmButton {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
    pub src: String,
    pub src_selected: String,
    pub name: String,
}

#[derive(Debug, Clone, Default)]
pub struct PrevMenu {
    pub buttons: Vec<PmButton>,
    pub hover: Option<usize>,
}

/// Evento de objeto encolado (se ejecuta como llamada-interrupción entre frames).
#[derive(Debug, Clone)]
pub struct ObjEvent {
    pub obj: String,
    pub prop: u8,
    pub script: String,
    pub params: Vec<Value>,
}

pub struct Options {
    pub screen: (u32, u32),
    /// Duración del placeholder de Cinema cuando no se sabe la del vídeo (ms).
    pub cinema_ms: f64,
    pub text_cps: f32,
    pub lookahead: usize,
    pub trace: bool,
    /// --trace-conds: valores de las condiciones If/Elseif/While en scripts de escenario.
    pub trace_conds: bool,
    /// --trace --pick N: ítem a elegir en menús.
    pub pick: Option<i64>,
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
    plays: HashMap<String, Playback>,
    pub queue: std::collections::VecDeque<ObjEvent>,
    yielded: bool,
    /// Funciones/comandos no implementados vistos (clave -> veces).
    pub unimpl: std::collections::BTreeMap<String, u64>,
    pub db: crate::db::Db,
    history_max: i64,
    /// Propiedades de la capa raíz/pantalla ("\x01" en LiveMaker).
    pub root_props: HashMap<u8, Value>,
    cabinets: HashMap<String, Vec<crate::scene::Obj>>,
    /// Temporizadores activos: objeto -> próximo disparo (ms).
    timers: HashMap<String, f64>,
    /// Animaciones GAL: objeto -> (archivo, inicio ms o None si aún no cargó).
    anims: HashMap<String, (String, Option<f64>)>,
    anim_clock: f64,
    /// Última caja que recibió TextIns (la "activa" para clics de pantalla).
    last_text_target: Option<String>,
    /// Último estado (tipo y propiedades) de objetos borrados, por nombre.
    graveyard: HashMap<String, (Kind, HashMap<u8, Value>)>,
    play_dbg: HashMap<String, f64>,
    /// Menús de previsualización (.lpm) por nombre de objeto.
    pub prevmenus: HashMap<String, PrevMenu>,
    /// LIVENYAN_CONDS=<parte del nombre de script>: condiciones a debug (también en GUI).
    cond_filter: Option<String>,
    /// Cinema en reproducción (placeholder temporizado): objeto -> fin (ms).
    cinemas: HashMap<String, f64>,
    /// Estado de ítems de PrevMenu/menús: (objeto, índice) -> valor.
    enabled: HashMap<(String, i64), bool>,
    selected: HashMap<(String, i64), bool>,
    /// Secuencia de scripts entrados (Call/Jump/eventos), para detectar ciclos en --trace.
    pub script_log: Vec<String>,
    /// Aristas Jump/Call "origen:pc -> destino:etiqueta" con su número de usos.
    pub edges: HashMap<String, u64>,
    cond_seen: HashMap<(usize, usize, bool), u8>,
    /// Posiciones (script, pc) distintas ejecutadas: para detectar bucles sin progreso.
    pub visited: std::collections::HashSet<(usize, usize)>,
    /// Objetos de mensaje creados implícitamente por TextIns: su texto espera al MesNew real.
    placeholders: std::collections::HashSet<String>,
    /// Objetos cuyo texto terminó sin manejador ONPLAYEND (se dispara si se asigna después).
    ended_unnotified: std::collections::HashSet<String>,
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
            plays: HashMap::new(),
            queue: Default::default(),
            yielded: false,
            unimpl: Default::default(),
            db: Default::default(),
            history_max: 0,
            root_props: HashMap::new(),
            cabinets: HashMap::new(),
            timers: HashMap::new(),
            anims: HashMap::new(),
            anim_clock: 0.0,
            last_text_target: None,
            graveyard: HashMap::new(),
            play_dbg: HashMap::new(),
            prevmenus: HashMap::new(),
            cond_filter: std::env::var("LIVENYAN_CONDS").ok().filter(|s| !s.is_empty()),
            cinemas: HashMap::new(),
            enabled: HashMap::new(),
            selected: HashMap::new(),
            script_log: vec![],
            edges: HashMap::new(),
            cond_seen: HashMap::new(),
            visited: Default::default(),
            placeholders: Default::default(),
            ended_unnotified: Default::default(),
        };
        vm.prefetch_from(&script, 0);
        vm.frames.push(Frame { script, pc: 0, locals: HashMap::new(), result_var: String::new(), params: vec![], saved_state: None, sender: None });
        Ok(vm)
    }

    pub fn location(&self) -> String {
        match self.frames.last() {
            Some(f) => format!("{}:{}", f.script.name, f.pc),
            None => "<ended>".into(),
        }
    }

    /// Firma de progreso para --trace: pila de scripts y número de comandos "con efecto"
    /// (no cuenta control de flujo, así un While de espera cuenta como inactivo).
    pub fn progress_sig(&self) -> (String, u64) {
        // Progreso = ejecutar algún comando nunca visto. Un bucle (de espera o de un
        // manejador) que repite las mismas posiciones cuenta como inactivo.
        let stack: Vec<&str> = self.frames.iter().map(|f| f.script.name.as_str()).collect();
        (stack.first().copied().unwrap_or_default().to_string(), self.visited.len() as u64)
    }

    fn warn(&mut self, key: &str, msg: &str) {
        if let Some(k) = key.strip_prefix("unimpl:") {
            *self.unimpl.entry(k.to_string()).or_default() += 1;
        }
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
                if s.len() > 2 && s.len() < 260
                    && let Some(n) = data.resolve(s, ALL_EXTS) {
                        self.assets.prefetch(&n);
                    }
            }
            if let Some(p) = c.page()
                && let Some(n) = crate::game::resolve_lsb(&data, &p.page) {
                    self.assets.prefetch(&n);
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
        if let Some(f) = self.frames.last_mut()
            && let Some(slot) = f.locals.get_mut(name) {
                *slot = v;
                return;
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

    /// Clic / avance del usuario: completa o libera el texto en reproducción.
    pub fn click(&mut self) {
        // 1) texto apareciendo -> mostrarlo entero
        let mut revealed = false;
        let names: Vec<String> = self.plays.keys().cloned().collect();
        for n in &names {
            let p = self.plays.get_mut(n).unwrap();
            if p.pos < p.cur.len() {
                let rest: Vec<TextItem> = p.cur[p.pos..].to_vec();
                p.pos = p.cur.len();
                if let Some(o) = self.scene.get_mut(n) {
                    o.text.extend(rest);
                    o.shown = o.text.len();
                    o.text_rev += 1;
                }
                revealed = true;
            }
        }
        if revealed {
            return;
        }
        // 2) esperando clic dentro del texto -> continuar
        let mut released = false;
        for p in self.plays.values_mut() {
            if p.waiting_click {
                p.waiting_click = false;
                released = true;
            }
        }
        if released {
            return;
        }
        if !self.cinemas.is_empty() {
            // clic salta los vídeos en curso
            for v in self.cinemas.values_mut() {
                *v = self.now;
            }
            return;
        }
        if let State::WaitUntil { skippable: true, .. } = self.state {
            self.state = State::Running;
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

    /// ¿Hay texto esperando un clic del usuario?
    pub fn wants_click(&self) -> bool {
        self.plays.values().any(|p| p.waiting_click || p.pos < p.cur.len())
    }

    /// Encola el manejador de evento `prop` del objeto, si lo tiene.
    pub fn fire(&mut self, obj: &str, prop: u8, params: Vec<Value>) -> bool {
        let script = self.scene.get(obj).map(|o| o.str(prop)).unwrap_or_default();
        if script.trim().is_empty() {
            log::debug!("event {obj}.{}: no handler", crate::lsb::prop_name(prop));
            return false;
        }
        log::debug!("event enqueue {obj}.{} -> {script} (queue {})", crate::lsb::prop_name(prop), self.queue.len() + 1);
        self.queue.push_back(ObjEvent { obj: obj.to_string(), prop, script, params });
        true
    }

    fn start_play(&mut self, target: &str, plan: Vec<Pending>) {
        let p = self.plays.entry(target.to_string()).or_default();
        p.plan.extend(plan);
        if p.next < self.now {
            p.next = self.now;
        }
    }

    /// Avanza la reproducción de texto de todos los objetos de mensaje.
    fn update_plays(&mut self) {
        let cps = if self.opts.text_cps <= 0.0 || self.skip { f64::INFINITY } else { self.opts.text_cps as f64 };
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
                let repeat = self.scene.get(&o).is_some_and(|x| x.get(Prop::REPEAT).is_some_and(|v| v.truthy()));
                let busy = self.scene.get(&o).is_some() && ((!repeat && self.anims.contains_key(&o)) || self.scene.motions.iter().any(|m| m.obj == o));
                if busy && self.now < deadline && !self.skip {
                    self.play_debug(&n, &format!("waiting-for-effect on {o:?}"));
                    continue;
                }
                log::debug!("text on {n}: WAIT on {o:?} done{}", if busy { " (deadline/skip)" } else { "" });
                if let Some(p) = self.plays.get_mut(&n) {
                    p.wait_for = None;
                }
            }
            if let Some(p) = self.plays.get(&n) {
                let st = if p.waiting_click {
                    "waiting-for-click".to_string()
                } else if p.next > self.now + 1.0 {
                    format!("timed wait {:.0} ms left", p.next - self.now)
                } else if p.pos < p.cur.len() {
                    format!("playing ({}/{} items of chunk, {} queued)", p.pos, p.cur.len(), p.plan.len())
                } else {
                    format!("between chunks ({} queued, last tag {:?})", p.plan.len(), p.last_tag)
                };
                self.play_debug(&n, &st);
            }
            loop {
                let Some(p) = self.plays.get_mut(&n) else { break };
                if p.next > self.now + 1.0 && p.pos >= p.cur.len() {
                    break; // espera temporizada (WAIT de efecto con tiempo)
                }
                if p.waiting_click && !self.skip {
                    let hidden = self.scene.get(&n).is_none_or(|o| !self.scene.effective_visible(o));
                    if hidden {
                        log::debug!("text on {n}: click-wait skipped (message box hidden)");
                        if let Some(p) = self.plays.get_mut(&n) {
                            p.waiting_click = false;
                        }
                        continue;
                    }
                }
                let Some(p) = self.plays.get_mut(&n) else { break };
                if p.waiting_click {
                    if self.skip {
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
                    }
                    if more {
                        break;
                    }
                    continue;
                }
                match p.plan.pop_front() {
                    Some(Pending::Text(items)) => {
                        p.cur = items;
                        p.pos = 0;
                        if p.next < self.now {
                            p.next = self.now;
                        }
                    }
                    Some(Pending::Click) => {
                        p.last_tag = "click-wait".into();
                        p.waiting_click = true
                    }
                    Some(Pending::Notify(e)) => {
                        p.last_tag = e.trim_start_matches('\x01').split("\r\n").next().unwrap_or("").to_string();
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
                    }
                    None => {
                        finished = true;
                        break;
                    }
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
                let parts: Vec<Value> = raw.split("\r\n").map(|x| Value::Str(x.to_string())).collect();
                log::debug!("text event on {n}: {:?} ({} params) -> ONNOTIFY", parts.first().map(|v| v.as_str()), parts.len());
                if !self.fire(&n, Prop::ONNOTIFY, parts) {
                    self.warn("notify-nohandler", "text <EVENT> but the message box has no ONNOTIFY handler");
                }
            }
            if finished {
                self.plays.remove(&n);
                log::debug!("playback end on {n}");
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

    /// Timer: dispara su manejador cada PR_DELAY ms (una vez salvo PR_REPEAT).
    /// Qué propiedad de evento usa el Timer original no está documentado: se prueba
    /// ONPLAYEND, luego ONNOTIFY, ONPLAYING e ONIDLE.
    fn update_timers(&mut self) {
        let due: Vec<String> = self.timers.iter().filter(|(_, t)| **t <= self.now).map(|(n, _)| n.clone()).collect();
        for n in due {
            let Some(o) = self.scene.get(&n) else {
                self.timers.remove(&n);
                continue;
            };
            if o.get(Prop::PAUSED).is_some_and(|v| v.truthy()) {
                continue;
            }
            let delay = o.get(Prop::DELAY).map(|v| v.as_f64()).unwrap_or(0.0).max(1.0);
            let repeat = o.get(Prop::REPEAT).is_some_and(|v| v.truthy());
            for p in [Prop::ONPLAYEND, Prop::ONNOTIFY, Prop::ONPLAYING, Prop::ONIDLE] {
                if self.fire(&n, p, vec![]) {
                    break;
                }
            }
            if repeat {
                self.timers.insert(n, self.now + delay);
            } else {
                self.timers.remove(&n);
            }
        }
    }

    /// Registra un objeto de imagen para animar si su GAL tiene varios frames.
    fn watch_anim(&mut self, obj: &str, src: &str) {
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
    fn update_anims(&mut self) {
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
            let t = if repeat { el.rem_euclid(total) } else { el.min(total - 0.001) };
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

    fn update_cinemas(&mut self) {
        let done: Vec<String> = self.cinemas.iter().filter(|(_, t)| **t <= self.now || self.skip).map(|(n, _)| n.clone()).collect();
        for n in done {
            self.cinemas.remove(&n);
            if let Some(o) = self.scene.get_mut(&n) {
                o.props.insert(Prop::PLAYING, Value::Bool(false));
            }
            self.fire(&n, Prop::ONPLAYEND, vec![]);
        }
    }

    /// Ruta de scripts que se repite al final de la ejecución: (periodo, repeticiones).
    pub fn cycle(&self) -> Option<(usize, usize)> {
        let l = &self.script_log;
        for p in 1..=400.min(l.len() / 3) {
            let tail = &l[l.len() - p..];
            let mut reps = 1;
            while (reps + 1) * p <= l.len() && &l[l.len() - (reps + 1) * p..l.len() - reps * p] == tail {
                reps += 1;
            }
            if reps >= 3 {
                return Some((p, reps));
            }
        }
        None
    }

    /// Ejecuta manejadores encolados como llamadas-interrupción.
    fn dispatch_events(&mut self) {
        if matches!(self.state, State::Menu { .. } | State::Ended) || self.frames.len() > 200 {
            return;
        }
        // un manejador a la vez: si ya hay uno activo, esperar a que termine
        if self.frames.iter().any(|f| f.saved_state.is_some()) && matches!(self.state, State::Running) {
            return;
        }
        if let Some(ev) = self.queue.pop_front() {
            log::debug!("event dispatch {}.{} -> {} (depth {})", ev.obj, crate::lsb::prop_name(ev.prop), ev.script, self.frames.len());
            if self.opts.trace {
                println!("[{:>8.0}ms] event {}.{} -> {}", self.now, ev.obj, crate::lsb::prop_name(ev.prop), ev.script);
            }
            let saved = std::mem::replace(&mut self.state, State::Running);
            if !self.goto_script(&ev.script, 0, true, String::new(), ev.params) {
                self.state = saved;
                return;
            }
            if let Some(f) = self.frames.last_mut() {
                f.saved_state = Some(saved);
                f.sender = Some(ev.obj.clone());
            }
        }
    }

    /// Avanza un frame: reloj, animaciones, texto, eventos y ejecución
    /// cooperativa (un While/Wait 0 cede el resto del frame).
    pub fn update(&mut self, now: f64, budget: usize) {
        self.now = now;
        self.scene.update(now);
        self.update_plays();
        if let State::WaitUntil { until, .. } = self.state {
            if now >= until || self.skip {
                self.state = State::Running;
            }
        }
        self.update_timers();
        self.update_anims();
        self.update_cinemas();
        self.dispatch_events();
        self.yielded = false;
        let mut n = 0;
        while matches!(self.state, State::Running) && n < budget && !self.yielded {
            self.step();
            n += 1;
            if !self.queue.is_empty() && self.frames.iter().all(|f| f.saved_state.is_none()) {
                self.dispatch_events();
            }
        }
        // el texto puede haber empezado en este frame
        self.update_plays();
    }

    // ---------- ejecución ----------

    fn goto_script(&mut self, page: &str, label: u32, push: bool, result_var: String, params: Vec<Value>) -> bool {
        // manejadores "script.lsc:ETIQUETA": empiezan en el comando Label con ese nombre
        let mut label = label;
        let mut named: Option<String> = None;
        let page = match page.rsplit_once(':') {
            Some((a, l)) if !l.is_empty() && !l.contains(['\\', '/', '.']) && [".lsc", ".lsb"].iter().any(|x| a.to_lowercase().ends_with(x)) => {
                named = Some(l.to_string());
                a
            }
            _ => page,
        };
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
        if let Some(l) = &named {
            match script.commands.iter().position(|c| CmdType::from(c.kind) == CmdType::Label && c.str("Name") == Some(l.as_str())) {
                Some(i) => label = i as u32,
                None => {
                    self.warn(&format!("label:{}:{l}", script.name), &format!("label {l:?} not found in {}", script.name));
                }
            }
        }
        log::debug!("enter {}:{label}{} ({})", script.name, named.as_ref().map(|l| format!(" (label {l:?})")).unwrap_or_default(), if push { "call" } else { "jump" });
        let pc = (label as usize).min(script.commands.len());
        if self.script_log.len() < 1_000_000 {
            self.script_log.push(script.name.clone());
        }
        self.prefetch_from(&script, pc);
        if push {
            let mut locals = HashMap::new();
            // Convención provisional: parámetros como Param1..N (no verificado con juegos reales).
            for (i, p) in params.iter().enumerate() {
                locals.insert(format!("Param{}", i + 1), p.clone());
            }
            self.frames.push(Frame { script, pc, locals, result_var, params, saved_state: None, sender: None });
        } else if let Some(f) = self.frames.last_mut() {
            f.script = script;
            f.pc = pc;
        }
        true
    }

    /// Resuelve un destino de bucle: índice de comando (como en pylivemaker) y,
    /// si no cuadra con el tipo esperado, se intenta como LineNo.
    /// Destino de salida de un While/Break: pylivemaker guarda en End el índice del comando
    /// siguiente al WhileLoop (no el WhileLoop). Se aceptan ambas formas y, si no, se busca
    /// estructuralmente el WhileLoop cuyo Start apunta a este While.
    fn loop_exit(script: &Lsb, v: u32, while_pc: Option<usize>) -> usize {
        let i = v as usize;
        let is_wl = |k: usize| script.commands.get(k).is_some_and(|c| CmdType::from(c.kind) == CmdType::WhileLoop);
        if is_wl(i) {
            return i + 1;
        }
        if i > 0 && is_wl(i - 1) {
            return i;
        }
        if let Some(w) = while_pc {
            if let Some(k) = (w + 1..script.commands.len())
                .find(|&k| is_wl(k) && script.commands[k].u32("Start").map(|s| Self::loop_target(script, s, CmdType::While)) == Some(w))
            {
                return k + 1;
            }
        }
        Self::loop_target(script, v, CmdType::WhileLoop) + 1
    }

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
        self.visited.insert((Arc::as_ptr(&script) as *const u8 as usize, pc));
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
                self.cond_trace(&script, pc, c, ok);
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
                    *self.edges.entry(format!("Jump {}:{} -> {}:{}", script.name, pc, p.page, p.label)).or_default() += 1;
                    self.trace(c, detail);
                    if !self.goto_script(&p.page, p.label, false, String::new(), vec![])
                        && let Some(f) = self.frames.last_mut() {
                            f.pc = next;
                        }
                    return;
                }
            }
            CmdType::Call => {
                if self.eval_bool(c.expr("Calc"), true) {
                    let p = c.page().cloned().unwrap_or_default();
                    let params: Vec<Value> = c.exprs("Params").to_vec().iter().map(|e| eval(e, self)).collect();
                    detail = format!("-> {}:{} ({} params)", p.page, p.label, params.len());
                    *self.edges.entry(format!("Call {}:{} -> {}:{}", script.name, pc, p.page, p.label)).or_default() += 1;
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
                // Exit es condicional (pylivemaker: "Return if Calc evaluates to True");
                // Calc vacío = salida incondicional.
                let v = self.eval(c.expr("Calc"));
                let go = matches!(v, Value::Empty) || v.truthy();
                if go {
                    self.trace(c, String::new());
                    self.do_return(v);
                    return;
                }
                detail = "(not taken)".into();
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
                    } else {
                        self.yielded = true;
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
                if let Some(f) = self.frames.last_mut()
                    && f.locals.remove(name).is_none() {
                        self.globals.remove(name);
                    }
            }
            CmdType::While => {
                let ok = self.eval_bool(c.expr("Calc"), true);
                self.cond_trace(&script, pc, c, ok);
                if !ok {
                    next = Self::loop_exit(&script, c.u32("End").unwrap_or(0), Some(pc));
                }
                detail = format!("-> {ok}");
            }
            CmdType::WhileInit => {
                self.eval(c.expr("Calc"));
            }
            CmdType::WhileLoop => {
                self.eval(c.expr("Calc"));
                next = Self::loop_target(&script, c.u32("Start").unwrap_or(0), CmdType::While);
                // LiveMaker es cooperativo: cada vuelta cede el frame (eventos, texto, timers)
                self.yielded = true;
            }
            CmdType::Break => {
                if self.eval_bool(c.expr("Calc"), true) {
                    next = Self::loop_exit(&script, c.u32("End").unwrap_or(0), None);
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
                if !src.is_empty()
                    && let Some(n) = self.assets.data().resolve(&src, IMAGE_EXTS) {
                        self.assets.request(&n);
                    }
                detail = format!("name={name:?}{}", if src.is_empty() { String::new() } else { format!(" src={src:?}") });
                if log::log_enabled!(log::Level::Debug) {
                    let resolved = if src.is_empty() { "-".to_string() } else { self.assets.data().resolve(&src, IMAGE_EXTS).unwrap_or_else(|| "NOT FOUND".into()) };
                    let g = |p: u8| props.get(&p).map(|v| v.as_int().to_string()).unwrap_or_else(|| "-".into());
                    log::debug!(
                        "{} {name:?} parent={:?} (exists: {}) src={src:?} -> {resolved} pos=({},{}) size={}x{} prio={} alpha={} visible={}",
                        c.name(),
                        props.get(&Prop::PARENT).map(|v| v.as_str()).unwrap_or_default(),
                        props.get(&Prop::PARENT).map(|v| v.as_str()).filter(|p| !p.is_empty()).map(|p| p == ROOT || self.scene.get(&p).is_some()).unwrap_or(true),
                        g(Prop::LEFT), g(Prop::TOP), g(Prop::WIDTH), g(Prop::HEIGHT), g(Prop::PRIORITY), g(Prop::ALPHA), g(Prop::VISIBLE)
                    );
                    if resolved == "NOT FOUND" {
                        log::warn!("asset not found: {src:?} for {name}");
                    }
                }
                if matches!(kind, CmdType::ImgNew | CmdType::Button | CmdType::MapImgNew) && !src.is_empty() {
                    self.watch_anim(&name, &src);
                }
                if kind == CmdType::Cinema {
                    // Sin decodificador de vídeo: placeholder temporizado (PR_DURATION o
                    // opción cinema_ms), saltable con clic; al terminar dispara ONPLAYEND.
                    let d = props.get(&Prop::DURATION).map(|v| v.as_f64()).filter(|d| *d > 0.0).unwrap_or(self.opts.cinema_ms);
                    self.cinemas.insert(name.clone(), self.now + if self.skip { 0.0 } else { d });
                    self.warn("unimpl:cmd Cinema (timed placeholder, no video decode)", "Cinema: video not decoded; timed placeholder");
                }
                if kind == CmdType::Timer {
                    let d = props.get(&Prop::DELAY).map(|v| v.as_f64()).unwrap_or(0.0).max(1.0);
                    self.timers.insert(name.clone(), self.now + d);
                }
                if matches!(kind, CmdType::ParticleNew | CmdType::FireNew | CmdType::EditNew | CmdType::MemoNew) {
                    self.warn(&format!("unimpl:cmd {} (placeholder)", c.name()), &format!("{} only creates a placeholder object", c.name()));
                }
                if log::log_enabled!(log::Level::Debug) {
                    for (p, v) in &props {
                        if crate::lsb::prop_name(*p).starts_with("ON") {
                            log::debug!("event prop: {} {name}.{} (id {}) = {:?}", c.name(), crate::lsb::prop_name(*p), p, v.as_str());
                        }
                    }
                }
                self.scene.create(&name, Kind::from_cmd(c.kind), props);
                if kind == CmdType::PrevMenuNew {
                    self.load_prevmenu(&name, &src);
                }
                self.object_defined(&name);
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
                if choices.is_empty()
                    && let Some(Value::Array(a)) = self.lookup("_tmp") {
                        choices = a.iter().map(|x| x.as_str()).filter(|s| !s.trim().is_empty()).collect();
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
                self.warn("unimpl:cmd Movie (skipped)", "Movie playback not implemented: skipping");
                self.events.push(Event::MovieSkipped { file: src });
            }
            CmdType::SetProp => {
                let obj = self.eval(c.expr("ObjName")).as_str();
                let pv = self.eval(c.expr("ObjProp"));
                let v = self.eval(c.expr("Value"));
                let p = prop_of(&pv);
                detail = format!("{obj:?}.{}", p.map(crate::lsb::prop_name).unwrap_or("?"));
                match p {
                    Some(p) => {
                        if crate::lsb::prop_name(p).starts_with("ON") {
                            log::debug!("event prop: SetProp {obj}.{} (id {p}) = {:?}", crate::lsb::prop_name(p), v.as_str());
                        }
                        if p == Prop::SOURCE
                            && let Some(n) = self.assets.data().resolve(&v.as_str(), IMAGE_EXTS) {
                                self.assets.request(&n);
                            }
                        if p == Prop::SOURCE && self.scene.get(&obj).is_some() {
                            let src = v.as_str();
                            self.watch_anim(&obj, &src);
                        }
                        if obj == ROOT {
                            log::debug!("SetProp on root layer: {} = {v:?}", crate::lsb::prop_name(p));
                            self.root_props.insert(p, v);
                        } else if !self.scene.set(&obj, p, v) && !obj.is_empty() {
                            self.warn(&format!("noobj:{obj}"), &format!("SetProp on missing object {obj:?}"));
                            log::debug!("SetProp on missing {obj:?} at {}:{pc} ({})", script.name, if self.graveyard.contains_key(&obj) { "deleted earlier" } else { "never created" });
                        } else if p == Prop::ONPLAYEND {
                            self.object_defined(&obj);
                        }
                    }
                    None => self.warn("setprop-bad", "SetProp with unknown property"),
                }
            }
            CmdType::GetProp => {
                let obj = self.eval(c.expr("ObjName")).as_str();
                let pv = self.eval(c.expr("ObjProp"));
                let var = c.str("VarName").unwrap_or("").to_string();
                let v = prop_of(&pv).map(|p| self.get_prop(&obj, p)).unwrap_or(Value::Int(0));
                detail = format!("{obj:?} -> {var}");
                self.assign(&var, v);
            }
            CmdType::ObjDel => {
                let name = self.eval(c.expr("Name")).as_str();
                let removed = self.scene.delete_names(&name);
                if removed.is_empty() {
                    log::debug!("ObjDel {:?}: no such object", name);
                }
                let mut gone = vec![];
                for o in removed {
                    let parent = o.str(Prop::PARENT);
                    log::debug!(
                        "ObjDel {name:?} at {}:{pc}: removed {:?} ({:?}){}",
                        script.name,
                        o.name,
                        o.kind,
                        if o.name == name { String::new() } else { format!(" cascade via parent {parent:?}") }
                    );
                    self.plays.remove(&o.name);
                    self.placeholders.remove(&o.name);
                    self.prevmenus.remove(&o.name);
                    gone.push(o.name.clone());
                    self.graveyard.insert(o.name.clone(), (o.kind, o.props));
                }
                detail = format!("{name:?} ({} objs: {})", gone.len(), gone.join(", "));
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
                let wait = self.eval_bool(c.expr("Wait"), true);
                self.run_text(&target, &tw, wait);
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
                // Transición: el efecto visual es un corte, pero se respeta su duración
                // (el script espera, saltable) y Delete borra los objetos al terminar.
                let t = self.eval(c.expr("Time")).as_f64().max(0.0);
                let targets: Vec<String> = c.exprs("Targets").to_vec().iter().map(|e| eval(e, self).as_str()).collect();
                let del = self.eval_bool(c.expr("Delete"), false);
                if del {
                    for n in &targets {
                        self.scene.delete(n);
                    }
                }
                detail = format!("{t}ms targets={} delete={del}", targets.len());
                if t > 0.0 && !self.skip {
                    self.state = State::WaitUntil { until: self.now + t, skippable: true };
                }
            }
            CmdType::SaveCabinet | CmdType::LoadCabinet => {
                let props = self.props_of(c);
                let cab = props.get(&Prop::NAME).map(|v| v.as_str()).unwrap_or_default();
                let act = self.eval_bool(c.expr("Act"), false);
                let targets: Vec<String> = c.exprs("Targets").to_vec().iter().map(|e| eval(e, self).as_str()).filter(|s| !s.is_empty()).collect();
                if kind == CmdType::SaveCabinet {
                    // Act=false: guarda los indicados; Act=true: todos menos los indicados
                    let sel: Vec<String> = if act {
                        self.scene.names().into_iter().filter(|n| !targets.contains(n)).collect()
                    } else {
                        targets.clone()
                    };
                    let mut saved = vec![];
                    for n in &sel {
                        let mut all = vec![n.clone()];
                        all.extend(self.scene.children(n, true));
                        for k in all {
                            if let Some(o) = self.scene.take(&k) {
                                saved.push(o);
                            }
                        }
                    }
                    detail = format!("{cab:?} saved {}", saved.len());
                    self.cabinets.insert(cab, saved);
                } else {
                    let objs = self.cabinets.remove(&cab).unwrap_or_default();
                    let mut keep = vec![];
                    let mut n = 0;
                    for o in objs {
                        let want = targets.is_empty() || targets.contains(&o.name) != act;
                        if want {
                            self.scene.put(o);
                            n += 1;
                        } else {
                            keep.push(o);
                        }
                    }
                    if !keep.is_empty() {
                        self.cabinets.insert(cab.clone(), keep);
                    }
                    detail = format!("{cab:?} restored {n}");
                }
            }
            CmdType::DoEvent => self.yielded = true,
            CmdType::MovieStop => {
                // detiene vídeos (placeholder): terminan ya y disparan ONPLAYEND
                for v in self.cinemas.values_mut() {
                    *v = self.now;
                }
            }
            CmdType::FormatHist | CmdType::ClrHist | CmdType::CallHist | CmdType::ClrRead => {
                // historial (backlog): aún no se guarda; se ignora en silencio
                log::debug!("{} ignored", c.name());
            }
            CmdType::IFDEF | CmdType::IFNDEF | CmdType::ENDIF => {}
            _ => {
                self.warn(&format!("unimpl:cmd {}", c.name()), &format!("unimplemented command {}", c.name()));
            }
        }
        self.trace(c, detail);
        if let Some(f) = self.frames.last_mut()
            && Arc::ptr_eq(&f.script, &script) && f.pc == pc {
                f.pc = next;
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
            if let Some(st) = f.saved_state {
                // vuelta de un manejador de evento: restaurar lo que hacía el script
                self.state = st;
                self.yielded = true;
            }
        }
    }

    /// --trace-conds: imprime la condición con los valores de sus variables, solo en scripts
    /// de escenario (fuera de ノベルシステム\) y las 2 primeras veces por (posición, resultado).
    /// Cadenas como <N chars> salvo --show-text.
    fn cond_trace(&mut self, s: &Lsb, pc: usize, c: &Command, ok: bool) {
        let dbg = self.cond_filter.as_ref().is_some_and(|f| s.name.contains(f.as_str()));
        if !dbg && (!self.opts.trace || !self.opts.trace_conds || s.name.starts_with("ノベルシステム")) {
            return;
        }
        let k = (s as *const Lsb as usize, pc, ok);
        let n = self.cond_seen.entry(k).or_default();
        if *n >= 2 {
            return;
        }
        *n += 1;
        let Some(e) = c.expr("Calc") else { return };
        let mut vars = vec![];
        for o in &e.entries {
            for p in &o.operands {
                if let Param::Var(v) = p {
                    if !v.starts_with("____") && !vars.iter().any(|(x, _): &(String, String)| x == v) {
                        let val = self.lookup(v).cloned().or_else(|| self.system_var(v)).unwrap_or(Value::Empty);
                        let shown = match &val {
                            Value::Str(t) if !self.opts.show_text => format!("<{} chars>", t.chars().count()),
                            Value::Array(a) => format!("[array {}]", a.len()),
                            Value::Empty => "(undef)".into(),
                            v => format!("{v:?}"),
                        };
                        vars.push((v.clone(), shown));
                    }
                }
            }
        }
        let line = format!(
            "[{:>8.0}ms] cond {}:{} {} {} -> {ok} {{{}}}",
            self.now,
            s.name,
            pc,
            c.name(),
            e.render(!self.opts.show_text),
            vars.iter().map(|(a, b)| format!("{a}={b}")).collect::<Vec<_>>().join(", ")
        );
        if dbg {
            log::debug!("{line}");
        } else {
            println!("{line}");
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
                            let ok = self.eval_bool(c.expr("Calc"), false);
                            let c2 = c.clone();
                            self.cond_trace(s, j, &c2, ok);
                            if ok {
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

    fn run_text(&mut self, target: &str, tw: &crate::lsb::TpWord, wait: bool) {
        self.last_text_target = Some(target.to_string());
        if self.scene.get(target).is_none() {
            let msgs: Vec<String> = self.scene.draw_order().into_iter().filter(|o| o.kind == Kind::Message).map(|o| format!("{:?}", o.name)).collect();
            log::debug!("TextIns target {target:?} (bytes {:02x?}) missing; existing message objects before stand-in: {msgs:?}", target.as_bytes());
            match self.graveyard.get(target).cloned() {
                Some((kind, props)) => {
                    let evs: Vec<&str> = props.keys().map(|p| crate::lsb::prop_name(*p)).filter(|n| n.starts_with("ON")).collect();
                    self.warn(&format!("mes:{target}"), &format!("TextIns into deleted message object {target:?}; re-creating it with its last properties"));
                    log::debug!("stand-in {target:?} inherits {} props of the deleted object (events {evs:?})", props.len());
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
                plan.push(Pending::Text(std::mem::take(cur)));
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
                        plan.push(Pending::Clear);
                    }
                    _ => {
                        flush(&mut plan, &mut cur);
                        plan.push(Pending::Clear);
                    }
                },
                Glyph::Event { event } => {
                    flush(&mut plan, &mut cur);
                    plan.push(Pending::Notify(event.clone()));
                }
                Glyph::Img { .. } => self.warn("unimpl:TextIns inline image", "TextIns inline images are ignored"),
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
                    Glyph::Event { event } => Some(format!("Event({})", event.trim_start_matches('\x01').split("\r\n").next().unwrap_or(""))),
                    Glyph::Img { .. } => Some("Img".into()),
                    _ => None,
                })
                .collect();
            log::debug!("TextIns {target}: {} glyphs, tags {tags:?}", tw.body.len());
        }
        self.start_play(target, plan);
        if wait && !self.placeholders.contains(target) {
            self.state = State::WaitText { target: target.to_string() };
        }
    }

    /// Carga los botones de un .lpm. Posición: si los dos primeros enteros del botón
    /// coinciden con el tamaño de su imagen son tamaño y la posición son unk15/unk16; si no,
    /// son la posición (sin verificar: se registran todos los números a debug).
    fn load_prevmenu(&mut self, name: &str, src: &str) {
        let data = self.assets.data().clone();
        let Some(file) = data.resolve(src, &[".lpm"]) else {
            log::warn!("PrevMenu {name}: {src:?} not found");
            return;
        };
        let parsed = data.read(&file).map_err(|e| e.to_string()).and_then(|d| crate::lpm::parse(&d));
        let (ver, btns) = match parsed {
            Ok(v) => v,
            Err(e) => {
                log::warn!("PrevMenu {name}: {file}: {e}");
                return;
            }
        };
        let mut pm = PrevMenu::default();
        for (i, b) in btns.iter().enumerate() {
            let res = data.resolve(&b.src, IMAGE_EXTS);
            let size = res.as_ref().and_then(|r| data.read_prefix(r, 4096)).and_then(|d| crate::gal::peek_size(&d)).map(|(w, h)| (w as i64, h as i64));
            if let Some(r) = &res {
                self.assets.request(r);
            }
            if let Some(r) = data.resolve(&b.src_selected, IMAGE_EXTS) {
                self.assets.request(&r);
            }
            let (w, h) = size.unwrap_or((b.a.0 as i64, b.a.1 as i64));
            let a_is_size = size == Some((b.a.0 as i64, b.a.1 as i64));
            let (x, y) = if a_is_size { (b.b.0 as i64, b.b.1 as i64) } else { (b.a.0 as i64, b.a.1 as i64) };
            log::debug!(
                "PrevMenu {name} (lpm v{ver}) button {i}: ints a=({},{}) b=({},{}) image {:?} size {:?} -> pos ({x},{y}) {w}x{h}",
                b.a.0, b.a.1, b.b.0, b.b.1, res, size
            );
            pm.buttons.push(PmButton { x, y, w, h, src: b.src.clone(), src_selected: b.src_selected.clone(), name: b.name.clone() });
        }
        self.prevmenus.insert(name.to_string(), pm);
    }

    /// Botón de PrevMenu bajo (x, y) lógicos: (menú, índice).
    fn prevmenu_hit(&self, x: f64, y: f64) -> Option<(String, usize)> {
        let mut best: Option<(i64, u64, String, usize)> = None;
        for (n, pm) in &self.prevmenus {
            let Some(o) = self.scene.get(n) else { continue };
            if !self.scene.effective_visible(o) {
                continue;
            }
            let (ox, oy) = self.scene.abs_pos(o);
            for (i, b) in pm.buttons.iter().enumerate() {
                let (bx, by) = (ox + b.x as f64, oy + b.y as f64);
                if x >= bx && y >= by && x < bx + b.w as f64 && y < by + b.h as f64 {
                    let k = (o.int(Prop::PRIORITY, 0), o.seq);
                    if best.as_ref().is_none_or(|b| (b.0, b.1) < k) {
                        best = Some((k.0, k.1, n.clone(), i));
                    }
                }
            }
        }
        best.map(|b| (b.2, b.3))
    }

    /// Movimiento del ratón: hover de PrevMenu (ONMAPIN/ONMAPOUT con @ParamStr = [nombre, índice]).
    pub fn mouse_move(&mut self, x: f64, y: f64) {
        let hit = self.prevmenu_hit(x, y);
        let names: Vec<String> = self.prevmenus.keys().cloned().collect();
        for n in names {
            let now = hit.as_ref().filter(|h| h.0 == n).map(|h| h.1);
            let before = self.prevmenus[&n].hover;
            if now == before {
                continue;
            }
            self.prevmenus.get_mut(&n).unwrap().hover = now;
            let bname = |vm: &Self, i: usize| vm.prevmenus[&n].buttons.get(i).map(|b| b.name.clone()).unwrap_or_default();
            if let Some(i) = before {
                let p = vec![Value::Str(bname(self, i)), Value::Int(i as i64)];
                self.fire(&n, Prop::ONMAPOUT, p);
            }
            if let Some(i) = now {
                log::debug!("PrevMenu {n}: hover button {i}");
                let p = vec![Value::Str(bname(self, i)), Value::Int(i as i64)];
                self.fire(&n, Prop::ONMAPIN, p);
            }
        }
    }

    /// Estado de reproducción de una caja, a debug, como mucho una vez por segundo y caja.
    fn play_debug(&mut self, name: &str, st: &str) {
        if !log::log_enabled!(log::Level::Debug) {
            return;
        }
        let last = self.play_dbg.get(name).copied().unwrap_or(f64::NEG_INFINITY);
        if self.now - last >= 1000.0 {
            self.play_dbg.insert(name.to_string(), self.now);
            log::debug!("text playback {name}: {st}");
        }
    }

    /// Eventos de sistema de LiveNovel incrustados en el texto ({CREATECG ...} etc.).
    /// CREATECG/CHANGECG crean/cambian objetos de imagen; el resto se registra y se ignora
    /// (WAITPLAY: los sonidos cuentan como terminados).
    /// Argumentos observados de CREATECG: [nombre, fuente ("$RRGGBB" = color sólido), efecto,
    /// "#n", alineación H (L/C/R), alineación V (T/C/B), ?, ?, tiempo ms, ?, ?, "", ?, ?, ?, alfa].
    /// Solo nombre, fuente, alineación y alfa se usan; el resto está sin verificar.
    fn system_text_event(&mut self, boxname: &str, name: &str, args: &[String]) {
        log::debug!("system text event on {boxname}: {name} args {args:?}");
        match name {
            "CREATECG" | "CHANGECG" => {
                let (Some(obj), Some(src)) = (args.first(), args.get(1)) else {
                    log::debug!("{name}: missing name/source");
                    return;
                };
                let obj = obj.clone();
                let color = src.strip_prefix('$').and_then(|h| u32::from_str_radix(h, 16).ok());
                // Posiciones: CREATECG [0]=nombre [1]=fuente/$color [2]=efecto (NORMAL/WAIT...)
                // [3]=#capa [4]=alin. H [5]=alin. V [8]=tiempo ms [15]=alfa (solo forma de 16 args).
                // CHANGECG [0]=nombre [1]=fuente/$color [2]=efecto [4]=tiempo ms.
                // El resto sin verificar: no se usa (nunca se deduce alfa por posición dudosa).
                let num = |i: usize| args.get(i).and_then(|a| a.trim().parse::<f64>().ok());
                let alpha = if name == "CREATECG" && args.len() >= 16 { num(15).filter(|a| (0.0..=255.0).contains(a)).map(|a| a as i64) } else { None };
                let time = if name == "CREATECG" { num(8) } else { num(4) }.unwrap_or(0.0).max(0.0);
                let time = if self.skip || self.opts.trace { 0.0 } else { time };
                let wait = args.get(2).is_some_and(|e| e == "WAIT");
                if wait && !self.opts.trace {
                    // WAIT: el texto de la caja no sigue hasta que terminen la animación del
                    // objeto (su PLAYEND) y el fundido; con imagen estática y tiempo 0, nada.
                    let deadline = self.now + time + 30_000.0;
                    if let Some(p) = self.plays.get_mut(boxname) {
                        p.wait_for = Some((obj.clone(), deadline));
                    }
                }
                log::debug!("{name} {obj:?}: effect {:?} time {time} ms{} alpha {alpha:?}", args.get(2), if wait { " (WAIT)" } else { "" });
                if name == "CHANGECG" && self.scene.get(&obj).is_some() && (color.is_none() == (self.scene.get(&obj).map(|o| o.kind) == Some(Kind::Image))) {
                    match color {
                        Some(c) => {
                            let tc = ((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF);
                            self.scene.set(&obj, Prop::COLOR, Value::Int(tc as i64));
                        }
                        None => {
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
                    .map(|o| [Prop::PRIORITY, Prop::PARENT].iter().filter_map(|p| o.get(*p).map(|v| (*p, v.clone()))).collect())
                    .unwrap_or_default();
                let (sw, sh) = (self.opts.screen.0 as i64, self.opts.screen.1 as i64);
                let mut props: HashMap<u8, Value> = HashMap::new();
                props.insert(Prop::NAME, Value::Str(obj.clone()));
                props.extend(keep);
                let target_alpha = alpha.unwrap_or(255);
                props.insert(Prop::ALPHA, Value::Int(if time > 0.0 { 0 } else { target_alpha }));
                let kind = match color {
                    Some(c) => {
                        let tc = ((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF);
                        props.insert(Prop::COLOR, Value::Int(tc as i64));
                        props.insert(Prop::WIDTH, Value::Int(sw));
                        props.insert(Prop::HEIGHT, Value::Int(sh));
                        Kind::Box
                    }
                    None => {
                        let resolved = self.assets.data().resolve(src, IMAGE_EXTS);
                        let size = resolved.as_ref().and_then(|r| self.assets.data().read_prefix(r, 4096)).and_then(|d| crate::gal::peek_size(&d));
                        if let Some(r) = &resolved {
                            self.assets.request(r);
                        } else {
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
                        Kind::Image
                    }
                };
                log::debug!("{name} creates {kind:?} {obj:?} src={src:?} props {:?}", props.keys().map(|p| crate::lsb::prop_name(*p)).collect::<Vec<_>>());
                if args.get(2).is_some_and(|e| e == "REPEAT") {
                    props.insert(Prop::REPEAT, Value::Bool(true));
                }
                self.scene.create(&obj, kind, props);
                if time > 0.0 {
                    self.scene.add_motion(Motion { obj: obj.clone(), prop: Prop::ALPHA, from: 0.0, to: target_alpha as f64, start: self.now, dur: time });
                }
                if kind == Kind::Image {
                    self.watch_anim(&obj, src);
                }
                self.object_defined(&obj);
            }
            "DELETECG" => {
                // [0]=nombres separados por comas, [2]=tiempo (fundido; aquí se borra ya)
                for obj in args.first().map(|s| s.as_str()).unwrap_or("").split(',').map(str::trim).filter(|s| !s.is_empty()) {
                    let gone = self.scene.delete_names(obj);
                    log::debug!("DELETECG {obj:?}: removed {} objects", gone.len());
                    for o in gone {
                        self.anims.remove(&o.name);
                        self.prevmenus.remove(&o.name);
                        self.graveyard.insert(o.name.clone(), (o.kind, o.props));
                    }
                }
            }
            "PLAYSND" | "STOPSND" => log::debug!("{name}: audio from text events not implemented"),
            "" => {}
            _ => {
                if name != "WAITPLAY" {
                    self.warn(&format!("unimpl:text sysevent {name}"), &format!("system text event {name} not implemented (ignored)"));
                }
            }
        }
    }

    /// Clic (izquierdo o derecho) en coordenadas lógicas: manejadores de objeto o avance.
    pub fn click_at(&mut self, x: f64, y: f64, right: bool) {
        if !right && let Some((n, i)) = self.prevmenu_hit(x, y) {
            // selección: ONSELECT y, al terminar (sin animación de selección), ONPLAYEND
            let bname = self.prevmenus[&n].buttons[i].name.clone();
            log::debug!("PrevMenu {n}: selected button {i}");
            let p = vec![Value::Str(bname), Value::Int(i as i64)];
            self.fire(&n, Prop::ONSELECT, p.clone());
            self.fire(&n, Prop::ONPLAYEND, p);
            return;
        }
        let props: &[u8] = if right { &[Prop::ONRBTNUP, Prop::ONRBTNDOWN] } else { &[Prop::ONLBTNUP, Prop::ONLBTNDOWN, Prop::ONSELECT] };
        if let Some(name) = self.scene.hit_handler(x, y, props).map(|o| o.name.clone()) {
            for &p in props {
                if self.fire(&name, p, vec![]) {
                    break;
                }
            }
            return;
        }
        if !right {
            self.click();
        }
    }

    /// Un objeto pasa a existir de verdad / recibe ONPLAYEND: libera el texto retenido
    /// y notifica un fin de reproducción que ocurrió sin manejador.
    fn object_defined(&mut self, name: &str) {
        if self.placeholders.remove(name) {
            log::debug!("message object {name} defined; releasing held text");
        }
        if self.ended_unnotified.contains(name) && self.fire(name, Prop::ONPLAYEND, vec![]) {
            self.ended_unnotified.remove(name);
        }
    }

    /// GetProp con valores por defecto del tipo adecuado: cadena vacía para propiedades de
    /// texto (PARENT, SOURCE, eventos...), VISIBLE/ALPHA/ZOOM por defecto si el objeto existe.
    fn get_prop(&self, obj: &str, p: u8) -> Value {
        let (exists, v) = if obj == ROOT {
            (true, self.root_props.get(&p).cloned())
        } else {
            match self.scene.get(obj) {
                Some(o) => (true, if p == Prop::NAME { Some(Value::Str(o.name.clone())) } else { o.get(p).cloned() }),
                None => (false, None),
            }
        };
        if let Some(v) = v {
            return v;
        }
        let name = crate::lsb::prop_name(p);
        if matches!(p, Prop::NAME | Prop::PARENT | Prop::SOURCE | Prop::TEXT | Prop::FONTNAME) || name.starts_with("ON") || name.ends_with("SOURCE") {
            return Value::Str(String::new());
        }
        match p {
            Prop::VISIBLE => Value::Bool(exists),
            Prop::ALPHA if exists => Value::Int(255),
            Prop::ZOOMX | Prop::ZOOMY if exists => Value::Int(100),
            Prop::WIDTH if obj == ROOT => Value::Int(self.opts.screen.0 as i64),
            Prop::HEIGHT if obj == ROOT => Value::Int(self.opts.screen.1 as i64),
            _ => Value::Int(0),
        }
    }

    /// Variables de sistema que el motor original provee.
    fn system_var(&self, name: &str) -> Option<Value> {
        let (w, h) = self.opts.screen;
        Some(match name {
            "@ScrWidth" => Value::Int(w as i64),
            "@ScrHeight" => Value::Int(h as i64),
            // por marco de llamada: cada Call apila los suyos y se recuperan al volver
            "@ParamStr" => Value::Array(self.frames.last().map(|f| f.params.iter().map(|v| Value::Str(v.as_str())).collect()).unwrap_or_default()),
            "@Sender" => Value::Str(self.frames.iter().rev().find_map(|f| f.sender.clone()).unwrap_or_default()),
            "@ParamCount" => Value::Int(self.frames.last().map(|f| f.params.len() as i64).unwrap_or(0)),
            "StatusSoundON" => Value::Bool(true),
            "ステータスフルスクリーン" => Value::Bool(false),
            "ステータスディスプレイモード" | "ステータスMIDIポート" => Value::Int(0),
            "StatusFontName" => Value::Str(String::new()),
            _ => return None,
        })
    }

    /// Solo --trace: clic global en pantalla, como lo enruta el motor a la caja de mensajes
    /// activa (la última que recibió TextIns), visible o no:
    /// 1) libera texto retenido (caja creada implícitamente) y lo completa/avanza;
    /// 2) si no había nada que avanzar, dispara ONLBTNUP/ONLBTNDOWN de la caja activa
    ///    o, si no tiene, su ONPLAYEND. Cada candidato descartado se explica en debug.
    pub fn click_message(&mut self) -> Option<String> {
        let held: Vec<String> = self.placeholders.drain().collect();
        for h in &held {
            log::debug!("screen click: releasing held text of {h}");
        }
        if self.wants_click() || !held.is_empty() {
            self.click();
            return Some(format!("advance text{}", if held.is_empty() { String::new() } else { format!(" (released {})", held.join(",")) }));
        }
        let mut cands: Vec<String> = vec![];
        if let Some(t) = self.last_text_target.clone() {
            cands.push(t);
        }
        for o in self.scene.draw_order().into_iter().rev() {
            if o.kind == Kind::Message && !cands.contains(&o.name) {
                cands.push(o.name.clone());
            }
        }
        if cands.is_empty() {
            log::debug!("screen click: no message objects");
        }
        for n in cands {
            if self.scene.get(&n).is_none() {
                log::debug!("screen click: skip {n}: object does not exist");
                continue;
            }
            if self.plays.contains_key(&n) {
                log::debug!("screen click: skip {n}: text still playing");
                continue;
            }
            for p in [Prop::ONLBTNUP, Prop::ONLBTNDOWN, Prop::ONPLAYEND] {
                if self.fire(&n, p, vec![]) {
                    return Some(format!("{n}.{}", crate::lsb::prop_name(p)));
                }
            }
            let evs: Vec<String> = self.scene.get(&n).map(|o| o.props.iter().filter(|(p, _)| crate::lsb::prop_name(**p).starts_with("ON")).map(|(p, v)| format!("{}={:?}", crate::lsb::prop_name(*p), v.as_str())).collect()).unwrap_or_default();
            log::debug!("screen click: skip {n}: no ONLBTNUP/ONLBTNDOWN/ONPLAYEND handler (kind {:?}, event props {evs:?})", self.scene.get(&n).map(|o| o.kind));
        }
        None
    }

    /// Solo --trace: dispara el manejador de clic de algún objeto visible (rotando).
    /// Solo --trace: elige un ítem de un menú (Menu/PrevMenu) con manejador, visible o no.
    /// Ítem: --pick N, o el primero habilitado (SetEnabled).
    pub fn auto_menu(&mut self) -> Option<String> {
        let name = self
            .scene
            .draw_order()
            .into_iter()
            .rev()
            .find(|o| o.kind == Kind::Menu && [Prop::ONSELECT, Prop::ONLBTNUP].iter().any(|p| o.get(*p).is_some_and(|v| !v.as_str().trim().is_empty())))
            .map(|o| o.name.clone())?;
        let idx = match self.opts.pick {
            Some(p) => p,
            None => (0..64).find(|i| *self.enabled.get(&(name.clone(), *i)).unwrap_or(&true)).unwrap_or(0),
        };
        // el texto del ítem (si el menú lo tiene en PR_TEXT, una línea por ítem)
        let item = self.scene.get(&name).map(|o| o.str(Prop::TEXT)).unwrap_or_default();
        let item = item.lines().nth(idx.max(0) as usize).unwrap_or("").to_string();
        if let Some(o) = self.scene.get_mut(&name) {
            o.props.insert(Prop::INDEX, Value::Int(idx));
            o.props.insert(Prop::VALUE, Value::Int(idx));
        }
        self.selected.insert((name.clone(), idx), true);
        for p in [Prop::ONSELECT, Prop::ONLBTNUP] {
            if self.fire(&name, p, vec![Value::Int(idx), Value::Str(item.clone())]) {
                return Some(format!("{name}[{idx}]"));
            }
        }
        None
    }

    /// Solo --trace: dispara el manejador de clic de algún objeto visible (rotando).
    /// No toca menús (auto_menu) ni el ONPLAYEND de mensajes con texto pendiente.
    pub fn auto_fire(&mut self, n: usize) -> Option<String> {
        let props = [Prop::ONLBTNUP, Prop::ONLBTNDOWN, Prop::ONSELECT];
        let cands: Vec<String> = self
            .scene
            .draw_order()
            .into_iter()
            .filter(|o| o.kind != Kind::Menu && !self.plays.contains_key(&o.name) && self.scene.effective_visible(o) && props.iter().any(|p| o.get(*p).is_some_and(|v| !v.as_str().trim().is_empty())))
            .map(|o| o.name.clone())
            .collect();
        if cands.is_empty() {
            return None;
        }
        let name = cands[n % cands.len()].clone();
        for p in props {
            if self.fire(&name, p, vec![]) {
                return Some(name);
            }
        }
        None
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
            None => match self.system_var(name) {
                Some(v) => v,
                None => {
                    // LiveMaker trata las variables sin declarar como 0/""; solo se registra en debug
                    if self.warned.insert(format!("undef:{name}")) {
                        log::debug!("read of undefined variable {name} (as 0/\"\")");
                    }
                    Value::Empty
                }
            },
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
                self.get_prop(&s(0), p)
            }
            Func::SETPROP => {
                let p = prop_of(a.get(1)?)?;
                if s(0) == ROOT {
                    self.root_props.insert(p, a.get(2).cloned().unwrap_or_default());
                    return Some(Value::Bool(true));
                }
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
            Func::INCABINET | Func::FILEEXISTS => Value::Bool(self.assets.data().find(&s(0)).is_some()),
            Func::GETHISTORYCOUNT => Value::Int(0),
            // ListCompo(arr, modo, objeto): nombres de los componentes hijos de `objeto`
            // en orden de creación (modo != 0: también descendientes; semántica inferida).
            Func::LISTCOMPO => {
                let list = self.scene.children(&s(2), a.get(1).map(|v| v.as_int() != 0).unwrap_or(false));
                let n = list.len() as i64;
                if let Some(Param::Var(var)) = raw.first() {
                    let var = var.clone();
                    self.assign(&var, Value::Array(list.into_iter().map(Value::Str).collect()));
                }
                Value::Int(n)
            }
            // Ítems de PrevMenu/menú: (objeto, índice[, valor]). Semántica inferida.
            Func::SETENABLED | Func::SETSELECTED => {
                let k = (s(0), a.get(1).map(|v| v.as_int()).unwrap_or(0));
                let v = a.get(2).map(|v| v.truthy()).unwrap_or(true);
                if f == Func::SETENABLED { self.enabled.insert(k, v) } else { self.selected.insert(k, v) };
                Value::Bool(true)
            }
            Func::GETENABLED => Value::Bool(*self.enabled.get(&(s(0), a.get(1).map(|v| v.as_int()).unwrap_or(0))).unwrap_or(&true)),
            Func::GETSELECTED => Value::Bool(*self.selected.get(&(s(0), a.get(1).map(|v| v.as_int()).unwrap_or(0))).unwrap_or(&false)),
            // InPrimary: no documentada; se asume "ventana principal activa" -> verdadero.
            Func::INPRIMARY => Value::Bool(true),
            Func::GETHISTORYMAXCOUNT => Value::Int(self.history_max),
            Func::SETHISTORYMAXCOUNT => {
                self.history_max = a.first().map(|v| v.as_int()).unwrap_or(0);
                Value::Bool(true)
            }
            // ventana = espacio lógico (letterbox lo resuelve SDL), así que la conversión es identidad
            Func::TOSCREENX | Func::TOSCREENY | Func::TOCLIENTX | Func::TOCLIENTY => a.last().cloned().unwrap_or(Value::Int(0)),
            Func::LOADTEXTFILE => {
                let d = self.assets.data().clone();
                d.resolve(&s(0), &[".txt"]).and_then(|n| d.read(&n).ok()).map(|b| Value::Str(crate::util::cp932(&b))).unwrap_or(Value::Str(String::new()))
            }
            Func::DBCREATETABLE..=Func::DBGETRECNO
            | Func::DBCLEARRECORD
            | Func::DBGETTABLECOUNT
            | Func::DBGETTABLE => {
                let d = self.assets.data().clone();
                let load = move |f: &str| d.resolve(f, &[".tsv", ".txt"]).and_then(|n| d.read(&n).ok()).map(|b| crate::util::cp932(&b));
                return self.db.call(f, a, &load);
            }
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
        let k = if msg.starts_with("unimplemented function") { format!("unimpl:func {key}") } else { key.to_string() };
        self.warn(&k, msg);
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
        let mut vm = Vm::new(assets, "ゲームメイン.lsb", Options { pick: None, trace_conds: false, screen: (800, 600), cinema_ms: 0.0, text_cps: 0.0, lookahead: 50, trace: false, show_text: false }).unwrap();
        let mut t = 0.0;
        for _ in 0..1000 {
            vm.update(t, 1000);
            if vm.ended() {
                break;
            }
            if vm.wants_click() {
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
