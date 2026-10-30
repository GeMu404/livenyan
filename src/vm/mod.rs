//! Intérprete de scripts LSB.
//!
//! El VM es independiente de SDL: el bucle principal (o el modo `--trace`) le da
//! la hora actual y la entrada; el VM devuelve eventos de audio/vídeo.

use crate::assets::{ALL_EXTS, Assets, SOUND_EXTS};
use crate::lft::PixelFontCatalog;
use crate::lsb::expr::{Env, Value, eval};
use crate::lsb::names::Func;
use crate::lsb::{CmdType, Command, LiveParser, Lsb, Param, Prop};
pub use crate::scene::ROOT;
use crate::scene::{Kind, Pending, Playback, Scene, TextItem, prop_of};
use crate::text::TextRenderer;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

mod media;
mod menu;
mod slider;
mod step_flow;
mod step_media;
mod step_menu;
mod step_obj;
mod step_prop;
mod step_save;
mod step_text;
mod text_hist;
mod wipe;

use menu::{MenuItemKey, menu_item_key};

fn translate_component_text(
    kind: CmdType,
    source: &str,
    translations: &HashMap<String, String>,
) -> Option<String> {
    if !matches!(
        kind,
        CmdType::Button | CmdType::Caption | CmdType::CGCaption
    ) {
        return None;
    }
    translations
        .get(&crate::text_export::text_item_id(source))
        .cloned()
}

pub(crate) fn caption_char_width(c: char) -> f64 {
    let code = c as u32;
    let wide = matches!(
        code,
        0x1100..=0x11ff
            | 0x2329..=0x232a
            | 0x2e80..=0xa4cf
            | 0xac00..=0xd7a3
            | 0xf900..=0xfaff
            | 0xfe10..=0xfe19
            | 0xfe30..=0xfe6f
            | 0xff01..=0xff60
            | 0xffe0..=0xffe6
            | 0x1f300..=0x1faff
            | 0x20000..=0x3fffd
    );
    if wide { 1.0 } else { 0.55 }
}

const VIDEO_EXTS: &[&str] = &[".wmv"];

/// Eventos hacia el host (audio, vídeo…).
#[derive(Debug, Clone)]
pub enum Event {
    PlaySound {
        channel: String,
        file: String,
        volume: f32,
        repeat: bool,
    },
    StopSound {
        channel: String,
    },
    FadeSound {
        channel: String,
        duration_ms: u64,
    },
}

#[derive(Debug, Clone)]
pub enum State {
    Running,
    WaitUntil {
        until: f64,
        skippable: bool,
    },
    /// TextIns con Wait: espera a que el objeto de mensaje termine de reproducir.
    WaitText {
        target: String,
    },
    Menu {
        name: String,
        choices: Vec<String>,
        values: Vec<String>,
        var: String,
    },
    Ended,
}

#[derive(Debug, Clone, Copy)]
struct TimerState {
    due_at: f64,
    paused_at: Option<f64>,
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
    event_prop: Option<u8>,
    auto_scrap_sender: bool,
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
    pub src_disabled: String,
    pub name: String,
    /// Grupo LPM. `-1` = toggle (オート / スキップ). `0` = pulsación normal (閉じる).
    pub group: i32,
}

#[derive(Debug, Clone, Default)]
pub struct PrevMenu {
    pub buttons: Vec<PmButton>,
    pub hover: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct CinemaPlayback {
    pub movie: crate::lcm::Movie,
    pub started_at: f64,
    pub duration_ms: f64,
    /// PR_REPEAT: al terminar la línea de tiempo vuelve al primer frame.
    pub repeat: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct HistoryEntry {
    page: String,
    text: Vec<TextItem>,
}

struct VideoPlayback {
    decoder: Option<crate::video::Decoder>,
    frames: VecDeque<VideoFrame>,
    frame_seq: u64,
    generation: u64,
    width: u32,
    height: u32,
    started_at_ms: f64,
    first_pts_seconds: Option<f64>,
    paused_at_ms: Option<f64>,
}

struct VideoFrame {
    pixels: Arc<[u8]>,
    pts_seconds: f64,
    duplicate: bool,
    sequence: u64,
}

/// Un reloj de medio por objeto. El placeholder, el LCM y el vídeo no conviven.
enum Clock {
    Video(VideoPlayback),
    Cinema {
        movie: Option<CinemaPlayback>,
        /// Plazo de una pasada. `None` si repite, o si ese plazo ya disparó ONPLAYEND.
        until: Option<f64>,
    },
}

pub struct VideoFramePair<'a> {
    pub generation: u64,
    pub sequence: u64,
    pub previous: Option<(&'a [u8], f64, bool, u64)>,
    pub current: &'a [u8],
    pub current_pts_seconds: f64,
    pub current_duplicate: bool,
    pub progress: f64,
    pub width: u32,
    pub height: u32,
}

fn video_pair_progress(
    now_ms: f64,
    started_at_ms: f64,
    first_pts_seconds: f64,
    previous_pts_seconds: f64,
    current_pts_seconds: f64,
) -> f64 {
    let interval = current_pts_seconds - previous_pts_seconds;
    if !interval.is_finite() || interval <= 0.0 {
        return 1.0;
    }
    let playhead = first_pts_seconds + ((now_ms - started_at_ms).max(0.0) / 1000.0);
    ((playhead - previous_pts_seconds) / interval).clamp(0.0, 1.0)
}

fn variable_scope_is_persistent(scope: u32) -> bool {
    matches!(scope, 0 | 3)
}

pub(crate) fn volume_gain(volume: f64) -> f32 {
    (volume / 1000.0).clamp(0.0, 1.0) as f32
}

pub(crate) fn resolve_system_sound(
    data: &crate::archive::GameData,
    source: &str,
) -> Option<String> {
    data.resolve(source, SOUND_EXTS).or_else(|| {
        let source = source.trim_start_matches(['\\', '/']);
        data.resolve(&format!("サウンド\\{source}"), SOUND_EXTS)
    })
}

impl CinemaPlayback {
    pub fn playhead_ms(&self, now: f64) -> f64 {
        let elapsed = (now - self.started_at).max(0.0);
        if self.repeat && self.duration_ms.is_finite() && self.duration_ms > 0.0 {
            elapsed.rem_euclid(self.duration_ms)
        } else {
            elapsed
        }
    }

    pub fn sprites_at(&self, now: f64) -> Vec<crate::lcm::Sprite> {
        self.movie.sprites_at(self.playhead_ms(now))
    }

    pub fn texts_at(&self, now: f64) -> Vec<crate::lcm::TextSprite> {
        self.movie.texts_at(self.playhead_ms(now))
    }
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

pub(crate) struct FlipPlan {
    /// Se dejan de dibujar, pero el script sigue viéndolos.
    park: Vec<String>,
    /// Vuelven a la pasada de dibujo.
    restore: Vec<String>,
    /// Se borran al terminar la espera.
    scrap: Vec<String>,
    fade: Vec<String>,
    /// Entra desde negro en la segunda mitad del tiempo.
    fade_in: Vec<String>,
    fade_out: bool,
    /// La primera placa archivada es la base (el título) y se conserva.
    remember_base: bool,
    /// Frame de esa primera placa. Lo creado después se tira con `Delete`.
    base_born: Option<u64>,
}

pub struct Vm {
    assets: Arc<Assets>,
    pub save_dir: PathBuf,
    translations: HashMap<String, String>,
    pending_component_dimensions: Option<(String, u32, u32)>,
    menu_fonts: HashMap<String, Option<TextRenderer>>,
    menu_font_override: Option<(PathBuf, u32)>,
    pixel_fonts: Arc<PixelFontCatalog>,
    frames: Vec<Frame>,
    globals: HashMap<String, Value>,
    /// Ámbito de cada global (`VarNew` Scope). Hace falta para `Reset`
    /// (conservar solo ステータス = scope 3). **fuente primaria** scopes 0|3
    /// persistentes en livenyan; 3 = status en `sequence.html`「リセット」.
    global_scopes: HashMap<String, u32>,
    pub scene: Scene,
    pub state: State,
    pub now: f64,
    pub opts: Options,
    pub events: Vec<Event>,
    selected_string: Value,
    active_component: String,
    hovered_object: Option<String>,
    /// Última posición lógica del ratón (-1,-1 = fuera).
    mouse: (f64, f64),
    slider_drag: Option<String>,
    slider_focus: Option<String>,
    edit_cursors: HashMap<String, usize>,
    video_generation: u64,
    pub stats: HashMap<&'static str, u64>,
    pub steps: u64,
    warned: HashSet<String>,
    rng: u64,
    prefetched_upto: HashMap<String, usize>,
    pub skip: bool,
    pub queue: std::collections::VecDeque<ObjEvent>,
    yielded: bool,
    budget_location: Option<String>,
    last_budget_report_ms: f64,
    pressed_keys: HashSet<usize>,
    /// Teclas cuyo flanco de pulsación aún no se ha consumido (`@KeyClick`).
    key_clicks: HashSet<usize>,
    /// Flanco o repetición (`@KeyRepeat`).
    key_repeats: HashSet<usize>,
    mouse_left: bool,
    mouse_right: bool,
    mouse_left_click: bool,
    mouse_right_click: bool,
    wheel_up: bool,
    wheel_down: bool,
    /// TextIns ya mostrados: (script, índice de comando). `ClrRead` lo vacía.
    read_texts: HashSet<(String, usize)>,
    /// Funciones/comandos no implementados vistos (clave -> veces).
    pub unimpl: std::collections::BTreeMap<String, u64>,
    pub db: crate::db::Db,
    history_max: i64,
    history: VecDeque<HistoryEntry>,
    history_formats: HashMap<String, String>,
    cabinets: HashMap<String, Vec<crate::scene::Obj>>,
    /// Temporizadores activos: objeto -> vencimiento y comienzo de pausa (ms).
    timers: HashMap<String, TimerState>,
    anim_clock: f64,
    /// Última caja que recibió TextIns (la "activa" para clics de pantalla).
    last_text_target: Option<String>,
    play_dbg: HashMap<String, f64>,
    /// Menús de previsualización (.lpm) por nombre de objeto.
    pub prevmenus: HashMap<String, PrevMenu>,
    /// LIVENYAN_CONDS=<parte del nombre de script>: condiciones a debug (también en GUI).
    cond_filter: Option<String>,
    /// Vídeo o cinema del objeto. Uno por nombre.
    clocks: HashMap<String, Clock>,
    /// Estado de ítems de PrevMenu/menús, por índice o nombre.
    enabled: HashMap<(String, MenuItemKey), bool>,
    visible: HashMap<(String, MenuItemKey), bool>,
    selected: HashMap<(String, MenuItemKey), bool>,
    /// Checkmarks de menú (`SetCheck`/`GetCheck` + `CHECKSOURCE`).
    checked: HashMap<(String, MenuItemKey), bool>,
    /// Secuencia de scripts entrados (Call/Jump/eventos), para detectar ciclos en --trace.
    pub script_log: Vec<String>,
    /// Aristas Jump/Call "origen:pc -> destino:etiqueta" con su número de usos.
    pub edges: HashMap<String, u64>,
    cond_seen: HashMap<(usize, usize, bool), u8>,
    /// Posiciones (script, pc) distintas ejecutadas: para detectar bucles sin progreso.
    pub visited: std::collections::HashSet<(usize, usize)>,
    /// Cambio de pantalla de un Flip, aplicado al terminar su espera.
    flip_plan: Option<FlipPlan>,
    /// Fundido de Flip: dos superficies, sin tocar ALPHA.
    wipe_fade: Option<wipe::WipeFade>,
    /// Placas archivadas (no hay un segundo framebuffer: una sola pasada).
    plates: Vec<std::collections::HashSet<String>>,
    /// Nombres de la primera placa. Al cubrir de nuevo no se archivan las sobras.
    base_plate: Option<std::collections::HashSet<String>>,
    /// Cohort de esa placa. Un `Delete` posterior borra lo nacido desde ahí.
    base_born: Option<u64>,
    frame_id: u64,
    /// `<TEXTSPD>` / `TXSP*`: ms/carácter y si es ABSOLUTE (script.html).
    /// Relativo: **interpretación** baseline [`TEXT_SPEED_BASELINE_CPS`].
    text_char_ms: HashMap<String, TextSpeedOverride>,
    /// `<QUAKE>` / `<STOPQUAKE>` (script.html): offset de dibujo por objeto o global.
    quake: Option<QuakeState>,
    /// Overlay Flip WHITE/BLACK/FLASH (**interpretación** I7).
    wipe_flash: Option<wipe::WipeFlash>,
    /// Archivos virtuales `mem:` (calc.html). Se descartan al salir del proceso.
    mem_files: HashMap<String, Vec<u8>>,
    /// Última captura CPU (solo tras SaveScreen on-demand; no cada frame).
    screen_rgba: Option<(u32, u32, Vec<u8>)>,
    /// SaveScreen diferido hasta el próximo render de escena (evita readback continuo).
    pub pending_savescreen: Option<SaveScreenRequest>,
    /// Imágenes decodificadas desde mem:/save: (caché de draw; RefCell → draw con `&Vm`).
    vfs_image_cache: std::cell::RefCell<HashMap<String, std::sync::Arc<crate::gal::Image>>>,
}

/// Pedido de captura calc.html SaveScreen (path + resample opcional).
#[derive(Debug, Clone)]
pub struct SaveScreenRequest {
    pub path: String,
    pub resample_w: u32,
    pub resample_h: u32,
}

/// Override de velocidad de texto desde tags LiveNovel.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TextSpeedOverride {
    /// Ms por carácter; `<= 0` = sin espera.
    pub ms: f64,
    /// `true` = ignora la opción de velocidad del jugador (ABSOLUTE ON).
    pub absolute: bool,
}

/// Estado de temblor de pantalla/objetos.
#[derive(Debug, Clone)]
pub(crate) struct QuakeState {
    /// Nombres a mover; vacío = offset global de escena.
    pub names: Vec<String>,
    /// Q=random, W=wave, B=bounce (script.html).
    pub kind: char,
    pub random_phase: bool,
    pub amp_x: f64,
    pub amp_y: f64,
    pub started_at: f64,
    pub until: f64,
    pub cycle_ms: f64,
    /// Si `Some`, amortigua hasta ese instante (STOPQUAKE TIME).
    pub stop_until: Option<f64>,
}

/// Baseline cps para TEXTSPD relativo (**interpretación livenyan** = `TextCfg::speed_cps` default).
pub(crate) const TEXT_SPEED_BASELINE_CPS: f64 = 60.0;
/// Factor TXSPS (スロー): periodo × N (**interpretación**; manual sin ratio).
pub(crate) const TEXT_SPEED_SLOW_FACTOR: f64 = 3.0;

/// Ranuras de `@KeyPush` / `@KeyClick` / `@KeyRepeat` (códigos de tecla de Windows).
const KEY_SLOTS: usize = 512;

fn sample_animation_frames(
    delays: &[u32],
    elapsed: f64,
    total: f64,
    repeat: bool,
) -> Option<(usize, usize, f64)> {
    if delays.len() < 2 || !elapsed.is_finite() {
        return None;
    }
    let time = if repeat {
        elapsed.rem_euclid(total)
    } else {
        elapsed.min(total - f64::EPSILON)
    };
    let mut cursor = 0.0;
    for (index, delay) in delays.iter().enumerate() {
        let duration = *delay as f64;
        if duration <= 0.0 {
            continue;
        }
        if time < cursor + duration {
            let next = delays[index + 1..]
                .iter()
                .position(|delay| *delay > 0)
                .map(|offset| index + 1 + offset)
                .or_else(|| repeat.then(|| delays.iter().position(|delay| *delay > 0))?)
                .or_else(|| (repeat && delays[index] > 0).then_some(index))?;
            let progress = ((time - cursor) / duration).clamp(0.0, 1.0);
            return Some((index, next, progress));
        }
        cursor += duration;
    }
    None
}

impl Vm {
    fn play_of(&self, name: &str) -> Option<&Playback> {
        self.scene.get(name)?.play.as_ref()
    }

    fn play_mut(&mut self, name: &str) -> Option<&mut Playback> {
        self.scene.get_mut(name)?.play.as_mut()
    }

    fn ensure_play(&mut self, name: &str) -> Option<&mut Playback> {
        let object = self.scene.get_mut(name)?;
        if object.play.is_none() {
            object.play = Some(Playback::default());
        }
        object.play.as_mut()
    }

    fn clear_play(&mut self, name: &str) {
        if let Some(object) = self.scene.get_mut(name) {
            object.play = None;
        }
    }

    fn play_names(&self) -> Vec<String> {
        self.scene
            .live()
            .filter(|object| object.play.is_some())
            .map(|object| object.name.clone())
            .collect()
    }

    fn anim_names(&self) -> Vec<String> {
        self.scene
            .live()
            .filter(|object| object.anim.is_some())
            .map(|object| object.name.clone())
            .collect()
    }

    fn video_of(&self, name: &str) -> Option<&VideoPlayback> {
        match self.clocks.get(name) {
            Some(Clock::Video(video)) => Some(video),
            _ => None,
        }
    }

    fn video_mut(&mut self, name: &str) -> Option<&mut VideoPlayback> {
        match self.clocks.get_mut(name) {
            Some(Clock::Video(video)) => Some(video),
            _ => None,
        }
    }

    fn video_names(&self) -> Vec<String> {
        self.clocks
            .iter()
            .filter_map(|(name, clock)| match clock {
                Clock::Video(_) => Some(name.clone()),
                Clock::Cinema { .. } => None,
            })
            .collect()
    }

    pub fn cinema(&self, name: &str) -> Option<&CinemaPlayback> {
        match self.clocks.get(name) {
            Some(Clock::Cinema {
                movie: Some(movie), ..
            }) => Some(movie),
            _ => None,
        }
    }

    fn cinema_mut(&mut self, name: &str) -> Option<&mut CinemaPlayback> {
        match self.clocks.get_mut(name) {
            Some(Clock::Cinema {
                movie: Some(movie), ..
            }) => Some(movie),
            _ => None,
        }
    }

    pub fn cinema_named(&self) -> impl Iterator<Item = (&str, &CinemaPlayback)> {
        self.clocks.iter().filter_map(|(name, clock)| match clock {
            Clock::Cinema {
                movie: Some(movie), ..
            } => Some((name.as_str(), movie)),
            _ => None,
        })
    }

    pub fn cinema_playbacks(&self) -> impl Iterator<Item = &CinemaPlayback> {
        self.clocks.values().filter_map(|clock| match clock {
            Clock::Cinema {
                movie: Some(movie), ..
            } => Some(movie),
            _ => None,
        })
    }

    fn cinema_names(&self) -> Vec<String> {
        self.clocks
            .iter()
            .filter_map(|(name, clock)| match clock {
                Clock::Cinema { movie: Some(_), .. } => Some(name.clone()),
                _ => None,
            })
            .collect()
    }

    #[cfg(test)]
    fn has_cinema_deadline(&self, name: &str) -> bool {
        matches!(
            self.clocks.get(name),
            Some(Clock::Cinema { until: Some(_), .. })
        )
    }

    fn one_shot_cinemas(&self) -> Vec<String> {
        self.clocks
            .iter()
            .filter_map(|(name, clock)| match clock {
                Clock::Cinema { until: Some(_), .. } => Some(name.clone()),
                _ => None,
            })
            .collect()
    }

    fn clear_clock(&mut self, name: &str) {
        self.clocks.remove(name);
    }

    /// Placeholder sin LCM, o plazo de una pasada sobre un cinema ya puesto.
    fn set_cinema_deadline(&mut self, name: &str, until: f64) {
        match self.clocks.get_mut(name) {
            Some(Clock::Cinema { until: slot, .. }) => *slot = Some(until),
            _ => {
                self.clocks.insert(
                    name.to_string(),
                    Clock::Cinema {
                        movie: None,
                        until: Some(until),
                    },
                );
            }
        }
    }

    fn menu_still_modal(&self, name: &str) -> bool {
        self.scene
            .get(name)
            .is_some_and(|object| object.kind == Kind::Menu && self.scene.effective_visible(object))
    }

    pub fn new(assets: Arc<Assets>, entry: &str, opts: Options) -> Result<Self, String> {
        let script = assets
            .script(entry)
            .ok_or_else(|| format!("cannot load entry script {entry}"))?;
        let pixel_fonts = Arc::new(PixelFontCatalog::load_from_game_data(assets.data()));
        let mut vm = Self {
            assets,
            save_dir: PathBuf::from("."),
            translations: HashMap::new(),
            pending_component_dimensions: None,
            menu_fonts: HashMap::new(),
            menu_font_override: None,
            pixel_fonts,
            frames: vec![],
            globals: HashMap::new(),
            global_scopes: HashMap::new(),
            scene: Scene::default(),
            state: State::Running,
            now: 0.0,
            opts,
            events: vec![],
            selected_string: Value::Str(String::new()),
            active_component: String::new(),
            hovered_object: None,
            mouse: (-1.0, -1.0),
            slider_drag: None,
            slider_focus: None,
            edit_cursors: HashMap::new(),
            video_generation: 0,
            stats: HashMap::new(),
            steps: 0,
            warned: HashSet::new(),
            rng: 0x9E37_79B9_7F4A_7C15,
            prefetched_upto: HashMap::new(),
            skip: false,
            queue: Default::default(),
            yielded: false,
            budget_location: None,
            last_budget_report_ms: f64::NEG_INFINITY,
            pressed_keys: HashSet::new(),
            key_clicks: HashSet::new(),
            key_repeats: HashSet::new(),
            mouse_left: false,
            mouse_right: false,
            mouse_left_click: false,
            mouse_right_click: false,
            wheel_up: false,
            wheel_down: false,
            read_texts: HashSet::new(),
            unimpl: Default::default(),
            db: Default::default(),
            history_max: 0,
            history: VecDeque::new(),
            history_formats: HashMap::new(),
            cabinets: HashMap::new(),
            timers: HashMap::new(),
            anim_clock: 0.0,
            last_text_target: None,
            play_dbg: HashMap::new(),
            prevmenus: HashMap::new(),
            cond_filter: std::env::var("LIVENYAN_CONDS")
                .ok()
                .filter(|s| !s.is_empty()),
            clocks: HashMap::new(),
            enabled: HashMap::new(),
            visible: HashMap::new(),
            selected: HashMap::new(),
            checked: HashMap::new(),
            script_log: vec![],
            edges: HashMap::new(),
            cond_seen: HashMap::new(),
            visited: Default::default(),
            flip_plan: None,
            wipe_fade: None,
            plates: Vec::new(),
            base_plate: None,
            base_born: None,
            frame_id: 0,
            text_char_ms: HashMap::new(),
            quake: None,
            wipe_flash: None,
            mem_files: HashMap::new(),
            screen_rgba: None,
            pending_savescreen: None,
            vfs_image_cache: std::cell::RefCell::new(HashMap::new()),
        };
        vm.prefetch_from(&script, 0);
        vm.frames.push(Frame {
            script,
            pc: 0,
            locals: HashMap::new(),
            result_var: String::new(),
            params: vec![],
            saved_state: None,
            sender: None,
            event_prop: None,
            auto_scrap_sender: false,
        });
        Ok(vm)
    }

    pub fn set_menu_font_override(&mut self, font: Option<(PathBuf, u32)>) {
        self.menu_font_override = font;
    }

    pub fn pixel_fonts(&self) -> Arc<PixelFontCatalog> {
        self.pixel_fonts.clone()
    }

    pub fn set_translations(&mut self, translations: HashMap<String, String>) {
        self.translations = translations;
    }

    pub fn location(&self) -> String {
        match self.frames.last() {
            Some(f) => format!("{}:{}", f.script.name, f.pc),
            None => "<ended>".into(),
        }
    }

    pub fn budget_location(&self) -> Option<&str> {
        self.budget_location.as_deref()
    }

    fn current_instruction_location(&self) -> Option<String> {
        let frame = self.frames.last()?;
        let command = frame.script.commands.get(frame.pc)?;
        Some(format!(
            "{} L{} {} (#{})",
            frame.script.name,
            command.line_no,
            command.name(),
            frame.pc
        ))
    }

    /// Firma de progreso para --trace: pila de scripts y número de comandos "con efecto"
    /// (no cuenta control de flujo, así un While de espera cuenta como inactivo).
    pub fn progress_sig(&self) -> (String, u64) {
        // Progreso = ejecutar algún comando nunca visto. Un bucle (de espera o de un
        // manejador) que repite las mismas posiciones cuenta como inactivo.
        let stack: Vec<&str> = self.frames.iter().map(|f| f.script.name.as_str()).collect();
        (
            stack.first().copied().unwrap_or_default().to_string(),
            self.visited.len() as u64,
        )
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
                if s.len() > 2
                    && s.len() < 260
                    && let Some(n) = data.resolve(s, ALL_EXTS)
                {
                    self.assets.prefetch(&n);
                }
            }
            if let Some(p) = c.page()
                && let Some(n) = crate::game::resolve_lsb(&data, &p.page)
            {
                self.assets.prefetch(&n);
            }
        }
        self.prefetched_upto
            .insert(script.name.clone(), end.max(upto));
    }

    // ---------- variables ----------

    fn lookup(&self, name: &str) -> Option<&Value> {
        if name == "@SelStr" {
            Some(&self.selected_string)
        } else {
            self.frames
                .iter()
                .rev()
                .find_map(|frame| frame.locals.get(name))
                .or_else(|| self.globals.get(name))
        }
    }

    pub fn var(&self, name: &str) -> Option<&Value> {
        self.lookup(name)
    }

    fn assign(&mut self, name: &str, v: Value) {
        if name == "@SelStr" {
            self.selected_string = v;
            return;
        }
        if name == "@ActiveCompo" {
            self.active_component = v.as_str();
            if let Some(edit) = self.scene.get(&self.active_component) {
                self.edit_cursors.insert(
                    self.active_component.clone(),
                    edit.str(Prop::TEXT).chars().count(),
                );
            }
            return;
        }
        if let Some(slot) = self
            .frames
            .iter_mut()
            .rev()
            .find_map(|frame| frame.locals.get_mut(name))
        {
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
        let names = self.play_names();
        for n in &names {
            let Some(p) = self.play_mut(n) else {
                continue;
            };
            if p.pos < p.cur.len() {
                let rest: Vec<TextItem> = p.cur[p.pos..].to_vec();
                p.pos = p.cur.len();
                if let Some(o) = self.scene.get_mut(n) {
                    o.text.extend(rest);
                    o.shown = o.text.len();
                    o.text_rev += 1;
                    let text: String = o
                        .text
                        .iter()
                        .map(|item| match item {
                            TextItem::Char(ch) => ch.as_str(),
                            TextItem::Newline => "\n",
                            TextItem::Image { .. } => "",
                        })
                        .collect();
                    o.props.insert(Prop::TEXT, Value::Str(text));
                }
                revealed = true;
            }
        }
        if revealed {
            return;
        }
        // 2) esperando clic dentro del texto -> continuar
        let mut released = false;
        let mut released_names = Vec::new();
        for name in self.play_names() {
            let Some(p) = self.play_mut(&name) else {
                continue;
            };
            if p.waiting_click {
                p.waiting_click = false;
                released = true;
                released_names.push(name);
            }
        }
        for name in &released_names {
            if let Some(o) = self.scene.get_mut(name) {
                o.props.insert(Prop::TEXTPAUSED, Value::Bool(false));
            }
        }
        if released {
            return;
        }
        let mut skipped_wait = false;
        for name in self.play_names() {
            let Some(playback) = self.play_mut(&name) else {
                continue;
            };
            if playback.click_skips_wait && playback.wait_for.take().is_some() {
                playback.click_skips_wait = false;
                skipped_wait = true;
            }
        }
        if skipped_wait {
            return;
        }
        let one_shots = self.one_shot_cinemas();
        if !one_shots.is_empty() {
            // clic salta los cinemas de una sola pasada; los que repiten (HUD) siguen
            for name in one_shots {
                if let Some(Clock::Cinema { movie, until }) = self.clocks.get_mut(&name) {
                    *until = Some(self.now);
                    if let Some(movie) = movie {
                        movie.started_at = self.now - movie.duration_ms;
                    }
                }
            }
            return;
        }
        if let State::WaitUntil {
            skippable: true, ..
        } = self.state
        {
            self.state = State::Running;
            self.finish_flip_wait();
        }
    }

    pub fn active_edit_name(&self) -> Option<&str> {
        if matches!(self.state, State::Menu { .. }) {
            return None;
        }
        let name = self.active_component.as_str();
        self.scene
            .get(name)
            .filter(|o| o.kind == Kind::Edit && self.scene.parents_visible(o))
            .map(|o| o.name.as_str())
    }

    pub fn focus_edit_at(&mut self, x: f64, y: f64) -> bool {
        let target = self
            .scene
            .draw_order()
            .into_iter()
            .rev()
            .find(|o| {
                if o.kind != Kind::Edit || !self.scene.effective_visible(o) {
                    return false;
                }
                let (left, top) = self.scene.abs_pos(o);
                let (width, height) = (o.int(Prop::WIDTH, 0) as f64, o.int(Prop::HEIGHT, 0) as f64);
                x >= left && y >= top && x < left + width && y < top + height
            })
            .map(|o| o.name.clone());
        let Some(target) = target else {
            if !self.active_component.is_empty() {
                let old = std::mem::take(&mut self.active_component);
                self.fire(&old, Prop::ONFOCUSOUT, vec![]);
            }
            return false;
        };
        if self.active_component != target {
            if !self.active_component.is_empty() {
                let old = std::mem::replace(&mut self.active_component, target.clone());
                self.fire(&old, Prop::ONFOCUSOUT, vec![]);
            } else {
                self.active_component = target.clone();
            }
            let text = self
                .scene
                .get(&target)
                .map(|o| o.str(Prop::TEXT))
                .unwrap_or_default();
            self.edit_cursors
                .insert(target.clone(), text.chars().count());
            self.fire(&target, Prop::ONFOCUSIN, vec![]);
        }
        true
    }

    pub fn edit_key(&mut self, key: i64) -> bool {
        let Some(name) = self.active_edit_name().map(str::to_owned) else {
            return false;
        };
        let text = self
            .scene
            .get(&name)
            .map(|o| o.str(Prop::TEXT))
            .unwrap_or_default();
        let mut chars: Vec<char> = text.chars().collect();
        let cursor = self.edit_cursors.entry(name.clone()).or_insert(chars.len());
        *cursor = (*cursor).min(chars.len());
        match key {
            8 if !self
                .scene
                .get(&name)
                .is_some_and(|o| o.get(Prop::READONLY).is_some_and(Value::truthy)) =>
            {
                if *cursor > 0 {
                    *cursor -= 1;
                    chars.remove(*cursor);
                    self.scene
                        .set(&name, Prop::TEXT, Value::Str(chars.iter().collect()));
                }
            }
            46 if !self
                .scene
                .get(&name)
                .is_some_and(|o| o.get(Prop::READONLY).is_some_and(Value::truthy)) =>
            {
                if *cursor < chars.len() {
                    chars.remove(*cursor);
                    self.scene
                        .set(&name, Prop::TEXT, Value::Str(chars.iter().collect()));
                }
            }
            37 => *cursor = cursor.saturating_sub(1),
            39 => *cursor = (*cursor + 1).min(chars.len()),
            36 => *cursor = 0,
            35 => *cursor = chars.len(),
            _ => {}
        }
        self.update_edit_caret(&name);
        self.fire(&name, Prop::ONNOTIFY, vec![Value::Int(key)]);
        true
    }

    pub fn insert_edit_text(&mut self, input: &str) -> bool {
        let Some(name) = self.active_edit_name().map(str::to_owned) else {
            return false;
        };
        let Some(edit) = self.scene.get(&name) else {
            return false;
        };
        if edit.get(Prop::READONLY).is_some_and(Value::truthy) {
            return true;
        }
        let mut chars: Vec<char> = edit.str(Prop::TEXT).chars().collect();
        let cursor = self.edit_cursors.entry(name.clone()).or_insert(chars.len());
        *cursor = (*cursor).min(chars.len());
        for ch in input.chars().filter(|ch| !ch.is_control()) {
            chars.insert(*cursor, ch);
            *cursor += 1;
        }
        self.scene
            .set(&name, Prop::TEXT, Value::Str(chars.iter().collect()));
        self.update_edit_caret(&name);
        true
    }

    fn update_edit_caret(&mut self, name: &str) {
        if let Some(o) = self.scene.get(name) {
            let cursor = self.edit_cursors.get(name).copied().unwrap_or(0);
            let text = o.str(Prop::TEXT);
            let font_height = o.int(Prop::FONTHEIGHT, 16).max(1);
            let width = text
                .chars()
                .take(cursor)
                .map(|ch| {
                    if ch.is_ascii() {
                        font_height / 2
                    } else {
                        font_height
                    }
                })
                .sum();
            self.scene.set(name, Prop::CARETX, Value::Int(width));
        }
    }

    /// Elección de menú (índice).
    pub fn choose(&mut self, i: usize) {
        let State::Menu {
            name,
            choices: _,
            values,
            var,
        } = &self.state
        else {
            return;
        };
        let (name, var, value) = (name.clone(), var.clone(), values.get(i).cloned());
        let Some(value) = value else { return };
        if self.opts.trace {
            println!(
                "  -> menu choice #{i}{}",
                if self.opts.show_text {
                    format!(" {value:?}")
                } else {
                    String::new()
                }
            );
        }
        self.assign(&var, Value::Str(value.clone()));
        self.assign("@SelStr", Value::Str(value.clone()));
        self.selected
            .insert((name.clone(), MenuItemKey::Index(i as i64)), true);
        if let Some(menu) = self.scene.get_mut(&name) {
            menu.props.insert(Prop::INDEX, Value::Int(i as i64));
            menu.props.insert(Prop::VALUE, Value::Int(i as i64));
        }
        // El menú sigue en `State::Menu`. ONSELECT se despacha igual; al volver
        // se restaura solo si el menú sigue en pantalla.
        let has_handler = self.fire(
            &name,
            Prop::ONSELECT,
            vec![Value::Str(value), Value::Int(i as i64)],
        );
        if !has_handler {
            self.state = State::Running;
            if self.component_auto_scrap(&name) {
                self.scrap_component(&name);
            }
        }
    }

    /// ¿Hay texto esperando un clic del usuario?
    pub fn wants_click(&self) -> bool {
        self.scene.live().any(|object| {
            object
                .play
                .as_ref()
                .is_some_and(|p| p.waiting_click || p.pos < p.cur.len())
        })
    }

    /// ONIDLE de cajas de mensaje (アイドル時.lsb): avance, menú de controles, etc.
    /// Solo si no hay evento anidado ya en curso y la caja no está tipeando glifos.
    fn fire_message_idle(&mut self) {
        if self.frames.iter().any(|f| f.saved_state.is_some()) || !self.queue.is_empty() {
            return;
        }
        let names: Vec<String> = self
            .scene
            .draw_order()
            .into_iter()
            .filter(|o| {
                o.kind == Kind::Message
                    && self.scene.effective_visible(o)
                    && o.get(Prop::ONIDLE)
                        .is_some_and(|v| !v.as_str().trim().is_empty())
            })
            .map(|o| o.name.clone())
            .collect();
        for name in names {
            let typing = self
                .play_of(&name)
                .is_some_and(|p| p.pos < p.cur.len() && !p.waiting_click);
            if typing {
                continue;
            }
            if self
                .queue
                .iter()
                .any(|e| e.obj == name && e.prop == Prop::ONIDLE)
            {
                continue;
            }
            self.fire(&name, Prop::ONIDLE, vec![]);
        }
    }

    /// Encola el manejador de evento `prop` del objeto, si lo tiene.
    pub fn fire(&mut self, obj: &str, prop: u8, params: Vec<Value>) -> bool {
        let script = self.scene.get(obj).map(|o| o.str(prop)).unwrap_or_default();
        if script.trim().is_empty() {
            log::debug!("event {obj}.{}: no handler", crate::lsb::prop_name(prop));
            return false;
        }
        log::debug!(
            "event enqueue {obj}.{} -> {script} (queue {})",
            crate::lsb::prop_name(prop),
            self.queue.len() + 1
        );
        self.queue.push_back(ObjEvent {
            obj: obj.to_string(),
            prop,
            script,
            params,
        });
        true
    }

    /// Ruta de scripts que se repite al final de la ejecución: (periodo, repeticiones).
    pub fn cycle(&self) -> Option<(usize, usize)> {
        let l = &self.script_log;
        for p in 1..=400.min(l.len() / 3) {
            let tail = &l[l.len() - p..];
            let mut reps = 1;
            while (reps + 1) * p <= l.len()
                && &l[l.len() - (reps + 1) * p..l.len() - reps * p] == tail
            {
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
        self.dispatch_events_with_nesting(false);
    }

    fn dispatch_events_from_do_event(&mut self) {
        self.dispatch_events_with_nesting(true);
    }

    fn dispatch_events_with_nesting(&mut self, allow_nested: bool) {
        let menu_event = self
            .queue
            .front()
            .is_some_and(|ev| matches!(ev.prop, Prop::ONOPENING | Prop::ONSELECT));
        if matches!(self.state, State::Ended)
            || (matches!(self.state, State::Menu { .. }) && !menu_event)
            || self.frames.len() > 200
        {
            return;
        }
        // un manejador a la vez: si ya hay uno activo, esperar a que termine
        if !allow_nested
            && self.frames.iter().any(|f| f.saved_state.is_some())
            && matches!(self.state, State::Running)
        {
            return;
        }
        if let Some(ev) = self.queue.pop_front() {
            log::debug!(
                "event dispatch {}.{} -> {} (depth {})",
                ev.obj,
                crate::lsb::prop_name(ev.prop),
                ev.script,
                self.frames.len()
            );
            if self.opts.trace {
                println!(
                    "[{:>8.0}ms] event {}.{} -> {}",
                    self.now,
                    ev.obj,
                    crate::lsb::prop_name(ev.prop),
                    ev.script
                );
            }
            let auto_scrap_sender = match ev.prop {
                Prop::ONSELECT => self.component_auto_scrap(&ev.obj),
                Prop::ONRBTNUP => self
                    .scene
                    .get(&ev.obj)
                    .is_some_and(|o| o.get(Prop::RCLICKSCRAP).is_some_and(Value::truthy)),
                Prop::ONCLOSING => self.component_auto_scrap(&ev.obj),
                _ => false,
            };
            let saved = std::mem::replace(&mut self.state, State::Running);
            if !self.goto_script(&ev.script, 0, true, String::new(), ev.params) {
                self.state = saved;
                if auto_scrap_sender {
                    self.scrap_component(&ev.obj);
                }
                return;
            }
            if let Some(f) = self.frames.last_mut() {
                f.saved_state = Some(saved);
                f.sender = Some(ev.obj.clone());
                f.event_prop = Some(ev.prop);
                f.auto_scrap_sender = auto_scrap_sender;
            }
        }
    }

    /// Avanza un frame: reloj, animaciones, texto, eventos y ejecución
    /// cooperativa (un While/Wait 0 cede el resto del frame).
    pub fn update(&mut self, now: f64, budget: usize) -> usize {
        self.now = now;
        self.frame_id = self.frame_id.saturating_add(1);
        self.scene.set_cohort(self.frame_id);
        self.scene.update(now);
        if self
            .quake
            .as_ref()
            .is_some_and(|q| now >= q.until || q.stop_until.is_some_and(|s| now >= s))
        {
            self.quake = None;
        }
        if self
            .wipe_flash
            .as_ref()
            .is_some_and(|f| f.rgba(now).is_none())
        {
            self.wipe_flash = None;
        }
        self.update_plays();
        if let State::WaitUntil { until, skippable } = self.state {
            // StopEvent 0: el skip de teclado no corta esta espera (step_wait).
            if now >= until || (self.skip && skippable) {
                self.state = State::Running;
                self.finish_flip_wait();
            }
        }
        self.update_timers();
        self.update_anims();
        self.update_cinemas();
        self.update_videos();
        self.fire_message_idle();
        self.dispatch_events();
        self.yielded = false;
        let mut n = 0;
        let mut budget_location = None;
        let mut sampled_locations: HashMap<String, u64> = HashMap::new();
        while matches!(self.state, State::Running) && n < budget && !self.yielded {
            if !self.opts.trace
                && n % 16 == 0
                && let Some(location) = self.current_instruction_location()
            {
                *sampled_locations.entry(location).or_default() += 1;
            }
            if n + 1 == budget {
                budget_location = self.current_instruction_location();
            }
            self.step();
            n += 1;
            if !self.queue.is_empty() && self.frames.iter().all(|f| f.saved_state.is_none()) {
                self.dispatch_events();
            }
        }
        self.budget_location =
            if n == budget && matches!(self.state, State::Running) && !self.yielded {
                budget_location
            } else {
                None
            };
        if self.budget_location.is_some() && now - self.last_budget_report_ms >= 1000.0 {
            self.last_budget_report_ms = now;
            let mut hottest: Vec<_> = sampled_locations.into_iter().collect();
            hottest.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            let summary = hottest
                .iter()
                .take(8)
                .map(|(location, samples)| format!("{location} ~{} steps", samples * 16))
                .collect::<Vec<_>>()
                .join("; ");
            log::warn!(
                "VM exhausted its {budget}-step frame budget at {}; sampled hotspots: {summary}",
                self.budget_location.as_deref().unwrap_or("<unknown>")
            );
        }
        // el texto puede haber empezado en este frame
        self.update_plays();
        self.mouse_left_click = false;
        self.mouse_right_click = false;
        self.key_clicks.clear();
        self.key_repeats.clear();
        self.wheel_up = false;
        self.wheel_down = false;
        n
    }

    pub fn set_key_push(&mut self, key: usize, pressed: bool) {
        if key >= KEY_SLOTS {
            return;
        }
        if pressed {
            if self.pressed_keys.insert(key) {
                self.key_clicks.insert(key);
                self.key_repeats.insert(key);
            }
        } else {
            self.pressed_keys.remove(&key);
        }
    }

    /// Repetición de tecla (el flanco ya se registró). `@KeyRepeat`, no `@KeyClick`.
    pub fn set_key_repeat(&mut self, key: usize) {
        if key < KEY_SLOTS {
            self.pressed_keys.insert(key);
            self.key_repeats.insert(key);
        }
    }

    /// Botón del ratón. `down` en el flanco de pulsación también deja `@LClick` / `@RClick`.
    pub fn set_mouse_button(&mut self, right: bool, down: bool) {
        if right {
            self.mouse_right = down;
            if down {
                self.mouse_right_click = true;
            }
        } else {
            self.mouse_left = down;
            if down {
                self.mouse_left_click = true;
            }
        }
    }

    fn key_flags(&self, keys: &HashSet<usize>) -> Value {
        Value::Array(
            (0..KEY_SLOTS)
                .map(|key| Value::Bool(keys.contains(&key)))
                .collect(),
        )
    }

    fn prevmenu_button(&self, menu: &str, item: Option<&Value>) -> Option<&PmButton> {
        let menu = self.prevmenus.get(menu)?;
        let index = match item {
            Some(Value::Str(name)) => name
                .trim()
                .parse::<usize>()
                .ok()
                .or_else(|| menu.buttons.iter().position(|button| button.name == *name)),
            Some(value) => usize::try_from(value.as_int()).ok(),
            None => None,
        }?;
        menu.buttons.get(index)
    }

    /// `true` si este TextIns ya se mostró en la sesión. `ClrRead` vacía el conjunto.
    fn note_text_read(&mut self, script: &str, pc: usize) -> bool {
        let key = (script.to_string(), pc);
        let already = self.read_texts.contains(&key);
        self.read_texts.insert(key);
        already
    }

    pub fn set_mouse_wheel(&mut self, up: bool, down: bool) {
        self.wheel_up |= up;
        self.wheel_down |= down;
    }

    pub fn is_hovered(&self, name: &str) -> bool {
        self.hovered_object.as_deref() == Some(name)
    }

    pub fn animation_frame_blend(&self, name: &str, delays: &[u32]) -> Option<(usize, usize, f64)> {
        let object = self.scene.get(name)?;
        let repeat = object.get(Prop::REPEAT).is_some_and(Value::truthy);
        let start = object.anim.as_ref()?.started?;
        let total = delays
            .iter()
            .map(|delay| *delay as f64)
            .sum::<f64>()
            .max(1.0);
        let elapsed = (self.now - start).max(0.0);
        sample_animation_frames(delays, elapsed, total, repeat)
    }

    // ---------- ejecución ----------

    fn goto_script(
        &mut self,
        page: &str,
        label: u32,
        push: bool,
        result_var: String,
        params: Vec<Value>,
    ) -> bool {
        // manejadores "script.lsc:ETIQUETA": empiezan en el comando Label con ese nombre
        let mut label = label;
        let mut named: Option<String> = None;
        let page = match page.rsplit_once(':') {
            Some((a, l))
                if !l.is_empty()
                    && !l.contains(['\\', '/', '.'])
                    && [".lsc", ".lsb"]
                        .iter()
                        .any(|x| a.to_lowercase().ends_with(x)) =>
            {
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
                    self.warn(
                        &format!("missing:{page}"),
                        &format!("script not found: {page}"),
                    );
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
            match script.commands.iter().position(|c| {
                CmdType::from(c.kind) == CmdType::Label && c.str("Name") == Some(l.as_str())
            }) {
                Some(i) => label = i as u32,
                None => {
                    self.warn(
                        &format!("label:{}:{l}", script.name),
                        &format!("label {l:?} not found in {}", script.name),
                    );
                }
            }
        }
        // Cada Call/Jump. El bucle del cuadro de mensaje lo repite en cada tick.
        log::debug!(
            "enter script {}:{label}{} ({})",
            script.name,
            named
                .as_ref()
                .map(|l| format!(" (label {l:?})"))
                .unwrap_or_default(),
            if push { "call" } else { "jump" }
        );
        let pc = (label as usize).min(script.commands.len());
        if self.opts.trace && self.script_log.len() < 1_000_000 {
            self.script_log.push(script.name.clone());
        }
        self.prefetch_from(&script, pc);
        if push {
            let mut locals = HashMap::new();
            // Convención provisional: parámetros como Param1..N (no verificado con juegos reales).
            for (i, p) in params.iter().enumerate() {
                locals.insert(format!("Param{}", i + 1), p.clone());
            }
            self.frames.push(Frame {
                script,
                pc,
                locals,
                result_var,
                params,
                saved_state: None,
                sender: None,
                event_prop: None,
                auto_scrap_sender: false,
            });
        } else if let Some(f) = self.frames.last_mut() {
            f.script = script;
            f.pc = pc;
        }
        true
    }

    fn props_of(&mut self, c: &Command) -> HashMap<u8, Value> {
        if matches!(
            CmdType::from(c.kind),
            CmdType::ImgNew | CmdType::MapImgNew | CmdType::TileNew | CmdType::Button
        ) {
            let name_expr = c
                .props()
                .iter()
                .find_map(|(prop, expr)| (*prop == Prop::NAME).then_some(expr))
                .filter(|expr| !expr.is_empty());
            let source_expr = c
                .props()
                .iter()
                .find_map(|(prop, expr)| (*prop == Prop::SOURCE).then_some(expr))
                .filter(|expr| !expr.is_empty());
            if let (Some(name_expr), Some(source_expr)) = (name_expr, source_expr) {
                let name = eval(name_expr, self).as_str();
                let source = eval(source_expr, self).as_str();
                let dimensions = self
                    .slider_asset_size(&source)
                    .map(|(width, height)| (name.clone(), width, height));
                let previous =
                    std::mem::replace(&mut self.pending_component_dimensions, dimensions);
                let mut props = HashMap::new();
                for (prop, expr) in c.props() {
                    if expr.is_empty() {
                        continue;
                    }
                    let value = match *prop {
                        Prop::NAME => Value::Str(name.clone()),
                        Prop::SOURCE => Value::Str(source.clone()),
                        _ => eval(expr, self),
                    };
                    props.insert(*prop, value);
                }
                self.pending_component_dimensions = previous;
                return props;
            }
        }

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
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(" {detail}")
                }
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
        if self.opts.trace {
            self.visited
                .insert((Arc::as_ptr(&script) as *const u8 as usize, pc));
        }
        let c = &script.commands[pc];
        self.steps += 1;
        if self.opts.trace {
            *self.stats.entry(c.name()).or_default() += 1;
        }
        let mut next = pc + 1;
        let kind = CmdType::from(c.kind);
        let mut detail = String::new();
        let show = self.opts.show_text;
        match kind {
            CmdType::Calc => {
                detail = self.step_calc(c, show);
            }
            CmdType::If => {
                next = self.step_if(&script, pc, c, next, &mut detail);
            }
            CmdType::Elseif | CmdType::Else => {
                next = self.step_elseif_else(&script, pc);
            }
            CmdType::Label | CmdType::Comment => {
                detail = self.step_label_comment(kind, c);
            }
            CmdType::Jump => {
                if self.step_jump(&script, pc, c, next, &mut detail) {
                    return;
                }
            }
            CmdType::Call => {
                if self.step_call(&script, pc, c, next, &mut detail) {
                    return;
                }
            }
            CmdType::PCReset => {
                if self.step_pc_reset(c, &mut detail) {
                    return;
                }
            }
            CmdType::Reset => {
                if self.step_reset(c, &mut detail) {
                    return;
                }
            }
            CmdType::IFDEF | CmdType::IFNDEF | CmdType::ENDIF => {
                // Directivas de compilación del chart; en LSB suelen no aparecer.
                // Si llegan, no-op (**no confirmado** runtime LiveMaker).
                detail = c.name().into();
            }
            CmdType::Exit => {
                if self.step_exit(c, &mut detail) {
                    return;
                }
            }
            CmdType::Terminate => {
                if self.step_terminate(c) {
                    return;
                }
            }
            CmdType::MenuClose => {
                detail = self.step_menu_close(c);
            }
            CmdType::Wait => {
                // Time 0: seguir en el comando mientras Calc sea verdadero
                // (sistema LiveNovel: ocultar el cuadro, menús, fin de reproducción).
                if self.step_wait(c, &mut detail) {
                    next = pc;
                    self.yielded = true;
                }
            }
            CmdType::VarNew => {
                self.step_var_new(c, &mut detail);
            }
            CmdType::VarDel => {
                self.step_var_del(c, &mut detail);
            }
            CmdType::While => {
                next = self.step_while(&script, pc, c, next, &mut detail);
            }
            CmdType::WhileInit => {
                self.step_while_init(c);
            }
            CmdType::WhileLoop => {
                next = self.step_while_loop(&script, c);
            }
            CmdType::Break => {
                next = self.step_break(&script, c, next);
            }
            CmdType::Continue => {
                next = self.step_continue(&script, c, next);
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
                detail = self.step_obj_new(kind, c);
            }
            CmdType::Menu => {
                detail = self.step_menu(c);
            }
            CmdType::Sound => {
                detail = self.step_sound(c);
            }
            CmdType::MediaPlay => {
                detail = self.step_media_play(c);
            }
            CmdType::Movie => {
                detail = self.step_movie(c);
            }
            CmdType::SetProp => {
                detail = self.step_set_prop(&script, pc, c);
            }
            CmdType::GetProp => {
                detail = self.step_get_prop(c);
            }
            CmdType::ObjDel => {
                detail = self.step_obj_del(&script, pc, c);
            }
            CmdType::PropMotion => {
                detail = self.step_prop_motion(c);
            }
            CmdType::TextIns => {
                self.step_text_ins(pc, c, next, show);
                return;
            }
            CmdType::TextClr => {
                self.step_text_clr(c);
            }
            CmdType::Flip => {
                detail = self.step_flip(c);
            }
            CmdType::SaveCabinet | CmdType::LoadCabinet => {
                detail = self.step_cabinet(kind, c);
            }
            CmdType::DoEvent => {
                self.step_do_event();
            }
            CmdType::MovieStop => {
                self.step_movie_stop(c);
            }
            CmdType::GameSave => {
                detail = self.step_game_save(c);
            }
            CmdType::GameLoad => {
                if self.step_game_load(c, &mut detail) {
                    return;
                }
            }
            CmdType::ClrHist => {
                self.step_clr_hist();
            }
            CmdType::ClrRead => {
                self.read_texts.clear();
            }
            CmdType::FormatHist => {
                self.step_format_hist(c);
            }
            CmdType::CallHist => {
                self.step_call_hist(c);
            }
            _ => {
                self.warn(
                    &format!("unimpl:cmd {}", c.name()),
                    &format!("unimplemented command {}", c.name()),
                );
            }
        }
        self.trace(c, detail);
        if let Some(f) = self.frames.last_mut()
            && Arc::ptr_eq(&f.script, &script)
            && f.pc == pc
        {
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
            let onselect = f.event_prop == Some(Prop::ONSELECT);
            let idle = f.event_prop == Some(Prop::ONIDLE);
            let sender = f.sender.clone();
            if f.event_prop == Some(Prop::ONCLOSING)
                && let Some(name) = f.sender.as_deref()
            {
                self.fire(name, Prop::ONCLOSED, vec![]);
            }
            if f.auto_scrap_sender
                && let Some(name) = f.sender.as_deref()
            {
                self.scrap_component(name);
            }
            if !f.result_var.is_empty() {
                let ret = self
                    .lookup("@Result")
                    .cloned()
                    .filter(|val| val != &Value::Empty)
                    .unwrap_or(v);
                self.assign(&f.result_var, ret);
            }
            if let Some(st) = f.saved_state {
                // vuelta de un manejador de evento: restaurar lo que hacía el script.
                // ONSELECT deja el menú modal solo si sigue en pantalla.
                // Ceder el frame: si el script de abajo sigue en el mismo turno,
                // pisa la animación que el evento acaba de arrancar.
                // ONIDLE no: el cuadro lo dispara cada tick y, al volver al instante,
                // se comería el turno de 終了待ち antes de ver __メッセージ終了.
                self.state = match st {
                    State::Menu { ref name, .. }
                        if onselect
                            && (sender.as_deref() != Some(name.as_str())
                                || !self.menu_still_modal(name)) =>
                    {
                        State::Running
                    }
                    other => other,
                };
                if !idle {
                    self.yielded = true;
                }
            }
        }
    }

    fn component_auto_scrap(&self, name: &str) -> bool {
        self.scene
            .get(name)
            .is_some_and(|o| o.get(Prop::AUTOSCRAP).is_some_and(Value::truthy))
    }

    fn scrap_component(&mut self, name: &str) {
        let gone = self.scene.delete_names(name);
        self.retire_objects(gone);
    }

    fn retire_objects(&mut self, objects: Vec<crate::scene::Obj>) {
        for o in objects {
            self.clocks.remove(&o.name);
            self.timers.remove(&o.name);
            self.prevmenus.remove(&o.name);
            self.forget_deleted_component(&o.name);
            self.scene.retain_deleted(o);
        }
    }

    fn forget_deleted_component(&mut self, name: &str) {
        self.edit_cursors.remove(name);
        self.timers.remove(name);
        self.enabled.retain(|(object, _), _| object != name);
        self.visible.retain(|(object, _), _| object != name);
        self.selected.retain(|(object, _), _| object != name);
        self.checked.retain(|(object, _), _| object != name);
        if self.active_component == name {
            self.active_component.clear();
        }
    }
}

struct HitSpot {
    name: String,
    kind: Kind,
    mouse: i64,
    ignores: bool,
    handler: bool,
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Image => "Image",
        Kind::Message => "Message",
        Kind::Box => "Box",
        Kind::Tile => "Tile",
        Kind::Button => "Button",
        Kind::Slider => "Slider",
        Kind::Edit => "Edit",
        Kind::Caption => "Caption",
        Kind::Menu => "Menu",
        Kind::Movie => "Movie",
        Kind::Other(_) => "Other",
    }
}

/// Primer botón bajo el punto que no es `except`.
fn buried_button(spots: &[HitSpot], except: &str) -> String {
    let Some(spot) = spots
        .iter()
        .find(|spot| spot.kind == Kind::Button && spot.name != except)
    else {
        return String::new();
    };
    let mut note = format!("under={}/Button", spot.name);
    if spot.ignores {
        note.push_str(&format!(" mouse={}", spot.mouse));
    } else if !spot.handler {
        note.push_str(" no-handler");
    }
    note
}

fn pointer_box(assets: &crate::assets::Assets, object: &crate::scene::Obj) -> (f64, f64) {
    let intrinsic = match object.kind {
        Kind::Image | Kind::Button | Kind::Tile => {
            slider::source_pixel_size(assets, &object.str(Prop::SOURCE))
        }
        _ => None,
    };
    crate::scene::pointer_box(
        object.int(Prop::WIDTH, 0),
        object.int(Prop::HEIGHT, 0),
        intrinsic,
    )
}

impl Vm {
    /// Movimiento del ratón: hover de PrevMenu (ONMAPIN/ONMAPOUT con @ParamStr = [nombre, índice]).
    pub fn mouse_move(&mut self, x: f64, y: f64) {
        self.mouse = (x, y);
        let hit = self.prevmenu_hit(x, y);
        let names: Vec<String> = self.prevmenus.keys().cloned().collect();
        for n in names {
            let now = hit.as_ref().filter(|h| h.0 == n).map(|h| h.1);
            let before = self.prevmenus[&n].hover;
            if now == before {
                continue;
            }
            self.prevmenus.get_mut(&n).unwrap().hover = now;
            let bname = |vm: &Self, i: usize| {
                vm.prevmenus[&n]
                    .buttons
                    .get(i)
                    .map(|b| b.name.clone())
                    .unwrap_or_default()
            };
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
        let assets = Arc::clone(&self.assets);
        let hovered = self
            .scene
            .draw_order()
            .into_iter()
            .rev()
            .find(|object| {
                let listens = matches!(object.kind, Kind::Button | Kind::Slider)
                    || object
                        .get(Prop::ONMOUSEIN)
                        .is_some_and(|v| !v.as_str().trim().is_empty());
                if !listens
                    || object.ignores_mouse(crate::scene::MouseHit::Hover)
                    || !self.scene.effective_visible(object)
                {
                    return false;
                }
                let (left, top, width, height) = if object.kind == Kind::Slider {
                    self.slider_bounds(object)
                } else {
                    let (width, height) = pointer_box(&assets, object);
                    self.scene.placed_rect(object, width, height)
                };
                width > 0.0
                    && height > 0.0
                    && x >= left
                    && y >= top
                    && x < left + width
                    && y < top + height
            })
            .map(|object| object.name.clone());
        if hovered != self.hovered_object {
            if let Some(old) = self.hovered_object.take() {
                self.fire(&old, Prop::ONMOUSEOUT, vec![]);
            }
            if let Some(new) = &hovered {
                self.fire(new, Prop::ONMOUSEIN, vec![]);
            }
            self.hovered_object = hovered;
        }
        if let Some(name) = self.slider_drag.clone() {
            self.set_slider_from_pointer(&name, x, y);
        }
    }

    /// Script de escena: el marco más cercano que no es el bucle del cuadro de mensaje.
    fn click_scene(&self) -> &str {
        self.frames
            .iter()
            .rev()
            .find(|frame| !frame.script.name.contains("メッセージボックス"))
            .or_else(|| self.frames.last())
            .map(|frame| frame.script.name.as_str())
            .unwrap_or("<none>")
    }

    /// Qué haría `click()` con el texto o la espera, sin ejecutarlo.
    fn click_advance(&self) -> &'static str {
        if self.scene.live().any(|object| {
            object
                .play
                .as_ref()
                .is_some_and(|play| play.pos < play.cur.len())
        }) {
            return "reveal";
        }
        if self
            .scene
            .live()
            .any(|object| object.play.as_ref().is_some_and(|play| play.waiting_click))
        {
            return "page";
        }
        if self.scene.live().any(|object| {
            object
                .play
                .as_ref()
                .is_some_and(|play| play.click_skips_wait && play.wait_for.is_some())
        }) {
            return "skip-wait";
        }
        if !self.one_shot_cinemas().is_empty() {
            return "cinema";
        }
        if matches!(
            self.state,
            State::WaitUntil {
                skippable: true,
                ..
            }
        ) {
            return "skip-wait";
        }
        "none"
    }

    /// Comando del script de escena (no el bucle del cuadro de mensaje) y si el menú de elección existe.
    fn stuck_choice(&self) -> String {
        let frame = self
            .frames
            .iter()
            .rev()
            .find(|frame| !frame.script.name.contains("メッセージボックス"))
            .or_else(|| self.frames.last());
        let cmd = match frame {
            Some(frame) => match frame.script.commands.get(frame.pc) {
                Some(command) => format!(
                    "cmd={} L{} {} #{}",
                    frame.script.name,
                    command.line_no,
                    command.name(),
                    frame.pc
                ),
                None => format!("cmd={} #{}", frame.script.name, frame.pc),
            },
            None => "cmd=<none>".to_string(),
        };
        let menu = if self.scene.get("選択メニュー").is_some() {
            "yes"
        } else {
            "no"
        };
        let top = match self.frames.last() {
            Some(frame) => match frame.script.commands.get(frame.pc) {
                Some(command) => format!(
                    "{} L{} {} #{}",
                    frame.script.name,
                    command.line_no,
                    command.name(),
                    frame.pc
                ),
                None => format!("{} #{}", frame.script.name, frame.pc),
            },
            None => "<none>".to_string(),
        };
        let flag = self
            .lookup("__メッセージ終了")
            .map(|value| format!("{value:?}"))
            .unwrap_or_else(|| "<unset>".to_string());
        format!(
            "{cmd} 選択メニュー={menu} top={top} state={:?} flag={flag} depth={}",
            self.state,
            self.frames.len()
        )
    }

    fn object_kind_name(&self, name: &str) -> &'static str {
        self.scene
            .get(name)
            .map(|object| kind_name(object.kind))
            .unwrap_or("-")
    }

    fn objects_under_pointer(
        &self,
        assets: &Assets,
        x: f64,
        y: f64,
        hit: crate::scene::MouseHit,
        props: &[u8],
    ) -> Vec<HitSpot> {
        self.scene
            .draw_order()
            .into_iter()
            .rev()
            .filter_map(|object| {
                if !self.scene.effective_visible(object) {
                    return None;
                }
                let (width, height) = pointer_box(assets, object);
                let (left, top, width, height) = self.scene.placed_rect(object, width, height);
                if !(width > 0.0
                    && height > 0.0
                    && x >= left
                    && y >= top
                    && x < left + width
                    && y < top + height)
                {
                    return None;
                }
                let handler = props.iter().any(|prop| {
                    object
                        .get(*prop)
                        .is_some_and(|value| !value.as_str().trim().is_empty())
                });
                Some(HitSpot {
                    name: object.name.clone(),
                    kind: object.kind,
                    mouse: object.int(Prop::IGNOREMOUSE, 0),
                    ignores: object.ignores_mouse(hit),
                    handler,
                })
            })
            .collect()
    }

    fn log_click(
        &self,
        x: f64,
        y: f64,
        right: bool,
        object: &str,
        kind: &str,
        result: &str,
        delivers_click: bool,
        extra: &str,
    ) {
        let effect = self.click_advance();
        let advance = if delivers_click {
            format!(" advance={effect}")
        } else if effect == "none" {
            String::new()
        } else {
            format!(" advance=held:{effect}")
        };
        let side = if right { " right" } else { "" };
        let extra = if extra.is_empty() {
            String::new()
        } else {
            format!(" {extra}")
        };
        let stall = if delivers_click && effect == "none" {
            format!(" {}", self.stuck_choice())
        } else {
            String::new()
        };
        log::info!(
            "click ({x:.0},{y:.0}){side} scene={} object={object} kind={kind} result={result}{advance}{extra}{stall}",
            self.click_scene()
        );
    }

    fn log_unhandled_click(&self, x: f64, y: f64, right: bool, spots: &[HitSpot]) {
        let delivers = !right;
        let Some(top) = spots.first() else {
            let result = if delivers && self.click_advance() != "none" {
                "advance"
            } else {
                "noop"
            };
            self.log_click(x, y, right, "(none)", "-", result, delivers, "");
            return;
        };
        let result = if top.ignores { "ignored" } else { "no-handler" };
        let mut extra = Vec::new();
        if top.ignores {
            extra.push(format!("mouse={}", top.mouse));
        }
        let buried = buried_button(spots, &top.name);
        if !buried.is_empty() {
            extra.push(buried);
        }
        self.log_click(
            x,
            y,
            right,
            &top.name,
            kind_name(top.kind),
            result,
            delivers,
            &extra.join(" "),
        );
    }

    /// Clic (izquierdo o derecho) en coordenadas lógicas: manejadores de objeto o avance.
    pub fn click_at(&mut self, x: f64, y: f64, right: bool) {
        let assets = Arc::clone(&self.assets);
        if right {
            let modal_menu = match &self.state {
                State::Menu { name, .. } => Some(name.clone()),
                _ => None,
            };
            let menu = modal_menu.or_else(|| {
                self.scene
                    .draw_order()
                    .into_iter()
                    .rev()
                    .find(|o| {
                        if o.kind != Kind::Menu
                            || !self.scene.effective_visible(o)
                            || !o.get(Prop::RCLICKSCRAP).is_some_and(Value::truthy)
                        {
                            return false;
                        }
                        let (width, height) = pointer_box(&assets, o);
                        let (left, top, width, height) = self.scene.placed_rect(o, width, height);
                        x >= left && y >= top && x < left + width && y < top + height
                    })
                    .filter(|o| o.get(Prop::RCLICKSCRAP).is_some_and(Value::truthy))
                    .map(|o| o.name.clone())
            });
            if let Some(name) = menu
                && self
                    .scene
                    .get(&name)
                    .is_some_and(|o| o.get(Prop::RCLICKSCRAP).is_some_and(Value::truthy))
            {
                if matches!(&self.state, State::Menu { name: active, .. } if active == &name) {
                    self.state = State::Running;
                }
                let next = self
                    .scene
                    .get(&name)
                    .map(|object| object.str(Prop::ONRBTNUP))
                    .unwrap_or_default();
                let kind = self.object_kind_name(&name);
                if next.trim().is_empty() {
                    self.log_click(x, y, true, &name, kind, "scrap", false, "");
                    self.scrap_component(&name);
                } else if self.fire(&name, Prop::ONRBTNUP, vec![]) {
                    self.log_click(
                        x,
                        y,
                        true,
                        &name,
                        kind,
                        "ONRBTNUP",
                        false,
                        &format!("script={next}"),
                    );
                } else {
                    self.log_click(x, y, true, &name, kind, "scrap", false, "");
                    self.scrap_component(&name);
                }
                return;
            }
        } else {
            if self.begin_slider_drag(x, y) {
                let name = self.slider_focus.clone().unwrap_or_else(|| "slider".into());
                let kind = self.object_kind_name(&name);
                self.log_click(x, y, false, &name, kind, "drag", false, "");
                return;
            }
            self.focus_edit_at(x, y);
        }
        let props: &[u8] = if right {
            &[Prop::ONRBTNUP, Prop::ONRBTNDOWN]
        } else {
            &[
                Prop::ONLBTNUP,
                Prop::ONLBTNDOWN,
                Prop::ONSELECT,
                Prop::ONNOTIFY,
            ]
        };
        let menu_hit = (!right).then(|| self.prevmenu_hit(x, y)).flatten();
        let menu_key = menu_hit.as_ref().and_then(|(name, _)| {
            self.scene
                .get(name)
                .map(|o| (o.int(Prop::PRIORITY, 0), o.seq))
        });
        let mouse_hit = if right {
            crate::scene::MouseHit::Right
        } else {
            crate::scene::MouseHit::Left
        };
        let object_hit = self
            .scene
            .hit_handler_with(x, y, props, mouse_hit, |object| {
                pointer_box(&assets, object)
            })
            .map(|o| (o.name.clone(), (o.int(Prop::PRIORITY, 0), o.seq)));

        if let Some((n, i)) = menu_hit
            && menu_key.is_some_and(|menu_key| {
                object_hit
                    .as_ref()
                    .is_none_or(|(_, object_key)| menu_key >= *object_key)
            })
        {
            // selección: ONSELECT y, al terminar (sin animación de selección), ONPLAYEND
            let bname = self.prevmenus[&n].buttons[i].name.clone();
            let group = self.prevmenus[&n].buttons[i].group;
            log::debug!("PrevMenu {n}: selected button {i} group {group}");
            // Grupo distinto de 0: el motor alterna SELECTED antes del ONSELECT
            // (docs/livenovel/option3.html: オート y スキップ son toggles, grupo -1).
            let selected = if group != 0 {
                let name_key = (n.clone(), MenuItemKey::Name(bname.clone()));
                let next = !self.selected.get(&name_key).copied().unwrap_or(false);
                if group > 0 {
                    let same_group: Vec<(usize, String)> = self.prevmenus[&n]
                        .buttons
                        .iter()
                        .enumerate()
                        .filter(|(_, button)| button.group == group)
                        .map(|(index, button)| (index, button.name.clone()))
                        .collect();
                    for (index, name) in same_group {
                        self.selected
                            .insert((n.clone(), MenuItemKey::Index(index as i64)), false);
                        self.selected
                            .insert((n.clone(), MenuItemKey::Name(name)), false);
                    }
                }
                next
            } else {
                true
            };
            self.selected
                .insert((n.clone(), MenuItemKey::Index(i as i64)), selected);
            self.selected
                .insert((n.clone(), MenuItemKey::Name(bname.clone())), selected);
            self.assign("@SelStr", Value::Str(bname.clone()));
            let p = vec![Value::Str(bname.clone()), Value::Int(i as i64)];
            let on_select = self
                .scene
                .get(&n)
                .map(|object| object.str(Prop::ONSELECT))
                .unwrap_or_default();
            let on_play_end = self
                .scene
                .get(&n)
                .map(|object| object.str(Prop::ONPLAYEND))
                .unwrap_or_default();
            let kind = self.object_kind_name(&n);
            let mut extra = format!("item={bname}");
            if !on_select.trim().is_empty() {
                extra.push_str(&format!(" script={on_select}"));
            }
            if !on_play_end.trim().is_empty() {
                extra.push_str(&format!(" then={on_play_end}"));
            }
            self.log_click(x, y, false, &n, kind, "ONSELECT", false, &extra);
            self.fire(&n, Prop::ONSELECT, p.clone());
            self.fire(&n, Prop::ONPLAYEND, p);
            return;
        }
        if let Some((name, _)) = object_hit {
            let spots = self.objects_under_pointer(&assets, x, y, mouse_hit, props);
            let buried = if self
                .scene
                .get(&name)
                .is_some_and(|object| object.kind == Kind::Button)
            {
                String::new()
            } else {
                buried_button(&spots, &name)
            };
            // Clic en message box con texto pendiente: avanzar reproducción, no disparar
            // ONNOTIFY vacío (la caja declara ONNOTIFY para tags del texto, no para el ratón).
            if !right
                && self
                    .scene
                    .get(&name)
                    .is_some_and(|o| o.kind == Kind::Message)
                && self.wants_click()
            {
                // El avance lo hace el clic del motor. Si también queda @LClick,
                // el ONIDLE del cuadro (アイドル時) vuelve a avanzar la misma página.
                self.mouse_left_click = false;
                let kind = self.object_kind_name(&name);
                self.log_click(x, y, false, &name, kind, "advance", true, &buried);
                self.click();
                return;
            }
            let mut fired = false;
            for &p in props {
                if !right
                    && p == Prop::ONNOTIFY
                    && self
                        .scene
                        .get(&name)
                        .is_some_and(|o| o.kind == Kind::Edit || o.kind == Kind::Message)
                {
                    continue;
                }
                // Menu nativo: ONSELECT lleva índice vía `choose` / State::Menu, no un
                // fire vacío sobre el rectángulo entero (reabría el diálogo de guardado).
                if !right
                    && p == Prop::ONSELECT
                    && self.scene.get(&name).is_some_and(|o| o.kind == Kind::Menu)
                {
                    continue;
                }
                if self.fire(&name, p, vec![]) {
                    let script = self
                        .scene
                        .get(&name)
                        .map(|object| object.str(p))
                        .unwrap_or_default();
                    let kind = self.object_kind_name(&name);
                    let mut extra = Vec::new();
                    if !script.trim().is_empty() {
                        extra.push(format!("script={script}"));
                    }
                    if !buried.is_empty() {
                        extra.push(buried.clone());
                    }
                    self.log_click(
                        x,
                        y,
                        right,
                        &name,
                        kind,
                        crate::lsb::prop_name(p),
                        false,
                        &extra.join(" "),
                    );
                    fired = true;
                    break;
                }
            }
            if !fired {
                let kind = self.object_kind_name(&name);
                let skipped = self
                    .scene
                    .get(&name)
                    .map(|object| {
                        props
                            .iter()
                            .filter(|prop| {
                                object
                                    .get(**prop)
                                    .is_some_and(|value| !value.as_str().trim().is_empty())
                            })
                            .map(|prop| crate::lsb::prop_name(*prop))
                            .collect::<Vec<_>>()
                            .join(",")
                    })
                    .unwrap_or_default();
                let mut extra = Vec::new();
                if !skipped.is_empty() {
                    extra.push(format!("skipped={skipped}"));
                }
                if !buried.is_empty() {
                    extra.push(buried);
                }
                self.log_click(
                    x,
                    y,
                    right,
                    &name,
                    kind,
                    "no-handler",
                    false,
                    &extra.join(" "),
                );
            }
            return;
        }
        if !right && self.wants_click() {
            self.mouse_left_click = false;
        }
        let spots = self.objects_under_pointer(&assets, x, y, mouse_hit, props);
        self.log_unhandled_click(x, y, right, &spots);
        if !right {
            self.click();
        }
    }

    /// Un objeto pasa a existir de verdad / recibe ONPLAYEND: libera el texto retenido
    /// y notifica un fin de reproducción que ocurrió sin manejador.
    fn object_defined(&mut self, name: &str) {
        if self
            .scene
            .get_mut(name)
            .is_some_and(|object| std::mem::replace(&mut object.placeholder, false))
        {
            log::debug!("message object {name} defined; releasing held text");
        }
        if self
            .scene
            .get(name)
            .is_some_and(|object| object.ended_unnotified)
            && self.fire(name, Prop::ONPLAYEND, vec![])
            && let Some(object) = self.scene.get_mut(name)
        {
            object.ended_unnotified = false;
        }
    }

    /// GetProp con valores por defecto del tipo adecuado: cadena vacía para propiedades de
    /// texto (PARENT, SOURCE, eventos...), VISIBLE=false si el objeto no existe
    /// (LiveNovel: `__vis = GetProp(土台, VISIBLE)` tras ObjDel de historial → caja
    /// empieza oculta; ONPLAYING la muestra al haber texto), ALPHA/ZOOM si existe.
    fn get_prop(&self, obj: &str, p: u8) -> Value {
        if let Some((name, width, height)) = &self.pending_component_dimensions
            && name == obj
        {
            let dimension = match p {
                Prop::WIDTH | Prop::ORIGWIDTH => Some(*width),
                Prop::HEIGHT | Prop::ORIGHEIGHT => Some(*height),
                _ => None,
            };
            if let Some(dimension) = dimension {
                return Value::Int(i64::from(dimension));
            }
        }

        let (exists, v) = if obj == ROOT {
            (
                true,
                self.scene
                    .get(ROOT)
                    .and_then(|object| object.get(p).cloned()),
            )
        } else {
            match self.scene.get(obj) {
                Some(o) => (
                    true,
                    if p == Prop::NAME {
                        Some(Value::Str(o.name.clone()))
                    } else {
                        o.get(p).cloned()
                    },
                ),
                None => (false, None),
            }
        };
        if let Some(v) = v {
            return v;
        }
        if let Some(o) = self.scene.get(obj) {
            if matches!(p, Prop::WIDTH | Prop::ORIGWIDTH) {
                if o.kind == Kind::Caption {
                    let text = o.str(Prop::TEXT);
                    let font_h = o.int(Prop::FONTHEIGHT, 18).max(1) as f64;
                    let max_w = text
                        .lines()
                        .map(|line| {
                            line.chars()
                                .map(|c| font_h * caption_char_width(c))
                                .sum::<f64>()
                        })
                        .fold(0.0f64, f64::max);
                    if max_w > 0.0 {
                        return Value::Int(max_w.ceil() as i64);
                    }
                }
                let src = o.str(Prop::SOURCE);
                if let Some((w, _)) = self.slider_asset_size(&src) {
                    return Value::Int(w as i64);
                }
            } else if matches!(p, Prop::HEIGHT | Prop::ORIGHEIGHT) {
                if o.kind == Kind::Caption {
                    let text = o.str(Prop::TEXT);
                    let font_h = o.int(Prop::FONTHEIGHT, 18).max(1);
                    let linespace = o.int(Prop::LINESPACE, 0);
                    let lines = text.lines().count().max(1) as i64;
                    return Value::Int(lines * (font_h + linespace));
                }
                let src = o.str(Prop::SOURCE);
                if let Some((_, h)) = self.slider_asset_size(&src) {
                    return Value::Int(h as i64);
                }
            }
        }
        let name = crate::lsb::prop_name(p);
        if matches!(
            p,
            Prop::NAME | Prop::PARENT | Prop::SOURCE | Prop::TEXT | Prop::FONTNAME
        ) || name.starts_with("ON")
            || name.ends_with("SOURCE")
        {
            return Value::Str(String::new());
        }
        match p {
            Prop::VISIBLE => Value::Bool(exists),
            Prop::ALPHA if exists => Value::Int(255),
            // docs/livenovel/graphic.html: 256 = 1×. Sin prop, el dibujo ya usa 1×.
            Prop::ZOOMX | Prop::ZOOMY if exists => Value::Int(256),
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
            "@WheelUp" => Value::Bool(self.wheel_up),
            "@WheelDown" => Value::Bool(self.wheel_down),
            "@SelStr" => self.selected_string.clone(),
            "@ActiveCompo" => Value::Str(self.active_component.clone()),
            "@LClick" => Value::Bool(self.mouse_left_click),
            "@RClick" => Value::Bool(self.mouse_right_click),
            "@LPush" => Value::Bool(self.mouse_left),
            "@RPush" => Value::Bool(self.mouse_right),
            // docs/livenovel/calc.html: clic, pulsación y repetición por código de tecla.
            "@KeyClick" => self.key_flags(&self.key_clicks),
            "@KeyPush" => self.key_flags(&self.pressed_keys),
            "@KeyRepeat" => self.key_flags(&self.key_repeats),
            // docs/livenovel/text.html: PR_TEXTDELAY 256 = velocidad normal.
            "StatusTextSpeed" => Value::Int(256),
            // El deslizador de auto guarda segundos × 1000. Sin valor de proyecto, 1 s.
            "StatusAutoTextWait" => Value::Int(1000),
            "StatusSkipRead" => Value::Bool(false),
            // por marco de llamada: cada Call apila los suyos y se recuperan al volver
            "@ParamStr" => Value::Array(
                self.frames
                    .last()
                    .map(|f| f.params.iter().map(|v| Value::Str(v.as_str())).collect())
                    .unwrap_or_default(),
            ),
            "@Sender" => Value::Str(
                self.frames
                    .iter()
                    .rev()
                    .find_map(|f| f.sender.clone())
                    .unwrap_or_default(),
            ),
            "@ParamCount" => Value::Int(
                self.frames
                    .last()
                    .map(|f| f.params.len() as i64)
                    .unwrap_or(0),
            ),
            "@Year" => {
                let s = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let days = (s / 86400) as i64;
                let year = 1970 + days * 400 / 146097;
                Value::Str(format!("{year}"))
            }
            "@Month" => Value::Int(10),
            "@Day" => Value::Int(25),
            "@Hour" => {
                let s = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                Value::Int(((s % 86400) / 3600) as i64)
            }
            "@Min" => {
                let s = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                Value::Int(((s % 3600) / 60) as i64)
            }
            "@Sec" => {
                let s = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                Value::Int((s % 60) as i64)
            }
            "StatusSoundON" => Value::Bool(true),
            "StatusSEVolume" => self.lookup("SE音量").cloned().unwrap_or(Value::Int(1000)),
            "ステータスフルスクリーン" => Value::Bool(false),
            "ステータスディスプレイモード" | "ステータスMIDIポート" => {
                Value::Int(0)
            }
            // LiveNovel: StatusFontName vacío → MS Gothic (FONTCHANGEABLED del MesNew).
            "StatusFontName" => Value::Str("ＭＳ ゴシック".into()),
            _ => return None,
        })
    }

    /// Solo --trace: clic global en pantalla, como lo enruta el motor a la caja de mensajes
    /// activa (la última que recibió TextIns), visible o no:
    /// 1) libera texto retenido (caja creada implícitamente) y lo completa/avanza;
    /// 2) si no había nada que avanzar, dispara ONLBTNUP/ONLBTNDOWN de la caja activa
    ///    o, si no tiene, su ONPLAYEND. Cada candidato descartado se explica en debug.
    pub fn click_message(&mut self) -> Option<String> {
        let held: Vec<String> = self
            .scene
            .live()
            .filter(|object| object.placeholder)
            .map(|object| object.name.clone())
            .collect();
        for h in &held {
            if let Some(object) = self.scene.get_mut(h) {
                object.placeholder = false;
            }
            log::debug!("screen click: releasing held text of {h}");
        }
        if self.wants_click() || !held.is_empty() {
            self.click();
            return Some(format!(
                "advance text{}",
                if held.is_empty() {
                    String::new()
                } else {
                    format!(" (released {})", held.join(","))
                }
            ));
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
            if self.play_of(&n).is_some() {
                log::debug!("screen click: skip {n}: text still playing");
                continue;
            }
            for p in [Prop::ONLBTNUP, Prop::ONLBTNDOWN, Prop::ONPLAYEND] {
                if self.fire(&n, p, vec![]) {
                    return Some(format!("{n}.{}", crate::lsb::prop_name(p)));
                }
            }
            let evs: Vec<String> = self
                .scene
                .get(&n)
                .map(|o| {
                    o.props
                        .iter()
                        .filter(|(p, _)| crate::lsb::prop_name(**p).starts_with("ON"))
                        .map(|(p, v)| format!("{}={:?}", crate::lsb::prop_name(*p), v.as_str()))
                        .collect()
                })
                .unwrap_or_default();
            log::debug!(
                "screen click: skip {n}: no ONLBTNUP/ONLBTNDOWN/ONPLAYEND handler (kind {:?}, event props {evs:?})",
                self.scene.get(&n).map(|o| o.kind)
            );
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
            .find(|o| {
                o.kind == Kind::Menu
                    && [Prop::ONSELECT, Prop::ONLBTNUP]
                        .iter()
                        .any(|p| o.get(*p).is_some_and(|v| !v.as_str().trim().is_empty()))
            })
            .map(|o| o.name.clone())?;
        let idx = match self.opts.pick {
            Some(p) => p,
            None => (0..64)
                .find(|i| {
                    *self
                        .enabled
                        .get(&(name.clone(), MenuItemKey::Index(*i)))
                        .unwrap_or(&true)
                })
                .unwrap_or(0),
        };
        // el texto del ítem (si el menú lo tiene en PR_TEXT, una línea por ítem)
        let item = self
            .scene
            .get(&name)
            .map(|o| o.str(Prop::TEXT))
            .unwrap_or_default();
        let item = item
            .lines()
            .nth(idx.max(0) as usize)
            .unwrap_or("")
            .to_string();
        self.assign("@SelStr", Value::Str(item.clone()));
        if let Some(o) = self.scene.get_mut(&name) {
            o.props.insert(Prop::INDEX, Value::Int(idx));
            o.props.insert(Prop::VALUE, Value::Int(idx));
        }
        self.selected
            .insert((name.clone(), MenuItemKey::Index(idx)), true);
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
            .filter(|o| {
                o.kind != Kind::Menu
                    && o.play.is_none()
                    && self.scene.effective_visible(o)
                    && props
                        .iter()
                        .any(|p| o.get(*p).is_some_and(|v| !v.as_str().trim().is_empty()))
            })
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
            Func::GETITEM => {
                let index = a
                    .get(1)
                    .map(Value::as_int)
                    .and_then(|i| usize::try_from(i).ok());
                Value::Str(
                    index
                        .and_then(|i| self.prevmenus.get(&s(0))?.buttons.get(i))
                        .map(|b| b.name.clone())
                        .unwrap_or_default(),
                )
            }
            Func::GETPROP => {
                let p = prop_of(a.get(1)?)?;
                if p == Prop::COUNT
                    && let Some(pm) = self.prevmenus.get(&s(0))
                {
                    return Some(Value::Int(pm.buttons.len() as i64));
                }
                self.get_prop(&s(0), p)
            }
            Func::SETPROP => {
                let p = prop_of(a.get(1)?)?;
                if s(0) == ROOT {
                    return Some(Value::Bool(self.scene.set(
                        ROOT,
                        p,
                        a.get(2).cloned().unwrap_or_default(),
                    )));
                }
                Value::Bool(
                    self.scene
                        .set(&s(0), p, a.get(2).cloned().unwrap_or_default()),
                )
            }
            Func::EXISTS => Value::Bool(self.scene.get(&s(0)).is_some()),
            Func::VAREXISTS => {
                let n = match raw.first() {
                    Some(Param::Var(n)) if !n.starts_with("____") => n.clone(),
                    _ => s(0),
                };
                Value::Bool(self.lookup(&n).is_some())
            }
            Func::GETVISIBLE => Value::Bool(if let Some(item) = a.get(1) {
                match item {
                    Value::Str(_) => self.menu_item_visible_key(&s(0), &menu_item_key(Some(item))),
                    _ => {
                        let Ok(index) = usize::try_from(item.as_int()) else {
                            return Some(Value::Bool(false));
                        };
                        self.menu_item_visible(&s(0), index)
                    }
                }
            } else {
                self.scene.get(&s(0)).is_some_and(|o| o.visible())
            }),
            Func::SETVISIBLE => {
                if let (Some(index), Some(value)) = (a.get(1), a.get(2)) {
                    if !matches!(index, Value::Str(_)) && usize::try_from(index.as_int()).is_err() {
                        return Some(Value::Bool(false));
                    }
                    self.visible
                        .insert((s(0), menu_item_key(Some(index))), value.truthy());
                    Value::Bool(true)
                } else {
                    Value::Bool(self.scene.set(
                        &s(0),
                        Prop::VISIBLE,
                        Value::Bool(a.get(1).is_some_and(|v| v.truthy())),
                    ))
                }
            }
            Func::INCABINET => Value::Bool(
                self.cabinets
                    .values()
                    .any(|objs| objs.iter().any(|o| o.name == s(0))),
            ),
            Func::FILEEXISTS => Value::Bool(self.assets.data().find(&s(0)).is_some()),
            Func::GETHISTORYCOUNT => Value::Int(self.history.len() as i64),
            Func::GETGROUPINDEX => {
                let (group, item_name) = self
                    .prevmenu_button(&s(0), a.get(1))
                    .map(|button| (button.group, button.name.clone()))
                    .unwrap_or((0, String::new()));
                if group != 0 {
                    return Some(Value::Int(i64::from(group)));
                }
                let object = if item_name.is_empty() {
                    self.scene.get(&s(0))
                } else {
                    self.scene.get(&item_name).or_else(|| self.scene.get(&s(0)))
                };
                Value::Int(
                    object
                        .map(|object| object.int(Prop::GROUPINDEX, 0))
                        .unwrap_or(0),
                )
            }
            // ListCompo(arr, modo, objeto): nombres de los componentes hijos de `objeto`
            // en orden de creación (modo != 0: también descendientes; semántica inferida).
            Func::LISTCOMPO => {
                let list = self
                    .scene
                    .children(&s(2), a.get(1).map(|v| v.as_int() != 0).unwrap_or(false));
                let n = list.len() as i64;
                if let Some(Param::Var(var)) = raw.first() {
                    let var = var.clone();
                    self.assign(
                        &var,
                        Value::Array(list.into_iter().map(Value::Str).collect()),
                    );
                }
                Value::Int(n)
            }
            // Ítems de PrevMenu/menú: (objeto, índice[, valor]). Semántica inferida.
            Func::SETENABLED | Func::SETSELECTED => {
                let k = (s(0), menu_item_key(a.get(1)));
                let v = a.get(2).map(|v| v.truthy()).unwrap_or(true);
                if f == Func::SETENABLED {
                    self.enabled.insert(k, v)
                } else {
                    self.selected.insert(k, v)
                };
                Value::Bool(true)
            }
            Func::GETENABLED => Value::Bool(
                *self
                    .enabled
                    .get(&(s(0), menu_item_key(a.get(1))))
                    .unwrap_or(&true),
            ),
            Func::ADDDELIMITER => {
                let has_content = match a.get(1) {
                    None | Some(Value::Empty) => false,
                    Some(Value::Array(values)) => !values.is_empty(),
                    Some(value) => !value.as_str().is_empty(),
                };
                Value::Str(if has_content { s(0) } else { String::new() })
            }
            Func::GETSELECTED => Value::Bool(
                *self
                    .selected
                    .get(&(s(0), menu_item_key(a.get(1))))
                    .unwrap_or(&false),
            ),
            // Menú: checkmark por índice/nombre (manual LiveNovel GetCheck/SetCheck).
            Func::SETCHECK => {
                let k = (s(0), menu_item_key(a.get(1)));
                let v = a.get(2).map(|v| v.truthy()).unwrap_or(true);
                self.checked.insert(k, v);
                Value::Bool(true)
            }
            Func::GETCHECK => Value::Bool(
                *self
                    .checked
                    .get(&(s(0), menu_item_key(a.get(1))))
                    .unwrap_or(&false),
            ),
            // InPrimary: no documentada; se asume "ventana principal activa" -> verdadero.
            Func::INPRIMARY => Value::Bool(true),
            Func::GETHISTORYMAXCOUNT => Value::Int(self.history_max),
            Func::SETHISTORYMAXCOUNT => {
                self.history_max = a.first().map(|v| v.as_int()).unwrap_or(0).max(0);
                if self.history_max > 0 {
                    while self.history.len() > self.history_max as usize {
                        self.history.pop_front();
                    }
                }
                Value::Bool(true)
            }
            // ventana = espacio lógico (letterbox lo resuelve SDL), así que la conversión es identidad
            Func::TOSCREENX | Func::TOSCREENY | Func::TOCLIENTX | Func::TOCLIENTY => {
                a.last().cloned().unwrap_or(Value::Int(0))
            }
            // docs/livenovel/calc.html: LoadTextFile(variable, archivo) deja en la
            // variable un array de una entrada por línea y devuelve verdadero si leyó.
            Func::LOADTEXTFILE => {
                let Some(var) = (match raw.first() {
                    Some(Param::Var(name)) if !name.starts_with("____") => Some(name.clone()),
                    Some(Param::Str(name)) if !name.is_empty() => Some(name.clone()),
                    _ => None,
                }) else {
                    return Some(Value::Bool(false));
                };
                let path = a.get(1).map(Value::as_str).unwrap_or_default();
                let data = self.assets.data().clone();
                let Some(name) = data.resolve(&path, &[".txt"]) else {
                    return Some(Value::Bool(false));
                };
                let started = std::time::Instant::now();
                let Some(bytes) = data.read(&name).ok() else {
                    return Some(Value::Bool(false));
                };
                let text = crate::util::cp932(&bytes);
                let lines: Vec<Value> = text
                    .lines()
                    .map(|line| Value::Str(line.to_string()))
                    .collect();
                log::info!(
                    "load text {name}: {} bytes, {} lines in {:.1} ms",
                    bytes.len(),
                    lines.len(),
                    started.elapsed().as_secs_f64() * 1000.0
                );
                self.assign(&var, Value::Array(lines));
                Value::Bool(true)
            }
            Func::DBCREATETABLE..=Func::DBGETRECNO
            | Func::DBCLEARRECORD
            | Func::DBGETTABLECOUNT
            | Func::DBGETTABLE => {
                let d = self.assets.data().clone();
                let load = move |f: &str| {
                    let name = d.resolve(f, &[".tsv", ".txt"])?;
                    let started = std::time::Instant::now();
                    let bytes = d.read(&name).ok()?;
                    log::info!(
                        "load table {name}: {} bytes in {:.1} ms",
                        bytes.len(),
                        started.elapsed().as_secs_f64() * 1000.0
                    );
                    Some(crate::util::cp932(&bytes))
                };
                return self.db.call(f, a, &load);
            }
            // **interpretación:** siempre modo ventana lógica (0); no hay exclusive fullscreen LM.
            Func::GETDISPLAYMODE => Value::Int(0),
            Func::GETSAVECAPTION => {
                let slot = a.first().map(|v| v.as_int()).unwrap_or(-1);
                let caption = crate::save::read_save_caption(&self.save_dir, slot);
                Value::Str(caption)
            }
            // script.html `<OPENURL>` / calc OPENURL: abrir en el navegador del SO.
            Func::OPENURL => {
                let url = a.first().map(Value::as_str).unwrap_or_default();
                Value::Bool(open_external_url(&url))
            }
            // calc.html SetWallPaper: wallpaper del SO. Sin API host → FALSE honesto.
            Func::SETWALLPAPER => {
                self.warn_once(
                    "wallpaper:unsupported",
                    "SetWallPaper: desktop wallpaper not supported (returns false)",
                );
                Value::Bool(false)
            }
            // calc.html SaveScreen: captura pantalla → .gal / mem: / save:.
            Func::SAVESCREEN => Value::Bool(self.save_screen(a)),
            // calc.html CopyFile: mem:/save:/host; sobrescribe destino.
            Func::COPYFILE => {
                let src = a.first().map(Value::as_str).unwrap_or_default();
                let dst = a.get(1).map(Value::as_str).unwrap_or_default();
                Value::Bool(copy_file(self, &src, &dst))
            }
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
        let k = if msg.starts_with("unimplemented function") {
            format!("unimpl:func {key}")
        } else {
            key.to_string()
        };
        self.warn(&k, msg);
    }
}

impl Vm {
    /// Imagen desde `mem:` / `save:` si el SOURCE no está en el archivo del juego.
    pub fn vfs_image(&self, src: &str) -> Option<std::sync::Arc<crate::gal::Image>> {
        if strip_vfs_prefix(src, "mem:").is_none() && strip_vfs_prefix(src, "save:").is_none() {
            return None;
        }
        if let Some(img) = self.vfs_image_cache.borrow().get(src) {
            return Some(img.clone());
        }
        let bytes = read_vfs_bytes(self, src)?;
        let img = std::sync::Arc::new(crate::gal::decode_any(&bytes).ok()?);
        self.vfs_image_cache
            .borrow_mut()
            .insert(src.to_string(), img.clone());
        Some(img)
    }

    /// ¿Hay un SaveScreen esperando readback de la escena?
    pub fn take_pending_savescreen(&mut self) -> Option<SaveScreenRequest> {
        self.pending_savescreen.take()
    }

    /// Completa un SaveScreen diferido con RGBA recién leído del target de escena.
    pub fn complete_savescreen(
        &mut self,
        req: &SaveScreenRequest,
        w: u32,
        h: u32,
        rgba: Vec<u8>,
    ) -> bool {
        if w == 0 || h == 0 || rgba.len() < (w as usize) * (h as usize) * 4 {
            return false;
        }
        self.screen_rgba = Some((w, h, rgba));
        self.apply_savescreen(req)
    }

    /// Solo tests / inyección: deja un frame en CPU sin pasar por SDL.
    #[cfg(test)]
    pub fn set_screen_rgba(&mut self, w: u32, h: u32, rgba: Vec<u8>) {
        if w == 0 || h == 0 || rgba.len() < (w as usize) * (h as usize) * 4 {
            return;
        }
        self.screen_rgba = Some((w, h, rgba));
    }

    /// calc.html SaveScreen. Si ya hay captura CPU, escribe ya; si no, encola readback
    /// una sola vez tras el render (**interpretación**: TRUE = se cumplirá este frame).
    fn save_screen(&mut self, a: &[Value]) -> bool {
        let path = a.first().map(Value::as_str).unwrap_or_default();
        if path.is_empty() {
            return false;
        }
        let rw = a.get(1).map(|v| v.as_int()).unwrap_or(0).max(0) as u32;
        let rh = a.get(2).map(|v| v.as_int()).unwrap_or(0).max(0) as u32;
        // ④ back buffer / ⑤ delete: un solo buffer lógico (**no confirmado** / omitido).
        let req = SaveScreenRequest {
            path,
            resample_w: rw,
            resample_h: rh,
        };
        if self.screen_rgba.is_some() {
            return self.apply_savescreen(&req);
        }
        self.pending_savescreen = Some(req);
        true
    }

    fn apply_savescreen(&mut self, req: &SaveScreenRequest) -> bool {
        let Some((sw, sh, rgba)) = self.screen_rgba.as_ref() else {
            return false;
        };
        let (w, h, pixels) = if req.resample_w > 0 && req.resample_h > 0 {
            (
                req.resample_w,
                req.resample_h,
                resample_nearest(rgba, *sw, *sh, req.resample_w, req.resample_h),
            )
        } else {
            (*sw, *sh, rgba.clone())
        };
        // Encode zlib es barato frente al readback; se puede mover a worker después.
        let gal = match crate::gal::encode_galex_rgba(w, h, &pixels) {
            Ok(g) => g,
            Err(e) => {
                log::warn!("SaveScreen encode: {e}");
                return false;
            }
        };
        let ok = write_vfs_bytes(self, &req.path, &gal);
        if ok {
            self.vfs_image_cache.borrow_mut().remove(&req.path);
        }
        ok
    }

    fn wipe_set_has(scene: &Scene, set: &HashSet<String>, name: &str) -> bool {
        let mut current = name.to_string();
        for _ in 0..64 {
            if set.contains(&current) {
                return true;
            }
            let Some(parent) = scene
                .get(&current)
                .map(|object| object.str(Prop::PARENT))
                .filter(|parent| !parent.is_empty())
            else {
                return false;
            };
            current = parent;
        }
        false
    }

    /// RGBA del overlay Flip WHITE/BLACK/FLASH, si activo.
    pub fn wipe_flash_rgba(&self) -> Option<(u8, u8, u8, u8)> {
        self.wipe_flash.as_ref().and_then(|f| f.rgba(self.now))
    }

    /// Factor 0–1 de la superficie de este objeto durante un Flip, si le toca.
    pub fn wipe_alpha_factor(&self, name: &str) -> Option<f64> {
        let fade = self.wipe_fade.as_ref()?;
        let progress = fade.progress(self.now);
        if Self::wipe_set_has(&self.scene, &fade.out, name) {
            Some(1.0 - progress)
        } else if Self::wipe_set_has(&self.scene, &fade.inn, name) {
            Some(progress)
        } else {
            None
        }
    }

    /// Objetos de la superficie nueva que el orden normal no dibuja (están archivados).
    pub fn append_wipe_incoming(&self, ordered: &mut Vec<crate::scene::Obj>) {
        let Some(fade) = &self.wipe_fade else {
            return;
        };
        if fade.inn.is_empty() {
            return;
        }
        let have: HashSet<String> = ordered.iter().map(|object| object.name.clone()).collect();
        let mut extra: Vec<_> = self
            .scene
            .live()
            .filter(|object| {
                !have.contains(&object.name)
                    && Self::wipe_set_has(&self.scene, &fade.inn, &object.name)
            })
            .cloned()
            .collect();
        extra.sort_by_key(|object| (object.int(Prop::PRIORITY, 0), object.seq));
        ordered.extend(extra);
    }

    /// Offset de dibujo por `<QUAKE>` (0,0 si no aplica).
    pub fn quake_draw_offset(&self, name: &str) -> (f64, f64) {
        let Some(q) = self.quake.as_ref() else {
            return (0.0, 0.0);
        };
        if self.now >= q.until {
            return (0.0, 0.0);
        }
        if !q.names.is_empty() && !q.names.iter().any(|n| n == name) {
            return (0.0, 0.0);
        }
        let t = (self.now - q.started_at).max(0.0);
        let amp_scale = if let Some(stop_at) = q.stop_until {
            if self.now >= stop_at {
                return (0.0, 0.0);
            }
            // STOPQUAKE: amortiguar en TIME ms (**inventado**: escala lineal al deadline).
            ((stop_at - self.now) / (stop_at - q.started_at).max(1.0)).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let cycle = q.cycle_ms.max(1.0);
        let phase = if q.random_phase {
            // desfase estable por nombre
            let h = name
                .bytes()
                .fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
            t + f64::from(h % 1000)
        } else {
            t
        };
        let (ax, ay) = (q.amp_x * amp_scale, q.amp_y * amp_scale);
        let (dx, dy) = match q.kind {
            'W' => {
                let w = std::f64::consts::TAU * (phase / cycle);
                (ax * w.sin(), ay * w.sin())
            }
            'B' => {
                // rebote: abs(sin)
                let w = std::f64::consts::TAU * (phase / cycle);
                (0.0, ay * w.sin().abs())
            }
            _ => {
                // Q: pseudo-aleatorio acotado (**inventado**: hash del tiempo)
                let n = ((phase * 12.9898).sin() * 43758.5453).fract();
                let m = ((phase * 78.233).sin() * 43758.5453).fract();
                (ax * (n * 2.0 - 1.0), ay * (m * 2.0 - 1.0))
            }
        };
        (dx, dy)
    }
}

/// Prefijo `mem:` / `save:` sin distinguir mayúsculas (calc.html).
fn strip_vfs_prefix<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    if path.len() >= prefix.len()
        && path.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
    {
        Some(&path[prefix.len()..])
    } else {
        None
    }
}

fn read_vfs_bytes(vm: &Vm, path: &str) -> Option<Vec<u8>> {
    if let Some(name) = strip_vfs_prefix(path, "mem:") {
        return vm
            .mem_files
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone());
    }
    if let Some(name) = strip_vfs_prefix(path, "save:") {
        return crate::save::read_save_blob(&vm.save_dir, name);
    }
    let src_path = if PathBuf::from(path).is_absolute() {
        PathBuf::from(path)
    } else {
        vm.save_dir.join(path.replace('\\', "/"))
    };
    std::fs::read(&src_path).ok()
}

fn resample_nearest(rgba: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let mut out = vec![0u8; (dw as usize) * (dh as usize) * 4];
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return out;
    }
    for y in 0..dh as usize {
        let sy = y * sh as usize / dh as usize;
        for x in 0..dw as usize {
            let sx = x * sw as usize / dw as usize;
            let si = (sy * sw as usize + sx) * 4;
            let di = (y * dw as usize + x) * 4;
            if si + 4 <= rgba.len() {
                out[di..di + 4].copy_from_slice(&rgba[si..si + 4]);
            }
        }
    }
    out
}

fn write_vfs_bytes(vm: &mut Vm, path: &str, data: &[u8]) -> bool {
    if let Some(name) = strip_vfs_prefix(path, "mem:") {
        if name.is_empty() {
            return false;
        }
        vm.mem_files.retain(|k, _| !k.eq_ignore_ascii_case(name));
        vm.mem_files.insert(name.to_string(), data.to_vec());
        vm.vfs_image_cache
            .borrow_mut()
            .retain(|k, _| strip_vfs_prefix(k, "mem:") != Some(name));
        return true;
    }
    if let Some(name) = strip_vfs_prefix(path, "save:") {
        if name.is_empty() {
            return false;
        }
        let ok = crate::save::write_save_blob(&vm.save_dir, name, data).is_ok();
        if ok {
            vm.vfs_image_cache
                .borrow_mut()
                .retain(|k, _| strip_vfs_prefix(k, "save:") != Some(name));
        }
        return ok;
    }
    let dst_path = if PathBuf::from(path).is_absolute() {
        PathBuf::from(path)
    } else {
        vm.save_dir.join(path.replace('\\', "/"))
    };
    if let Some(parent) = dst_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&dst_path, data) {
        Ok(()) => true,
        Err(e) => {
            log::warn!("CopyFile write {dst_path:?}: {e}");
            false
        }
    }
}

/// CopyFile (calc.html): copia mem/save/archivos de host; sobrescribe destino.
fn copy_file(vm: &mut Vm, src: &str, dst: &str) -> bool {
    if src.is_empty() || dst.is_empty() {
        return false;
    }
    let Some(data) = read_vfs_bytes(vm, src) else {
        log::warn!("CopyFile: source not found {src:?}");
        return false;
    };
    write_vfs_bytes(vm, dst, &data)
}

/// Abre una URL con el manejador del sistema. **fuente primaria** script.html OPENURL.
pub(crate) fn open_external_url(url: &str) -> bool {
    let url = url.trim();
    if url.is_empty() {
        return false;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
            .is_ok()
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn().is_ok()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::GameData;
    use crate::lsb::tpword::Glyph;

    #[test]
    fn caption_width_estimate_treats_latin_accents_as_half_width() {
        assert_eq!(caption_char_width('ó'), 0.55);
        assert_eq!(caption_char_width('¿'), 0.55);
        assert_eq!(caption_char_width('姉'), 1.0);
    }

    #[test]
    fn get_prop_visible_defaults_true_for_missing_objects() {
        let path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
        let data = GameData::open(&path).unwrap();
        let assets = Arc::new(Assets::new(data, 2, 64));
        let mut vm = Vm::new(
            assets,
            "ゲームメイン.lsb",
            Options {
                screen: (800, 600),
                cinema_ms: 0.0,
                text_cps: 0.0,
                lookahead: 0,
                trace: false,
                trace_conds: false,
                pick: None,
                show_text: false,
            },
        )
        .unwrap();
        // Tras ObjDel, GetProp(VISIBLE) es false → __vis oculta el objeto hasta ONPLAYING.
        assert_eq!(
            vm.get_prop("deleted-object", Prop::VISIBLE),
            Value::Bool(false)
        );
        assert_eq!(
            vm.get_prop("no-such-object", Prop::VISIBLE),
            Value::Bool(false)
        );
        assert_eq!(
            vm.call_engine(Func::EXISTS, &[Value::Str("no-such-object".into())], &[]),
            Some(Value::Bool(false))
        );
    }

    #[test]
    fn set_check_and_get_check_track_menu_item_state() {
        let path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
        let data = GameData::open(&path).unwrap();
        let assets = Arc::new(Assets::new(data, 2, 64));
        let mut vm = Vm::new(
            assets,
            "ゲームメイン.lsb",
            Options {
                screen: (800, 600),
                cinema_ms: 0.0,
                text_cps: 0.0,
                lookahead: 0,
                trace: false,
                trace_conds: false,
                pick: None,
                show_text: false,
            },
        )
        .unwrap();
        assert_eq!(
            vm.call_engine(
                Func::GETCHECK,
                &[Value::Str("ロードメニュー".into()), Value::Int(3)],
                &[]
            ),
            Some(Value::Bool(false))
        );
        assert_eq!(
            vm.call_engine(
                Func::SETCHECK,
                &[
                    Value::Str("ロードメニュー".into()),
                    Value::Int(3),
                    Value::Bool(true),
                ],
                &[]
            ),
            Some(Value::Bool(true))
        );
        assert_eq!(
            vm.call_engine(
                Func::GETCHECK,
                &[Value::Str("ロードメニュー".into()), Value::Int(3)],
                &[]
            ),
            Some(Value::Bool(true))
        );
        // Sin captura aún → TRUE diferido (pedido encolado para el próximo render).
        assert_eq!(
            vm.call_engine(
                Func::SAVESCREEN,
                &[Value::Str("mem:__workscr.gal".into())],
                &[]
            ),
            Some(Value::Bool(true)),
            "SaveScreen queues capture when no CPU frame yet"
        );
        assert!(vm.pending_savescreen.is_some());
        let _ = vm.take_pending_savescreen();
        let tmp = std::env::temp_dir().join("livenyan_copyfile_vfs");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        vm.save_dir = tmp.clone();
        // 2x2 rojo opaco → escribe en el acto
        vm.set_screen_rgba(
            2,
            2,
            vec![
                255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255,
            ],
        );
        assert_eq!(
            vm.call_engine(
                Func::SAVESCREEN,
                &[Value::Str("mem:__savescr.gal".into())],
                &[]
            ),
            Some(Value::Bool(true))
        );
        assert!(vm.mem_files.contains_key("__savescr.gal"));
        assert_eq!(
            vm.call_engine(
                Func::COPYFILE,
                &[
                    Value::Str("mem:__savescr.gal".into()),
                    Value::Str("save:__savescr3.gal".into()),
                ],
                &[]
            ),
            Some(Value::Bool(true))
        );
        assert!(crate::save::read_save_blob(&tmp, "__savescr3.gal").is_some());
        let _ = std::fs::remove_dir_all(&tmp);
        assert_eq!(
            vm.call_engine(Func::SETWALLPAPER, &[Value::Str("x.gal".into())], &[]),
            Some(Value::Bool(false))
        );
        assert!(
            !vm.warned
                .iter()
                .any(|w| w.contains("unimpl:func") || w.contains("SetCheck")),
            "must not emit unimpl:func for documented unsupported APIs: {:?}",
            vm.warned
        );
    }

    #[test]
    fn wipe_flash_peaks_mid_for_white_black() {
        let f = wipe::WipeFlash::white_black(0.0, 1000.0, true);
        assert_eq!(f.rgba(0.0).map(|c| c.3), Some(0));
        assert_eq!(f.rgba(500.0).map(|c| c.3), Some(255));
        assert_eq!(f.rgba(1000.0), None);
        let flash = wipe::WipeFlash::flash(0.0, 1000.0);
        assert_eq!(flash.rgba(0.0).map(|c| c.3), Some(255));
        assert!(flash.rgba(500.0).map(|c| c.3).unwrap() < 255);
    }

    fn literal(value: Param) -> LiveParser {
        LiveParser {
            entries: vec![crate::lsb::OpeData {
                op: crate::lsb::op::TO,
                name: String::new(),
                func: None,
                operands: vec![value],
            }],
        }
    }

    fn flip_command(act: i32, time: i32, delete: bool, wipe: i32, targets: &[&str]) -> Command {
        Command {
            kind: CmdType::Flip as u8,
            indent: 0,
            mute: false,
            not_update: false,
            line_no: 0,
            fields: vec![
                ("Act", crate::lsb::Field::Expr(literal(Param::Int(act)))),
                ("Time", crate::lsb::Field::Expr(literal(Param::Int(time)))),
                (
                    "Delete",
                    crate::lsb::Field::Expr(literal(Param::Int(i32::from(delete)))),
                ),
                ("Wipe", crate::lsb::Field::Expr(literal(Param::Int(wipe)))),
                (
                    "Targets",
                    crate::lsb::Field::Exprs(
                        targets
                            .iter()
                            .map(|name| literal(Param::Str((*name).into())))
                            .collect(),
                    ),
                ),
            ],
        }
    }

    #[test]
    fn click_uses_the_image_file_when_width_and_height_are_unset() {
        let mut vm = fixture_vm();
        let mut props = HashMap::new();
        props.insert(Prop::SOURCE, Value::Str("bg.gal".into()));
        props.insert(Prop::LEFT, Value::Int(0));
        props.insert(Prop::TOP, Value::Int(0));
        props.insert(Prop::ONLBTNDOWN, Value::Str("pick".into()));
        vm.scene.create("thumb", Kind::Image, props);
        vm.click_at(1.0, 1.0, false);
        assert!(vm.queue.iter().any(|event| event.obj == "thumb"));

        vm.scene.set("thumb", Prop::IGNOREMOUSE, Value::Int(1));
        let queued = vm.queue.len();
        vm.click_at(1.0, 1.0, false);
        assert_eq!(vm.queue.len(), queued);
    }

    #[test]
    fn empty_stay_parks_the_old_plate_and_keeps_this_frame() {
        let mut vm = fixture_vm();
        vm.scene.set_cohort(1);
        vm.scene.create("title", Kind::Image, HashMap::new());
        vm.scene.create("box", Kind::Message, HashMap::new());
        vm.scene.set_cohort(2);
        vm.scene.create("gallery", Kind::Image, HashMap::new());
        vm.scene.set_cohort(2);
        vm.step_flip(&flip_command(1, 0, true, 0, &[]));
        assert!(vm.scene.get("title").is_some());
        assert!(!vm.scene.names().iter().any(|name| name == "title"));
        assert!(vm.scene.names().iter().any(|name| name == "gallery"));
        assert!(vm.scene.names().iter().any(|name| name == "box"));

        vm.scene.set_cohort(3);
        vm.scene.create("thumb", Kind::Image, HashMap::new());
        vm.scene.set_cohort(4);
        vm.step_flip(&flip_command(1, 0, true, 0, &[]));
        assert!(vm.scene.names().iter().any(|name| name == "title"));
        assert!(vm.scene.get("gallery").is_none());
        assert!(vm.scene.get("thumb").is_none());
        assert!(vm.scene.get("box").is_some());

        vm.scene.set_cohort(5);
        vm.scene.create("gallery", Kind::Image, HashMap::new());
        vm.scene.set_cohort(5);
        vm.step_flip(&flip_command(1, 0, true, 0, &[]));
        assert!(vm.scene.get("thumb").is_none());
        assert!(vm.scene.names().iter().any(|name| name == "gallery"));
        assert!(!vm.scene.names().iter().any(|name| name == "title"));
    }

    #[test]
    fn enter_fade_lasts_the_whole_flip_time() {
        let mut vm = fixture_vm();
        vm.now = 1000.0;
        vm.scene.set_cohort(1);
        vm.scene.create("title", Kind::Image, HashMap::new());
        vm.scene.set_cohort(2);
        vm.scene.create("gallery", Kind::Image, HashMap::new());
        vm.scene.set_cohort(2);
        vm.step_flip(&flip_command(1, 500, true, 1, &[]));
        let fade = vm.wipe_fade.as_ref().unwrap();
        assert_eq!(fade.dur_ms, 500.0);
        assert!(fade.out.contains("title"));
        assert!(fade.inn.contains("gallery"));
        assert!(vm.scene.motions.is_empty());
        assert_eq!(vm.scene.get("gallery").unwrap().int(Prop::ALPHA, 255), 255);
        assert_eq!(vm.scene.get("title").unwrap().int(Prop::ALPHA, 255), 255);
    }

    #[test]
    fn named_flip_only_fades_or_deletes_its_targets() {
        let mut vm = fixture_vm();
        vm.scene.create("actor", Kind::Image, HashMap::new());
        let mut child = HashMap::new();
        child.insert(Prop::PARENT, Value::Str("actor".into()));
        vm.scene.create("face", Kind::Image, child);
        vm.scene.create("bg", Kind::Image, HashMap::new());
        vm.step_flip(&flip_command(0, 0, true, 0, &["actor"]));
        assert!(vm.scene.get("actor").is_none());
        assert!(vm.scene.get("face").is_none());
        assert!(vm.scene.names().iter().any(|name| name == "bg"));

        vm.scene.create("actor", Kind::Image, HashMap::new());
        vm.step_flip(&flip_command(0, 0, false, 0, &["actor"]));
        assert!(vm.scene.get("actor").is_some());
        assert!(!vm.scene.names().iter().any(|name| name == "actor"));
        assert_eq!(vm.scene.get("actor").unwrap().int(Prop::ALPHA, 255), 255);
    }

    /// MESOFF: Flip FL_OUT con targets → `back`. 再生中 solo SetProp VISIBLE=1
    /// (sin Flip de entrada); debe desarchivar para que el cuadro vuelva a dibujarse.
    #[test]
    fn setprop_visible_true_unmarks_flip_back_surface() {
        let mut vm = fixture_vm();
        vm.scene.create("土台", Kind::Image, HashMap::new());
        vm.step_flip(&flip_command(0, 0, false, 0, &["土台"]));
        assert!(!vm.scene.names().iter().any(|name| name == "土台"));
        let cmd = Command {
            kind: CmdType::SetProp as u8,
            indent: 0,
            mute: false,
            not_update: false,
            line_no: 0,
            fields: vec![
                (
                    "ObjName",
                    crate::lsb::Field::Expr(literal(Param::Str("土台".into()))),
                ),
                ("ObjProp", crate::lsb::Field::Expr(literal(Param::Int(47)))),
                ("Value", crate::lsb::Field::Expr(literal(Param::Int(1)))),
            ],
        };
        let script = Lsb {
            name: "setprop.lsb".into(),
            version: 117,
            flags: 0,
            command_params: vec![],
            commands: vec![],
        };
        vm.step_set_prop(&script, 0, &cmd);
        assert!(vm.scene.names().iter().any(|name| name == "土台"));
    }

    /// FACE Flip archiva 土台 y メッセージボックス; 再生中 solo VISIBLE en el 土台.
    #[test]
    fn setprop_visible_unmarks_back_descendants() {
        let mut vm = fixture_vm();
        vm.scene.create("土台", Kind::Box, HashMap::new());
        let mut child = HashMap::new();
        child.insert(Prop::PARENT, Value::Str("土台".into()));
        vm.scene.create("メッセージボックス", Kind::Message, child);
        vm.step_flip(&flip_command(
            0,
            0,
            false,
            0,
            &["土台", "メッセージボックス"],
        ));
        assert!(!vm.scene.names().iter().any(|name| name == "土台"));
        assert!(
            !vm.scene
                .names()
                .iter()
                .any(|name| name == "メッセージボックス")
        );
        let cmd = Command {
            kind: CmdType::SetProp as u8,
            indent: 0,
            mute: false,
            not_update: false,
            line_no: 0,
            fields: vec![
                (
                    "ObjName",
                    crate::lsb::Field::Expr(literal(Param::Str("土台".into()))),
                ),
                ("ObjProp", crate::lsb::Field::Expr(literal(Param::Int(47)))),
                ("Value", crate::lsb::Field::Expr(literal(Param::Int(1)))),
            ],
        };
        let script = Lsb {
            name: "setprop.lsb".into(),
            version: 117,
            flags: 0,
            command_params: vec![],
            commands: vec![],
        };
        vm.step_set_prop(&script, 0, &cmd);
        assert!(vm.scene.names().iter().any(|name| name == "土台"));
        assert!(
            vm.scene
                .names()
                .iter()
                .any(|name| name == "メッセージボックス")
        );
    }

    #[test]
    fn image_components_can_query_source_dimensions_while_their_props_are_evaluated() {
        let path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
        let data = GameData::open(&path).unwrap();
        let assets = Arc::new(Assets::new(data, 2, 64));
        let mut vm = Vm::new(
            assets.clone(),
            "ゲームメイン.lsb",
            Options {
                screen: (800, 600),
                cinema_ms: 1000.0,
                text_cps: 30.0,
                lookahead: 0,
                trace: false,
                trace_conds: false,
                pick: None,
                show_text: false,
            },
        )
        .unwrap();
        let literal = |value: Param| LiveParser {
            entries: vec![crate::lsb::OpeData {
                op: crate::lsb::op::TO,
                name: String::new(),
                func: None,
                operands: vec![value],
            }],
        };
        let get_prop = |name: &str, property| LiveParser {
            entries: vec![crate::lsb::OpeData {
                op: crate::lsb::op::FUNC,
                name: String::new(),
                func: Some(Func::GETPROP),
                operands: vec![Param::Str(name.into()), Param::Int(property)],
            }],
        };

        let expected = match assets.get("bg.gal") {
            crate::assets::Asset::Image(image) => (image.w, image.h),
            _ => panic!("fixture bg.gal must be an image"),
        };
        for (kind, name) in [(CmdType::ImgNew, "image"), (CmdType::TileNew, "tile")] {
            let command = Command {
                kind: kind as u8,
                indent: 0,
                mute: false,
                not_update: false,
                line_no: 0,
                fields: vec![(
                    "components",
                    crate::lsb::Field::Props(vec![
                        (Prop::NAME, literal(Param::Str(name.into()))),
                        (Prop::SOURCE, literal(Param::Str("bg.gal".into()))),
                        (Prop::WIDTH, get_prop(name, 90)),
                        (Prop::HEIGHT, get_prop(name, 91)),
                    ]),
                )],
            };
            let props = vm.props_of(&command);
            assert_eq!(props[&Prop::WIDTH].as_int(), i64::from(expected.0));
            assert_eq!(props[&Prop::HEIGHT].as_int(), i64::from(expected.1));
        }
    }

    #[test]
    fn translates_component_captions_and_buttons_without_translating_edit_data() {
        let translations = HashMap::from([
            (
                crate::text_export::text_item_id("姉の職業は？（15文字まで）"),
                "¿Cuál es la profesión de tu hermana? (máx. 15 caracteres)".into(),
            ),
            (
                crate::text_export::text_item_id("姉の職業"),
                "Profesión de tu hermana".into(),
            ),
            (crate::text_export::text_item_id("OK"), "Aceptar".into()),
            (
                crate::text_export::text_item_id("キャンセル"),
                "Cancelar".into(),
            ),
        ]);

        assert_eq!(
            translate_component_text(
                CmdType::Caption,
                "姉の職業は？（15文字まで）",
                &translations
            ),
            Some("¿Cuál es la profesión de tu hermana? (máx. 15 caracteres)".into())
        );
        assert_eq!(
            translate_component_text(CmdType::Button, "OK", &translations),
            Some("Aceptar".into())
        );
        assert_eq!(
            translate_component_text(CmdType::Button, "キャンセル", &translations),
            Some("Cancelar".into())
        );
        assert_eq!(
            translate_component_text(CmdType::EditNew, "texto del usuario", &translations),
            None
        );
    }

    #[test]
    fn game_level_variable_scopes_survive_script_returns() {
        assert!(variable_scope_is_persistent(0));
        assert!(variable_scope_is_persistent(3));
        assert!(!variable_scope_is_persistent(1));
        assert!(!variable_scope_is_persistent(2));
    }

    #[test]
    fn reset_keeps_status_globals_and_clears_components() {
        // sequence.html「リセット」: componentes + vars (salvo status) + stack → Page.
        let path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
        let data = GameData::open(&path).unwrap();
        let assets = Arc::new(Assets::new(data, 2, 64));
        let mut vm = Vm::new(
            assets,
            "ゲームメイン.lsb",
            Options {
                screen: (800, 600),
                cinema_ms: 0.0,
                text_cps: 0.0,
                lookahead: 0,
                trace: false,
                trace_conds: false,
                pick: None,
                show_text: false,
            },
        )
        .unwrap();
        vm.scene
            .create("tmp", crate::scene::Kind::Box, HashMap::new());
        vm.globals.insert("game_flag".into(), Value::Int(1));
        vm.global_scopes.insert("game_flag".into(), 0);
        vm.globals.insert("status_flag".into(), Value::Int(9));
        vm.global_scopes.insert("status_flag".into(), 3);
        let entry = vm.frames[0].script.name.clone();
        let mut detail = String::new();
        let cmd = Command {
            kind: CmdType::Reset as u8,
            indent: 0,
            mute: false,
            not_update: false,
            line_no: 0,
            fields: vec![
                (
                    "Page",
                    crate::lsb::Field::Page(crate::lsb::LabelRef {
                        page: entry,
                        label: 0,
                    }),
                ),
                ("AllClear", crate::lsb::Field::Byte(1)),
            ],
        };
        assert!(vm.step_reset(&cmd, &mut detail));
        assert!(vm.scene.get("tmp").is_none());
        assert!(!vm.globals.contains_key("game_flag"));
        assert_eq!(vm.globals.get("status_flag"), Some(&Value::Int(9)));
        assert_eq!(vm.frames.len(), 1);
    }

    #[test]
    fn pc_reset_all_clear_drops_call_stack() {
        // sequence.html「PCリセット」AllClear ON: stack vacío tras el salto.
        let path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
        let data = GameData::open(&path).unwrap();
        let assets = Arc::new(Assets::new(data, 2, 64));
        let mut vm = Vm::new(
            assets,
            "ゲームメイン.lsb",
            Options {
                screen: (800, 600),
                cinema_ms: 0.0,
                text_cps: 0.0,
                lookahead: 0,
                trace: false,
                trace_conds: false,
                pick: None,
                show_text: false,
            },
        )
        .unwrap();
        let script = vm.frames[0].script.clone();
        let entry = script.name.clone();
        vm.frames.push(Frame {
            script,
            pc: 3,
            locals: HashMap::from([("temp".into(), Value::Int(1))]),
            result_var: String::new(),
            params: vec![],
            saved_state: None,
            sender: None,
            event_prop: None,
            auto_scrap_sender: false,
        });
        assert_eq!(vm.frames.len(), 2);
        let mut detail = String::new();
        let cmd = Command {
            kind: CmdType::PCReset as u8,
            indent: 0,
            mute: false,
            not_update: false,
            line_no: 0,
            fields: vec![
                (
                    "Page",
                    crate::lsb::Field::Page(crate::lsb::LabelRef {
                        page: entry,
                        label: 0,
                    }),
                ),
                ("AllClear", crate::lsb::Field::Byte(1)),
            ],
        };
        assert!(vm.step_pc_reset(&cmd, &mut detail));
        assert_eq!(vm.frames.len(), 1);
        assert!(vm.frames[0].locals.is_empty());
    }

    #[test]
    fn game_audio_volume_uses_thousand_point_scale() {
        assert_eq!(volume_gain(0.0), 0.0);
        assert_eq!(volume_gain(500.0), 0.5);
        assert_eq!(volume_gain(1000.0), 1.0);
        assert_eq!(volume_gain(1500.0), 1.0);
    }

    #[test]
    fn timer_uses_onplayend_and_respects_repeat_pause_and_deletion() {
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
        vm.scene.create(
            "timer",
            Kind::Other(CmdType::Timer as u8),
            HashMap::from([
                (Prop::DELAY, Value::Int(50)),
                (Prop::REPEAT, Value::Bool(true)),
                (Prop::PAUSED, Value::Bool(false)),
                (Prop::ONPLAYEND, Value::Str("timer-tick.lsc".into())),
                (Prop::ONNOTIFY, Value::Str("wrong-event.lsc".into())),
            ]),
        );
        vm.now = 100.0;
        vm.timers.insert(
            "timer".into(),
            TimerState {
                due_at: 100.0,
                paused_at: None,
            },
        );

        vm.update_timers();

        assert_eq!(vm.queue.len(), 1);
        assert_eq!(vm.queue[0].prop, Prop::ONPLAYEND);
        assert_eq!(vm.queue[0].script, "timer-tick.lsc");
        assert_eq!(vm.timers.get("timer").unwrap().due_at, 150.0);

        vm.scene.set("timer", Prop::PAUSED, Value::Bool(true));
        vm.now = 120.0;
        vm.update_timers();
        assert_eq!(vm.timers.get("timer").unwrap().paused_at, Some(120.0));

        vm.now = 200.0;
        vm.update_timers();
        assert_eq!(vm.queue.len(), 1);
        assert_eq!(vm.timers.get("timer").unwrap().due_at, 150.0);

        vm.scene.set("timer", Prop::PAUSED, Value::Bool(false));
        vm.now = 300.0;
        vm.update_timers();
        assert_eq!(vm.queue.len(), 1);
        assert_eq!(vm.timers.get("timer").unwrap().due_at, 330.0);

        vm.now = 329.0;
        vm.update_timers();
        assert_eq!(vm.queue.len(), 1);

        vm.now = 330.0;
        vm.update_timers();
        assert_eq!(vm.queue.len(), 2);
        assert_eq!(vm.timers.get("timer").unwrap().due_at, 380.0);

        vm.enabled
            .insert(("timer".into(), MenuItemKey::Name("choice".into())), false);
        vm.visible
            .insert(("timer".into(), MenuItemKey::Index(1)), false);
        vm.selected
            .insert(("timer".into(), MenuItemKey::Index(1)), true);
        vm.scrap_component("timer");
        assert!(vm.timers.is_empty());
        assert!(vm.enabled.is_empty());
        assert!(vm.visible.is_empty());
        assert!(vm.selected.is_empty());

        vm.now = 400.0;
        vm.update_timers();
        assert!(vm.timers.is_empty());
        assert_eq!(vm.queue.len(), 2);
    }

    #[test]
    fn timer_falls_back_to_onnotify_when_onplayend_is_absent() {
        // LiveNovel: un Timer puede definir solo ONNOTIFY.
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
        vm.scene.create(
            "tick-timer",
            Kind::Other(CmdType::Timer as u8),
            HashMap::from([
                (Prop::DELAY, Value::Int(0)),
                (Prop::ONNOTIFY, Value::Str("00000001.lsc:00000010".into())),
            ]),
        );
        vm.now = 10.0;
        vm.timers.insert(
            "tick-timer".into(),
            TimerState {
                due_at: 10.0,
                paused_at: None,
            },
        );

        vm.update_timers();

        assert_eq!(vm.queue.len(), 1);
        assert_eq!(vm.queue[0].prop, Prop::ONNOTIFY);
        assert_eq!(vm.queue[0].script, "00000001.lsc:00000010");
        assert!(vm.timers.is_empty());
    }

    #[test]
    fn system_se_volume_uses_game_setting_or_default() {
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

        assert_eq!(vm.system_var("StatusSEVolume"), Some(Value::Int(1000)));
        vm.globals.insert("SE音量".into(), Value::Int(425));
        assert_eq!(vm.system_var("StatusSEVolume"), Some(Value::Int(425)));
    }

    #[test]
    fn nested_calls_resolve_nearest_caller_local_variable() {
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
        vm.frames[0].locals.insert("SE音量".into(), Value::Int(80));
        let script = Arc::clone(&vm.frames[0].script);
        vm.frames.push(Frame {
            script,
            pc: 0,
            locals: HashMap::new(),
            result_var: String::new(),
            params: vec![],
            saved_state: None,
            sender: None,
            event_prop: None,
            auto_scrap_sender: false,
        });

        assert_eq!(vm.lookup("SE音量"), Some(&Value::Int(80)));
        vm.assign("SE音量", Value::Int(60));
        assert_eq!(vm.frames[0].locals.get("SE音量"), Some(&Value::Int(60)));

        vm.frames[1].locals.insert("SE音量".into(), Value::Int(20));
        assert_eq!(vm.lookup("SE音量"), Some(&Value::Int(20)));
        vm.assign("SE音量", Value::Int(10));
        assert_eq!(vm.frames[1].locals.get("SE音量"), Some(&Value::Int(10)));
    }

    #[test]
    fn prevmenu_click_updates_the_selected_string_system_variable() {
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
        vm.scene.create("scene-menu", Kind::Menu, HashMap::new());
        vm.prevmenus.insert(
            "scene-menu".into(),
            PrevMenu {
                buttons: vec![PmButton {
                    x: 0,
                    y: 0,
                    w: 40,
                    h: 40,
                    src: String::new(),
                    src_selected: String::new(),
                    src_disabled: String::new(),
                    name: "scene-a".into(),
                    group: 0,
                }],
                hover: None,
            },
        );

        vm.click_at(10.0, 10.0, false);

        assert_eq!(vm.lookup("@SelStr"), Some(&Value::Str("scene-a".into())));
        assert_eq!(
            vm.call_engine(
                Func::GETSELECTED,
                &[
                    Value::Str("scene-menu".into()),
                    Value::Str("scene-a".into()),
                ],
                &[],
            ),
            Some(Value::Bool(true))
        );
        assert_eq!(
            vm.call_engine(
                Func::GETSELECTED,
                &[Value::Str("scene-menu".into()), Value::Str("0".into()),],
                &[],
            ),
            Some(Value::Bool(true))
        );
    }

    #[test]
    fn editable_component_accepts_unicode_and_keyboard_editing() {
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
            "edit",
            Kind::Edit,
            HashMap::from([
                (Prop::LEFT, Value::Int(10)),
                (Prop::TOP, Value::Int(10)),
                (Prop::WIDTH, Value::Int(200)),
                (Prop::HEIGHT, Value::Int(24)),
                (Prop::TEXT, Value::Str("猫".into())),
                (Prop::ONNOTIFY, Value::Str("ゲームメイン.lsb".into())),
            ]),
        );

        vm.assign("@ActiveCompo", Value::Str("edit".into()));
        assert_eq!(
            vm.system_var("@ActiveCompo"),
            Some(Value::Str("edit".into()))
        );
        assert!(vm.focus_edit_at(12.0, 12.0));
        assert!(vm.active_edit_name().is_some());
        assert!(vm.edit_key(37));
        assert!(vm.insert_edit_text("犬"));
        assert_eq!(vm.get_prop("edit", Prop::TEXT), Value::Str("犬猫".into()));
        assert!(vm.edit_key(8));
        assert_eq!(vm.get_prop("edit", Prop::TEXT), Value::Str("猫".into()));
        assert!(
            vm.queue
                .iter()
                .any(|event| event.obj == "edit" && event.params == vec![Value::Int(37)])
        );
    }

    #[test]
    fn explicitly_active_invisible_edit_accepts_text_and_respects_hidden_parent() {
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
        vm.scene.create("form", Kind::Box, HashMap::new());
        vm.scene.create(
            "edit",
            Kind::Edit,
            HashMap::from([
                (Prop::PARENT, Value::Str("form".into())),
                (Prop::VISIBLE, Value::Bool(false)),
                (Prop::TEXT, Value::Str(String::new())),
                (Prop::WIDTH, Value::Int(120)),
                (Prop::HEIGHT, Value::Int(16)),
            ]),
        );

        vm.assign("@ActiveCompo", Value::Str("edit".into()));
        assert_eq!(vm.active_edit_name(), Some("edit"));
        assert!(vm.insert_edit_text("entrada"));
        assert_eq!(
            vm.get_prop("edit", Prop::TEXT),
            Value::Str("entrada".into())
        );

        vm.scene.set("form", Prop::VISIBLE, Value::Bool(false));
        assert_eq!(vm.active_edit_name(), None);
    }

    #[test]
    fn right_click_scraps_modal_menu_when_script_property_allows_it() {
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
            "menu",
            Kind::Menu,
            HashMap::from([(Prop::RCLICKSCRAP, Value::Bool(true))]),
        );
        vm.state = State::Menu {
            name: "menu".into(),
            choices: vec!["choice".into()],
            values: vec!["choice".into()],
            var: String::new(),
        };

        vm.click_at(-1.0, -1.0, true);

        assert!(matches!(vm.state, State::Running));
        assert!(vm.scene.get("menu").is_none());
    }

    #[test]
    fn right_click_scraps_after_menu_handler_finishes() {
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
            "menu",
            Kind::Menu,
            HashMap::from([
                (Prop::RCLICKSCRAP, Value::Bool(true)),
                (Prop::ONRBTNUP, Value::Str("ゲームメイン.lsb".into())),
            ]),
        );
        vm.state = State::Menu {
            name: "menu".into(),
            choices: vec!["choice".into()],
            values: vec!["choice".into()],
            var: String::new(),
        };

        vm.click_at(-1.0, -1.0, true);
        assert!(vm.scene.get("menu").is_some());
        vm.dispatch_events();
        assert!(vm.frames.last().unwrap().auto_scrap_sender);
        vm.do_return(Value::Empty);

        assert!(vm.scene.get("menu").is_none());
    }

    #[test]
    fn choosing_auto_scrap_menu_without_handler_removes_it() {
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
            "menu",
            Kind::Menu,
            HashMap::from([(Prop::AUTOSCRAP, Value::Bool(true))]),
        );
        vm.state = State::Menu {
            name: "menu".into(),
            choices: vec!["first".into()],
            values: vec!["first".into()],
            var: String::new(),
        };

        vm.choose(0);

        assert!(vm.scene.get("menu").is_none());
    }

    #[test]
    fn auto_scrap_menu_is_removed_after_onselect_handler_returns() {
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
            "menu",
            Kind::Menu,
            HashMap::from([
                (Prop::AUTOSCRAP, Value::Bool(true)),
                (Prop::ONSELECT, Value::Str("ゲームメイン.lsb".into())),
            ]),
        );
        vm.state = State::Menu {
            name: "menu".into(),
            choices: vec!["first".into()],
            values: vec!["first".into()],
            var: String::new(),
        };

        vm.choose(0);
        assert!(vm.scene.get("menu").is_some());
        vm.dispatch_events();

        assert!(vm.frames.last().unwrap().auto_scrap_sender);

        vm.do_return(Value::Empty);

        assert!(vm.scene.get("menu").is_none());
        assert!(vm.scene.was_deleted("menu"));
        assert!(matches!(vm.state, State::Running));
    }

    #[test]
    fn stay_open_menu_restores_modal_after_onselect() {
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
            "menu",
            Kind::Menu,
            HashMap::from([(Prop::ONSELECT, Value::Str("ゲームメイン.lsb".into()))]),
        );
        vm.state = State::Menu {
            name: "menu".into(),
            choices: vec!["a".into(), "b".into()],
            values: vec!["a".into(), "b".into()],
            var: "sel".into(),
        };

        vm.choose(1);
        assert!(matches!(vm.state, State::Menu { .. }));
        vm.dispatch_events();
        assert!(matches!(vm.state, State::Running));
        vm.do_return(Value::Empty);

        assert!(matches!(
            vm.state,
            State::Menu {
                name,
                ..
            } if name == "menu"
        ));
        assert!(vm.scene.get("menu").is_some());
    }

    #[test]
    fn higher_priority_button_wins_over_overlapping_prevmenu() {
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
            "menu",
            Kind::Menu,
            HashMap::from([
                (Prop::PRIORITY, Value::Int(100)),
                (Prop::ONSELECT, Value::Str("menu.lsc".into())),
            ]),
        );
        vm.scene.create(
            "button",
            Kind::Button,
            HashMap::from([
                (Prop::LEFT, Value::Int(0)),
                (Prop::TOP, Value::Int(0)),
                (Prop::WIDTH, Value::Int(40)),
                (Prop::HEIGHT, Value::Int(40)),
                (Prop::PRIORITY, Value::Int(101)),
                (Prop::ONNOTIFY, Value::Str("button.lsc".into())),
            ]),
        );
        vm.prevmenus.insert(
            "menu".into(),
            PrevMenu {
                buttons: vec![PmButton {
                    x: 0,
                    y: 0,
                    w: 40,
                    h: 40,
                    src: String::new(),
                    src_selected: String::new(),
                    src_disabled: String::new(),
                    name: "item".into(),
                    group: 0,
                }],
                hover: None,
            },
        );

        vm.click_at(10.0, 10.0, false);

        let event = vm.queue.pop_front().unwrap();
        assert_eq!(event.obj, "button");
        assert_eq!(event.script, "button.lsc");
        assert_eq!(vm.lookup("@SelStr"), Some(&Value::Str(String::new())));
    }

    #[test]
    fn runs_fixture_to_end() {
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
        assert!(vm.stats.is_empty());
        assert!(
            vm.events
                .iter()
                .any(|e| matches!(e, Event::PlaySound { file, .. } if file == "se.wav"))
        );
    }

    #[test]
    fn applies_catalog_translation_to_static_textins() {
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
        vm.set_translations(HashMap::from([(
            "00000001.lsb:line=13:command=12:field=Text".to_string(),
            "Hola\nmundo".to_string(),
        )]));

        let mut time = 0.0;
        let mut observed = String::new();
        for _ in 0..100 {
            vm.update(time, 1000);
            if let Some(message) = vm.scene.get("msg")
                && !message.text.is_empty()
            {
                observed = message
                    .text
                    .iter()
                    .map(|item| match item {
                        TextItem::Char(character) => character.as_str(),
                        TextItem::Newline => "\n",
                        TextItem::Image { .. } => "",
                    })
                    .collect();
                break;
            }
            time += 50.0;
        }
        assert!(observed.starts_with("Hola"), "{observed:?}");
    }

    #[test]
    fn translates_selected_menu_values_when_inserted_into_text() {
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
        let source = "●断面を表示する";
        vm.set_translations(HashMap::from([(
            crate::text_export::menu_item_id(source),
            "●Mostrar cortes".to_string(),
        )]));
        vm.assign("selected", Value::Str(source.to_string()));
        vm.scene.create("msg", Kind::Message, HashMap::new());
        let text = crate::lsb::TpWord {
            body: vec![Glyph::Var {
                name: "selected".into(),
                hist: false,
            }],
            ..crate::lsb::TpWord::default()
        };

        vm.run_text("msg", &text, false, false, false);
        vm.update_plays();

        let observed = vm
            .scene
            .get("msg")
            .unwrap()
            .text
            .iter()
            .map(|item| match item {
                TextItem::Char(character) => character.as_str(),
                TextItem::Newline => "\n",
                TextItem::Image { .. } => "",
            })
            .collect::<String>();
        assert_eq!(observed, "●Mostrar cortes");
        assert_eq!(vm.lookup("selected").unwrap().as_str(), source);
    }

    #[test]
    fn backward_jump_yields_the_rest_of_the_frame() {
        let p =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
        let data = GameData::open(&p).unwrap();
        let assets = Arc::new(Assets::new(data, 2, 64));
        let mut vm = Vm::new(
            assets,
            "00000001.lsb",
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

        let steps = vm.update(0.0, 1_000);

        assert_eq!(steps, 6);
        assert_eq!(vm.location(), "00000001.lsb:2");
    }

    #[test]
    fn button_notify_fires_on_click_not_mouse_hover() {
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
            "hover",
            Kind::Button,
            HashMap::from([
                (Prop::LEFT, Value::Int(10)),
                (Prop::TOP, Value::Int(10)),
                (Prop::WIDTH, Value::Int(100)),
                (Prop::HEIGHT, Value::Int(100)),
                (Prop::ONNOTIFY, Value::Str("game.lsc:hover".into())),
            ]),
        );

        vm.mouse_move(20.0, 20.0);
        assert!(vm.queue.is_empty());
        vm.mouse_move(150.0, 20.0);
        assert!(vm.queue.is_empty());
        vm.click_at(20.0, 20.0, false);
        assert_eq!(vm.queue.len(), 1);
        assert_eq!(vm.queue.front().unwrap().prop, Prop::ONNOTIFY);
        vm.mouse_move(30.0, 30.0);
        assert_eq!(vm.queue.len(), 1);
        vm.click_at(150.0, 20.0, false);
        assert_eq!(vm.queue.len(), 1);
    }

    #[test]
    fn do_event_dispatches_notifications_nested_in_an_event_handler() {
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
            "button",
            Kind::Button,
            HashMap::from([(Prop::ONNOTIFY, Value::Str("ゲームメイン.lsb".into()))]),
        );
        vm.frames.last_mut().unwrap().saved_state = Some(State::Running);
        assert!(vm.fire("button", Prop::ONNOTIFY, vec![]));

        vm.dispatch_events();
        assert_eq!(vm.queue.len(), 1);
        assert_eq!(vm.frames.len(), 1);

        vm.dispatch_events_from_do_event();

        assert!(vm.queue.is_empty());
        assert_eq!(vm.frames.len(), 2);
        assert_eq!(vm.frames.last().unwrap().sender.as_deref(), Some("button"));
    }

    #[test]
    fn wheel_and_arrow_inputs_are_exposed_to_scripts() {
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

        vm.set_mouse_wheel(true, false);
        vm.set_key_push(38, true);
        vm.set_key_push(40, true);

        assert_eq!(vm.system_var("@WheelUp"), Some(Value::Bool(true)));
        assert_eq!(vm.system_var("@WheelDown"), Some(Value::Bool(false)));
        let Some(Value::Array(keys)) = vm.system_var("@KeyPush") else {
            panic!("@KeyPush should be an indexed array");
        };
        assert_eq!(keys[38], Value::Bool(true));
        assert_eq!(keys[40], Value::Bool(true));
    }

    fn fixture_vm() -> Vm {
        let path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
        let data = GameData::open(&path).unwrap();
        let assets = Arc::new(Assets::new(data, 1, 8));
        Vm::new(
            assets,
            "ゲームメイン.lsb",
            Options {
                screen: (800, 600),
                cinema_ms: 0.0,
                text_cps: 0.0,
                lookahead: 0,
                trace: false,
                trace_conds: false,
                pick: None,
                show_text: false,
            },
        )
        .unwrap()
    }

    #[test]
    fn only_idle_handlers_return_without_yielding_the_frame() {
        let mut vm = fixture_vm();
        let script = vm.frames[0].script.clone();
        for (prop, expect_yield) in [(Prop::ONNOTIFY, true), (Prop::ONIDLE, false)] {
            vm.yielded = false;
            vm.frames.push(Frame {
                script: script.clone(),
                pc: 0,
                locals: HashMap::new(),
                result_var: String::new(),
                params: vec![],
                saved_state: Some(State::Running),
                sender: None,
                event_prop: Some(prop),
                auto_scrap_sender: false,
            });
            vm.do_return(Value::Empty);
            assert_eq!(vm.yielded, expect_yield, "{}", crate::lsb::prop_name(prop));
        }
    }

    fn int_expr(value: i32) -> LiveParser {
        LiveParser {
            entries: vec![crate::lsb::OpeData {
                op: crate::lsb::op::TO,
                name: String::new(),
                func: None,
                operands: vec![Param::Int(value)],
            }],
        }
    }

    fn var_expr(name: &str) -> LiveParser {
        LiveParser {
            entries: vec![crate::lsb::OpeData {
                op: crate::lsb::op::TO,
                name: String::new(),
                func: None,
                operands: vec![Param::Var(name.into())],
            }],
        }
    }

    fn install_script(vm: &mut Vm, commands: Vec<Command>) {
        let frame = vm.frames.last_mut().unwrap();
        frame.script = Arc::new(Lsb {
            name: "wait.lsb".into(),
            version: 117,
            flags: 0,
            command_params: vec![],
            commands,
        });
        frame.pc = 0;
    }

    fn wait_then_flag(calc: LiveParser) -> Vec<Command> {
        vec![
            Command {
                kind: CmdType::Wait as u8,
                indent: 0,
                mute: false,
                not_update: false,
                line_no: 1,
                fields: vec![
                    ("Calc", crate::lsb::Field::Expr(calc)),
                    ("Time", crate::lsb::Field::Expr(int_expr(0))),
                    ("StopEvent", crate::lsb::Field::Expr(int_expr(0))),
                ],
            },
            Command {
                kind: CmdType::Calc as u8,
                indent: 0,
                mute: false,
                not_update: false,
                line_no: 2,
                fields: vec![(
                    "Calc",
                    crate::lsb::Field::Expr(LiveParser {
                        entries: vec![crate::lsb::OpeData {
                            op: crate::lsb::op::TO,
                            name: "done".into(),
                            func: None,
                            operands: vec![Param::Int(1)],
                        }],
                    }),
                )],
            },
        ]
    }

    #[test]
    fn zero_time_wait_stays_while_its_condition_holds() {
        let mut vm = fixture_vm();
        vm.assign("hold", Value::Int(1));
        install_script(&mut vm, wait_then_flag(var_expr("hold")));
        vm.update(0.0, 32);
        vm.update(16.0, 32);
        assert_eq!(vm.frames.last().map(|frame| frame.pc), Some(0));
        assert!(vm.var("done").is_none());
        vm.assign("hold", Value::Int(0));
        vm.update(32.0, 32);
        assert_eq!(vm.var("done"), Some(&Value::Int(1)));
    }

    #[test]
    fn zero_time_literal_wait_advances_on_the_next_frame() {
        let mut vm = fixture_vm();
        install_script(&mut vm, wait_then_flag(int_expr(1)));
        vm.update(0.0, 32);
        assert_eq!(vm.frames.last().map(|frame| frame.pc), Some(1));
        assert!(vm.var("done").is_none());
        vm.update(16.0, 32);
        assert_eq!(vm.var("done"), Some(&Value::Int(1)));
    }

    #[test]
    fn prevmenu_group_minus_one_toggles_and_reports_its_group() {
        let mut vm = fixture_vm();
        vm.scene.create("menu", Kind::Menu, HashMap::new());
        vm.prevmenus.insert(
            "menu".into(),
            PrevMenu {
                buttons: vec![PmButton {
                    x: 0,
                    y: 0,
                    w: 40,
                    h: 40,
                    src: String::new(),
                    src_selected: "down.gal".into(),
                    src_disabled: String::new(),
                    name: "オート".into(),
                    group: -1,
                }],
                hover: None,
            },
        );

        vm.click_at(10.0, 10.0, false);
        assert_eq!(
            vm.call_engine(
                Func::GETSELECTED,
                &[Value::Str("menu".into()), Value::Str("オート".into())],
                &[],
            ),
            Some(Value::Bool(true))
        );
        assert_eq!(
            vm.call_engine(
                Func::GETGROUPINDEX,
                &[Value::Str("menu".into()), Value::Int(0)],
                &[],
            ),
            Some(Value::Int(-1))
        );
        vm.click_at(10.0, 10.0, false);
        assert_eq!(
            vm.call_engine(
                Func::GETSELECTED,
                &[Value::Str("menu".into()), Value::Str("オート".into())],
                &[],
            ),
            Some(Value::Bool(false))
        );
    }

    #[test]
    fn mouse_and_key_edges_are_system_variables() {
        let mut vm = fixture_vm();
        vm.set_mouse_button(false, true);
        assert_eq!(vm.system_var("@LClick"), Some(Value::Bool(true)));
        assert_eq!(vm.system_var("@LPush"), Some(Value::Bool(true)));
        vm.set_mouse_button(false, false);
        assert_eq!(vm.system_var("@LPush"), Some(Value::Bool(false)));
        assert_eq!(vm.system_var("@LClick"), Some(Value::Bool(true)));
        vm.set_key_push(32, true);
        let Some(Value::Array(clicks)) = vm.system_var("@KeyClick") else {
            panic!("@KeyClick");
        };
        assert_eq!(clicks.len(), 512);
        assert_eq!(clicks[32], Value::Bool(true));
        assert_eq!(clicks[13], Value::Bool(false));
    }

    #[test]
    fn already_read_text_reports_one_on_play_start() {
        let mut vm = fixture_vm();
        vm.scene.create(
            "msg",
            Kind::Message,
            HashMap::from([(Prop::ONPLAYSTART, Value::Str("probe.lsb".into()))]),
        );
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
        assert!(!vm.note_text_read("scene.lsb", 4));
        vm.run_text("msg", &text, false, false, false);
        vm.update_plays();
        assert_eq!(vm.queue[0].params, vec![Value::Int(0)]);
        vm.queue.clear();
        assert!(vm.note_text_read("scene.lsb", 4));
        vm.run_text("msg", &text, false, false, true);
        vm.update_plays();
        assert_eq!(vm.queue[0].params, vec![Value::Int(1)]);
    }

    #[test]
    fn load_text_file_stores_one_array_entry_per_line() {
        let dir = std::env::temp_dir().join(format!("livenyan-ltf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/evt/終了待ち.lsb"),
            dir.join("entry.lsb"),
        )
        .unwrap();
        std::fs::write(dir.join("list.txt"), "a/1\r\n\r\nb/2\r\n").unwrap();
        let data = GameData::open(&dir).unwrap();
        let assets = Arc::new(Assets::new(data, 1, 8));
        let mut vm = Vm::new(
            assets,
            "entry.lsb",
            Options {
                screen: (800, 600),
                cinema_ms: 0.0,
                text_cps: 0.0,
                lookahead: 0,
                trace: false,
                trace_conds: false,
                pick: None,
                show_text: false,
            },
        )
        .unwrap();
        let loaded = vm
            .call_engine(
                Func::LOADTEXTFILE,
                &[Value::Empty, Value::Str("list.txt".into())],
                &[Param::Var("list".into()), Param::Str("list.txt".into())],
            )
            .unwrap();
        assert_eq!(loaded, Value::Bool(true));
        assert_eq!(
            vm.var("list"),
            Some(&Value::Array(vec![
                Value::Str("a/1".into()),
                Value::Str(String::new()),
                Value::Str("b/2".into()),
            ]))
        );
        let missing = vm
            .call_engine(
                Func::LOADTEXTFILE,
                &[Value::Empty, Value::Str("missing.txt".into())],
                &[
                    Param::Var("missing".into()),
                    Param::Str("missing.txt".into()),
                ],
            )
            .unwrap();
        assert_eq!(missing, Value::Bool(false));
        assert!(vm.var("missing").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
