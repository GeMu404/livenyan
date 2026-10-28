use super::{HistoryEntry, State, Vm};
use crate::lsb::Command;
use crate::lsb::expr::{Value, eval};
use crate::lsb::names::Prop;
use crate::scene::{Motion, TextItem};

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
        if let Some(translated) = self.translations.get(&translation_key) {
            if !tw.replace_translation_text(translated) {
                log::debug!(
                    "skipping translation with missing or invalid runtime placeholders: {translation_key}"
                );
            }
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

    /// Flip: transición sobre Targets durante Time (espera saltable).
    ///
    /// Wipe (`WP_*`): **fuente primaria** `graphic.html`.
    /// - `0` NONE, `1` FADE; `12`–`14` WHITE/BLACK/FLASH → overlay (I7).
    /// - Resto → fundido ALPHA (**interpretación I6**, sin `unimpl`).
    ///
    /// Act: dirección de fundido hoy; manual = capas (**no confirmado**).
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
        // FL_OUT=0 → fundido de salida; FL_STAY=1 → entrada; Reverse invierte.
        let fade_out = (act == 0) != reverse;
        let label = super::wipe::wipe_label(wipe);
        match wipe {
            0 => {}
            1 => self.apply_flip_fade(&targets, fade_out, t),
            super::wipe::WP_WHITE => {
                self.wipe_flash = Some(super::wipe::WipeFlash::white_black(self.now, t, true));
                self.apply_flip_fade(&targets, fade_out, t);
            }
            super::wipe::WP_BLACK => {
                self.wipe_flash = Some(super::wipe::WipeFlash::white_black(self.now, t, false));
                self.apply_flip_fade(&targets, fade_out, t);
            }
            super::wipe::WP_FLASH => {
                self.wipe_flash = Some(super::wipe::WipeFlash::flash(self.now, t));
                self.apply_flip_fade(&targets, fade_out, t);
            }
            _ => {
                // I6: wipe geométrico → fade (sin spam unimpl).
                log::debug!("Flip wipe {label}: using fade interpretation (I6)");
                self.apply_flip_fade(&targets, fade_out, t);
            }
        }
        let defer_delete = del && t > 0.0 && !self.skip;
        if del && !defer_delete {
            for n in &targets {
                self.scrap_component(n);
            }
        }
        if defer_delete {
            self.flip_pending_delete = targets.clone();
        }
        let detail = format!(
            "wipe={label} {t}ms act={act} reverse={reverse} targets={} delete={del}",
            targets.len()
        );
        if t > 0.0 && !self.skip {
            self.state = State::WaitUntil {
                until: self.now + t,
                skippable: true,
            };
        }
        detail
    }

    /// Fundido de `ALPHA` sobre Targets (WP_FADE y aproximación de otros wipes).
    fn apply_flip_fade(&mut self, targets: &[String], fade_out: bool, t: f64) {
        let animate = t > 0.0 && !self.skip;
        for name in targets {
            let Some(cur) = self
                .scene
                .get(name)
                .map(|obj| obj.int(Prop::ALPHA, 255) as f64)
            else {
                continue;
            };
            if fade_out {
                if animate {
                    self.scene.add_motion(Motion {
                        obj: name.clone(),
                        prop: Prop::ALPHA,
                        from: cur,
                        to: 0.0,
                        start: self.now,
                        dur: t,
                    });
                } else {
                    self.scene.set(name, Prop::ALPHA, Value::Int(0));
                }
            } else {
                let to = if cur > 0.0 { cur } else { 255.0 };
                if animate {
                    self.scene.set(name, Prop::ALPHA, Value::Int(0));
                    self.scene.add_motion(Motion {
                        obj: name.clone(),
                        prop: Prop::ALPHA,
                        from: 0.0,
                        to,
                        start: self.now,
                        dur: t,
                    });
                } else {
                    self.scene
                        .set(name, Prop::ALPHA, Value::Int(to.round() as i64));
                }
            }
        }
    }

    /// Tras WaitUntil de un Flip: borra Targets pendientes (Delete diferido).
    pub(crate) fn finish_flip_wait(&mut self) {
        let pending = std::mem::take(&mut self.flip_pending_delete);
        for n in pending {
            self.scrap_component(&n);
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
