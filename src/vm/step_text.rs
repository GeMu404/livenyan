use super::{HistoryEntry, State, Vm};
use crate::lsb::Command;
use crate::lsb::expr::{Value, eval};
use crate::lsb::names::Prop;
use crate::scene::TextItem;

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
    /// **Interpretación livenyan** de superficies: los objetos nuevos que aún no
    /// se presentaron forman la superficie entrante. Los ya presentados pasan a
    /// ser salientes en el siguiente Flip. La cohorte avanza al completar el
    /// Flip, no en cada frame renderizado.
    /// Mensajes y sus padres no entran en la placa.
    ///
    /// Wipe (`WP_*`): **fuente primaria** `graphic.html`.
    /// - `0` NONE, `1` FADE; `2`/`3` BLINDH/BLINDV → franjas (`graphic.html`).
    /// - `12`–`14` WHITE/BLACK/FLASH → overlay (I7).
    /// - Resto → fundido ALPHA de lo que se va (**interpretación I6**).
    pub(crate) fn step_flip(&mut self, c: &Command) -> String {
        let wipe = self.eval(c.expr("Wipe")).as_int();
        let t = self.eval(c.expr("Time")).as_f64().max(0.0);
        let reverse = self.eval_bool(c.expr("Reverse"), false);
        let act = self.eval(c.expr("Act")).as_int();
        let del = self.eval_bool(c.expr("Delete"), false);
        let params: Vec<Value> = c.exprs("Param").iter().map(|e| eval(e, self)).collect();
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
        let stay = act == 1;
        let plan = self.plan_flip(stay, del, &targets);
        let label = super::wipe::wipe_label(wipe);
        log::info!(
            "Flip {label}: cohort={} stay={stay} targets={} delete={del} park={} restore={} scrap={} fade={} incoming={}",
            self.scene.cohort(),
            targets.len(),
            plan.park.len(),
            plan.restore.len(),
            plan.scrap.len(),
            plan.fade.len(),
            plan.fade_in.len()
        );
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
                1 | super::wipe::WP_BLINDH | super::wipe::WP_BLINDV => {}
                _ => log::debug!("Flip wipe {label}: using fade interpretation (I6)"),
            }
        }
        let kind = match wipe {
            super::wipe::WP_BLINDH | super::wipe::WP_BLINDV => {
                let span = params
                    .first()
                    .map(Value::as_int)
                    .unwrap_or(0)
                    .clamp(1, i64::from(i32::MAX)) as i32;
                let appear_ms = params.get(1).map(Value::as_f64).unwrap_or(0.0).max(0.0);
                super::wipe::WipeKind::Blind {
                    horizontal: wipe == super::wipe::WP_BLINDV,
                    span,
                    appear_ms,
                    reverse,
                }
            }
            _ => super::wipe::WipeKind::Fade,
        };
        // El fundido es de las dos superficies. El Time es el del comando.
        // WHITE/BLACK/FLASH siguen en su overlay.
        let color_overlay = matches!(
            wipe,
            super::wipe::WP_WHITE | super::wipe::WP_BLACK | super::wipe::WP_FLASH
        );
        if t > 0.0 && !self.skip && wipe != 0 && !color_overlay {
            self.wipe_fade = Some(super::wipe::WipeFade {
                started_at: self.now,
                dur_ms: t,
                out: plan.fade.iter().cloned().collect(),
                inn: plan.fade_in.iter().cloned().collect(),
                kind,
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
            self.advance_screen_cohort();
        }
        format!(
            "wipe={label} {t}ms act={act} reverse={reverse} targets={} delete={del}",
            targets.len()
        )
    }

    fn plan_flip(&mut self, stay: bool, delete: bool, targets: &[String]) -> super::FlipPlan {
        let shown = if targets.is_empty() {
            self.plate_names()
        } else {
            self.scene.names()
        };
        let frame = self.scene.cohort();
        let fresh: Vec<String> = if targets.is_empty() {
            let keep = self.scene.message_ui_names();
            self.scene
                .live()
                .filter(|object| {
                    self.scene.born(&object.name) == frame
                        && !self.scene.was_presented(&object.name)
                        && !keep.contains(&object.name)
                })
                .map(|object| object.name.clone())
                .collect()
        } else {
            shown
                .iter()
                .filter(|name| self.scene.born(name) == frame && !self.scene.was_presented(name))
                .cloned()
                .collect()
        };
        let mut restore: Vec<String> = if fresh.is_empty() {
            self.plates.pop().unwrap_or_default().into_iter().collect()
        } else {
            Vec::new()
        };
        let message_ui = self.scene.message_ui_names();
        restore.extend(
            message_ui
                .iter()
                .filter(|name| {
                    self.scene.on_back(name) && !flip_specified(&self.scene, name, targets)
                })
                .cloned(),
        );
        let park: Vec<String> = shown
            .into_iter()
            .filter(|name| self.scene.born(name) != frame || self.scene.was_presented(name))
            .filter(|name| {
                let selected = flip_specified(&self.scene, name, targets);
                let keep_message_ui = message_ui.contains(name) && !selected;
                !keep_message_ui && if stay { !selected } else { selected }
            })
            .collect();
        let scrap: Vec<String> = if delete {
            self.visual_names()
                .into_iter()
                .filter(|name| {
                    let keep_message_ui =
                        message_ui.contains(name) && !flip_specified(&self.scene, name, targets);
                    !flip_specified(&self.scene, name, &restore)
                        && !flip_specified(&self.scene, name, &fresh)
                        && !keep_message_ui
                        && (flip_specified(&self.scene, name, &park) || self.scene.on_back(name))
                })
                .collect()
        } else {
            Vec::new()
        };
        let mut fade = park.clone();
        fade.extend(scrap.iter().cloned());
        let fade_in = if fresh.is_empty() {
            restore.clone()
        } else {
            fresh
        };
        super::FlipPlan {
            park,
            restore,
            scrap,
            fade,
            fade_in,
        }
    }

    fn visual_names(&self) -> Vec<String> {
        self.scene
            .live()
            .map(|object| object.name.clone())
            .collect()
    }

    fn plate_names(&self) -> Vec<String> {
        let keep = self.scene.message_ui_names();
        self.scene
            .names()
            .into_iter()
            .filter(|name| !keep.contains(name))
            .collect()
    }

    fn apply_flip_plan(&mut self, plan: super::FlipPlan) {
        if !plan.park.is_empty() {
            self.plates.push(plan.park.iter().cloned().collect());
            for name in &plan.park {
                self.scene.mark_back(name);
            }
        }
        for name in &plan.restore {
            self.scene.unmark_back(name);
        }
        let scrap: std::collections::HashSet<String> = plan
            .scrap
            .iter()
            .filter(|name| self.scene.contains(name))
            .cloned()
            .collect();
        let detach: Vec<(String, f64, f64)> = self
            .scene
            .live()
            .filter(|object| {
                !scrap.contains(&object.name) && scrap.contains(&object.str(Prop::PARENT))
            })
            .map(|object| {
                let (x, y) = self.scene.abs_pos(object);
                (object.name.clone(), x, y)
            })
            .collect();
        for (name, x, y) in detach {
            self.scene
                .set(&name, Prop::PARENT, Value::Str(String::new()));
            self.scene
                .set(&name, Prop::LEFT, Value::Int(x.round() as i64));
            self.scene
                .set(&name, Prop::TOP, Value::Int(y.round() as i64));
        }
        let removed = self.scene.delete_names_exact(&plan.scrap);
        self.retire_objects(removed);
        self.scene.reset_presented();
        self.reapply_modal_menu();
        let live_names: std::collections::HashSet<String> = self
            .scene
            .live()
            .map(|object| object.name.clone())
            .collect();
        for plate in &mut self.plates {
            plate.retain(|name| live_names.contains(name));
        }
        self.plates.retain(|plate| !plate.is_empty());
    }

    fn advance_screen_cohort(&mut self) {
        let next = self.scene.cohort().saturating_add(1);
        self.scene.set_cohort(next);
    }

    /// Tras WaitUntil de un Flip: aplica el cambio de pantalla.
    pub(crate) fn finish_flip_wait(&mut self) {
        self.wipe_fade = None;
        if let Some(plan) = self.flip_plan.take() {
            self.apply_flip_plan(plan);
            self.advance_screen_cohort();
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
