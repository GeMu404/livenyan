//! Modelo de objetos de pantalla (ImgNew/MesNew/BoxNew/TileNew/Button/…).

use crate::lsb::expr::Value;
use crate::lsb::{CmdType, Prop, names::prop_id};
use std::collections::HashMap;

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
}

impl Obj {
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
}

#[derive(Debug, Clone)]
pub struct Motion {
    pub obj: String,
    pub prop: u8,
    pub from: f64,
    pub to: f64,
    pub start: f64,
    pub dur: f64,
}

#[derive(Default)]
pub struct Scene {
    pub objs: HashMap<String, Obj>,
    seq: u64,
    pub motions: Vec<Motion>,
}

/// Convierte el valor de ObjProp (id numérico o nombre) en PropertyType.
pub fn prop_of(v: &Value) -> Option<u8> {
    match v {
        // Los índices numéricos de propiedad son base 0 (igual que los bits de command_params:
        // bit j -> PropertyType j+1). Confirmado con un juego real: 1=PARENT, 3=LEFT, 4=TOP.
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
    pub fn create(&mut self, name: &str, kind: Kind, props: HashMap<u8, Value>) {
        // Replacing an object updates it in place: preserve its draw order and text state.
        let (text, shown, text_rev, seq) = match self.objs.remove(name) {
            Some(old) => (old.text, old.shown, old.text_rev + 1, old.seq),
            None => {
                self.seq += 1;
                (vec![], 0, 0, self.seq)
            }
        };
        let o = Obj {
            name: name.to_string(),
            kind,
            seq,
            props,
            text,
            shown,
            text_rev,
            frame: 0,
        };
        self.objs.insert(name.to_string(), o);
    }

    pub fn get(&self, name: &str) -> Option<&Obj> {
        self.objs.get(name)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut Obj> {
        self.objs.get_mut(name)
    }

    pub fn set(&mut self, name: &str, prop: u8, v: Value) -> bool {
        match self.objs.get_mut(name) {
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

    /// Borra el objeto y, recursivamente, sus hijos (PR_PARENT).
    /// Borra el objeto y sus hijos (PR_PARENT), devolviendo los nombres borrados.
    /// Borra `name` y sus descendientes (por PR_PARENT); devuelve los objetos borrados.
    pub fn delete_names(&mut self, name: &str) -> Vec<Obj> {
        if name.is_empty() {
            return vec![];
        }
        let mut todo = vec![name.to_string()];
        let mut out = vec![];
        while let Some(x) = todo.pop() {
            if let Some(o) = self.objs.remove(&x) {
                out.push(o);
            }
            self.motions.retain(|m| m.obj != x);
            for (k, o) in &self.objs {
                if o.str(Prop::PARENT) == x {
                    todo.push(k.clone());
                }
            }
        }
        out
    }

    /// Hijos (PR_PARENT == name) en orden de creación; `recursive` incluye descendientes.
    pub fn children(&self, name: &str, recursive: bool) -> Vec<String> {
        let mut out = vec![];
        let mut todo = vec![name.to_string()];
        while let Some(p) = todo.pop() {
            let mut v: Vec<&Obj> = self
                .objs
                .values()
                .filter(|o| o.str(Prop::PARENT) == p)
                .collect();
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

    /// Saca objetos de la escena (para SaveCabinet).
    pub fn take(&mut self, name: &str) -> Option<Obj> {
        self.objs.remove(name)
    }

    pub fn put(&mut self, o: Obj) {
        self.objs.insert(o.name.clone(), o);
    }

    pub fn names(&self) -> Vec<String> {
        self.objs.keys().cloned().collect()
    }

    pub fn delete(&mut self, name: &str) -> usize {
        if name.is_empty() {
            return 0;
        }
        let mut todo = vec![name.to_string()];
        let mut n = 0;
        while let Some(x) = todo.pop() {
            if self.objs.remove(&x).is_some() {
                n += 1;
            }
            self.motions.retain(|m| m.obj != x);
            for (k, o) in &self.objs {
                if o.str(Prop::PARENT) == x {
                    todo.push(k.clone());
                }
            }
        }
        n
    }

    pub fn clear(&mut self) {
        self.objs.clear();
        self.motions.clear();
    }

    /// Posición absoluta sumando las de los padres.
    pub fn abs_pos(&self, o: &Obj) -> (f64, f64) {
        let mut x = o.int(Prop::LEFT, 0) as f64;
        let mut y = o.int(Prop::TOP, 0) as f64;
        let mut p = o.str(Prop::PARENT);
        let mut guard = 0;
        while !p.is_empty() && guard < 32 {
            match self.objs.get(&p) {
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
        if !o.visible() {
            return false;
        }
        let mut p = o.str(Prop::PARENT);
        let mut guard = 0;
        while !p.is_empty() && guard < 32 {
            match self.objs.get(&p) {
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
    /// (Antes era un orden global por PRIORITY: un "土台" con prioridad alta tapaba a sus hijos.)
    pub fn draw_order(&self) -> Vec<&Obj> {
        let mut kids: HashMap<&str, Vec<&Obj>> = HashMap::new();
        let mut roots = vec![];
        for o in self.objs.values() {
            let p = o
                .props
                .get(&Prop::PARENT)
                .map(|v| v.as_str())
                .unwrap_or_default();
            if !p.is_empty() && self.objs.contains_key(&p) && p != o.name {
                kids.entry(self.objs.get_key_value(&p).unwrap().0.as_str())
                    .or_default()
                    .push(o);
            } else {
                roots.push(o);
            }
        }
        let key = |o: &&Obj| (o.int(Prop::PRIORITY, 0), o.seq);
        roots.sort_by_key(key);
        for v in kids.values_mut() {
            v.sort_by_key(key);
        }
        let mut out = Vec::with_capacity(self.objs.len());
        let mut stack: Vec<&Obj> = roots.into_iter().rev().collect();
        while let Some(o) = stack.pop() {
            if out.len() > self.objs.len() {
                break; // ciclo de padres
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
            let t = if m.dur <= 0.0 {
                1.0
            } else {
                ((now - m.start) / m.dur).clamp(0.0, 1.0)
            };
            let v = m.from + (m.to - m.from) * t;
            if let Some(o) = self.objs.get_mut(&m.obj) {
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
    pub fn hit_handler(&self, x: f64, y: f64, props: &[u8]) -> Option<&Obj> {
        self.draw_order().into_iter().rev().find(|o| {
            if !props
                .iter()
                .any(|p| o.get(*p).is_some_and(|v| !v.as_str().trim().is_empty()))
                || !self.effective_visible(o)
            {
                return false;
            }
            let (ox, oy) = self.abs_pos(o);
            let (w, h) = (o.int(Prop::WIDTH, 0) as f64, o.int(Prop::HEIGHT, 0) as f64);
            x >= ox && y >= oy && x < ox + w && y < oy + h
        })
    }

    /// Objeto clicable (Button) más alto bajo el punto lógico.
    pub fn hit_button(&self, x: f64, y: f64) -> Option<&Obj> {
        self.draw_order().into_iter().rev().find(|o| {
            if o.kind != Kind::Button || !self.effective_visible(o) {
                return false;
            }
            let (ox, oy) = self.abs_pos(o);
            let (w, h) = (o.int(Prop::WIDTH, 0) as f64, o.int(Prop::HEIGHT, 0) as f64);
            x >= ox && y >= oy && x < ox + w && y < oy + h
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
