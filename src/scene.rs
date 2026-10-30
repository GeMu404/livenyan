//! Modelo de objetos de pantalla (ImgNew/MesNew/BoxNew/TileNew/Button/…).

use crate::lsb::expr::Value;
use crate::lsb::{CmdType, Prop, names::prop_id};
use std::collections::{HashMap, VecDeque};
use std::ops::Range;

/// Nombre de la capa raíz (pantalla) en LiveMaker: el carácter 0x01.
pub const ROOT: &str = "\u{1}";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Image,
    Message,
    Box,
    Tile,
    Button,
    Slider,
    Edit,
    Caption,
    Menu,
    Movie,
    Other(u8),
}

pub(crate) fn menu_line_height(font_height: f64, line_space: f64) -> i64 {
    (font_height + line_space).round().max(1.0) as i64
}

pub(crate) fn menu_visible_range(
    choice_count: usize,
    row_count: i64,
    top_index: i64,
) -> Range<usize> {
    let visible_count = if row_count > 0 {
        choice_count.min(usize::try_from(row_count).unwrap_or(usize::MAX))
    } else {
        choice_count
    };
    let max_start = choice_count.saturating_sub(visible_count);
    let start = if row_count > 0 {
        usize::try_from(top_index.max(0)).unwrap_or(usize::MAX)
    } else {
        0
    }
    .min(max_start);
    start..start + visible_count
}

pub(crate) fn native_menu_size(
    choice_count: usize,
    screen_width: u32,
    font_height: f64,
    line_space: f64,
    margin_y: i64,
) -> (i64, i64) {
    let width = (screen_width as f64 * 0.6).round() as i64;
    let height =
        choice_count as i64 * menu_line_height(font_height, line_space) + 2 * margin_y.max(0);
    (width, height)
}

impl Kind {
    pub fn from_cmd(c: u8) -> Self {
        match CmdType::from(c) {
            CmdType::ImgNew | CmdType::MapImgNew | CmdType::Cinema => Kind::Image,
            CmdType::Movie => Kind::Movie,
            CmdType::MesNew => Kind::Message,
            CmdType::BoxNew => Kind::Box,
            CmdType::TileNew => Kind::Tile,
            CmdType::Button => Kind::Button,
            CmdType::EditNew => Kind::Edit,
            CmdType::Caption | CmdType::CGCaption => Kind::Caption,
            CmdType::SliderNew | CmdType::ScrollbarNew | CmdType::GaugeNew => Kind::Slider,
            CmdType::Menu | CmdType::PrevMenuNew => Kind::Menu,
            _ => Kind::Other(c),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TextItem {
    Char(String),
    Newline,
    Image { source: String, align: u8 },
}

/// Elementos pendientes de la reproducción de texto de un objeto de mensaje.
#[derive(Debug, Clone)]
pub enum Pending {
    Clear,
    /// `already_read`: este TextIns ya se mostró (docs/livenovel/text.html, 再生開始).
    Text {
        items: Vec<TextItem>,
        already_read: bool,
    },
    Click,
    /// Evento en línea (TWdOpeEvent) -> ONNOTIFY.
    Notify(String),
}

/// Reproducción de texto del mensaje. Vive en el objeto: al borrarlo, se va con él.
#[derive(Debug, Clone, Default)]
pub struct Playback {
    pub plan: VecDeque<Pending>,
    pub cur: Vec<TextItem>,
    pub pos: usize,
    pub next: f64,
    pub waiting_click: bool,
    /// El TextIns ya se había mostrado (docs/livenovel/text.html, 再生開始).
    pub already_read: bool,
    /// Efecto WAIT: el texto espera a que termine la animación o el fundido.
    pub wait_for: Option<(String, f64)>,
    /// `WAITPLAY … CLICK`: un clic corta la espera.
    pub click_skips_wait: bool,
    /// Última etiqueta procesada (solo depuración).
    pub last_tag: String,
}

/// Animación GAL del objeto.
#[derive(Debug, Clone)]
pub struct Anim {
    pub source: String,
    pub started: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct Obj {
    pub name: String,
    pub kind: Kind,
    pub seq: u64,
    pub props: HashMap<u8, Value>,
    /// Texto acumulado (objetos de mensaje).
    pub text: Vec<TextItem>,
    /// Cuántos elementos de `text` están visibles (efecto máquina de escribir).
    pub shown: usize,
    /// Se incrementa al cambiar algo visual del texto (invalidación de caché).
    pub text_rev: u64,
    /// Frame actual de una animación GAL (lo avanza el VM).
    pub frame: usize,
    /// Archivado por Flip: el script lo ve; el dibujo y el clic no.
    archived: bool,
    /// Frame en que se creó. Lo usa el cambio de placa.
    born: u64,
    /// Texto en curso. Muere con el objeto.
    pub play: Option<Playback>,
    pub anim: Option<Anim>,
    /// TextIns llegó antes que el MesNew.
    pub placeholder: bool,
    /// El texto terminó y todavía no había ONPLAYEND.
    pub ended_unnotified: bool,
}

impl Obj {
    fn blank(name: String, kind: Kind, seq: u64, props: HashMap<u8, Value>) -> Self {
        Self {
            name,
            kind,
            seq,
            props,
            text: vec![],
            shown: 0,
            text_rev: 0,
            frame: 0,
            archived: false,
            born: 0,
            play: None,
            anim: None,
            placeholder: false,
            ended_unnotified: false,
        }
    }

    pub fn get(&self, p: u8) -> Option<&Value> {
        self.props.get(&p)
    }
    pub fn int(&self, p: u8, def: i64) -> i64 {
        self.props.get(&p).map(|v| v.as_int()).unwrap_or(def)
    }
    pub fn str(&self, p: u8) -> String {
        self.props.get(&p).map(|v| v.as_str()).unwrap_or_default()
    }
    pub fn visible(&self) -> bool {
        self.props
            .get(&Prop::VISIBLE)
            .map(|v| v.truthy())
            .unwrap_or(true)
    }

    /// Escala de dibujo. **fuente primaria** (`docs/livenovel/media.html` Movie):
    /// `PR_FLOATZOOM*` 1 = 1×; `PR_ZOOM*` 256 = 1×. Sin prop → 1×.
    pub fn zoom_xy(&self) -> (f64, f64) {
        zoom_xy_from_props(&self.props)
    }

    /// `docs/livenovel/graphic.html` (マウス透過) nombra IM_NONE / IM_ALL /
    /// IM_CLICK / IM_LCLICK / IM_RCLICK y no da los enteros. El novel system
    /// escribe 0 (recibe), 1 (ignora todo) y 4 (ignora el clic izquierdo).
    /// 2 = ambos clics y 8 = clic derecho: no confirmado.
    pub fn ignores_mouse(&self, hit: MouseHit) -> bool {
        let flags = self.int(Prop::IGNOREMOUSE, 0);
        if flags & 1 != 0 {
            return true;
        }
        match hit {
            MouseHit::Hover => false,
            MouseHit::Left => flags & 0b0110 != 0,
            MouseHit::Right => flags & 0b1010 != 0,
        }
    }
}

/// Evento de ratón que se está probando contra `PR_IGNOREMOUSE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseHit {
    Hover,
    Left,
    Right,
}

/// Igual que [`Obj::zoom_xy`], sobre un mapa de props (p. ej. al crear un Movie).
pub fn zoom_xy_from_props(props: &HashMap<u8, Value>) -> (f64, f64) {
    let float = |p| props.get(&p).map(Value::as_f64);
    let fixed = |p| props.get(&p).map(|v| v.as_f64() / 256.0);
    let positive = |z: f64| z.is_finite().then_some(z).filter(|z| *z > 0.0);
    let x = float(Prop::FLOATZOOMX)
        .or_else(|| fixed(Prop::ZOOMX))
        .and_then(positive)
        .unwrap_or(1.0);
    let y = float(Prop::FLOATZOOMY)
        .or_else(|| fixed(Prop::ZOOMY))
        .and_then(positive)
        .unwrap_or(x);
    (x, y)
}

#[cfg(test)]
mod zoom_tests {
    use super::zoom_xy_from_props;
    use crate::lsb::expr::Value;
    use crate::lsb::names::Prop;
    use std::collections::HashMap;

    #[test]
    fn zoom_defaults_to_identity_and_honors_fixed_point_256() {
        assert_eq!(zoom_xy_from_props(&HashMap::new()), (1.0, 1.0));
        let props = HashMap::from([
            (Prop::ZOOMX, Value::Int(128)),
            (Prop::ZOOMY, Value::Int(512)),
        ]);
        let (x, y) = zoom_xy_from_props(&props);
        assert!((x - 0.5).abs() < 1e-9);
        assert!((y - 2.0).abs() < 1e-9);
    }

    #[test]
    fn float_zoom_overrides_fixed_point() {
        let props = HashMap::from([
            (Prop::ZOOMX, Value::Int(256)),
            (Prop::FLOATZOOMX, Value::Float(1.5)),
        ]);
        let (x, y) = zoom_xy_from_props(&props);
        assert!((x - 1.5).abs() < 1e-9);
        assert!((y - 1.5).abs() < 1e-9);
    }
}

#[derive(Debug, Clone)]
pub struct Motion {
    pub obj: String,
    pub prop: u8,
    pub from: f64,
    pub to: f64,
    pub start: f64,
    pub dur: f64,
    /// Entrada: smoothstep. La lineal sale de negro de golpe.
    pub soft: bool,
}

#[allow(clippy::large_enum_variant)] // Gone es transitorio; boxear Live añadiría una indirección a cada acceso
enum Slot {
    /// El script lo ve. `Obj::archived` es la placa de atrás.
    Live(Obj),
    /// Borrado. GetProp no lo ve; un TextIns posterior hereda las props.
    Gone {
        kind: Kind,
        props: HashMap<u8, Value>,
    },
}

pub struct Scene {
    slots: HashMap<String, Slot>,
    /// Capa `\x01`. No entra en el orden de dibujo.
    root: Obj,
    /// Frame actual. Los objetos creados ahora nacen con este valor.
    cohort: u64,
    seq: u64,
    pub motions: Vec<Motion>,
}

impl Default for Scene {
    fn default() -> Self {
        Self {
            slots: HashMap::new(),
            root: Obj::blank(ROOT.to_string(), Kind::Other(0), 0, HashMap::new()),
            cohort: 0,
            seq: 0,
            motions: Vec::new(),
        }
    }
}

/// `docs/livenovel/graphic.html` (画像/タイル, 明るさ): 255 deja el rectángulo
/// blanco y -255 lo deja negro. 0 no cambia el sprite.
///
/// El primer valor es el multiplicador RGB del sprite (0 negro, 255 igual).
/// El segundo es la capa blanca encima, de 0 a 255.
pub fn brightness_draw(brightness: i64) -> (u8, u8) {
    let b = brightness.clamp(-255, 255);
    if b >= 0 {
        ((255 - b) as u8, b as u8)
    } else {
        ((255 + b) as u8, 0)
    }
}

/// Tamaño sin zoom para dibujo y clic. Si la propiedad es 0, vale el archivo.
pub fn pointer_box(prop_w: i64, prop_h: i64, intrinsic: Option<(u32, u32)>) -> (f64, f64) {
    let width = if prop_w > 0 {
        prop_w as f64
    } else {
        intrinsic.map(|(width, _)| f64::from(width)).unwrap_or(0.0)
    };
    let height = if prop_h > 0 {
        prop_h as f64
    } else {
        intrinsic
            .map(|(_, height)| f64::from(height))
            .unwrap_or(0.0)
    };
    (width, height)
}

/// Convierte el valor de ObjProp (id numérico o nombre) en PropertyType.
pub fn prop_of(v: &Value) -> Option<u8> {
    match v {
        // Los índices numéricos de propiedad son base 0 (igual que los bits de command_params:
        // bit j -> PropertyType j+1). **experimento** H24: 1=PARENT, 3=LEFT, 4=TOP.
        Value::Int(i) if (0..=254).contains(i) => Some(*i as u8 + 1),
        Value::Float(f) if (0.0..=254.0).contains(f) => Some(*f as u8 + 1),
        Value::Str(s) => s
            .trim()
            .parse::<u8>()
            .ok()
            .filter(|&i| i < 255)
            .map(|i| i + 1)
            .or_else(|| prop_id(s.trim())),
        _ => None,
    }
}

impl Scene {
    fn live_ref(&self, name: &str) -> Option<&Obj> {
        match self.slots.get(name) {
            Some(Slot::Live(obj)) => Some(obj),
            _ => None,
        }
    }

    fn live_mut(&mut self, name: &str) -> Option<&mut Obj> {
        match self.slots.get_mut(name) {
            Some(Slot::Live(obj)) => Some(obj),
            _ => None,
        }
    }

    /// Objetos que el script ve, archivados incluidos. La raíz no entra.
    pub fn live(&self) -> impl Iterator<Item = &Obj> {
        self.slots.values().filter_map(|slot| match slot {
            Slot::Live(obj) => Some(obj),
            Slot::Gone { .. } => None,
        })
    }

    pub fn contains(&self, name: &str) -> bool {
        self.live_ref(name).is_some()
    }

    pub fn is_empty(&self) -> bool {
        !self
            .slots
            .values()
            .any(|slot| matches!(slot, Slot::Live(_)))
    }

    /// Props del último ObjDel, para el stand-in de TextIns.
    pub fn retained(&self, name: &str) -> Option<(Kind, HashMap<u8, Value>)> {
        match self.slots.get(name) {
            Some(Slot::Gone { kind, props }) => Some((*kind, props.clone())),
            _ => None,
        }
    }

    pub fn was_deleted(&self, name: &str) -> bool {
        matches!(self.slots.get(name), Some(Slot::Gone { .. }))
    }

    /// Guarda kind y props de un objeto ya sacado de la escena.
    pub fn retain_deleted(&mut self, object: Obj) {
        self.slots.insert(
            object.name.clone(),
            Slot::Gone {
                kind: object.kind,
                props: object.props,
            },
        );
    }

    pub fn create(&mut self, name: &str, kind: Kind, props: HashMap<u8, Value>) {
        // Recrear un objeto lo actualiza en su sitio: conserva orden de dibujo y estado del texto.
        let (text, shown, text_rev, seq, play, anim, placeholder, ended_unnotified) =
            match self.slots.remove(name) {
                Some(Slot::Live(old)) => {
                    // Recrear cuenta como objeto de este frame: si no, el Flip
                    // lo trata como sobra de la visita anterior y lo borra.
                    (
                        old.text,
                        old.shown,
                        old.text_rev + 1,
                        old.seq,
                        old.play,
                        old.anim,
                        old.placeholder,
                        old.ended_unnotified,
                    )
                }
                _ => {
                    self.seq += 1;
                    (vec![], 0, 0, self.seq, None, None, false, false)
                }
            };
        let mut object = Obj::blank(name.to_string(), kind, seq, props);
        object.text = text;
        object.shown = shown;
        object.text_rev = text_rev;
        object.born = self.cohort;
        object.play = play;
        object.anim = anim;
        object.placeholder = placeholder;
        object.ended_unnotified = ended_unnotified;
        self.slots.insert(name.to_string(), Slot::Live(object));
    }

    pub fn get(&self, name: &str) -> Option<&Obj> {
        if name == ROOT {
            return Some(&self.root);
        }
        self.live_ref(name)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut Obj> {
        if name == ROOT {
            return Some(&mut self.root);
        }
        self.live_mut(name)
    }

    pub fn set(&mut self, name: &str, prop: u8, v: Value) -> bool {
        match self.get_mut(name) {
            Some(o) => {
                if prop == Prop::TEXT && matches!(o.kind, Kind::Message | Kind::Caption) {
                    if o.kind == Kind::Message {
                        o.text = v
                            .as_str()
                            .chars()
                            .map(|c| {
                                if c == '\n' {
                                    TextItem::Newline
                                } else {
                                    TextItem::Char(c.to_string())
                                }
                            })
                            .collect();
                        o.shown = o.text.len();
                    }
                    o.text_rev += 1;
                }
                o.props.insert(prop, v);
                true
            }
            None => false,
        }
    }

    /// Borra `name` y sus descendientes (por PR_PARENT); devuelve los objetos borrados.
    pub fn delete_names(&mut self, name: &str) -> Vec<Obj> {
        if name.is_empty() {
            return vec![];
        }
        let mut todo = vec![name.to_string()];
        let mut out = vec![];
        while let Some(x) = todo.pop() {
            if let Some(Slot::Live(object)) = self.slots.remove(&x) {
                out.push(object);
            }
            self.motions.retain(|m| m.obj != x);
            let kids: Vec<String> = self
                .live()
                .filter(|object| object.str(Prop::PARENT) == x)
                .map(|object| object.name.clone())
                .collect();
            todo.extend(kids);
        }
        out
    }

    /// Hijos (PR_PARENT == name) en orden de creación; `recursive` incluye descendientes.
    pub fn children(&self, name: &str, recursive: bool) -> Vec<String> {
        let mut out = vec![];
        let mut todo = vec![name.to_string()];
        while let Some(p) = todo.pop() {
            let mut v: Vec<&Obj> = self.live().filter(|o| o.str(Prop::PARENT) == p).collect();
            v.sort_by_key(|o| o.seq);
            for o in v {
                out.push(o.name.clone());
                if recursive {
                    todo.push(o.name.clone());
                }
            }
        }
        out
    }

    /// Saca objetos de la escena (para SaveCabinet). Sale de la placa de atrás.
    pub fn take(&mut self, name: &str) -> Option<Obj> {
        match self.slots.remove(name) {
            Some(Slot::Live(mut object)) => {
                object.archived = false;
                Some(object)
            }
            Some(gone) => {
                self.slots.insert(name.to_string(), gone);
                None
            }
            None => None,
        }
    }

    pub fn put(&mut self, object: Obj) {
        self.slots.insert(object.name.clone(), Slot::Live(object));
    }

    pub fn names(&self) -> Vec<String> {
        self.live()
            .filter(|object| !self.on_back(&object.name))
            .map(|object| object.name.clone())
            .collect()
    }

    pub fn on_back(&self, name: &str) -> bool {
        let mut current = name.to_string();
        for _ in 0..64 {
            let Some(object) = self.live_ref(&current) else {
                return false;
            };
            if object.archived {
                return true;
            }
            let parent = object.str(Prop::PARENT);
            if parent.is_empty() {
                return false;
            }
            current = parent;
        }
        false
    }

    pub fn mark_back(&mut self, name: &str) {
        if self.live_mut(name).is_some() {
            self.motions.retain(|motion| motion.obj != name);
            if let Some(object) = self.live_mut(name) {
                object.archived = true;
            }
        }
    }

    pub fn unmark_back(&mut self, name: &str) {
        if let Some(object) = self.live_mut(name) {
            object.archived = false;
        }
        // Flip con targets puede archivar padre e hijo (FACE: 土台+メッセージボックス).
        // 再生中/MESON solo SetProp VISIBLE en el 土台: hay que desarchivar la
        // descendencia o el Message sigue fuera de `draw_order`.
        let mut stack = vec![name.to_string()];
        while let Some(parent) = stack.pop() {
            let children: Vec<String> = self
                .live()
                .filter(|object| object.str(Prop::PARENT) == parent)
                .map(|object| object.name.clone())
                .collect();
            for child in children {
                if let Some(object) = self.live_mut(&child) {
                    object.archived = false;
                }
                stack.push(child);
            }
        }
    }

    pub fn set_cohort(&mut self, cohort: u64) {
        self.cohort = cohort;
    }

    pub fn cohort(&self) -> u64 {
        self.cohort
    }

    pub fn born(&self, name: &str) -> u64 {
        self.live_ref(name).map(|object| object.born).unwrap_or(0)
    }

    pub fn clear_back(&mut self) {
        for slot in self.slots.values_mut() {
            if let Slot::Live(object) = slot {
                object.archived = false;
            }
        }
    }

    pub fn clear_plays(&mut self) {
        for slot in self.slots.values_mut() {
            if let Slot::Live(object) = slot {
                object.play = None;
            }
        }
    }

    pub fn back_names(&self) -> Vec<String> {
        self.live()
            .filter(|object| self.on_back(&object.name))
            .map(|object| object.name.clone())
            .collect()
    }

    pub fn delete(&mut self, name: &str) -> usize {
        if name.is_empty() {
            return 0;
        }
        let mut todo = vec![name.to_string()];
        let mut n = 0;
        while let Some(x) = todo.pop() {
            if matches!(self.slots.remove(&x), Some(Slot::Live(_))) {
                n += 1;
            }
            self.motions.retain(|m| m.obj != x);
            let kids: Vec<String> = self
                .live()
                .filter(|object| object.str(Prop::PARENT) == x)
                .map(|object| object.name.clone())
                .collect();
            todo.extend(kids);
        }
        n
    }

    pub fn clear(&mut self) {
        self.slots.clear();
        self.motions.clear();
    }

    /// Rectángulo visible.
    ///
    /// `docs/livenovel/graphic.html`: zoom 256 = 1×; el ancla (`PR_OFFSETX/Y`)
    /// queda alineada con X/Y. `PR_POSZOOMED`: X/Y reales = X/Y × zoom.
    /// `width`/`height` son el tamaño sin zoom (la prop, o el intrínseco).
    pub fn placed_rect(&self, o: &Obj, width: f64, height: f64) -> (f64, f64, f64, f64) {
        let (mut x, mut y) = self.abs_pos(o);
        let (zx, zy) = o.zoom_xy();
        if o.get(Prop::POSZOOMED).is_some_and(Value::truthy) {
            x *= zx;
            y *= zy;
        }
        let (w, h) = (width * zx, height * zy);
        (
            x - o.int(Prop::OFFSETX, 0) as f64 * zx,
            y - o.int(Prop::OFFSETY, 0) as f64 * zy,
            w,
            h,
        )
    }

    /// Posición absoluta sumando las de los padres.
    pub fn abs_pos(&self, o: &Obj) -> (f64, f64) {
        let mut x = o.int(Prop::LEFT, 0) as f64;
        let mut y = o.int(Prop::TOP, 0) as f64;
        let mut p = o.str(Prop::PARENT);
        let mut guard = 0;
        while !p.is_empty() && guard < 32 {
            match self.live_ref(&p) {
                Some(po) => {
                    x += po.int(Prop::LEFT, 0) as f64;
                    y += po.int(Prop::TOP, 0) as f64;
                    p = po.str(Prop::PARENT);
                }
                None => break,
            }
            guard += 1;
        }
        (x, y)
    }

    /// Visibilidad efectiva (el objeto y todos sus padres).
    pub fn effective_visible(&self, o: &Obj) -> bool {
        o.visible() && self.parents_visible(o)
    }

    pub fn parents_visible(&self, o: &Obj) -> bool {
        let mut p = o.str(Prop::PARENT);
        let mut guard = 0;
        while !p.is_empty() && guard < 32 {
            match self.live_ref(&p) {
                Some(po) if !po.visible() => return false,
                Some(po) => p = po.str(Prop::PARENT),
                None => break,
            }
            guard += 1;
        }
        true
    }

    /// Objetos en orden de dibujo (prioridad, luego creación).
    /// Orden de dibujo jerárquico: cada objeto se dibuja después de su padre y sus hijos
    /// justo encima de él; entre hermanos manda PRIORITY y luego el orden de creación.
    pub fn draw_order(&self) -> Vec<&Obj> {
        let mut kids: HashMap<&str, Vec<&Obj>> = HashMap::new();
        let mut roots = vec![];
        let live: Vec<&Obj> = self.live().collect();
        for o in &live {
            let p = o
                .props
                .get(&Prop::PARENT)
                .map(|v| v.as_str())
                .unwrap_or_default();
            if !p.is_empty() && self.contains(&p) && p != o.name {
                kids.entry(self.live_ref(&p).unwrap().name.as_str())
                    .or_default()
                    .push(*o);
            } else {
                roots.push(*o);
            }
        }
        let key = |o: &&Obj| (o.int(Prop::PRIORITY, 0), o.seq);
        roots.sort_by_key(key);
        for v in kids.values_mut() {
            v.sort_by_key(key);
        }
        let mut out = Vec::with_capacity(live.len());
        let mut stack: Vec<&Obj> = roots.into_iter().rev().collect();
        while let Some(o) = stack.pop() {
            if out.len() > live.len() {
                break; // ciclo de padres
            }
            if self.on_back(&o.name) {
                continue;
            }
            out.push(o);
            if let Some(ch) = kids.get(o.name.as_str()) {
                stack.extend(ch.iter().rev());
            }
        }
        out
    }

    pub fn add_motion(&mut self, m: Motion) {
        self.motions
            .retain(|x| !(x.obj == m.obj && x.prop == m.prop));
        self.motions.push(m);
    }

    /// Avanza las animaciones de propiedades (interpolación lineal).
    pub fn update(&mut self, now: f64) {
        let mut done = vec![];
        for (i, m) in self.motions.iter().enumerate() {
            if now < m.start {
                continue;
            }
            let t = if m.dur <= 0.0 {
                1.0
            } else {
                ((now - m.start) / m.dur).clamp(0.0, 1.0)
            };
            let t = if m.soft { t * t * (3.0 - 2.0 * t) } else { t };
            let v = m.from + (m.to - m.from) * t;
            if let Some(Slot::Live(o)) = self.slots.get_mut(&m.obj) {
                o.props.insert(m.prop, Value::Int(v.round() as i64));
            }
            if t >= 1.0 {
                done.push(i);
            }
        }
        for i in done.into_iter().rev() {
            self.motions.remove(i);
        }
    }

    pub fn finish_motions(&mut self) {
        self.update(f64::INFINITY);
    }

    /// Objeto visible más alto bajo el punto que tenga alguno de los manejadores `props`.
    pub fn hit_handler(&self, x: f64, y: f64, props: &[u8], hit: MouseHit) -> Option<&Obj> {
        self.hit_handler_with(x, y, props, hit, |object| {
            (
                object.int(Prop::WIDTH, 0) as f64,
                object.int(Prop::HEIGHT, 0) as f64,
            )
        })
    }

    pub fn hit_handler_with<F>(
        &self,
        x: f64,
        y: f64,
        props: &[u8],
        hit: MouseHit,
        size: F,
    ) -> Option<&Obj>
    where
        F: Fn(&Obj) -> (f64, f64),
    {
        self.draw_order().into_iter().rev().find(|o| {
            if o.ignores_mouse(hit)
                || !props
                    .iter()
                    .any(|p| o.get(*p).is_some_and(|v| !v.as_str().trim().is_empty()))
                || !self.effective_visible(o)
            {
                return false;
            }
            let (width, height) = size(o);
            let (ox, oy, w, h) = self.placed_rect(o, width, height);
            w > 0.0 && h > 0.0 && x >= ox && y >= oy && x < ox + w && y < oy + h
        })
    }

    /// Objeto clicable (Button) más alto bajo el punto lógico.
    pub fn hit_button(&self, x: f64, y: f64) -> Option<&Obj> {
        self.draw_order().into_iter().rev().find(|o| {
            if o.kind != Kind::Button
                || o.ignores_mouse(MouseHit::Left)
                || !self.effective_visible(o)
            {
                return false;
            }
            let (ox, oy, w, h) = self.placed_rect(
                o,
                o.int(Prop::WIDTH, 0) as f64,
                o.int(Prop::HEIGHT, 0) as f64,
            );
            w > 0.0 && h > 0.0 && x >= ox && y >= oy && x < ox + w && y < oy + h
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_visible_range_respects_row_count_and_top_index() {
        assert_eq!(menu_visible_range(30, 10, 12), 12..22);
        assert_eq!(menu_visible_range(30, 10, 100), 20..30);
        assert_eq!(menu_visible_range(3, 10, 2), 0..3);
        assert_eq!(menu_visible_range(30, 0, 12), 0..30);
    }

    #[test]
    fn caption_commands_create_renderable_caption_objects() {
        assert_eq!(Kind::from_cmd(CmdType::Caption as u8), Kind::Caption);
        assert_eq!(Kind::from_cmd(CmdType::CGCaption as u8), Kind::Caption);
    }

    #[test]
    fn deleting_empty_name_does_not_remove_root_objects() {
        let mut scene = Scene::default();
        scene.create("root", Kind::Box, HashMap::new());
        let mut child_props = HashMap::new();
        child_props.insert(Prop::PARENT, Value::Str("root".into()));
        scene.create("child", Kind::Image, child_props);

        assert_eq!(scene.delete(""), 0);
        assert!(scene.delete_names("").is_empty());
        assert!(scene.get("root").is_some());
        assert!(scene.get("child").is_some());
    }

    #[test]
    fn replacing_an_object_preserves_its_draw_order() {
        let mut scene = Scene::default();
        scene.create("background", Kind::Image, HashMap::new());
        scene.create("logo", Kind::Image, HashMap::new());
        scene.create("background", Kind::Box, HashMap::new());

        let order: Vec<_> = scene
            .draw_order()
            .into_iter()
            .map(|obj| obj.name.as_str())
            .collect();
        assert_eq!(order, ["background", "logo"]);
    }

    fn thumb(zoom: i64) -> Obj {
        let mut props = HashMap::new();
        props.insert(Prop::LEFT, Value::Int(150));
        props.insert(Prop::TOP, Value::Int(80));
        props.insert(Prop::WIDTH, Value::Int(100));
        props.insert(Prop::HEIGHT, Value::Int(60));
        props.insert(Prop::OFFSETX, Value::Int(50));
        props.insert(Prop::OFFSETY, Value::Int(30));
        props.insert(Prop::ZOOMX, Value::Int(zoom));
        props.insert(Prop::ZOOMY, Value::Int(zoom));
        props.insert(Prop::ONLBTNDOWN, Value::Str("click".into()));
        props.insert(Prop::ONRBTNDOWN, Value::Str("right".into()));
        Obj {
            name: "thumb".into(),
            kind: Kind::Image,
            seq: 1,
            props,
            text: vec![],
            shown: 0,
            text_rev: 0,
            frame: 0,
            archived: false,
            born: 0,
            play: None,
            anim: None,
            placeholder: false,
            ended_unnotified: false,
        }
    }

    #[test]
    fn placed_rect_anchors_the_offset_and_scales_with_zoom() {
        let scene = Scene::default();
        let full = thumb(256);
        let (x, y, w, h) = scene.placed_rect(&full, 100.0, 60.0);
        assert!((x - 100.0).abs() < 1e-9 && (y - 50.0).abs() < 1e-9);
        assert!((w - 100.0).abs() < 1e-9 && (h - 60.0).abs() < 1e-9);

        let half = thumb(128);
        let (x, y, w, h) = scene.placed_rect(&half, 100.0, 60.0);
        assert!((x - 125.0).abs() < 1e-9 && (y - 65.0).abs() < 1e-9);
        assert!((w - 50.0).abs() < 1e-9 && (h - 30.0).abs() < 1e-9);
    }

    #[test]
    fn brightness_runs_from_black_to_white() {
        assert_eq!(brightness_draw(0), (255, 0));
        assert_eq!(brightness_draw(-255), (0, 0));
        assert_eq!(brightness_draw(255), (0, 255));
        assert_eq!(brightness_draw(-128), (127, 0));
        assert_eq!(brightness_draw(128), (127, 128));
    }

    #[test]
    fn pointer_box_uses_the_file_when_width_and_height_are_unset() {
        assert_eq!(pointer_box(0, 0, Some((133, 100))), (133.0, 100.0));
        assert_eq!(pointer_box(40, 0, Some((133, 100))), (40.0, 100.0));
        assert_eq!(pointer_box(0, 0, None), (0.0, 0.0));
    }

    #[test]
    fn left_click_passes_through_ignore_mouse_4_to_the_object_below() {
        let mut scene = Scene::default();
        let mut cover = HashMap::new();
        cover.insert(Prop::LEFT, Value::Int(0));
        cover.insert(Prop::TOP, Value::Int(0));
        cover.insert(Prop::WIDTH, Value::Int(200));
        cover.insert(Prop::HEIGHT, Value::Int(200));
        cover.insert(Prop::PRIORITY, Value::Int(1100));
        cover.insert(Prop::IGNOREMOUSE, Value::Int(4));
        cover.insert(Prop::ONNOTIFY, Value::Str("box".into()));
        cover.insert(Prop::ONRBTNDOWN, Value::Str("box-right".into()));
        scene.create("box", Kind::Message, cover);

        let mut under = HashMap::new();
        under.insert(Prop::LEFT, Value::Int(10));
        under.insert(Prop::TOP, Value::Int(10));
        under.insert(Prop::WIDTH, Value::Int(40));
        under.insert(Prop::HEIGHT, Value::Int(40));
        under.insert(Prop::PRIORITY, Value::Int(100));
        under.insert(Prop::ONLBTNDOWN, Value::Str("thumb".into()));
        under.insert(Prop::ONRBTNDOWN, Value::Str("thumb-right".into()));
        scene.create("thumb", Kind::Image, under);

        assert_eq!(
            scene
                .hit_handler(
                    20.0,
                    20.0,
                    &[Prop::ONLBTNDOWN, Prop::ONNOTIFY],
                    MouseHit::Left
                )
                .map(|o| o.name.as_str()),
            Some("thumb")
        );
        assert_eq!(
            scene
                .hit_handler(20.0, 20.0, &[Prop::ONRBTNDOWN], MouseHit::Right)
                .map(|o| o.name.as_str()),
            Some("box")
        );
    }
}
