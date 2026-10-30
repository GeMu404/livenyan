use super::{HistoryEntry, State, Vm};
use crate::lsb::Command;
use crate::lsb::expr::{Value, eval};
use crate::lsb::names::Prop;
use crate::scene::TextItem;

fn message_lineage(scene: &crate::scene::Scene) -> std::collections::HashSet<String> {
    let mut keep = std::collections::HashSet::new();
    for object in scene.live() {
        if object.kind != crate::scene::Kind::Message {
            continue;
        }
        let mut current = object.name.clone();
        for _ in 0..64 {
            if !keep.insert(current.clone()) {
                break;
            }
            let Some(parent) = scene
                .get(&current)
                .map(|object| object.str(Prop::PARENT))
                .filter(|parent| !parent.is_empty())
            else {
                break;
            };
            current = parent;
        }
    }
    keep
}

fn flip_specified(scene: &crate::scene::Scene, name: &str, targets: &[String]) -> bool {
    let mut current = name.to_string();
    for _ in 0..64 {
        if targets.iter().any(|target| target == &current) {
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

fn redact(s: &str, show: bool) -> String {
    if show {
        format!("{s:?}")
    } else {
        format!("<{} chars>", s.chars().count())
    }
}

impl Vm {
    /// TextIns: traduce, avanza el pc y reproduce. `step` retorna de inmediato.
    pub(crate) fn step_text_ins(&mut self, pc: usize, c: &Command, next: usize, show: bool) {
        let target = self.eval(c.expr("Target")).as_str();
        let mut tw = c.text().cloned().unwrap_or_default();
        let frame = self
            .frames
            .last()
            .expect("TextIns has an active script frame");
        let translation_key = format!(
            "{}:line={}:command={pc}:field=Text",
            frame.script.name, c.line_no
        );
        if let Some(translated) = self.translations.get(&translation_key)
            && !tw.replace_translation_text(translated)
        {
            log::debug!(
                "skipping translation with missing or invalid runtime placeholders: {translation_key}"
            );
        }
        let mut detail = format!("target={target:?} chars={}", tw.char_count());
        if show {
            detail += &format!(" text={}", redact(&tw.plain_text(), true));
        }
        self.trace(c, detail);
        if let Some(f) = self.frames.last_mut() {
            f.pc = next;
        }
        let wait = self.eval_bool(c.expr("Wait"), true);
        let keep_history = self.eval_bool(c.expr("Hist"), false);
        let read_script = self
            .frames
            .last()
            .map(|frame| frame.script.name.clone())
            .unwrap_or_default();
        let already_read = self.note_text_read(&read_script, pc);
        self.run_text(&target, &tw, wait, keep_history, already_read);
    }

    /// TextClr: vacía el texto mostrado del objeto.
    pub(crate) fn step_text_clr(&mut self, c: &Command) {
        let t = self.eval(c.expr("Target")).as_str();
        if let Some(o) = self.scene.get_mut(&t) {
            o.text.clear();
            o.shown = 0;
            o.text_rev += 1;
        }
    }

    /// Flip. Una sola pasada de dibujo: no hay framebuffer de atrás.
    ///
    /// Con targets, fundido (y `Delete`) solo de esos nombres. El manual habla
    /// de capas (`graphic.html`); eso, aplicado a todo el árbol, apaga el cuadro
    /// de mensaje y la pantalla recién creada.
    ///
    /// **interpretación livenyan** con targets vacíos y `FL_STAY`: lo creado en
    /// este frame se queda; lo anterior se archiva y deja de dibujarse. Si no
    /// se creó nada y hay una placa archivada, esa vuelve y `Delete` tira lo
    /// que estaba a la vista. La primera placa (el título) se conserva. Los
    /// cuadros de mensaje y sus padres no entran en la placa.
    ///
    /// Wipe (`WP_*`): **fuente primaria** `graphic.html`.
    /// - `0` NONE, `1` FADE; `12`–`14` WHITE/BLACK/FLASH → overlay (I7).
    /// - Resto → fundido ALPHA de lo que se va (**interpretación I6**).
    pub(crate) fn step_flip(&mut self, c: &Command) -> String {
        let wipe = self.eval(c.expr("Wipe")).as_int();
        let t = self.eval(c.expr("Time")).as_f64().max(0.0);
        let reverse = self.eval_bool(c.expr("Reverse"), false);
        let act = self.eval(c.expr("Act")).as_int();
        let del = self.eval_bool(c.expr("Delete"), false);
        let _params: Vec<Value> = c.exprs("Param").iter().map(|e| eval(e, self)).collect();
        let _source = self.eval(c.expr("Source")).as_str();
        let _stop_event = self.eval_bool(c.expr("StopEvent"), false);
        let _difference_only = self.eval_bool(c.expr("DifferenceOnly"), false);
        let targets: Vec<String> = c
            .exprs("Targets")
            .iter()
            .flat_map(|e| {
                eval(e, self)
                    .as_str()
                    .split(['\r', '\n'])
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .collect();
        let stay = (act == 1) != reverse;
        let plan = if targets.is_empty() {
            self.plan_empty_flip(stay, del)
        } else {
            let fade: Vec<String> = self
                .scene
                .names()
                .into_iter()
                .filter(|name| flip_specified(&self.scene, name, &targets))
                .collect();
            super::FlipPlan {
                park: Vec::new(),
                restore: Vec::new(),
                scrap: if del { targets.clone() } else { Vec::new() },
                fade,
                fade_in: Vec::new(),
                fade_out: (act == 0) != reverse,
                remember_base: false,
                base_born: None,
            }
        };
        let label = super::wipe::wipe_label(wipe);
        if wipe != 0 {
            match wipe {
                super::wipe::WP_WHITE => {
                    self.wipe_flash = Some(super::wipe::WipeFlash::white_black(self.now, t, true));
                }
                super::wipe::WP_BLACK => {
                    self.wipe_flash = Some(super::wipe::WipeFlash::white_black(self.now, t, false));
                }
                super::wipe::WP_FLASH => {
                    self.wipe_flash = Some(super::wipe::WipeFlash::flash(self.now, t));
                }
                1 => {}
                _ => log::debug!("Flip wipe {label}: using fade interpretation (I6)"),
            }
        }
        // El fundido es de las dos superficies. El Time es el del comando.
        // WHITE/BLACK/FLASH siguen en su overlay.
        let color_overlay = matches!(
            wipe,
            super::wipe::WP_WHITE | super::wipe::WP_BLACK | super::wipe::WP_FLASH
        );
        if t > 0.0 && !self.skip && wipe != 0 && !color_overlay {
            let (out, inn) = if plan.fade_out {
                (plan.fade.clone(), plan.fade_in.clone())
            } else {
                (Vec::new(), plan.fade.clone())
            };
            self.wipe_fade = Some(super::wipe::WipeFade {
                started_at: self.now,
                dur_ms: t,
                out: out.into_iter().collect(),
                inn: inn.into_iter().collect(),
            });
        } else {
            self.wipe_fade = None;
        }
        let defer = t > 0.0 && !self.skip;
        if defer {
            self.flip_plan = Some(plan);
            self.state = State::WaitUntil {
                until: self.now + t,
                skippable: true,
            };
        } else {
            self.flip_plan = None;
            self.apply_flip_plan(plan);
        }
        format!(
            "wipe={label} {t}ms act={act} reverse={reverse} targets={} delete={del}",
            targets.len()
        )
    }

    fn plan_empty_flip(&mut self, stay: bool, delete: bool) -> super::FlipPlan {
        if !stay {
            let restore: Vec<String> = self.plates.pop().unwrap_or_default().into_iter().collect();
            return super::FlipPlan {
                fade: restore.clone(),
                fade_in: Vec::new(),
                fade_out: false,
                restore,
                park: Vec::new(),
                scrap: Vec::new(),
                remember_base: false,
                base_born: None,
            };
        }
        let shown = self.plate_names();
        let frame = self.scene.cohort();
        let fresh: Vec<String> = shown
            .iter()
            .filter(|name| self.scene.born(name) == frame)
            .cloned()
            .collect();
        let stale: Vec<String> = shown
            .iter()
            .filter(|name| self.scene.born(name) != frame)
            .cloned()
            .collect();
        // Delete tira lo creado desde la primera placa (miniaturas incluidas,
        // aunque hayan quedado archivadas). La placa base vuelve si no hay
        // pantalla nueva en este frame.
        if delete && let Some(origin) = self.base_born {
            let mut scrap = Vec::new();
            let mut restore = Vec::new();
            for name in self.visual_names() {
                let born = self.scene.born(&name);
                if born >= origin && born != frame {
                    scrap.push(name);
                } else if fresh.is_empty() && born < origin {
                    restore.push(name);
                }
            }
            let park = if fresh.is_empty() {
                Vec::new()
            } else {
                stale
                    .into_iter()
                    .filter(|name| self.scene.born(name) < origin)
                    .collect()
            };
            let mut fade = scrap.clone();
            fade.extend(park.iter().cloned());
            let fade_in = if fresh.is_empty() {
                restore.clone()
            } else {
                fresh.clone()
            };
            return super::FlipPlan {
                fade,
                fade_in,
                fade_out: true,
                restore,
                park,
                scrap,
                remember_base: false,
                base_born: None,
            };
        }
        if fresh.is_empty() && !self.plates.is_empty() {
            let restore: Vec<String> = self.plates.pop().unwrap().into_iter().collect();
            let fade = if delete { shown.clone() } else { Vec::new() };
            return super::FlipPlan {
                fade_in: restore.clone(),
                fade,
                fade_out: true,
                restore,
                park: Vec::new(),
                scrap: if delete { shown } else { Vec::new() },
                remember_base: false,
                base_born: None,
            };
        }
        let (park, drop) = match &self.base_plate {
            Some(base) => {
                let mut park = Vec::new();
                let mut drop = Vec::new();
                for name in stale {
                    if base.contains(&name) {
                        park.push(name);
                    } else {
                        drop.push(name);
                    }
                }
                (park, drop)
            }
            None => (stale, Vec::new()),
        };
        let mut fade = park.clone();
        if delete {
            fade.extend(drop.iter().cloned());
        }
        let remember_base = self.base_plate.is_none() && !park.is_empty();
        super::FlipPlan {
            remember_base,
            base_born: remember_base.then_some(frame),
            park,
            restore: Vec::new(),
            scrap: if delete { drop } else { Vec::new() },
            fade_in: fresh,
            fade,
            fade_out: true,
        }
    }

    fn visual_names(&self) -> Vec<String> {
        let keep = message_lineage(&self.scene);
        self.scene
            .live()
            .map(|object| object.name.clone())
            .filter(|name| !keep.contains(name))
            .collect()
    }

    fn plate_names(&self) -> Vec<String> {
        let keep = message_lineage(&self.scene);
        self.scene
            .names()
            .into_iter()
            .filter(|name| !keep.contains(name))
            .collect()
    }

    fn apply_flip_plan(&mut self, plan: super::FlipPlan) {
        if plan.remember_base {
            self.base_plate = Some(plan.park.iter().cloned().collect());
            self.base_born = plan.base_born;
        }
        if !plan.park.is_empty() {
            self.plates.push(plan.park.iter().cloned().collect());
            for name in &plan.park {
                self.scene.mark_back(name);
            }
        }
        for name in &plan.restore {
            self.scene.unmark_back(name);
        }
        // FL_OUT sin Delete: el objetivo queda en la superficie de atrás, no con ALPHA 0.
        if plan.fade_out && plan.scrap.is_empty() && plan.park.is_empty() && plan.fade_in.is_empty()
        {
            for name in &plan.fade {
                self.scene.mark_back(name);
            }
        }
        for name in &plan.scrap {
            self.scrap_component(name);
        }
    }

    /// Tras WaitUntil de un Flip: aplica el cambio de pantalla.
    pub(crate) fn finish_flip_wait(&mut self) {
        self.wipe_fade = None;
        if let Some(plan) = self.flip_plan.take() {
            self.apply_flip_plan(plan);
        }
    }

    /// DoEvent: despacha la cola y cede el turno.
    pub(crate) fn step_do_event(&mut self) {
        self.dispatch_events_from_do_event();
        self.yielded = true;
    }

    /// ClrHist: vacía el historial de texto.
    pub(crate) fn step_clr_hist(&mut self) {
        self.history.clear();
    }

    /// FormatHist: registra el nombre de formato y su objeto de mensaje.
    pub(crate) fn step_format_hist(&mut self, c: &Command) {
        let name = self.eval(c.expr("Name")).as_str();
        let target = c
            .expr("Target")
            .map(|expr| eval(expr, self).as_str())
            .filter(|target| !target.is_empty())
            .or_else(|| self.last_text_target.clone())
            .unwrap_or_default();
        self.history_formats.insert(name, target);
    }

    /// CallHist: vuelca entradas del historial en el objeto de mensaje.
    pub(crate) fn step_call_hist(&mut self, c: &Command) {
        let target = self.eval(c.expr("Target")).as_str();
        let index = self.eval(c.expr("Index")).as_int().max(0) as usize;
        let count = self.eval(c.expr("Count")).as_int();
        let cut_break = self.eval_bool(c.expr("CutBreak"), false);
        let format_name = c
            .expr("FormatName")
            .map(|expr| eval(expr, self).as_str())
            .unwrap_or_default();
        let entries: Vec<HistoryEntry> = self
            .history
            .iter()
            .skip(index)
            .take(count.max(0) as usize)
            .cloned()
            .collect();
        let mut text = Vec::new();
        let mut previous_page: Option<String> = None;
        for (entry_index, entry) in entries.into_iter().enumerate() {
            if entry_index > 0 {
                text.push(TextItem::Newline);
                if !cut_break
                    && previous_page
                        .as_deref()
                        .is_some_and(|page| page != entry.page)
                {
                    text.push(TextItem::Newline);
                }
            }
            text.extend(entry.text);
            previous_page = Some(entry.page);
        }
        if self.scene.get(&target).is_some() {
            if let Some(obj) = self.scene.get_mut(&target) {
                obj.text = text;
                obj.shown = obj.text.len();
                obj.text_rev += 1;
            }
        } else {
            self.warn(
                &format!("hist-target:{target}"),
                &format!("CallHist target message object {target:?} does not exist"),
            );
        }
        if !format_name.is_empty() && !self.history_formats.contains_key(&format_name) {
            self.warn(
                &format!("hist-format:{format_name}"),
                &format!("CallHist history format {format_name:?} is not registered"),
            );
        }
    }
}
