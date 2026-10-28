use super::Vm;
use crate::lsb::expr::eval;
use crate::lsb::names::Prop;
use crate::lsb::{CmdType, Command};

impl Vm {
    /// SaveCabinet / LoadCabinet: guarda o restaura objetos de la escena.
    pub(crate) fn step_cabinet(&mut self, kind: CmdType, c: &Command) -> String {
        let props = self.props_of(c);
        let cab = props
            .get(&Prop::NAME)
            .map(|v| v.as_str())
            .unwrap_or_default();
        let act = self.eval_bool(c.expr("Act"), false);
        let targets: Vec<String> = c
            .exprs("Targets")
            .to_vec()
            .iter()
            .map(|e| eval(e, self).as_str())
            .filter(|s| !s.is_empty())
            .collect();
        if kind == CmdType::SaveCabinet {
            // Act=false: guarda los indicados; Act=true: todos menos los indicados
            let sel: Vec<String> = if act {
                self.scene
                    .names()
                    .into_iter()
                    .filter(|n| !targets.contains(n))
                    .collect()
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
            let detail = format!("{cab:?} saved {}", saved.len());
            self.cabinets.insert(cab, saved);
            detail
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
            format!("{cab:?} restored {n}")
        }
    }

    /// GameSave: escribe el slot con la página, etiqueta y caption del comando.
    pub(crate) fn step_game_save(&mut self, c: &Command) -> String {
        let slot = self.eval(c.expr("No")).as_int();
        let page = c.str("Page").unwrap_or_default();
        let label = c.u32("Label").unwrap_or(0);
        let caption = self.eval(c.expr("Caption")).as_str();
        let detail = format!("slot={slot} page={page:?} label={label} caption={caption:?}");
        if let Err(e) =
            crate::save::save_game(&self.save_dir, slot, &page, label, &caption, &self.globals)
        {
            log::error!("GameSave error: {e}");
        }
        detail
    }

    /// GameLoad: `true` si cargó y `step` debe retornar (el fallo sigue el flujo normal).
    pub(crate) fn step_game_load(&mut self, c: &Command, detail: &mut String) -> bool {
        let slot = self.eval(c.expr("No")).as_int();
        *detail = format!("slot={slot}");
        match crate::save::load_game(&self.save_dir, slot) {
            Ok(saved) => {
                self.globals.extend(saved.globals);
                self.trace(c, detail.clone());
                if self.goto_script(&saved.page, saved.label, false, String::new(), vec![]) {
                    self.yielded = true;
                }
                true
            }
            Err(e) => {
                log::error!("GameLoad error: {e}");
                false
            }
        }
    }
}
