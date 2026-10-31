use super::{PmButton, PrevMenu, Vm};
use crate::assets::{Assets, IMAGE_EXTS};
use crate::lsb::expr::Value;
use crate::lsb::names::Prop;
use crate::text::TextRenderer;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum MenuItemKey {
    Index(i64),
    Name(String),
}

pub(crate) fn menu_item_key(value: Option<&Value>) -> MenuItemKey {
    match value {
        Some(Value::Str(name)) => name
            .trim()
            .parse::<i64>()
            .map(MenuItemKey::Index)
            .unwrap_or_else(|_| MenuItemKey::Name(name.clone())),
        Some(value) => MenuItemKey::Index(value.as_int()),
        None => MenuItemKey::Index(0),
    }
}

/// Índice y nombre del ítem PrevMenu al que apunta `item` (docs/livenovel/calc.html:
/// GetSelected/SetSelected aceptan índice o nombre). Sin PrevMenu, solo la clave pedida.
pub(crate) fn prevmenu_item_keys(
    menu: Option<&PrevMenu>,
    item: Option<&Value>,
) -> Vec<MenuItemKey> {
    let primary = menu_item_key(item);
    let Some(menu) = menu else {
        return vec![primary];
    };
    let index = match item {
        Some(Value::Str(name)) => name
            .trim()
            .parse::<usize>()
            .ok()
            .or_else(|| menu.buttons.iter().position(|button| button.name == *name)),
        Some(value) => usize::try_from(value.as_int()).ok(),
        None => None,
    };
    let Some(index) = index else {
        return vec![primary];
    };
    let Some(button) = menu.buttons.get(index) else {
        return vec![primary];
    };
    let index_key = MenuItemKey::Index(index as i64);
    let name_key = MenuItemKey::Name(button.name.clone());
    if index_key == name_key {
        vec![index_key]
    } else {
        vec![index_key, name_key]
    }
}

fn menu_text_lines(text: &str) -> Vec<String> {
    text.split(['\n'])
        .map(|line| line.trim_end_matches('\r').to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

/// interface.html: `CHECKSOURCE` es la tira [normal, cursor encima]. El hueco es un estado.
fn menu_check_width(width: u32, height: u32) -> Option<f32> {
    (height > 0 && width >= 2).then_some((width / 2) as f32)
}

fn menu_image_size(assets: &Assets, source: &str) -> Option<(u32, u32)> {
    if source.is_empty() {
        return None;
    }
    let name = assets.data().resolve(source, IMAGE_EXTS)?;
    match assets.get(&name) {
        crate::assets::Asset::Image(image) => Some((image.w, image.h)),
        _ => None,
    }
}

impl Vm {
    pub fn menu_item_visible(&self, name: &str, index: usize) -> bool {
        self.menu_item_visible_key(name, &MenuItemKey::Index(index as i64))
    }

    pub(crate) fn menu_item_visible_key(&self, name: &str, key: &MenuItemKey) -> bool {
        self.scene.get(name).is_some_and(|o| o.visible())
            && *self
                .visible
                .get(&(name.to_string(), key.clone()))
                .unwrap_or(&true)
    }

    /// calc.html: índice de `label` entre los ítems del menú. `-1` si no está.
    pub(crate) fn menu_label_index(&self, name: &str, label: &str) -> i64 {
        self.menu_labels(name)
            .iter()
            .position(|item| item == label)
            .map(|index| index as i64)
            .unwrap_or(-1)
    }

    pub(crate) fn menu_item_checked(&self, name: &str, index: usize) -> bool {
        self.menu_item_flag(&self.checked, name, Some(&Value::Int(index as i64)), false)
    }

    fn menu_labels(&self, name: &str) -> Vec<String> {
        if let super::State::Menu {
            name: active,
            choices,
            ..
        } = &self.state
            && active == name
        {
            return choices.clone();
        }
        if let Some(menu) = self.prevmenus.get(name) {
            return menu
                .buttons
                .iter()
                .map(|button| button.name.clone())
                .collect();
        }
        let Some(object) = self.scene.get(name) else {
            return Vec::new();
        };
        match object.get(Prop::TEXT) {
            Some(Value::Array(items)) => items
                .iter()
                .map(Value::as_str)
                .filter(|item| !item.is_empty())
                .collect(),
            Some(value) => menu_text_lines(&value.as_str()),
            None => Vec::new(),
        }
    }

    /// Lee el flag de un ítem PrevMenu. `default == false` → OR (selected);
    /// `default == true` → AND (enabled/visible). Nombre e índice quedan alineados
    /// tras SetSelected/SetEnabled/SetVisible.
    pub(crate) fn menu_item_flag(
        &self,
        map: &HashMap<(String, MenuItemKey), bool>,
        menu: &str,
        item: Option<&Value>,
        default: bool,
    ) -> bool {
        let keys = prevmenu_item_keys(self.prevmenus.get(menu), item);
        let mut values = keys.iter().map(|key| {
            *map.get(&(menu.to_string(), key.clone()))
                .unwrap_or(&default)
        });
        let Some(first) = values.next() else {
            return default;
        };
        if default {
            values.fold(first, |a, b| a && b)
        } else {
            values.fold(first, |a, b| a || b)
        }
    }

    pub(crate) fn set_menu_item_selected(&mut self, menu: &str, item: Option<&Value>, value: bool) {
        let keys = prevmenu_item_keys(self.prevmenus.get(menu), item);
        for key in keys {
            self.selected.insert((menu.to_string(), key), value);
        }
    }

    pub(crate) fn set_menu_item_enabled(&mut self, menu: &str, item: Option<&Value>, value: bool) {
        let keys = prevmenu_item_keys(self.prevmenus.get(menu), item);
        for key in keys {
            self.enabled.insert((menu.to_string(), key), value);
        }
    }

    pub(crate) fn set_menu_item_visible_flag(
        &mut self,
        menu: &str,
        item: Option<&Value>,
        value: bool,
    ) {
        let keys = prevmenu_item_keys(self.prevmenus.get(menu), item);
        for key in keys {
            self.visible.insert((menu.to_string(), key), value);
        }
    }

    /// Imagen a dibujar. Un ítem deshabilitado se muestra igual
    /// (`docs/livenovel/selimg.html`, 無効時の画像) y no acepta el ratón.
    pub fn prevmenu_button_image<'a>(
        &self,
        menu: &str,
        index: usize,
        button: &'a PmButton,
        hover: bool,
    ) -> &'a str {
        if !self.prevmenu_item_enabled(menu, index, &button.name) {
            return if button.src_disabled.is_empty() {
                button.src.as_str()
            } else {
                button.src_disabled.as_str()
            };
        }
        if (self.prevmenu_item_selected(menu, index, &button.name) || hover)
            && !button.src_selected.is_empty()
        {
            button.src_selected.as_str()
        } else {
            button.src.as_str()
        }
    }

    pub fn prevmenu_item_visible(&self, name: &str, index: usize, item_name: &str) -> bool {
        self.menu_item_visible(name, index)
            && self.menu_item_visible_key(name, &MenuItemKey::Name(item_name.to_string()))
    }

    pub(crate) fn menu_item_enabled(&self, name: &str, index: usize) -> bool {
        *self
            .enabled
            .get(&(name.to_string(), MenuItemKey::Index(index as i64)))
            .unwrap_or(&true)
    }

    pub fn prevmenu_item_selected(&self, name: &str, index: usize, item_name: &str) -> bool {
        *self
            .selected
            .get(&(name.to_string(), MenuItemKey::Name(item_name.to_string())))
            .unwrap_or(&false)
            || *self
                .selected
                .get(&(name.to_string(), MenuItemKey::Index(index as i64)))
                .unwrap_or(&false)
    }

    pub(crate) fn prevmenu_item_enabled(&self, name: &str, index: usize, item_name: &str) -> bool {
        self.menu_item_enabled(name, index)
            && *self
                .enabled
                .get(&(name.to_string(), MenuItemKey::Name(item_name.to_string())))
                .unwrap_or(&true)
    }

    fn menu_text_width(
        &mut self,
        font_family: &str,
        font_height: f64,
        choices: &[String],
    ) -> Option<f32> {
        let key = font_family.to_string();
        if !self.menu_fonts.contains_key(&key) {
            let loaded = TextRenderer::load_family(
                font_family,
                &self.pixel_fonts,
                self.menu_font_override.clone(),
            );
            if loaded.is_none() {
                log::warn!(
                    "cannot measure menu width for font {font_family:?}; ONOPENING may observe WIDTH=0"
                );
            }
            self.menu_fonts.insert(key.clone(), loaded);
        }
        let renderer = self.menu_fonts.get_mut(&key)?.as_mut()?;
        let font_size = renderer.size_for_cell_height(font_height.max(1.0) as f32);
        choices
            .iter()
            .map(|choice| renderer.measure_text_width(choice, font_size))
            .reduce(f32::max)
    }

    fn menu_intrinsic_width(
        &mut self,
        props: &HashMap<u8, Value>,
        choices: &[String],
        visible_rows: usize,
    ) -> Option<f32> {
        let font_height = props
            .get(&Prop::FONTHEIGHT)
            .map(Value::as_f64)
            .filter(|height| *height > 0.0)
            .unwrap_or(24.0);
        let font_family = props
            .get(&Prop::FONTNAME)
            .map(Value::as_str)
            .unwrap_or_default();
        let text_width = self.menu_text_width(&font_family, font_height, choices)?;

        let border_size = |prop| {
            props
                .get(&prop)
                .map(Value::as_str)
                .and_then(|source| menu_image_size(&self.assets, &source))
                .unwrap_or((0, 0))
        };
        let left_inset = [
            Prop::BORDERSOURCETL,
            Prop::BORDERSOURCECL,
            Prop::BORDERSOURCEBL,
        ]
        .into_iter()
        .map(|prop| border_size(prop).0)
        .max()
        .unwrap_or(0) as f32;
        let right_inset = [
            Prop::BORDERSOURCETR,
            Prop::BORDERSOURCECR,
            Prop::BORDERSOURCEBR,
        ]
        .into_iter()
        .map(|prop| border_size(prop).0)
        .max()
        .unwrap_or(0) as f32;

        let (scrollbar_width, scrollbar_gap) = if visible_rows < choices.len() {
            let source = props
                .get(&Prop::SCROLLSOURCE)
                .map(Value::as_str)
                .unwrap_or_default();
            menu_image_size(&self.assets, &source)
                .filter(|(width, height)| *width >= 7 && *height > 0)
                .map(|(width, _)| ((width / 7) as f32, 4.0))
                .unwrap_or((0.0, 0.0))
        } else {
            (0.0, 0.0)
        };
        let margin_x = props
            .get(&Prop::MARGINX)
            .map(Value::as_int)
            .unwrap_or(0)
            .max(0) as f32;
        let check_width = props
            .get(&Prop::CHECKSOURCE)
            .map(Value::as_str)
            .filter(|source| !source.is_empty())
            .and_then(|source| menu_image_size(&self.assets, &source))
            .and_then(|(width, height)| menu_check_width(width, height))
            .unwrap_or(0.0);
        // interface.html: PR_FONTBORDER es el ancho del contorno. El raster lo dibuja a 1 px.
        let border_pad = if props.get(&Prop::FONTBORDER).is_some_and(Value::truthy) {
            2.0
        } else {
            0.0
        };
        Some(
            (text_width
                + 2.0 * margin_x
                + left_inset
                + right_inset
                + scrollbar_width
                + scrollbar_gap
                + check_width
                + border_pad
                + font_height as f32 * 0.5)
                .min((self.opts.screen.0 as f32 - left_inset - right_inset).max(1.0)),
        )
    }

    pub(crate) fn set_menu_intrinsic_width(
        &mut self,
        props: &mut HashMap<u8, Value>,
        choices: &[String],
        visible_rows: usize,
    ) {
        if props
            .get(&Prop::WIDTH)
            .is_some_and(|width| width.as_int() > 0)
        {
            return;
        }
        let Some(width) = self.menu_intrinsic_width(props, choices, visible_rows) else {
            return;
        };
        let width = width.ceil() as i64;
        props.insert(Prop::WIDTH, Value::Int(width));
        if props
            .get(&Prop::ORIGWIDTH)
            .is_none_or(|original| original.as_int() <= 0)
        {
            props.insert(Prop::ORIGWIDTH, Value::Int(width));
        }
    }

    /// Carga los botones de un .lpm. Posición: si los dos primeros enteros del botón
    /// coinciden con el tamaño de su imagen son tamaño y la posición son unk15/unk16; si no,
    /// son la posición (**no confirmado**, H22; los números van a debug).
    pub(crate) fn load_prevmenu(&mut self, name: &str, src: &str) {
        let data = self.assets.data().clone();
        let Some(file) = data.resolve(src, &[".lpm"]) else {
            log::warn!("PrevMenu {name}: {src:?} not found");
            return;
        };
        let started = std::time::Instant::now();
        let parsed = data
            .read(&file)
            .map_err(|e| e.to_string())
            .and_then(|d| crate::lpm::parse(&d));
        let (ver, btns) = match parsed {
            Ok(v) => v,
            Err(e) => {
                log::warn!("PrevMenu {name}: {file}: {e}");
                return;
            }
        };
        log::info!(
            "load prevmenu {file}: v{ver} {} buttons in {:.1} ms",
            btns.len(),
            started.elapsed().as_secs_f64() * 1000.0
        );
        let mut pm = PrevMenu::default();
        for (i, b) in btns.iter().enumerate() {
            let res = data.resolve_relative(&file, &b.src, IMAGE_EXTS);
            let res_selected = data.resolve_relative(&file, &b.src_selected, IMAGE_EXTS);
            let res_disabled = data.resolve_relative(&file, &b.src_disabled, IMAGE_EXTS);
            let size = res
                .as_ref()
                .and_then(|r| data.read_prefix(r, 4096))
                .and_then(|d| crate::gal::peek_size(&d))
                .map(|(w, h)| (w as i64, h as i64));
            if let Some(r) = &res {
                self.assets.request(r);
            }
            if let Some(r) = &res_selected {
                self.assets.request(r);
            }
            if let Some(r) = &res_disabled {
                self.assets.request(r);
            }
            let (w, h) = size.unwrap_or((b.a.0 as i64, b.a.1 as i64));
            let a_is_size = size == Some((b.a.0 as i64, b.a.1 as i64));
            let (x, y) = if a_is_size {
                (b.b.0 as i64, b.b.1 as i64)
            } else {
                (b.a.0 as i64, b.a.1 as i64)
            };
            log::debug!(
                "PrevMenu {name} (lpm v{ver}) button {i}: ints a=({},{}) b=({},{}) image {:?} size {:?} -> pos ({x},{y}) {w}x{h} src={:?} name={:?} sel={:?}",
                b.a.0,
                b.a.1,
                b.b.0,
                b.b.1,
                res,
                size,
                b.src,
                b.name,
                b.src_selected
            );
            pm.buttons.push(PmButton {
                x,
                y,
                w,
                h,
                src: res.unwrap_or_else(|| b.src.clone()),
                src_selected: res_selected.unwrap_or_else(|| b.src_selected.clone()),
                src_disabled: res_disabled.unwrap_or_else(|| b.src_disabled.clone()),
                name: b.name.clone(),
                group: b.group,
            });
        }
        self.prevmenus.insert(name.to_string(), pm);
    }

    /// Botón de PrevMenu bajo (x, y) lógicos: (menú, índice).
    ///
    /// `PR_IGNOREMOUSE` (manual: マウス透過 / `IM_*`) afecta si el objeto recibe
    /// eventos de ratón — **no** la visibilidad del dibujo. No usarlo para ocultar chrome.
    pub(crate) fn prevmenu_hit(&self, x: f64, y: f64) -> Option<(String, usize)> {
        let mut best: Option<(i64, u64, String, usize)> = None;
        for (n, pm) in &self.prevmenus {
            let Some(o) = self.scene.get(n) else { continue };
            if !self.scene.effective_visible(o) {
                continue;
            }
            let (ox, oy) = self.scene.abs_pos(o);
            for (i, b) in pm.buttons.iter().enumerate() {
                if !self.prevmenu_item_visible(n, i, &b.name)
                    || !self.prevmenu_item_enabled(n, i, &b.name)
                {
                    continue;
                }
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
}

#[cfg(test)]
mod tests {
    use super::super::step_menu::translate_menu_choices;
    use super::super::{Options, PmButton, PrevMenu, State, Vm};
    use crate::archive::GameData;
    use crate::assets::Assets;
    use crate::lsb::expr::{Env, Value};
    use crate::lsb::names::{Func, Prop};
    use crate::scene::Kind;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[test]
    fn menu_width_is_measured_before_opening_handlers_run() {
        let path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
        let data = GameData::open(&path).unwrap();
        let assets = Arc::new(Assets::new(data, 2, 64));
        let mut vm = Vm::new(
            assets,
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
        vm.set_menu_font_override(crate::text::find_font(None, 0));
        let choices = vec!["セーブする".into(), "ロードする".into()];
        let mut props = HashMap::from([
            (Prop::FONTHEIGHT, Value::Int(22)),
            (Prop::MARGINX, Value::Int(16)),
            (Prop::ROWCOUNT, Value::Int(10)),
        ]);

        vm.set_menu_intrinsic_width(&mut props, &choices, choices.len());

        let width = props.get(&Prop::WIDTH).unwrap().as_int();
        assert!(width > 0 && width < 400);
        assert_eq!(
            props.get(&Prop::ORIGWIDTH).unwrap().as_int(),
            width,
            "ONOPENING must also see the intrinsic original width"
        );
    }

    #[test]
    fn translates_visible_menu_labels_without_changing_choice_values() {
        let source = vec!["セーブ".to_string(), "ロード".to_string()];
        let translations = HashMap::from([(
            crate::text_export::menu_item_id("セーブ"),
            "Guardar".to_string(),
        )]);
        assert_eq!(
            translate_menu_choices(&source, &translations),
            vec!["Guardar", "ロード"]
        );
        assert_eq!(source, vec!["セーブ", "ロード"]);
    }

    #[test]
    fn choosing_menu_item_dispatches_onselect() {
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
            HashMap::from([(Prop::ONSELECT, Value::Str("selection.lsc".into()))]),
        );
        vm.state = State::Menu {
            name: "menu".into(),
            choices: vec!["Primero".into(), "Segundo".into()],
            values: vec!["first".into(), "second".into()],
            var: "selection".into(),
        };

        vm.choose(1);

        assert!(matches!(vm.state, State::Menu { .. }));
        assert_eq!(vm.lookup("selection"), Some(&Value::Str("second".into())));
        assert_eq!(vm.lookup("@SelStr"), Some(&Value::Str("second".into())));
        assert_eq!(
            vm.scene.get("menu").and_then(|o| o.get(Prop::INDEX)),
            Some(&Value::Int(1))
        );
        assert_eq!(
            vm.scene.get("menu").and_then(|o| o.get(Prop::VALUE)),
            Some(&Value::Int(1))
        );
        let event = vm.queue.pop_front().unwrap();
        assert_eq!(event.script, "selection.lsc");
        assert_eq!(
            event.params,
            vec![Value::Str("second".into()), Value::Int(1)]
        );
    }

    #[test]
    fn menu_onopening_handler_runs_before_modal_menu_waits() {
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
            HashMap::from([(Prop::ONOPENING, Value::Str("ゲームメイン.lsb".into()))]),
        );
        vm.state = State::Menu {
            name: "menu".into(),
            choices: vec!["first".into()],
            values: vec!["first".into()],
            var: String::new(),
        };
        assert!(vm.fire("menu", Prop::ONOPENING, vec![]));

        vm.dispatch_events();

        assert!(matches!(vm.state, State::Running));
        let handler = vm.frames.last().unwrap();
        assert_eq!(handler.sender.as_deref(), Some("menu"));
        assert!(matches!(handler.saved_state, Some(State::Menu { .. })));
        vm.do_return(Value::Empty);
        assert!(matches!(vm.state, State::Menu { .. }));
    }

    #[test]
    fn prevmenu_helpers_preserve_numeric_and_named_item_state() {
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
        vm.scene.create("menu", Kind::Menu, HashMap::new());
        vm.prevmenus.insert(
            "menu".into(),
            PrevMenu {
                buttons: vec![
                    PmButton {
                        x: 0,
                        y: 0,
                        w: 1,
                        h: 1,
                        src: String::new(),
                        src_selected: String::new(),
                        src_disabled: String::new(),
                        name: "first".into(),
                        group: 0,
                    },
                    PmButton {
                        x: 0,
                        y: 0,
                        w: 1,
                        h: 1,
                        src: String::new(),
                        src_selected: String::new(),
                        src_disabled: String::new(),
                        name: "second".into(),
                        group: 0,
                    },
                ],
                hover: None,
            },
        );

        assert_eq!(
            vm.call_engine(
                Func::GETITEM,
                &[Value::Str("menu".into()), Value::Int(1)],
                &[]
            ),
            Some(Value::Str("second".into()))
        );
        assert_eq!(
            vm.call_engine(
                Func::GETITEM,
                &[Value::Str("menu".into()), Value::Int(-1)],
                &[]
            ),
            Some(Value::Str(String::new()))
        );
        assert_eq!(
            vm.call_engine(
                Func::GETVISIBLE,
                &[Value::Str("menu".into()), Value::Int(0)],
                &[]
            ),
            Some(Value::Bool(true))
        );
        assert_eq!(
            vm.call_engine(
                Func::SETVISIBLE,
                &[Value::Str("menu".into()), Value::Int(1), Value::Bool(false)],
                &[],
            ),
            Some(Value::Bool(true))
        );
        assert_eq!(
            vm.call_engine(
                Func::GETVISIBLE,
                &[Value::Str("menu".into()), Value::Int(1)],
                &[]
            ),
            Some(Value::Bool(false))
        );
        assert_eq!(
            vm.call_engine(
                Func::SETVISIBLE,
                &[
                    Value::Str("menu".into()),
                    Value::Str("スキップ".into()),
                    Value::Bool(false),
                ],
                &[]
            ),
            Some(Value::Bool(true))
        );
        assert_eq!(
            vm.call_engine(
                Func::GETVISIBLE,
                &[Value::Str("menu".into()), Value::Str("スキップ".into())],
                &[]
            ),
            Some(Value::Bool(false))
        );
        assert!(!vm.prevmenu_item_visible("menu", 0, "スキップ"));
        assert!(vm.prevmenu_item_visible("menu", 0, "オート"));
        // IGNOREMOUSE (IM_*) = passthrough de eventos, no oculta el dibujo.
        vm.scene.set("menu", Prop::IGNOREMOUSE, Value::Int(4));
        assert!(vm.prevmenu_item_visible("menu", 0, "オート"));
        for (item, selected) in [("オート", true), ("スキップ", false)] {
            assert_eq!(
                vm.call_engine(
                    Func::SETSELECTED,
                    &[
                        Value::Str("menu".into()),
                        Value::Str(item.into()),
                        Value::Bool(selected),
                    ],
                    &[]
                ),
                Some(Value::Bool(true))
            );
        }
        assert_eq!(
            vm.call_engine(
                Func::SETCHECK,
                &[Value::Str("menu".into()), Value::Int(1), Value::Bool(true),],
                &[]
            ),
            Some(Value::Bool(true))
        );
        assert_eq!(
            vm.call_engine(
                Func::GETCHECK,
                &[Value::Str("menu".into()), Value::Int(1)],
                &[]
            ),
            Some(Value::Bool(true))
        );
        assert_eq!(
            vm.call_engine(
                Func::GETCHECK,
                &[Value::Str("menu".into()), Value::Int(0)],
                &[]
            ),
            Some(Value::Bool(false))
        );
        assert_eq!(
            vm.call_engine(
                Func::GETSELECTED,
                &[Value::Str("menu".into()), Value::Str("オート".into())],
                &[]
            ),
            Some(Value::Bool(true))
        );
        assert_eq!(
            vm.call_engine(
                Func::GETSELECTED,
                &[Value::Str("menu".into()), Value::Str("スキップ".into())],
                &[]
            ),
            Some(Value::Bool(false))
        );
        // Nombre e índice del mismo botón PrevMenu quedan alineados.
        assert_eq!(
            vm.call_engine(
                Func::SETSELECTED,
                &[
                    Value::Str("menu".into()),
                    Value::Str("first".into()),
                    Value::Bool(true),
                ],
                &[],
            ),
            Some(Value::Bool(true))
        );
        assert_eq!(
            vm.call_engine(
                Func::GETSELECTED,
                &[Value::Str("menu".into()), Value::Int(0)],
                &[]
            ),
            Some(Value::Bool(true))
        );
        assert!(vm.prevmenu_item_selected("menu", 0, "first"));
        assert_eq!(
            vm.call_engine(
                Func::SETENABLED,
                &[
                    Value::Str("menu".into()),
                    Value::Str("first".into()),
                    Value::Bool(false),
                ],
                &[]
            ),
            Some(Value::Bool(true))
        );
        assert_eq!(
            vm.call_engine(
                Func::GETENABLED,
                &[Value::Str("menu".into()), Value::Str("first".into())],
                &[]
            ),
            Some(Value::Bool(false))
        );
        assert_eq!(
            vm.call_engine(
                Func::GETENABLED,
                &[Value::Str("menu".into()), Value::Str("second".into())],
                &[]
            ),
            Some(Value::Bool(true))
        );
        assert_eq!(
            vm.call_engine(
                Func::GETENABLED,
                &[Value::Str("menu".into()), Value::Int(0)],
                &[]
            ),
            Some(Value::Bool(false))
        );
        assert!(!vm.prevmenu_item_enabled("menu", 0, "first"));
        assert!(vm.prevmenu_item_enabled("menu", 1, "second"));
        // AddDelimiter es calc de expresión (expr.rs / calc.html), no call_engine.
        assert!(vm.scene.get("menu").unwrap().visible());
        let skip = PmButton {
            x: 0,
            y: 0,
            w: 1,
            h: 1,
            src: "skip.gal".into(),
            src_selected: "skip-on.gal".into(),
            src_disabled: "skip-off.gal".into(),
            name: "first".into(),
            group: -1,
        };
        // first ya está deshabilitado arriba; imagen de inválido.
        assert_eq!(
            vm.prevmenu_button_image("menu", 0, &skip, true),
            "skip-off.gal"
        );
        vm.call_engine(
            Func::SETENABLED,
            &[
                Value::Str("menu".into()),
                Value::Str("first".into()),
                Value::Bool(true),
            ],
            &[],
        );
        assert_eq!(
            vm.prevmenu_button_image("menu", 0, &skip, true),
            "skip-on.gal"
        );
    }
}
