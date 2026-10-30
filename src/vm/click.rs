use super::*;

struct HitSpot {
    name: String,
    kind: Kind,
    mouse: i64,
    ignores: bool,
    handler: bool,
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Image => "Image",
        Kind::Message => "Message",
        Kind::Box => "Box",
        Kind::Tile => "Tile",
        Kind::Button => "Button",
        Kind::Slider => "Slider",
        Kind::Edit => "Edit",
        Kind::Caption => "Caption",
        Kind::Menu => "Menu",
        Kind::Movie => "Movie",
        Kind::Other(_) => "Other",
    }
}

/// Primer botón bajo el punto que no es `except`.
fn buried_button(spots: &[HitSpot], except: &str) -> String {
    let Some(spot) = spots
        .iter()
        .find(|spot| spot.kind == Kind::Button && spot.name != except)
    else {
        return String::new();
    };
    let mut note = format!("under={}/Button", spot.name);
    if spot.ignores {
        note.push_str(&format!(" mouse={}", spot.mouse));
    } else if !spot.handler {
        note.push_str(" no-handler");
    }
    note
}

fn pointer_box(assets: &crate::assets::Assets, object: &crate::scene::Obj) -> (f64, f64) {
    let intrinsic = match object.kind {
        Kind::Image | Kind::Button | Kind::Tile => {
            slider::source_pixel_size(assets, &object.str(Prop::SOURCE))
        }
        _ => None,
    };
    crate::scene::pointer_box(
        object.int(Prop::WIDTH, 0),
        object.int(Prop::HEIGHT, 0),
        intrinsic,
    )
}

impl Vm {
    /// Movimiento del ratón: hover de PrevMenu (ONMAPIN/ONMAPOUT con @ParamStr = [nombre, índice]).
    pub fn mouse_move(&mut self, x: f64, y: f64) {
        self.mouse = (x, y);
        let hit = self.prevmenu_hit(x, y);
        let names: Vec<String> = self.prevmenus.keys().cloned().collect();
        for n in names {
            let now = hit.as_ref().filter(|h| h.0 == n).map(|h| h.1);
            let before = self.prevmenus[&n].hover;
            if now == before {
                continue;
            }
            self.prevmenus.get_mut(&n).unwrap().hover = now;
            let bname = |vm: &Self, i: usize| {
                vm.prevmenus[&n]
                    .buttons
                    .get(i)
                    .map(|b| b.name.clone())
                    .unwrap_or_default()
            };
            if let Some(i) = before {
                let p = vec![Value::Str(bname(self, i)), Value::Int(i as i64)];
                self.fire(&n, Prop::ONMAPOUT, p);
            }
            if let Some(i) = now {
                log::debug!("PrevMenu {n}: hover button {i}");
                let p = vec![Value::Str(bname(self, i)), Value::Int(i as i64)];
                self.fire(&n, Prop::ONMAPIN, p);
            }
        }
        let assets = Arc::clone(&self.assets);
        let hovered = self
            .scene
            .draw_order()
            .into_iter()
            .rev()
            .find(|object| {
                let listens = matches!(object.kind, Kind::Button | Kind::Slider)
                    || object
                        .get(Prop::ONMOUSEIN)
                        .is_some_and(|v| !v.as_str().trim().is_empty());
                if !listens
                    || object.ignores_mouse(crate::scene::MouseHit::Hover)
                    || !self.scene.effective_visible(object)
                {
                    return false;
                }
                let (left, top, width, height) = if object.kind == Kind::Slider {
                    self.slider_bounds(object)
                } else {
                    let (width, height) = pointer_box(&assets, object);
                    self.scene.placed_rect(object, width, height)
                };
                width > 0.0
                    && height > 0.0
                    && x >= left
                    && y >= top
                    && x < left + width
                    && y < top + height
            })
            .map(|object| object.name.clone());
        if hovered != self.hovered_object {
            if let Some(old) = self.hovered_object.take() {
                self.fire(&old, Prop::ONMOUSEOUT, vec![]);
            }
            if let Some(new) = &hovered {
                self.fire(new, Prop::ONMOUSEIN, vec![]);
            }
            self.hovered_object = hovered;
        }
        if let Some(name) = self.slider_drag.clone() {
            self.set_slider_from_pointer(&name, x, y);
        }
    }

    /// Script de escena: el marco más cercano que no es el bucle del cuadro de mensaje.
    fn click_scene(&self) -> &str {
        self.frames
            .iter()
            .rev()
            .find(|frame| !frame.script.name.contains("メッセージボックス"))
            .or_else(|| self.frames.last())
            .map(|frame| frame.script.name.as_str())
            .unwrap_or("<none>")
    }

    /// Qué haría `click()` con el texto o la espera, sin ejecutarlo.
    fn click_advance(&self) -> &'static str {
        if self.scene.live().any(|object| {
            object
                .play
                .as_ref()
                .is_some_and(|play| play.pos < play.cur.len())
        }) {
            return "reveal";
        }
        if self
            .scene
            .live()
            .any(|object| object.play.as_ref().is_some_and(|play| play.waiting_click))
        {
            return "page";
        }
        if self.scene.live().any(|object| {
            object
                .play
                .as_ref()
                .is_some_and(|play| play.click_skips_wait && play.wait_for.is_some())
        }) {
            return "skip-wait";
        }
        if !self.one_shot_cinemas().is_empty() {
            return "cinema";
        }
        if matches!(
            self.state,
            State::WaitUntil {
                skippable: true,
                ..
            }
        ) {
            return "skip-wait";
        }
        "none"
    }

    /// Comando del script de escena (no el bucle del cuadro de mensaje) y si el menú de elección existe.
    fn stuck_choice(&self) -> String {
        let frame = self
            .frames
            .iter()
            .rev()
            .find(|frame| !frame.script.name.contains("メッセージボックス"))
            .or_else(|| self.frames.last());
        let cmd = match frame {
            Some(frame) => match frame.script.commands.get(frame.pc) {
                Some(command) => format!(
                    "cmd={} L{} {} #{}",
                    frame.script.name,
                    command.line_no,
                    command.name(),
                    frame.pc
                ),
                None => format!("cmd={} #{}", frame.script.name, frame.pc),
            },
            None => "cmd=<none>".to_string(),
        };
        let menu = if self.scene.get("選択メニュー").is_some() {
            "yes"
        } else {
            "no"
        };
        let top = match self.frames.last() {
            Some(frame) => match frame.script.commands.get(frame.pc) {
                Some(command) => format!(
                    "{} L{} {} #{}",
                    frame.script.name,
                    command.line_no,
                    command.name(),
                    frame.pc
                ),
                None => format!("{} #{}", frame.script.name, frame.pc),
            },
            None => "<none>".to_string(),
        };
        let flag = self
            .lookup("__メッセージ終了")
            .map(|value| format!("{value:?}"))
            .unwrap_or_else(|| "<unset>".to_string());
        format!(
            "{cmd} 選択メニュー={menu} top={top} state={:?} flag={flag} depth={}",
            self.state,
            self.frames.len()
        )
    }

    fn object_kind_name(&self, name: &str) -> &'static str {
        self.scene
            .get(name)
            .map(|object| kind_name(object.kind))
            .unwrap_or("-")
    }

    fn objects_under_pointer(
        &self,
        assets: &Assets,
        x: f64,
        y: f64,
        hit: crate::scene::MouseHit,
        props: &[u8],
    ) -> Vec<HitSpot> {
        self.scene
            .draw_order()
            .into_iter()
            .rev()
            .filter_map(|object| {
                if !self.scene.effective_visible(object) {
                    return None;
                }
                let (width, height) = pointer_box(assets, object);
                let (left, top, width, height) = self.scene.placed_rect(object, width, height);
                if !(width > 0.0
                    && height > 0.0
                    && x >= left
                    && y >= top
                    && x < left + width
                    && y < top + height)
                {
                    return None;
                }
                let handler = props.iter().any(|prop| {
                    object
                        .get(*prop)
                        .is_some_and(|value| !value.as_str().trim().is_empty())
                });
                Some(HitSpot {
                    name: object.name.clone(),
                    kind: object.kind,
                    mouse: object.int(Prop::IGNOREMOUSE, 0),
                    ignores: object.ignores_mouse(hit),
                    handler,
                })
            })
            .collect()
    }

    fn log_click(
        &self,
        x: f64,
        y: f64,
        right: bool,
        object: &str,
        kind: &str,
        result: &str,
        delivers_click: bool,
        extra: &str,
    ) {
        let effect = self.click_advance();
        let advance = if delivers_click {
            format!(" advance={effect}")
        } else if effect == "none" {
            String::new()
        } else {
            format!(" advance=held:{effect}")
        };
        let side = if right { " right" } else { "" };
        let extra = if extra.is_empty() {
            String::new()
        } else {
            format!(" {extra}")
        };
        let stall = if delivers_click && effect == "none" {
            format!(" {}", self.stuck_choice())
        } else {
            String::new()
        };
        log::info!(
            "click ({x:.0},{y:.0}){side} scene={} object={object} kind={kind} result={result}{advance}{extra}{stall}",
            self.click_scene()
        );
    }

    fn log_unhandled_click(&self, x: f64, y: f64, right: bool, spots: &[HitSpot]) {
        let delivers = !right;
        let Some(top) = spots.first() else {
            let result = if delivers && self.click_advance() != "none" {
                "advance"
            } else {
                "noop"
            };
            self.log_click(x, y, right, "(none)", "-", result, delivers, "");
            return;
        };
        let result = if top.ignores { "ignored" } else { "no-handler" };
        let mut extra = Vec::new();
        if top.ignores {
            extra.push(format!("mouse={}", top.mouse));
        }
        let buried = buried_button(spots, &top.name);
        if !buried.is_empty() {
            extra.push(buried);
        }
        self.log_click(
            x,
            y,
            right,
            &top.name,
            kind_name(top.kind),
            result,
            delivers,
            &extra.join(" "),
        );
    }

    /// Clic (izquierdo o derecho) en coordenadas lógicas: manejadores de objeto o avance.
    pub fn click_at(&mut self, x: f64, y: f64, right: bool) {
        let assets = Arc::clone(&self.assets);
        if right {
            let modal_menu = match &self.state {
                State::Menu { name, .. } => Some(name.clone()),
                _ => None,
            };
            let menu = modal_menu.or_else(|| {
                self.scene
                    .draw_order()
                    .into_iter()
                    .rev()
                    .find(|o| {
                        if o.kind != Kind::Menu
                            || !self.scene.effective_visible(o)
                            || !o.get(Prop::RCLICKSCRAP).is_some_and(Value::truthy)
                        {
                            return false;
                        }
                        let (width, height) = pointer_box(&assets, o);
                        let (left, top, width, height) = self.scene.placed_rect(o, width, height);
                        x >= left && y >= top && x < left + width && y < top + height
                    })
                    .filter(|o| o.get(Prop::RCLICKSCRAP).is_some_and(Value::truthy))
                    .map(|o| o.name.clone())
            });
            if let Some(name) = menu
                && self
                    .scene
                    .get(&name)
                    .is_some_and(|o| o.get(Prop::RCLICKSCRAP).is_some_and(Value::truthy))
            {
                if matches!(&self.state, State::Menu { name: active, .. } if active == &name) {
                    self.state = State::Running;
                }
                let next = self
                    .scene
                    .get(&name)
                    .map(|object| object.str(Prop::ONRBTNUP))
                    .unwrap_or_default();
                let kind = self.object_kind_name(&name);
                if next.trim().is_empty() {
                    self.log_click(x, y, true, &name, kind, "scrap", false, "");
                    self.scrap_component(&name);
                } else if self.fire(&name, Prop::ONRBTNUP, vec![]) {
                    self.log_click(
                        x,
                        y,
                        true,
                        &name,
                        kind,
                        "ONRBTNUP",
                        false,
                        &format!("script={next}"),
                    );
                } else {
                    self.log_click(x, y, true, &name, kind, "scrap", false, "");
                    self.scrap_component(&name);
                }
                return;
            }
        } else {
            if self.begin_slider_drag(x, y) {
                let name = self.slider_focus.clone().unwrap_or_else(|| "slider".into());
                let kind = self.object_kind_name(&name);
                self.log_click(x, y, false, &name, kind, "drag", false, "");
                return;
            }
            self.focus_edit_at(x, y);
        }
        let props: &[u8] = if right {
            &[Prop::ONRBTNUP, Prop::ONRBTNDOWN]
        } else {
            &[
                Prop::ONLBTNUP,
                Prop::ONLBTNDOWN,
                Prop::ONSELECT,
                Prop::ONNOTIFY,
            ]
        };
        let menu_hit = (!right).then(|| self.prevmenu_hit(x, y)).flatten();
        let menu_key = menu_hit.as_ref().and_then(|(name, _)| {
            self.scene
                .get(name)
                .map(|o| (o.int(Prop::PRIORITY, 0), o.seq))
        });
        let mouse_hit = if right {
            crate::scene::MouseHit::Right
        } else {
            crate::scene::MouseHit::Left
        };
        let object_hit = self
            .scene
            .hit_handler_with(x, y, props, mouse_hit, |object| {
                pointer_box(&assets, object)
            })
            .map(|o| (o.name.clone(), (o.int(Prop::PRIORITY, 0), o.seq)));

        if let Some((n, i)) = menu_hit
            && menu_key.is_some_and(|menu_key| {
                object_hit
                    .as_ref()
                    .is_none_or(|(_, object_key)| menu_key >= *object_key)
            })
        {
            // selección: ONSELECT y, al terminar (sin animación de selección), ONPLAYEND
            let bname = self.prevmenus[&n].buttons[i].name.clone();
            let group = self.prevmenus[&n].buttons[i].group;
            log::debug!("PrevMenu {n}: selected button {i} group {group}");
            // Grupo distinto de 0: el motor alterna SELECTED antes del ONSELECT
            // (docs/livenovel/option3.html: オート y スキップ son toggles, grupo -1).
            let selected = if group != 0 {
                let name_key = (n.clone(), MenuItemKey::Name(bname.clone()));
                let next = !self.selected.get(&name_key).copied().unwrap_or(false);
                if group > 0 {
                    let same_group: Vec<(usize, String)> = self.prevmenus[&n]
                        .buttons
                        .iter()
                        .enumerate()
                        .filter(|(_, button)| button.group == group)
                        .map(|(index, button)| (index, button.name.clone()))
                        .collect();
                    for (index, name) in same_group {
                        self.selected
                            .insert((n.clone(), MenuItemKey::Index(index as i64)), false);
                        self.selected
                            .insert((n.clone(), MenuItemKey::Name(name)), false);
                    }
                }
                next
            } else {
                true
            };
            self.selected
                .insert((n.clone(), MenuItemKey::Index(i as i64)), selected);
            self.selected
                .insert((n.clone(), MenuItemKey::Name(bname.clone())), selected);
            self.assign("@SelStr", Value::Str(bname.clone()));
            let p = vec![Value::Str(bname.clone()), Value::Int(i as i64)];
            let on_select = self
                .scene
                .get(&n)
                .map(|object| object.str(Prop::ONSELECT))
                .unwrap_or_default();
            let on_play_end = self
                .scene
                .get(&n)
                .map(|object| object.str(Prop::ONPLAYEND))
                .unwrap_or_default();
            let kind = self.object_kind_name(&n);
            let mut extra = format!("item={bname}");
            if !on_select.trim().is_empty() {
                extra.push_str(&format!(" script={on_select}"));
            }
            if !on_play_end.trim().is_empty() {
                extra.push_str(&format!(" then={on_play_end}"));
            }
            self.log_click(x, y, false, &n, kind, "ONSELECT", false, &extra);
            self.fire(&n, Prop::ONSELECT, p.clone());
            self.fire(&n, Prop::ONPLAYEND, p);
            return;
        }
        if let Some((name, _)) = object_hit {
            let spots = self.objects_under_pointer(&assets, x, y, mouse_hit, props);
            let buried = if self
                .scene
                .get(&name)
                .is_some_and(|object| object.kind == Kind::Button)
            {
                String::new()
            } else {
                buried_button(&spots, &name)
            };
            // Clic en message box con texto pendiente: avanzar reproducción, no disparar
            // ONNOTIFY vacío (la caja declara ONNOTIFY para tags del texto, no para el ratón).
            if !right
                && self
                    .scene
                    .get(&name)
                    .is_some_and(|o| o.kind == Kind::Message)
                && self.wants_click()
            {
                // El avance lo hace el clic del motor. Si también queda @LClick,
                // el ONIDLE del cuadro (アイドル時) vuelve a avanzar la misma página.
                self.mouse_left_click = false;
                let kind = self.object_kind_name(&name);
                self.log_click(x, y, false, &name, kind, "advance", true, &buried);
                self.click();
                return;
            }
            let mut fired = false;
            for &p in props {
                if !right
                    && p == Prop::ONNOTIFY
                    && self
                        .scene
                        .get(&name)
                        .is_some_and(|o| o.kind == Kind::Edit || o.kind == Kind::Message)
                {
                    continue;
                }
                // Menu nativo: ONSELECT lleva índice vía `choose` / State::Menu, no un
                // fire vacío sobre el rectángulo entero (reabría el diálogo de guardado).
                if !right
                    && p == Prop::ONSELECT
                    && self.scene.get(&name).is_some_and(|o| o.kind == Kind::Menu)
                {
                    continue;
                }
                if self.fire(&name, p, vec![]) {
                    let script = self
                        .scene
                        .get(&name)
                        .map(|object| object.str(p))
                        .unwrap_or_default();
                    let kind = self.object_kind_name(&name);
                    let mut extra = Vec::new();
                    if !script.trim().is_empty() {
                        extra.push(format!("script={script}"));
                    }
                    if !buried.is_empty() {
                        extra.push(buried.clone());
                    }
                    self.log_click(
                        x,
                        y,
                        right,
                        &name,
                        kind,
                        crate::lsb::prop_name(p),
                        false,
                        &extra.join(" "),
                    );
                    fired = true;
                    break;
                }
            }
            if !fired {
                let kind = self.object_kind_name(&name);
                let skipped = self
                    .scene
                    .get(&name)
                    .map(|object| {
                        props
                            .iter()
                            .filter(|prop| {
                                object
                                    .get(**prop)
                                    .is_some_and(|value| !value.as_str().trim().is_empty())
                            })
                            .map(|prop| crate::lsb::prop_name(*prop))
                            .collect::<Vec<_>>()
                            .join(",")
                    })
                    .unwrap_or_default();
                let mut extra = Vec::new();
                if !skipped.is_empty() {
                    extra.push(format!("skipped={skipped}"));
                }
                if !buried.is_empty() {
                    extra.push(buried);
                }
                self.log_click(
                    x,
                    y,
                    right,
                    &name,
                    kind,
                    "no-handler",
                    false,
                    &extra.join(" "),
                );
            }
            return;
        }
        if !right && self.wants_click() {
            self.mouse_left_click = false;
        }
        let spots = self.objects_under_pointer(&assets, x, y, mouse_hit, props);
        self.log_unhandled_click(x, y, right, &spots);
        if !right {
            self.click();
        }
    }
}
