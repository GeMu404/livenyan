use super::{Event, ROOT, Vm};
use crate::assets::IMAGE_EXTS;
use crate::lsb::expr::Value;
use crate::lsb::names::Prop;
use crate::lsb::{CmdType, Command, Lsb};
use crate::scene::{Kind, Motion, prop_of};

impl Vm {
    /// SetProp: asigna una propiedad de objeto (o de la capa raíz).
    ///
    /// Semántica LiveMaker: si el objeto no existe, no-op (sin error). No crear
    /// stubs: scripts como la barra de historial o botones opcionales SetProp
    /// aunque el juego nunca haya hecho GaugeNew/Button.
    pub(crate) fn step_set_prop(&mut self, script: &Lsb, pc: usize, c: &Command) -> String {
        let obj = self.eval(c.expr("ObjName")).as_str();
        let pv = self.eval(c.expr("ObjProp"));
        let v = self.eval(c.expr("Value"));
        let p = prop_of(&pv);
        let detail = format!("{obj:?}.{}", p.map(crate::lsb::prop_name).unwrap_or("?"));
        match p {
            Some(p) => {
                if crate::lsb::prop_name(p).starts_with("ON") {
                    log::debug!(
                        "event prop: SetProp {obj}.{} (id {p}) = {:?}",
                        crate::lsb::prop_name(p),
                        v.as_str()
                    );
                }
                if p == Prop::SOURCE
                    && let Some(n) = self.assets.data().resolve(&v.as_str(), IMAGE_EXTS)
                {
                    self.assets.request(&n);
                }
                if p == Prop::SOURCE && self.scene.get(&obj).is_some() {
                    let src = v.as_str();
                    self.watch_anim(&obj, &src);
                }
                let clear_text_pause = p == Prop::TEXTPAUSED && !v.truthy();
                if obj == ROOT {
                    log::debug!(
                        "SetProp on root layer: {} = {v:?}",
                        crate::lsb::prop_name(p)
                    );
                    self.root_props.insert(p, v);
                } else if !self.scene.set(&obj, p, v) && !obj.is_empty() {
                    // LiveMaker: SetProp sobre objeto inexistente es no-op (no error).
                    // Los scripts novel a menudo tocan controles opcionales (barra de
                    // historial, botones de avance, MesNew diferidos) sin Exists().
                    log::debug!(
                        "SetProp no-op on missing {obj:?} at {}:{pc} ({})",
                        script.name,
                        if self.graveyard.contains_key(&obj) {
                            "deleted earlier"
                        } else {
                            "never created"
                        }
                    );
                } else if p == Prop::ONPLAYEND {
                    self.object_defined(&obj);
                }
                // El ONIDLE del cuadro suelta el clic poniendo PR_TEXTPAUSED a falso
                // (アイドル時.lsb, auto / skip / clic).
                if clear_text_pause && let Some(play) = self.plays.get_mut(&obj) {
                    play.waiting_click = false;
                }
            }
            None => self.warn("setprop-bad", "SetProp with unknown property"),
        }
        detail
    }

    /// GetProp: lee una propiedad y la asigna a la variable indicada.
    pub(crate) fn step_get_prop(&mut self, c: &Command) -> String {
        let obj = self.eval(c.expr("ObjName")).as_str();
        let pv = self.eval(c.expr("ObjProp"));
        let var = c.str("VarName").unwrap_or("").to_string();
        let v = prop_of(&pv)
            .map(|p| self.get_prop(&obj, p))
            .unwrap_or(Value::Int(0));
        let detail = format!("{obj:?} -> {var}");
        self.assign(&var, v);
        detail
    }

    /// ObjDel: borra el objeto y su cascada, y limpia el estado asociado.
    pub(crate) fn step_obj_del(&mut self, script: &Lsb, pc: usize, c: &Command) -> String {
        let name = self.eval(c.expr("Name")).as_str();
        let removed = self.scene.delete_names(&name);
        if removed.is_empty() {
            log::debug!("ObjDel {:?}: no such object", name);
        }
        let mut gone = vec![];
        for o in removed {
            let parent = o.str(Prop::PARENT);
            log::debug!(
                "ObjDel {name:?} at {}:{pc}: removed {:?} ({:?}){}",
                script.name,
                o.name,
                o.kind,
                if o.name == name {
                    String::new()
                } else {
                    format!(" cascade via parent {parent:?}")
                }
            );
            if o.kind == Kind::Other(CmdType::Sound as u8) {
                self.events.push(Event::StopSound {
                    channel: o.name.clone(),
                });
            }
            self.plays.remove(&o.name);
            self.timers.remove(&o.name);
            self.placeholders.remove(&o.name);
            self.prevmenus.remove(&o.name);
            self.videos.remove(&o.name);
            self.cinema_movies.remove(&o.name);
            self.cinemas.remove(&o.name);
            self.forget_deleted_component(&o.name);
            gone.push(o.name.clone());
            self.graveyard.insert(o.name.clone(), (o.kind, o.props));
        }
        format!("{name:?} ({} objs: {})", gone.len(), gone.join(", "))
    }

    /// PropMotion: anima una propiedad numérica desde su valor actual hasta el destino.
    pub(crate) fn step_prop_motion(&mut self, c: &Command) -> String {
        let obj = self.eval(c.expr("ObjName")).as_str();
        let pv = self.eval(c.expr("ObjProp"));
        let to = self.eval(c.expr("Value")).as_f64();
        let dur = self.eval(c.expr("Time")).as_f64();
        if let Some(p) = prop_of(&pv) {
            let from = self
                .scene
                .get(&obj)
                .map(|o| o.int(p, 0) as f64)
                .unwrap_or(0.0);
            let detail = format!(
                "{obj:?}.{} {from}->{to} in {dur}ms",
                crate::lsb::prop_name(p)
            );
            self.scene.add_motion(Motion {
                obj,
                prop: p,
                from,
                to,
                start: self.now,
                dur,
            });
            detail
        } else {
            String::new()
        }
    }
}
