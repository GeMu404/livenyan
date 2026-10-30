use super::Vm;
use crate::assets::Assets;
use crate::assets::IMAGE_EXTS;
use crate::lsb::expr::Value;
use crate::lsb::names::Prop;
use crate::scene::Kind;
use crate::slider::{SliderPart, slider_size};

pub(super) fn source_pixel_size(assets: &Assets, source: &str) -> Option<(u32, u32)> {
    if source.is_empty() {
        return None;
    }
    let name = assets.data().resolve(source, IMAGE_EXTS)?;
    if let Some(crate::assets::Asset::Image(image)) = assets.try_get(&name) {
        return Some((image.w, image.h));
    }
    // Solo la cabecera. El decode del frame en este hilo retrasa el zoom.
    let peeked = assets
        .data()
        .read_prefix(&name, 65536)
        .and_then(|bytes| crate::gal::peek_size(&bytes))
        .or_else(|| {
            let bytes = assets.data().read(&name).ok()?;
            crate::gal::peek_size(&bytes)
        });
    assets.request(&name);
    peeked
}

impl Vm {
    pub(crate) fn slider_visual_state(&self, name: &str) -> u32 {
        if self.slider_drag.as_deref() == Some(name) {
            2
        } else if self.is_hovered(name) {
            1
        } else {
            0
        }
    }

    pub(crate) fn end_slider_drag(&mut self) {
        self.slider_drag = None;
    }

    pub(crate) fn slider_keydown(&mut self, key: i64) -> bool {
        let Some(name) = self.slider_focus.clone() else {
            return false;
        };
        let Some(object) = self.scene.get(&name) else {
            self.slider_focus = None;
            return false;
        };
        if object.kind != Kind::Slider {
            return false;
        }
        let orientation = object.int(Prop::ORIENTATION, 0);
        let step = object.int(Prop::SMALLCHANGE, 1).max(1);
        let delta = match key {
            37 if orientation == 0 => -step,
            38 if orientation != 0 => -step,
            39 if orientation == 0 => step,
            40 if orientation != 0 => step,
            33 => object.int(Prop::LARGECHANGE, step).max(step),
            34 => -object.int(Prop::LARGECHANGE, step).max(step),
            _ => return false,
        };
        let current = object.int(Prop::VALUE, 0);
        self.set_slider_value(&name, current.saturating_add(delta), true);
        self.fire(&name, Prop::ONKEYDOWN, vec![Value::Int(key)]);
        true
    }

    fn set_slider_value(&mut self, name: &str, value: i64, notify: bool) -> bool {
        let Some(object) = self.scene.get_mut(name) else {
            return false;
        };
        if object.kind != Kind::Slider {
            return false;
        }
        let min = object.int(Prop::MIN, 0);
        let max = object.int(Prop::MAX, 100);
        let previous = object.int(Prop::VALUE, min);
        let value = value.clamp(min.min(max), min.max(max));
        if previous == value {
            return false;
        }
        object.props.insert(Prop::VALUE, Value::Int(value));
        log::debug!("slider {name} value {previous}->{value}");
        if notify {
            self.fire(name, Prop::ONNOTIFY, vec![Value::Int(value)]);
        }
        true
    }

    pub(crate) fn set_slider_from_pointer(&mut self, name: &str, x: f64, y: f64) -> bool {
        let Some(object) = self.scene.get(name) else {
            return false;
        };
        let (left, top) = self.scene.abs_pos(object);
        let horizontal = object.int(Prop::ORIENTATION, 0) == 0;
        let (bar_part, thumb_part, button_part) = self.slider_parts(object, horizontal);
        let size = slider_size(
            object.int(Prop::WIDTH, 0),
            object.int(Prop::HEIGHT, 0),
            horizontal,
            bar_part,
            thumb_part,
            button_part,
        );
        let width = size.width.max(1) as f64;
        let height = size.height.max(1) as f64;
        let (length, pointer, thumb, button) = if horizontal {
            (
                width,
                x - left,
                thumb_part
                    .map(|part| part.cell_size.0 as f64)
                    .unwrap_or(0.0),
                button_part
                    .map(|part| part.cell_size.0 as f64)
                    .unwrap_or(0.0),
            )
        } else {
            (
                height,
                y - top,
                thumb_part
                    .map(|part| part.cell_size.1 as f64)
                    .unwrap_or(0.0),
                button_part
                    .map(|part| part.cell_size.1 as f64)
                    .unwrap_or(0.0),
            )
        };
        let inset = button + thumb / 2.0;
        let travel = (length - 2.0 * button - thumb).max(1.0);
        let fraction = ((pointer - inset) / travel).clamp(0.0, 1.0);
        let min = object.int(Prop::MIN, 0);
        let max = object.int(Prop::MAX, 100);
        let value = (min as f64 + (max as f64 - min as f64) * fraction).round() as i64;
        self.set_slider_value(name, value, true)
    }

    pub(crate) fn slider_asset_size(&self, source: &str) -> Option<(u32, u32)> {
        source_pixel_size(&self.assets, source)
    }

    fn slider_parts(
        &self,
        object: &crate::scene::Obj,
        horizontal: bool,
    ) -> (Option<SliderPart>, Option<SliderPart>, Option<SliderPart>) {
        (
            SliderPart::new(
                self.slider_asset_size(object.str(Prop::SOURCE).as_str()),
                2,
                horizontal,
            ),
            SliderPart::new(
                self.slider_asset_size(object.str(Prop::THUMBSOURCE).as_str()),
                3,
                horizontal,
            ),
            SliderPart::new(
                self.slider_asset_size(object.str(Prop::BUTTONSOURCE).as_str()),
                6,
                horizontal,
            ),
        )
    }

    pub(crate) fn slider_bounds(&self, object: &crate::scene::Obj) -> (f64, f64, f64, f64) {
        let (left, top) = self.scene.abs_pos(object);
        let horizontal = object.int(Prop::ORIENTATION, 0) == 0;
        let (bar, thumb, buttons) = self.slider_parts(object, horizontal);
        let size = slider_size(
            object.int(Prop::WIDTH, 0),
            object.int(Prop::HEIGHT, 0),
            horizontal,
            bar,
            thumb,
            buttons,
        );
        (left, top, size.width as f64, size.height as f64)
    }

    pub(crate) fn begin_slider_drag(&mut self, x: f64, y: f64) -> bool {
        let name = self
            .scene
            .draw_order()
            .into_iter()
            .rev()
            .find(|object| {
                if object.kind != Kind::Slider || !self.scene.effective_visible(object) {
                    return false;
                }
                let (left, top, width, height) = self.slider_bounds(object);
                width > 0.0
                    && height > 0.0
                    && x >= left
                    && y >= top
                    && x < left + width
                    && y < top + height
            })
            .map(|object| object.name.clone());
        let Some(name) = name else {
            return false;
        };
        log::debug!("slider pointer down {name} at ({x:.1},{y:.1})");
        if !self.active_component.is_empty() {
            let old = std::mem::take(&mut self.active_component);
            self.fire(&old, Prop::ONFOCUSOUT, vec![]);
        }
        self.slider_focus = Some(name.clone());
        let Some(object) = self.scene.get(&name) else {
            return false;
        };
        let (left, top) = self.scene.abs_pos(object);
        let horizontal = object.int(Prop::ORIENTATION, 0) == 0;
        let (bar_part, thumb_part, button_part) = self.slider_parts(object, horizontal);
        let size = slider_size(
            object.int(Prop::WIDTH, 0),
            object.int(Prop::HEIGHT, 0),
            horizontal,
            bar_part,
            thumb_part,
            button_part,
        );
        let (length, pointer, button_size) = if horizontal {
            (
                size.width.max(1) as f64,
                x - left,
                button_part
                    .map(|part| part.cell_size.0 as f64)
                    .unwrap_or(0.0),
            )
        } else {
            (
                size.height.max(1) as f64,
                y - top,
                button_part
                    .map(|part| part.cell_size.1 as f64)
                    .unwrap_or(0.0),
            )
        };
        if button_size > 0.0 && pointer < button_size {
            let step = object.int(Prop::SMALLCHANGE, 1).max(1);
            self.set_slider_value(&name, object.int(Prop::VALUE, 0).saturating_sub(step), true);
            return true;
        }
        if button_size > 0.0 && pointer >= length - button_size {
            let step = object.int(Prop::SMALLCHANGE, 1).max(1);
            self.set_slider_value(&name, object.int(Prop::VALUE, 0).saturating_add(step), true);
            return true;
        }
        self.slider_drag = Some(name.clone());
        self.set_slider_from_pointer(&name, x, y);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::super::Vm;
    use crate::archive::GameData;
    use crate::assets::Assets;
    use crate::lsb::expr::Value;
    use crate::lsb::names::Prop;
    use crate::scene::{Kind, Motion};
    use crate::vm::Options;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[test]
    fn slider_pointer_drag_and_keyboard_update_script_value() {
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
            "slider",
            Kind::Slider,
            HashMap::from([
                (Prop::LEFT, Value::Int(10)),
                (Prop::TOP, Value::Int(20)),
                (Prop::WIDTH, Value::Int(100)),
                (Prop::HEIGHT, Value::Int(10)),
                (Prop::MIN, Value::Int(0)),
                (Prop::MAX, Value::Int(10)),
                (Prop::VALUE, Value::Int(0)),
                (Prop::SMALLCHANGE, Value::Int(1)),
            ]),
        );
        vm.scene.create(
            "edit",
            Kind::Edit,
            HashMap::from([
                (Prop::LEFT, Value::Int(200)),
                (Prop::TOP, Value::Int(20)),
                (Prop::WIDTH, Value::Int(100)),
                (Prop::HEIGHT, Value::Int(24)),
                (Prop::ONFOCUSOUT, Value::Str("ゲームメイン.lsb".into())),
            ]),
        );
        vm.scene.add_motion(Motion {
            obj: "slider".into(),
            prop: Prop::LEFT,
            from: 10.0,
            to: 40.0,
            start: 0.0,
            dur: 100.0,
            soft: false,
        });
        vm.scene.update(50.0);
        assert!(vm.focus_edit_at(210.0, 25.0));

        vm.click_at(100.0, 25.0, false);
        assert_eq!(vm.get_prop("slider", Prop::VALUE), Value::Int(8));
        assert!(vm.active_edit_name().is_none());
        assert!(
            vm.queue
                .iter()
                .any(|event| event.obj == "edit" && event.prop == Prop::ONFOCUSOUT)
        );
        vm.mouse_move(125.0, 25.0);
        assert_eq!(vm.get_prop("slider", Prop::VALUE), Value::Int(10));
        assert!(vm.slider_keydown(37));
        assert_eq!(vm.get_prop("slider", Prop::VALUE), Value::Int(9));
        vm.end_slider_drag();
    }
}
