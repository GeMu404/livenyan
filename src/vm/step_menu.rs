use super::{State, Vm};
use crate::lsb::Command;
use crate::lsb::expr::Value;
use crate::lsb::names::Prop;
use crate::scene::Kind;
use std::collections::HashMap;

pub(crate) fn translate_menu_choices(
    source: &[String],
    translations: &HashMap<String, String>,
) -> Vec<String> {
    source
        .iter()
        .map(|choice| {
            translations
                .get(&crate::text_export::menu_item_id(choice))
                .cloned()
                .unwrap_or_else(|| choice.clone())
        })
        .collect()
}

impl Vm {
    /// MenuClose: cierra el menú modal y dispara ONCLOSING/ONCLOSED.
    pub(crate) fn step_menu_close(&mut self, c: &Command) -> String {
        let target = self.eval(c.expr("Target")).as_str();
        let detail = format!("target={target:?}");
        if matches!(&self.state, State::Menu { name, .. } if name == &target) {
            self.state = State::Running;
        }
        if !target.is_empty() {
            self.scene.set(&target, Prop::REVERSE, Value::Bool(true));
            self.scene.set(&target, Prop::VISIBLE, Value::Bool(false));
            if !self.fire(&target, Prop::ONCLOSING, vec![]) {
                self.fire(&target, Prop::ONCLOSED, vec![]);
                if self.component_auto_scrap(&target) {
                    self.scrap_component(&target);
                }
            }
        }
        detail
    }

    /// Menu: crea el menú, traduce etiquetas visibles y entra en estado modal.
    ///
    /// Opciones: `PR_TEXT` (p. ej. `@ParamStr[0]` del novel system); si vacío,
    /// array `_tmp`. Al elegir, `choose` asigna a `選択値` y dispara ONSELECT con
    /// `[texto, índice]` (el handler `選択.lsb` vuelve a escribir `選択値`/`選択番号`).
    pub(crate) fn step_menu(&mut self, c: &Command) -> String {
        let mut props = self.props_of(c);
        let name = props
            .get(&Prop::NAME)
            .map(|v| v.as_str())
            .unwrap_or_default();
        let mut choices: Vec<String> = props
            .get(&Prop::TEXT)
            .map(|v| match v {
                Value::Array(a) => a.iter().map(|x| x.as_str()).collect(),
                v => v
                    .as_str()
                    .split(['\n'])
                    .map(|s| s.trim_end_matches('\r').to_string())
                    .filter(|s| !s.is_empty())
                    .collect(),
            })
            .unwrap_or_default();
        if choices.is_empty()
            && let Some(Value::Array(a)) = self.lookup("_tmp")
        {
            choices = a
                .iter()
                .map(|x| x.as_str())
                .filter(|s| !s.trim().is_empty())
                .collect();
        }
        let values = choices.clone();
        let translated_count = choices
            .iter()
            .filter(|choice| {
                self.translations
                    .contains_key(&crate::text_export::menu_item_id(choice))
            })
            .count();
        choices = translate_menu_choices(&choices, &self.translations);
        log::debug!(
            "Menu {name:?}: translated {translated_count}/{} visible option labels",
            choices.len()
        );
        let detail = format!("name={name:?} choices={}", choices.len());
        self.prevmenus.remove(&name);
        let font_height = props
            .get(&Prop::FONTHEIGHT)
            .map(Value::as_f64)
            .unwrap_or(24.0);
        let line_space = props
            .get(&Prop::LINESPACE)
            .map(Value::as_f64)
            .unwrap_or(0.0);
        let margin_y = props.get(&Prop::MARGINY).map(Value::as_int).unwrap_or(0);
        let visible_rows = crate::scene::menu_visible_range(
            choices.len(),
            props.get(&Prop::ROWCOUNT).map(Value::as_int).unwrap_or(0),
            props.get(&Prop::TOPINDEX).map(Value::as_int).unwrap_or(0),
        )
        .len();
        self.set_menu_intrinsic_width(&mut props, &choices, visible_rows);
        let (_, menu_height) = crate::scene::native_menu_size(
            visible_rows,
            self.opts.screen.0,
            font_height,
            line_space,
            margin_y,
        );
        props.entry(Prop::HEIGHT).or_insert(Value::Int(menu_height));
        props
            .entry(Prop::ORIGHEIGHT)
            .or_insert(Value::Int(menu_height));
        self.scene.create(&name, Kind::Menu, props);
        self.fire(&name, Prop::ONOPENING, vec![]);
        if !choices.is_empty() {
            self.state = State::Menu {
                name,
                choices,
                values,
                var: "選択値".into(),
            };
        }
        detail
    }
}
