use super::{State, Vm, variable_scope_is_persistent};
use crate::lsb::expr::{Value, eval};
use crate::lsb::{CmdType, Command, Lsb, Param};

impl Vm {
    /// Resuelve un destino de bucle: índice de comando (como en pylivemaker) y,
    /// si no cuadra con el tipo esperado, se intenta como LineNo.
    /// Destino de salida de un While/Break: pylivemaker guarda en End el índice del comando
    /// siguiente al WhileLoop (no el WhileLoop). Se aceptan ambas formas y, si no, se busca
    /// estructuralmente el WhileLoop cuyo Start apunta a este While.
    fn loop_exit(script: &Lsb, v: u32, while_pc: Option<usize>) -> usize {
        let i = v as usize;
        let is_wl = |k: usize| {
            script
                .commands
                .get(k)
                .is_some_and(|c| CmdType::from(c.kind) == CmdType::WhileLoop)
        };
        if is_wl(i) {
            return i + 1;
        }
        if i > 0 && is_wl(i - 1) {
            return i;
        }
        if let Some(w) = while_pc {
            if let Some(k) = (w + 1..script.commands.len()).find(|&k| {
                is_wl(k)
                    && script.commands[k]
                        .u32("Start")
                        .map(|s| Self::loop_target(script, s, CmdType::While))
                        == Some(w)
            }) {
                return k + 1;
            }
        }
        Self::loop_target(script, v, CmdType::WhileLoop) + 1
    }

    fn loop_target(script: &Lsb, v: u32, want: CmdType) -> usize {
        let i = v as usize;
        if script
            .commands
            .get(i)
            .is_some_and(|c| CmdType::from(c.kind) == want)
        {
            return i;
        }
        script.index_of_line(v).unwrap_or(i)
    }

    /// --trace-conds: imprime la condición con los valores de sus variables, solo en scripts
    /// de escenario (fuera de ノベルシステム\) y las 2 primeras veces por (posición, resultado).
    /// Cadenas como <N chars> salvo --show-text.
    fn cond_trace(&mut self, s: &Lsb, pc: usize, c: &Command, ok: bool) {
        let dbg = self
            .cond_filter
            .as_ref()
            .is_some_and(|f| s.name.contains(f.as_str()));
        if !dbg
            && (!self.opts.trace || !self.opts.trace_conds || s.name.starts_with("ノベルシステム"))
        {
            return;
        }
        let k = (s as *const Lsb as usize, pc, ok);
        let n = self.cond_seen.entry(k).or_default();
        if *n >= 2 {
            return;
        }
        *n += 1;
        let Some(e) = c.expr("Calc") else { return };
        let mut vars = vec![];
        for o in &e.entries {
            for p in &o.operands {
                if let Param::Var(v) = p {
                    if !v.starts_with("____")
                        && !vars.iter().any(|(x, _): &(String, String)| x == v)
                    {
                        let val = self
                            .lookup(v)
                            .cloned()
                            .or_else(|| self.system_var(v))
                            .unwrap_or(Value::Empty);
                        let shown = match &val {
                            Value::Str(t) if !self.opts.show_text => {
                                format!("<{} chars>", t.chars().count())
                            }
                            Value::Array(a) => format!("[array {}]", a.len()),
                            Value::Empty => "(undef)".into(),
                            v => format!("{v:?}"),
                        };
                        vars.push((v.clone(), shown));
                    }
                }
            }
        }
        let line = format!(
            "[{:>8.0}ms] cond {}:{} {} {} -> {ok} {{{}}}",
            self.now,
            s.name,
            pc,
            c.name(),
            e.render(!self.opts.show_text),
            vars.iter()
                .map(|(a, b)| format!("{a}={b}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        if dbg {
            log::debug!("{line}");
        } else {
            println!("{line}");
        }
    }

    /// If falso: busca la siguiente rama Elseif/Else del mismo nivel, o el final del bloque.
    fn next_branch(&mut self, s: &Lsb, pc: usize) -> usize {
        let ind = s.commands[pc].indent;
        let mut j = pc + 1;
        while j < s.commands.len() {
            let c = &s.commands[j];
            if c.indent <= ind {
                if c.indent == ind {
                    match CmdType::from(c.kind) {
                        CmdType::Elseif => {
                            if self.opts.trace {
                                *self.stats.entry("Elseif").or_default() += 1;
                            }
                            let ok = self.eval_bool(c.expr("Calc"), false);
                            let c2 = c.clone();
                            self.cond_trace(s, j, &c2, ok);
                            if ok {
                                return j + 1;
                            }
                            j += 1;
                            continue;
                        }
                        CmdType::Else => return j + 1,
                        _ => {}
                    }
                }
                return j;
            }
            j += 1;
        }
        j
    }

    /// Salta el resto de una cadena If/Elseif/Else ya resuelta.
    fn skip_chain(&self, s: &Lsb, pc: usize) -> usize {
        let ind = s.commands[pc].indent;
        let mut j = pc + 1;
        while j < s.commands.len() {
            let c = &s.commands[j];
            if c.indent < ind
                || (c.indent == ind
                    && !matches!(CmdType::from(c.kind), CmdType::Elseif | CmdType::Else))
            {
                return j;
            }
            j += 1;
        }
        j
    }

    pub(crate) fn step_calc(&mut self, c: &Command, show: bool) -> String {
        let detail = if self.opts.trace {
            c.expr("Calc").map(|e| e.render(!show)).unwrap_or_default()
        } else {
            String::new()
        };
        self.eval(c.expr("Calc"));
        detail
    }

    pub(crate) fn step_if(
        &mut self,
        script: &Lsb,
        pc: usize,
        c: &Command,
        mut next: usize,
        detail: &mut String,
    ) -> usize {
        let ok = self.eval_bool(c.expr("Calc"), false);
        *detail = format!("-> {ok}");
        self.cond_trace(script, pc, c, ok);
        if !ok {
            next = self.next_branch(script, pc);
        }
        next
    }

    pub(crate) fn step_elseif_else(&self, script: &Lsb, pc: usize) -> usize {
        // Llegar aquí en flujo normal = la rama anterior se ejecutó: saltar el resto.
        self.skip_chain(script, pc)
    }

    pub(crate) fn step_label_comment(&self, kind: CmdType, c: &Command) -> String {
        if self.opts.trace && kind == CmdType::Label {
            c.str("Name").unwrap_or("").to_string()
        } else {
            String::new()
        }
    }

    /// `true` si `step` debe retornar de inmediato.
    pub(crate) fn step_jump(
        &mut self,
        script: &Lsb,
        pc: usize,
        c: &Command,
        next: usize,
        detail: &mut String,
    ) -> bool {
        if self.eval_bool(c.expr("Calc"), true) {
            let p = c.page().cloned().unwrap_or_default();
            *detail = format!("-> {}:{}", p.page, p.label);
            let same_script = p.page.is_empty()
                || crate::game::resolve_lsb(self.assets.data(), &p.page).as_deref()
                    == Some(script.name.as_str());
            let yields_frame = same_script && (p.label as usize) <= pc;
            if self.opts.trace {
                *self
                    .edges
                    .entry(format!(
                        "Jump {}:{} -> {}:{}",
                        script.name, pc, p.page, p.label
                    ))
                    .or_default() += 1;
            }
            self.trace(c, detail.clone());
            if self.goto_script(&p.page, p.label, false, String::new(), vec![]) {
                if yields_frame {
                    self.yielded = true;
                }
            } else if let Some(f) = self.frames.last_mut() {
                f.pc = next;
            }
            return true;
        }
        false
    }

    /// `true` si `step` debe retornar de inmediato.
    pub(crate) fn step_call(
        &mut self,
        script: &Lsb,
        pc: usize,
        c: &Command,
        next: usize,
        detail: &mut String,
    ) -> bool {
        if self.eval_bool(c.expr("Calc"), true) {
            let p = c.page().cloned().unwrap_or_default();
            let params: Vec<Value> = c
                .exprs("Params")
                .to_vec()
                .iter()
                .map(|e| eval(e, self))
                .collect();
            *detail = format!("-> {}:{} ({} params)", p.page, p.label, params.len());
            if self.opts.trace {
                *self
                    .edges
                    .entry(format!(
                        "Call {}:{} -> {}:{}",
                        script.name, pc, p.page, p.label
                    ))
                    .or_default() += 1;
            }
            self.trace(c, detail.clone());
            if let Some(f) = self.frames.last_mut() {
                f.pc = next;
            }
            if self.frames.len() > 256 {
                self.warn("stack", "call stack overflow (>256), ignoring Call");
                return true;
            }
            self.goto_script(
                &p.page,
                p.label,
                true,
                c.str("Result").unwrap_or("").to_string(),
                params,
            );
            return true;
        }
        false
    }

    /// `true` si `step` debe retornar de inmediato.
    pub(crate) fn step_exit(&mut self, c: &Command, detail: &mut String) -> bool {
        // Exit es condicional (pylivemaker: "Return if Calc evaluates to True");
        // Calc vacío = salida incondicional.
        let v = self.eval(c.expr("Calc"));
        let go = matches!(v, Value::Empty) || v.truthy();
        if go {
            self.trace(c, String::new());
            self.do_return(v);
            return true;
        }
        *detail = "(not taken)".into();
        false
    }

    /// `true` si `step` debe retornar de inmediato.
    pub(crate) fn step_terminate(&mut self, c: &Command) -> bool {
        self.trace(c, String::new());
        self.state = State::Ended;
        true
    }

    /// `true` si el PC debe repetir este Wait.
    ///
    /// Tiempo > 0: espera esos milisegundos. Tiempo 0 con condición literal
    /// (`Wait 1`, el relleno que el compilador deja tras un Jump): cede un frame
    /// y sigue. Tiempo 0 con otra condición: el novel system se queda mientras
    /// siga siendo verdadera (galería, modo CG, fin de mensaje). El manual no
    /// documenta el comando.
    pub(crate) fn step_wait(&mut self, c: &Command, detail: &mut String) -> bool {
        let calc = c.expr("Calc");
        if !self.eval_bool(calc, true) {
            return false;
        }
        let t = self.eval(c.expr("Time")).as_f64();
        let skippable = self.eval_bool(c.expr("StopEvent"), true);
        *detail = format!("{t}ms");
        if t > 0.0 {
            self.state = State::WaitUntil {
                until: self.now + t,
                skippable,
            };
            return false;
        }
        self.yielded = true;
        calc.is_some_and(|expr| !expr.is_empty() && expr.literal().is_none())
    }

    pub(crate) fn step_var_new(&mut self, c: &Command, detail: &mut String) {
        let name = c.str("Name").unwrap_or("").to_string();
        let ty = c.u32("Type").unwrap_or(0) as u8;
        let scope = c.u32("Scope").unwrap_or(0);
        let mut v = self.eval(c.expr("InitVal"));
        if v == Value::Empty {
            v = Value::default_for(ty);
        }
        *detail = format!("{name} scope={scope}");
        if !variable_scope_is_persistent(scope) {
            if let Some(f) = self.frames.last_mut() {
                f.locals.insert(name, v);
            }
        } else {
            self.globals.insert(name, v);
        }
    }

    pub(crate) fn step_var_del(&mut self, c: &Command, detail: &mut String) {
        let name = c.str("Name").unwrap_or("");
        *detail = name.to_string();
        if let Some(frame) = self
            .frames
            .iter_mut()
            .rev()
            .find(|frame| frame.locals.contains_key(name))
        {
            frame.locals.remove(name);
        } else {
            self.globals.remove(name);
        }
    }

    pub(crate) fn step_while(
        &mut self,
        script: &Lsb,
        pc: usize,
        c: &Command,
        mut next: usize,
        detail: &mut String,
    ) -> usize {
        let ok = self.eval_bool(c.expr("Calc"), true);
        self.cond_trace(script, pc, c, ok);
        if !ok {
            next = Self::loop_exit(script, c.u32("End").unwrap_or(0), Some(pc));
        }
        *detail = format!("-> {ok}");
        next
    }

    pub(crate) fn step_while_init(&mut self, c: &Command) {
        self.eval(c.expr("Calc"));
    }

    pub(crate) fn step_while_loop(&mut self, script: &Lsb, c: &Command) -> usize {
        self.eval(c.expr("Calc"));
        let next = Self::loop_target(script, c.u32("Start").unwrap_or(0), CmdType::While);
        // LiveMaker es cooperativo: cada vuelta cede el frame (eventos, texto, timers)
        self.yielded = true;
        next
    }

    pub(crate) fn step_break(&mut self, script: &Lsb, c: &Command, next: usize) -> usize {
        if self.eval_bool(c.expr("Calc"), true) {
            Self::loop_exit(script, c.u32("End").unwrap_or(0), None)
        } else {
            next
        }
    }

    pub(crate) fn step_continue(&mut self, script: &Lsb, c: &Command, next: usize) -> usize {
        if self.eval_bool(c.expr("Calc"), true) {
            Self::loop_target(script, c.u32("Start").unwrap_or(0), CmdType::WhileLoop)
        } else {
            next
        }
    }
}
