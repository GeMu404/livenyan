use super::{HistoryEntry, Pending, State, Vm};
use crate::assets::IMAGE_EXTS;
use crate::lsb::tpword::{Glyph, brk};
use crate::scene::{Kind, TextItem};
use std::collections::HashMap;

/// Nombre del evento de sistema LiveNovel en un `Notify` (`\x01NAME\r\n...`).
fn system_text_event_name(event: &str) -> Option<&str> {
    let body = event.strip_prefix('\x01').unwrap_or(event);
    let name = body.split("\r\n").next().unwrap_or("").trim();
    (!name.is_empty()).then_some(name)
}

/// El plan ya exige un clic de lectura justo antes de este punto (p. ej. `<PS>` o
/// `{PAUSE}`), ignorando limpiezas intermedias (`Clear` / `{TEXTCLEAR}`). Así un
/// `<PG>` posterior no pide un segundo clic. **fuente primaria:** `script.html`
/// (`<PS>` clic; `<PG>` clic+改ページ); la fusión evita duplicar esperas en el
/// mismo `TextIns` sin anular un `<PG>` solo.
fn plan_has_trailing_click_wait(plan: &[Pending]) -> bool {
    for pending in plan.iter().rev() {
        match pending {
            Pending::Clear => continue,
            Pending::Notify(event) if system_text_event_name(event) == Some("TEXTCLEAR") => {
                continue;
            }
            Pending::Click => return true,
            Pending::Notify(event) if system_text_event_name(event) == Some("PAUSE") => {
                return true;
            }
            _ => return false,
        }
    }
    false
}

impl Vm {
    pub(crate) fn append_history(&mut self, page: String, text: Vec<TextItem>) {
        if text.is_empty() {
            return;
        }
        self.history.push_back(HistoryEntry { page, text });
        if self.history_max > 0 {
            while self.history.len() > self.history_max as usize {
                self.history.pop_front();
            }
        }
    }

    pub(crate) fn run_text(
        &mut self,
        target: &str,
        tw: &crate::lsb::TpWord,
        wait: bool,
        keep_history: bool,
        already_read: bool,
    ) {
        self.last_text_target = Some(target.to_string());
        if self.scene.get(target).is_none() {
            let msgs: Vec<String> = self
                .scene
                .draw_order()
                .into_iter()
                .filter(|o| o.kind == Kind::Message)
                .map(|o| format!("{:?}", o.name))
                .collect();
            log::debug!(
                "TextIns target {target:?} (bytes {:02x?}) missing; existing message objects before stand-in: {msgs:?}",
                target.as_bytes()
            );
            match self.scene.retained(target) {
                Some((kind, props)) => {
                    let evs: Vec<&str> = props
                        .keys()
                        .map(|p| crate::lsb::prop_name(*p))
                        .filter(|n| n.starts_with("ON"))
                        .collect();
                    self.warn(&format!("mes:{target}"), &format!("TextIns into deleted message object {target:?}; re-creating it with its last properties"));
                    log::debug!(
                        "stand-in {target:?} inherits {} props of the deleted object (events {evs:?})",
                        props.len()
                    );
                    self.scene.create(target, kind, props);
                }
                None => {
                    self.warn(&format!("mes:{target}"), &format!("TextIns into never-created message object {target:?}; creating an empty stand-in (no event handlers)"));
                    self.scene.create(target, Kind::Message, HashMap::new());
                    // Texto retenido hasta que llegue el MesNew real
                    if let Some(object) = self.scene.get_mut(target) {
                        object.placeholder = true;
                    }
                }
            }
        }
        if let Some(object) = self.scene.get_mut(target) {
            object.ended_unnotified = false;
        }
        let mut plan: Vec<Pending> = vec![];
        let mut cur: Vec<TextItem> = vec![];
        let flush = |plan: &mut Vec<Pending>, cur: &mut Vec<TextItem>| {
            if !cur.is_empty() {
                plan.push(Pending::Text {
                    items: std::mem::take(cur),
                    already_read,
                });
            }
        };
        let has_chars = tw.body.iter().any(|g| matches!(g, Glyph::Char { .. }));
        for g in &tw.body {
            match g {
                Glyph::Char { ch, .. } => cur.push(TextItem::Char(ch.clone())),
                Glyph::Var { name, .. } => {
                    let v = self.lookup(name).cloned().unwrap_or_default().as_str();
                    let displayed = self
                        .translations
                        .get(&crate::text_export::menu_item_id(&v))
                        .map(String::as_str)
                        .unwrap_or(&v);
                    cur.extend(displayed.chars().map(|c| TextItem::Char(c.to_string())));
                }
                Glyph::Return { kind } => match *kind {
                    brk::LINE => cur.push(TextItem::Newline),
                    // <PS> sin letras (solo tags CG/audio): no es pausa de lectura.
                    brk::PAUSE if !has_chars => {
                        flush(&mut plan, &mut cur);
                    }
                    brk::PAUSE => {
                        flush(&mut plan, &mut cur);
                        plan.push(Pending::Click);
                    }
                    // <PG> con diálogo: clic + 改ページ (script.html). Si ya hubo
                    // `<PS>`/`{PAUSE}` en el mismo TextIns, solo Clear (coalesce).
                    //
                    // <PG> **sin** Glyph::Char (logo/título: CREATECG→WAITPLAY→CHANGECG→<PG>→MESEND):
                    // solo Clear. **experimento** (COMPATIBILITY H10):
                    // el clic de lectura sobra (WAITPLAY CLICK o el botón ya cubren input);
                    // forzar clic deja negro muerto tras el logo / tras Iniciar.
                    //
                    // Tras `{PAUSE}`+`TEXTCLEAR` el `cur` ya se flusheó en el Event: un
                    // `<PG>` con `cur` vacío no debe pedir otro clic (**experimento** H10:
                    // el último `<PG>` tras STOPSND dejaba el cuadro
                    // vacío esperando un clic fantasma).
                    brk::PAGE if !has_chars => {
                        flush(&mut plan, &mut cur);
                        plan.push(Pending::Clear);
                    }
                    brk::PAGE => {
                        let had_visible = !cur.is_empty();
                        flush(&mut plan, &mut cur);
                        if had_visible && !plan_has_trailing_click_wait(&plan) {
                            plan.push(Pending::Click);
                        }
                        plan.push(Pending::Clear);
                    }
                    // <CLR> / other breaks: clear without an implied click-wait.
                    _ => {
                        flush(&mut plan, &mut cur);
                        plan.push(Pending::Clear);
                    }
                },
                Glyph::Event { event } => {
                    flush(&mut plan, &mut cur);
                    plan.push(Pending::Notify(event.clone()));
                }
                Glyph::Img { src, align } => {
                    if let Some(source) = self.assets.data().resolve(src, IMAGE_EXTS) {
                        self.assets.request(&source);
                        cur.push(TextItem::Image {
                            source,
                            align: *align,
                        });
                    } else {
                        self.warn(
                            &format!("inline-image:{src}"),
                            &format!("TextIns inline image not found: {src}"),
                        );
                    }
                }
                _ => {}
            }
        }
        flush(&mut plan, &mut cur);
        if log::log_enabled!(log::Level::Debug) {
            let tags: Vec<String> = tw
                .body
                .iter()
                .filter_map(|g| match g {
                    Glyph::Return { kind } => Some(format!("Return({kind})")),
                    Glyph::Event { event } => Some(format!(
                        "Event({})",
                        event
                            .trim_start_matches('\x01')
                            .split("\r\n")
                            .next()
                            .unwrap_or("")
                    )),
                    Glyph::Img { .. } => Some("Img".into()),
                    _ => None,
                })
                .collect();
            log::debug!("TextIns {target}: {} glyphs, tags {tags:?}", tw.body.len());
        }
        if keep_history {
            let entry = plan
                .iter()
                .filter_map(|pending| match pending {
                    Pending::Text { items, .. } => Some(items.as_slice()),
                    Pending::Clear | Pending::Click | Pending::Notify(_) => None,
                })
                .flatten()
                .cloned()
                .collect();
            let page = self
                .frames
                .last()
                .map(|frame| frame.script.name.clone())
                .unwrap_or_default();
            self.append_history(page, entry);
        }
        self.start_play(target, plan);
        if wait
            && !self
                .scene
                .get(target)
                .is_some_and(|object| object.placeholder)
        {
            self.state = State::WaitText {
                target: target.to_string(),
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Event, Options, Playback, PmButton, PrevMenu, Vm};
    use crate::archive::GameData;
    use crate::assets::Assets;
    use crate::lsb::expr::{Env, Value};
    use crate::lsb::names::{Func, Prop};
    use crate::lsb::tpword::Glyph;
    use crate::scene::{Kind, TextItem};
    use std::collections::HashMap;
    use std::sync::Arc;

    #[test]
    fn textspd_and_txspf_set_speed_override() {
        // script.html TEXTSPD / TXSPF
        let path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
        let data = GameData::open(&path).unwrap();
        let assets = Arc::new(Assets::new(data, 2, 64));
        let mut vm = Vm::new(
            assets,
            "ゲームメイン.lsb",
            Options {
                pick: None,
                trace_conds: false,
                screen: (800, 600),
                cinema_ms: 0.0,
                text_cps: 60.0,
                lookahead: 0,
                trace: false,
                show_text: false,
            },
        )
        .unwrap();
        vm.system_text_event("msg", "TEXTSPD", &["50".into(), "ON".into()]);
        let over = vm.text_char_ms.get("msg").copied().expect("TEXTSPD");
        assert!((over.ms - 50.0).abs() < f64::EPSILON);
        assert!(over.absolute);
        vm.system_text_event("msg", "TXSPF", &[]);
        assert_eq!(vm.text_char_ms["msg"].ms, 0.0);
        vm.system_text_event("msg", "TXSPN", &[]);
        assert!(!vm.text_char_ms.contains_key("msg"));
    }

    #[test]
    fn history_count_and_group_index_are_available() {
        let path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
        let data = GameData::open(&path).unwrap();
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

        vm.append_history("page".into(), vec![TextItem::Char("A".into())]);
        vm.append_history("page".into(), vec![TextItem::Char("B".into())]);
        assert_eq!(
            vm.call_engine(Func::GETHISTORYCOUNT, &[], &[]),
            Some(Value::Int(2))
        );
        assert!(!vm.unimpl.contains_key("func GetHistoryCount"));

        vm.scene.create(
            "grouped",
            Kind::Button,
            HashMap::from([(Prop::GROUPINDEX, Value::Int(7))]),
        );
        assert_eq!(
            vm.call_engine(Func::GETGROUPINDEX, &[Value::Str("grouped".into())], &[]),
            Some(Value::Int(7))
        );
        vm.prevmenus.insert(
            "menu".into(),
            PrevMenu {
                buttons: vec![PmButton {
                    x: 0,
                    y: 0,
                    w: 0,
                    h: 0,
                    src: String::new(),
                    src_selected: String::new(),
                    src_disabled: String::new(),
                    name: "grouped".into(),
                    group: 0,
                }],
                hover: None,
            },
        );
        assert_eq!(
            vm.call_engine(
                Func::GETGROUPINDEX,
                &[Value::Str("menu".into()), Value::Int(0)],
                &[],
            ),
            Some(Value::Int(7))
        );
    }

    #[test]
    fn text_audio_events_start_and_stop_named_channels() {
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

        vm.system_text_event(
            "msg",
            "PLAYSND",
            &[
                "se.wav".into(),
                "VOICE".into(),
                "REPEAT".into(),
                "500".into(),
                "0".into(),
            ],
        );
        assert!(matches!(
            vm.events.last(),
            Some(Event::PlaySound {
                channel,
                file,
                volume,
                repeat: true,
            }) if channel == "VOICE" && file == "se.wav" && *volume == 0.5
        ));

        vm.system_text_event(
            "msg",
            "STOPSND",
            &["BGM".into(), "100".into(), "PASS".into()],
        );
        assert!(matches!(
            vm.events.last(),
            Some(Event::FadeSound {
                channel,
                duration_ms: 100,
            }) if channel == "BGM"
        ));
    }

    #[test]
    fn text_pause_and_mesoff_events_update_message_playback() {
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
            "msg",
            Kind::Message,
            HashMap::from([(Prop::ONNOTIFY, Value::Str("ev.lsc".into()))]),
        );
        vm.ensure_play("msg");

        vm.system_text_event("msg", "PAUSE", &[]);
        assert!(vm.play_of("msg").unwrap().waiting_click);
        assert!(vm.get_prop("msg", Prop::TEXTPAUSED).truthy());

        // MESOFF encola ONNOTIFY; el script del juego oculta el 土台 (no hardcode local).
        vm.queue.clear();
        vm.system_text_event("msg", "MESOFF", &["100".into()]);
        assert!(
            vm.queue
                .iter()
                .any(|e| e.obj == "msg" && e.prop == Prop::ONNOTIFY),
            "MESOFF must notify the message box script"
        );
    }

    #[test]
    fn pause_holds_and_idle_waitplay_click_does_not() {
        use super::super::Pending;
        use std::collections::VecDeque;

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
            "msg",
            Kind::Message,
            HashMap::from([(Prop::VISIBLE, Value::Bool(true))]),
        );
        // {PAUSE} (Notify de sistema) espera clic; el plan sigue con Clear (como <CLR>),
        // no con un glifo <PG>, así que no hay segunda espera (H10). WAITPLAY CLICK solo
        // espera mientras el objetivo sigue ocupado.
        let mut plan = VecDeque::new();
        plan.push_back(Pending::Text {
            items: vec![TextItem::Char("あ".into())],
            already_read: false,
        });
        plan.push_back(Pending::Notify("\x01PAUSE\r\n".into()));
        plan.push_back(Pending::Notify("\x01TEXTCLEAR".into()));
        plan.push_back(Pending::Clear);
        plan.push_back(Pending::Notify("\x01MESOFF".into()));
        plan.push_back(Pending::Notify("\x01WAITPLAY\r\n背景\r\nCLICK".into()));
        plan.push_back(Pending::Notify("MESEND".into()));
        vm.scene.get_mut("msg").unwrap().play = Some(Playback {
            plan,
            ..Playback::default()
        });

        vm.update(0.0, 0);
        assert!(vm.play_of("msg").unwrap().waiting_click, "PAUSE must wait");
        assert!(vm.play_of("msg").is_some(), "must not reach MESEND yet");

        vm.click();
        vm.update(16.0, 0);
        // WAITPLAY 背景 CLICK con el objetivo ya quieto no abre otro clic
        // (イベント.lsb: la espera es solo mientras sigue reproduciéndose).
        let mut saw_wait = false;
        for step in 0..6 {
            vm.update(32.0 + step as f64 * 16.0, 0);
            if vm
                .play_of("msg")
                .is_some_and(|playback| playback.waiting_click || playback.wait_for.is_some())
            {
                saw_wait = true;
            }
        }
        assert!(
            !saw_wait,
            "idle WAITPLAY CLICK must not wait for another click"
        );
        assert!(
            !vm.play_of("msg").is_some(),
            "MESEND should finish playback without an extra click"
        );
    }

    #[test]
    fn page_break_without_chars_does_not_wait() {
        // Logo/título: solo events + <PG> + MESEND → no clic de lectura.
        use crate::lsb::tpword::{TpWord, brk};

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
            "msg",
            Kind::Message,
            HashMap::from([(Prop::VISIBLE, Value::Bool(true))]),
        );
        let tw = TpWord {
            version: 105,
            body: vec![
                Glyph::Event {
                    event: "\x01WAITPLAY\r\n背景\r\nCLICK".into(),
                },
                Glyph::Return { kind: brk::PAGE },
                Glyph::Event {
                    event: "MESEND".into(),
                },
            ],
            ..TpWord::default()
        };
        vm.run_text("msg", &tw, true, false, false);
        vm.update(0.0, 0);
        assert!(
            !vm.wants_click(),
            "CG-only <PG> must not demand a reading click"
        );
    }

    #[test]
    fn page_break_glyph_waits_for_click_then_clears() {
        // docs/livenovel/script.html <PG>: espera clic y luego cambia de página.
        use crate::lsb::tpword::{TpWord, brk};

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
            "msg",
            Kind::Message,
            HashMap::from([(Prop::VISIBLE, Value::Bool(true))]),
        );
        let tw = TpWord {
            version: 105,
            body: vec![
                Glyph::Char {
                    ch: "a".into(),
                    decorator: -1,
                    link: -1,
                    speed: 0,
                    condition: -1,
                },
                Glyph::Return { kind: brk::PAGE },
            ],
            ..TpWord::default()
        };
        vm.run_text("msg", &tw, true, false, false);
        vm.update(0.0, 0);
        assert!(
            vm.wants_click(),
            "<PG> must wait for a click after showing text"
        );
        assert!(vm.play_of("msg").is_some());
        vm.click();
        vm.update(16.0, 0);
        assert!(!vm.wants_click(), "click should release the <PG> wait");
        assert!(
            !vm.play_of("msg").is_some(),
            "page break should finish playback after the click"
        );
        let text = vm
            .scene
            .get("msg")
            .map(|obj| obj.str(Prop::TEXT))
            .unwrap_or_default();
        assert!(
            text.is_empty(),
            "page break clears the message after the click, got {text:?}"
        );
    }

    #[test]
    fn page_break_after_pause_glyph_does_not_demand_a_second_click() {
        // `<PS>` y luego `<PG>` en un TextIns: una espera de clic y luego Clear.
        use crate::lsb::tpword::{TpWord, brk};

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
            "msg",
            Kind::Message,
            HashMap::from([(Prop::VISIBLE, Value::Bool(true))]),
        );
        let tw = TpWord {
            version: 105,
            body: vec![
                Glyph::Char {
                    ch: "a".into(),
                    decorator: -1,
                    link: -1,
                    speed: 0,
                    condition: -1,
                },
                Glyph::Return { kind: brk::PAUSE },
                Glyph::Return { kind: brk::PAGE },
            ],
            ..TpWord::default()
        };
        vm.run_text("msg", &tw, true, false, false);
        vm.update(0.0, 0);
        assert!(vm.wants_click(), "first click-wait from <PS>/<PG> coalesce");
        vm.click();
        vm.update(16.0, 0);
        assert!(
            !vm.wants_click(),
            "coalesced <PG> must not require a second click"
        );
        assert!(!vm.play_of("msg").is_some());
    }

    #[test]
    fn page_break_after_pause_system_event_does_not_demand_a_second_click() {
        // LiveNovel pattern: {PAUSE}, optional {TEXTCLEAR}, then <PG>.
        use crate::lsb::tpword::{TpWord, brk};

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
            "msg",
            Kind::Message,
            HashMap::from([(Prop::VISIBLE, Value::Bool(true))]),
        );
        let tw = TpWord {
            version: 105,
            body: vec![
                Glyph::Char {
                    ch: "a".into(),
                    decorator: -1,
                    link: -1,
                    speed: 0,
                    condition: -1,
                },
                Glyph::Event {
                    event: "\x01PAUSE\r\n".into(),
                },
                Glyph::Event {
                    event: "\x01TEXTCLEAR".into(),
                },
                Glyph::Return { kind: brk::PAGE },
            ],
            ..TpWord::default()
        };
        vm.run_text("msg", &tw, true, false, false);
        vm.update(0.0, 0);
        assert!(vm.wants_click(), "{{PAUSE}} must wait");
        vm.click();
        vm.update(16.0, 0);
        assert!(
            !vm.wants_click(),
            "<PG> after {{PAUSE}}/{{TEXTCLEAR}} must not wait again"
        );
        assert!(!vm.play_of("msg").is_some());
    }

    /// Novela LiveNovel: `{PAUSE}` + `TEXTCLEAR` (ONNOTIFY, sin `\x01`) + `<PG>`, luego
    /// otro `<PG>` tras STOPSND. El segundo no debe dejar clic con el cuadro vacío.
    #[test]
    fn trailing_page_after_pause_does_not_phantom_click() {
        use crate::lsb::tpword::{TpWord, brk};

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
            "msg",
            Kind::Message,
            HashMap::from([(Prop::VISIBLE, Value::Bool(true))]),
        );
        let tw = TpWord {
            version: 105,
            body: vec![
                Glyph::Char {
                    ch: "a".into(),
                    decorator: -1,
                    link: -1,
                    speed: 0,
                    condition: -1,
                },
                Glyph::Event {
                    event: "\x01PAUSE\r\n".into(),
                },
                Glyph::Event {
                    event: "TEXTCLEAR".into(),
                },
                Glyph::Return { kind: brk::PAGE },
                Glyph::Event {
                    event: "\x01STOPSND\r\n".into(),
                },
                Glyph::Return { kind: brk::PAGE },
                Glyph::Event {
                    event: "MESEND".into(),
                },
            ],
            ..TpWord::default()
        };
        vm.run_text("msg", &tw, true, false, false);
        vm.update(0.0, 0);
        assert!(vm.wants_click(), "first PAUSE waits");
        vm.click();
        for step in 0..8 {
            vm.update(16.0 + step as f64, 0);
            assert!(
                !vm.wants_click(),
                "no phantom click after PAUSE/TEXTCLEAR/<PG> (step {step})"
            );
            if !vm.play_of("msg").is_some() {
                break;
            }
        }
        assert!(
            !vm.play_of("msg").is_some(),
            "MESEND should finish without a second click"
        );
    }

    #[test]
    fn message_caret_advances_with_shown_text() {
        use super::super::Pending;
        use std::collections::VecDeque;

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
            "msg",
            Kind::Message,
            HashMap::from([
                (Prop::FONTHEIGHT, Value::Int(22)),
                (Prop::WIDTH, Value::Int(600)),
            ]),
        );
        let mut plan = VecDeque::new();
        plan.push_back(Pending::Text {
            items: vec![TextItem::Char("あ".into()), TextItem::Char("い".into())],
            already_read: false,
        });
        plan.push_back(Pending::Click);
        vm.scene.get_mut("msg").unwrap().play = Some(Playback {
            plan,
            ..Playback::default()
        });
        vm.update(0.0, 0);
        let caret_x = vm.get_prop("msg", Prop::CARETX).as_int();
        let caret_h = vm.get_prop("msg", Prop::CARETHEIGHT).as_int();
        assert_eq!(caret_h, 22);
        assert!(
            caret_x > 20,
            "CARETX must advance past the start for メッセージカーソル (got {caret_x})"
        );
    }

    #[test]
    fn click_reveal_advances_message_caret() {
        use super::super::Pending;
        use std::collections::VecDeque;

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
                // Un glifo por tick: el clic tiene que volcar el resto y mover CARETX.
                text_cps: 1.0,
                lookahead: 50,
                trace: false,
                show_text: false,
            },
        )
        .unwrap();
        vm.scene.create(
            "msg",
            Kind::Message,
            HashMap::from([
                (Prop::FONTHEIGHT, Value::Int(22)),
                (Prop::WIDTH, Value::Int(600)),
                (Prop::TEXTDELAY, Value::Int(1000)),
            ]),
        );
        let chars = ["あ", "い", "う", "え", "お", "か", "き", "く"];
        let mut plan = VecDeque::new();
        plan.push_back(Pending::Text {
            items: chars
                .iter()
                .map(|ch| TextItem::Char((*ch).into()))
                .collect(),
            already_read: false,
        });
        plan.push_back(Pending::Click);
        vm.scene.get_mut("msg").unwrap().play = Some(Playback {
            plan,
            ..Playback::default()
        });
        vm.now = 0.0;
        vm.update(0.0, 0);
        let mid = vm.get_prop("msg", Prop::CARETX).as_int();
        assert!(mid > 0, "typewriter should have moved the caret a bit");
        vm.click();
        let after = vm.get_prop("msg", Prop::CARETX).as_int();
        assert!(
            after > mid,
            "click reveal must sync CARETX past the mid-typewriter pen ({mid} -> {after})"
        );
    }

    #[test]
    fn playmovie_resolves_wmv_path_to_existing_movie_source() {
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
            "bg",
            Kind::Movie,
            HashMap::from([(
                Prop::SOURCE,
                Value::Str("MOVIE\\なつみ家外観（昼のみ）.wmv".into()),
            )]),
        );
        vm.scene.create("msg", Kind::Message, HashMap::new());
        vm.ensure_play("msg");

        vm.system_text_event(
            "msg",
            "PLAYMOVIE",
            &[
                "なつみ家外観（昼のみ）.wmv".into(),
                "256".into(),
                "C".into(),
                "C".into(),
                "WAIT".into(),
                "TRUE".into(),
                "0".into(),
            ],
        );

        assert!(vm.warned.contains("playmovie:bg"));
        assert!(!vm.warned.contains("playmovie:なつみ家外観（昼のみ）.wmv"));
        assert!(!vm.warned.contains("playmovie:missing-target"));
        assert!(vm.play_of("msg").unwrap().wait_for.is_none());
    }

    #[test]
    fn history_text_is_recorded_only_when_requested_and_obeys_the_limit() {
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
        vm.scene.create("msg", Kind::Message, HashMap::new());
        let text = crate::lsb::TpWord {
            body: vec![Glyph::Char {
                ch: "A".into(),
                decorator: -1,
                link: -1,
                speed: 0,
                condition: -1,
            }],
            ..crate::lsb::TpWord::default()
        };

        vm.run_text("msg", &text, false, false, false);
        assert_eq!(vm.history.len(), 0);
        vm.scene.clear_plays();
        vm.run_text("msg", &text, false, true, false);
        assert_eq!(vm.history.len(), 1);
        vm.append_history("page".into(), vec![TextItem::Char("B".into())]);
        vm.history_max = 1;
        vm.append_history("page".into(), vec![TextItem::Char("C".into())]);
        assert_eq!(vm.history.len(), 1);
        assert_eq!(
            vm.history.front().map(|entry| entry.text.as_slice()),
            Some([TextItem::Char("C".into())].as_slice())
        );
    }
}
