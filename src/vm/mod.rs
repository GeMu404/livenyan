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

mod click;
mod engine;
pub(crate) use engine::resample_nearest;
mod media;
mod menu;
mod screen;
mod slider;
mod step_flow;
mod step_media;
mod step_menu;
mod step_obj;
mod step_prop;
mod step_save;
mod step_text;
mod sysevent;
mod text_hist;
mod wipe;
use engine::*;

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
        // REPEAT de un cine de imágenes no rebobina los clips ya vistos.
        // El último GAL se queda y su ciclo (duración del clip) vuelve a empezar.
        // Un cine de texto (cuenta atrás) sigue dando la vuelta a toda la línea.
        if self.repeat
            && let Some((start, len)) = self.movie.image_tail_loop()
        {
            let start_ms = f64::from(start) * 1000.0 / 60.0;
            let len_ms = f64::from(len) * 1000.0 / 60.0;
            if elapsed > start_ms && len_ms > 0.0 {
                return start_ms + (elapsed - start_ms).rem_euclid(len_ms);
            }
            return elapsed;
        }
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
            // Clic salta los cinemas de una sola pasada; los que repiten (HUD) siguen
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
        // Un manejador a la vez: si ya hay uno activo, esperar a que termine
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
        // El texto puede haber empezado en este frame
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

    fn goto_script(
        &mut self,
        page: &str,
        label: u32,
        push: bool,
        result_var: String,
        params: Vec<Value>,
    ) -> bool {
        // Manejadores "script.lsc:ETIQUETA": empiezan en el comando Label con ese nombre
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
            // Param1..N: **provisional** (COMPATIBILITY H1).
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
            // Fin de script sin Exit: retorna
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
                // Vuelta de un manejador de evento: restaurar lo que hacía el script.
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

impl Vm {
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
            // Por marco de llamada: cada Call apila los suyos y se recuperan al volver
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
        // El texto del ítem (si el menú lo tiene en PR_TEXT, una línea por ítem)
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

#[cfg(test)]
mod tests;
