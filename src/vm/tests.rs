use super::*;
use crate::archive::GameData;
use crate::lsb::tpword::Glyph;

#[test]
fn caption_width_estimate_treats_latin_accents_as_half_width() {
    assert_eq!(caption_char_width('ó'), 0.55);
    assert_eq!(caption_char_width('¿'), 0.55);
    assert_eq!(caption_char_width('姉'), 1.0);
}

#[test]
fn get_prop_visible_defaults_true_for_missing_objects() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
    let data = GameData::open(&path).unwrap();
    let assets = Arc::new(Assets::new(data, 2, 64));
    let mut vm = Vm::new(
        assets,
        "ゲームメイン.lsb",
        Options {
            screen: (800, 600),
            cinema_ms: 0.0,
            text_cps: 0.0,
            lookahead: 0,
            trace: false,
            trace_conds: false,
            pick: None,
            show_text: false,
        },
    )
    .unwrap();
    // Tras ObjDel, GetProp(VISIBLE) es false → __vis oculta el objeto hasta ONPLAYING.
    assert_eq!(
        vm.get_prop("deleted-object", Prop::VISIBLE),
        Value::Bool(false)
    );
    assert_eq!(
        vm.get_prop("no-such-object", Prop::VISIBLE),
        Value::Bool(false)
    );
    assert_eq!(
        vm.call_engine(Func::EXISTS, &[Value::Str("no-such-object".into())], &[]),
        Some(Value::Bool(false))
    );
}

#[test]
fn set_check_and_get_check_track_menu_item_state() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
    let data = GameData::open(&path).unwrap();
    let assets = Arc::new(Assets::new(data, 2, 64));
    let mut vm = Vm::new(
        assets,
        "ゲームメイン.lsb",
        Options {
            screen: (800, 600),
            cinema_ms: 0.0,
            text_cps: 0.0,
            lookahead: 0,
            trace: false,
            trace_conds: false,
            pick: None,
            show_text: false,
        },
    )
    .unwrap();
    assert_eq!(
        vm.call_engine(
            Func::GETCHECK,
            &[Value::Str("ロードメニュー".into()), Value::Int(3)],
            &[]
        ),
        Some(Value::Bool(false))
    );
    assert_eq!(
        vm.call_engine(
            Func::SETCHECK,
            &[
                Value::Str("ロードメニュー".into()),
                Value::Int(3),
                Value::Bool(true),
            ],
            &[]
        ),
        Some(Value::Bool(true))
    );
    assert_eq!(
        vm.call_engine(
            Func::GETCHECK,
            &[Value::Str("ロードメニュー".into()), Value::Int(3)],
            &[]
        ),
        Some(Value::Bool(true))
    );
    let stale_gal = crate::gal::encode_galex_rgba(1, 1, &[10, 20, 30, 255]).unwrap();
    let stale_image = Arc::new(crate::gal::decode_any(&stale_gal).unwrap());
    vm.mem_files.insert("__workscr.gal".into(), stale_gal);
    vm.vfs_image_cache
        .borrow_mut()
        .insert("mem:__workscr.gal".into(), stale_image);
    // SaveScreen siempre pide una captura nueva y descarta el `mem:` anterior.
    assert_eq!(
        vm.call_engine(
            Func::SAVESCREEN,
            &[Value::Str("mem:__workscr.gal".into())],
            &[]
        ),
        Some(Value::Bool(true)),
        "SaveScreen queues a fresh capture"
    );
    assert!(!vm.mem_files.contains_key("__workscr.gal"));
    assert!(
        !vm.vfs_image_cache
            .borrow()
            .contains_key("mem:__workscr.gal")
    );
    assert_eq!(vm.pending_savescreen.len(), 1);
    assert_eq!(vm.take_pending_savescreen().len(), 1);
    let tmp = std::env::temp_dir().join("livenyan_copyfile_vfs");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    vm.save_dir = tmp.clone();
    assert_eq!(
        vm.call_engine(
            Func::SAVESCREEN,
            &[Value::Str("mem:__savescr.gal".into())],
            &[]
        ),
        Some(Value::Bool(true))
    );
    let req = vm.take_pending_savescreen().pop().unwrap();
    assert!(vm.complete_savescreen(
        &req,
        2,
        2,
        vec![
            255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255,
        ],
    ));
    assert!(vm.mem_files.contains_key("__savescr.gal"));
    assert_eq!(
        vm.savescreen_layout("mem:__savescr.gal"),
        Some(vm.opts.screen)
    );
    let saved = crate::gal::decode_any(vm.mem_files.get("__savescr.gal").unwrap()).unwrap();
    assert_eq!((saved.w, saved.h), (2, 2));
    assert_eq!(
        vm.call_engine(
            Func::COPYFILE,
            &[
                Value::Str("mem:__savescr.gal".into()),
                Value::Str("save:__savescr3.gal".into()),
            ],
            &[]
        ),
        Some(Value::Bool(true))
    );
    assert!(crate::save::read_save_blob(&tmp, "__savescr3.gal").is_some());
    let _ = std::fs::remove_dir_all(&tmp);
    assert_eq!(
        vm.call_engine(Func::SETWALLPAPER, &[Value::Str("x.gal".into())], &[]),
        Some(Value::Bool(false))
    );
    assert!(
        !vm.warned
            .iter()
            .any(|w| w.contains("unimpl:func") || w.contains("SetCheck")),
        "must not emit unimpl:func for documented unsupported APIs: {:?}",
        vm.warned
    );
}

#[test]
fn index_of_menu_returns_the_matching_item_or_minus_one() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
    let data = GameData::open(&path).unwrap();
    let assets = Arc::new(Assets::new(data, 2, 64));
    let mut vm = Vm::new(
        assets,
        "ゲームメイン.lsb",
        Options {
            screen: (800, 600),
            cinema_ms: 0.0,
            text_cps: 0.0,
            lookahead: 0,
            trace: false,
            trace_conds: false,
            pick: None,
            show_text: false,
        },
    )
    .unwrap();
    vm.scene.create(
        "menu",
        Kind::Menu,
        std::collections::HashMap::from([(Prop::TEXT, Value::Str("one\r\ntwo".into()))]),
    );
    assert_eq!(
        vm.call_engine(
            Func::INDEXOFMENU,
            &[Value::Str("menu".into()), Value::Str("two".into())],
            &[]
        ),
        Some(Value::Int(1))
    );
    assert_eq!(
        vm.call_engine(
            Func::INDEXOFMENU,
            &[Value::Str("menu".into()), Value::Str("missing".into())],
            &[]
        ),
        Some(Value::Int(-1))
    );
    vm.state = State::Menu {
        name: "menu".into(),
        choices: vec!["only".into()],
        values: vec!["only".into()],
        var: String::new(),
    };
    assert_eq!(
        vm.call_engine(
            Func::INDEXOFMENU,
            &[Value::Str("menu".into()), Value::Str("only".into())],
            &[]
        ),
        Some(Value::Int(0))
    );
}

#[test]
fn wipe_flash_peaks_mid_for_white_black() {
    let f = wipe::WipeFlash::white_black(0.0, 1000.0, true);
    assert_eq!(f.rgba(0.0).map(|c| c.3), Some(0));
    assert_eq!(f.rgba(500.0).map(|c| c.3), Some(255));
    assert_eq!(f.rgba(1000.0), None);
    let flash = wipe::WipeFlash::flash(0.0, 1000.0);
    assert_eq!(flash.rgba(0.0).map(|c| c.3), Some(255));
    assert!(flash.rgba(500.0).map(|c| c.3).unwrap() < 255);
}

fn literal(value: Param) -> LiveParser {
    LiveParser {
        entries: vec![crate::lsb::OpeData {
            op: crate::lsb::op::TO,
            name: String::new(),
            func: None,
            operands: vec![value],
        }],
    }
}

fn flip_command(act: i32, time: i32, delete: bool, wipe: i32, targets: &[&str]) -> Command {
    Command {
        kind: CmdType::Flip as u8,
        indent: 0,
        mute: false,
        not_update: false,
        line_no: 0,
        fields: vec![
            ("Act", crate::lsb::Field::Expr(literal(Param::Int(act)))),
            ("Time", crate::lsb::Field::Expr(literal(Param::Int(time)))),
            (
                "Delete",
                crate::lsb::Field::Expr(literal(Param::Int(i32::from(delete)))),
            ),
            ("Wipe", crate::lsb::Field::Expr(literal(Param::Int(wipe)))),
            (
                "Targets",
                crate::lsb::Field::Exprs(
                    targets
                        .iter()
                        .map(|name| literal(Param::Str((*name).into())))
                        .collect(),
                ),
            ),
        ],
    }
}

fn flip_command_with_reverse(
    act: i32,
    time: i32,
    delete: bool,
    wipe: i32,
    targets: &[&str],
    reverse: bool,
) -> Command {
    let mut command = flip_command(act, time, delete, wipe, targets);
    command.fields.push((
        "Reverse",
        crate::lsb::Field::Expr(literal(Param::Int(i32::from(reverse)))),
    ));
    command
}

fn flip_command_with_params(
    act: i32,
    time: i32,
    delete: bool,
    wipe: i32,
    targets: &[&str],
    params: &[i32],
) -> Command {
    let mut command = flip_command(act, time, delete, wipe, targets);
    command.fields.push((
        "Param",
        crate::lsb::Field::Exprs(
            params
                .iter()
                .map(|value| literal(Param::Int(*value)))
                .collect(),
        ),
    ));
    command
}

#[test]
fn click_uses_the_image_file_when_width_and_height_are_unset() {
    let mut vm = fixture_vm();
    let mut props = HashMap::new();
    props.insert(Prop::SOURCE, Value::Str("bg.gal".into()));
    props.insert(Prop::LEFT, Value::Int(0));
    props.insert(Prop::TOP, Value::Int(0));
    props.insert(Prop::ONLBTNDOWN, Value::Str("pick".into()));
    vm.scene.create("thumb", Kind::Image, props);
    vm.click_at(1.0, 1.0, false);
    assert!(vm.queue.iter().any(|event| event.obj == "thumb"));

    vm.scene.set("thumb", Prop::IGNOREMOUSE, Value::Int(1));
    let queued = vm.queue.len();
    vm.click_at(1.0, 1.0, false);
    assert_eq!(vm.queue.len(), queued);
}

#[test]
fn button_with_ignore_mouse_4_receives_left_click_only() {
    let mut vm = fixture_vm();
    vm.scene.create(
        "dialog-ok",
        Kind::Button,
        HashMap::from([
            (Prop::LEFT, Value::Int(0)),
            (Prop::TOP, Value::Int(0)),
            (Prop::WIDTH, Value::Int(100)),
            (Prop::HEIGHT, Value::Int(40)),
            (Prop::IGNOREMOUSE, Value::Int(4)),
            (Prop::ONNOTIFY, Value::Str("dialog.lsc:ok".into())),
            (Prop::ONRBTNDOWN, Value::Str("dialog.lsc:right".into())),
        ]),
    );

    vm.click_at(10.0, 10.0, false);
    assert_eq!(vm.queue.len(), 1);
    assert_eq!(vm.queue.front().unwrap().script, "dialog.lsc:ok");

    vm.queue.clear();
    vm.click_at(10.0, 10.0, true);
    assert!(vm.queue.is_empty());
}

#[test]
fn flip_cohort_spans_yielded_frames_and_advances_on_completion() {
    let mut vm = fixture_vm();
    vm.scene.set_cohort(1);
    vm.scene.create("old", Kind::Image, HashMap::new());
    vm.scene.set_cohort(2);
    vm.scene.create("plate", Kind::Image, HashMap::new());

    vm.update(0.0, 0);
    vm.update(16.0, 0);
    assert_eq!(vm.scene.cohort(), 2);

    vm.step_flip(&flip_command(1, 0, false, 0, &[]));

    assert!(vm.scene.names().iter().any(|name| name == "plate"));
    assert!(!vm.scene.names().iter().any(|name| name == "old"));
    assert_eq!(vm.scene.cohort(), 3);
}

#[test]
fn delayed_flip_keeps_its_cohort_until_the_surface_switch() {
    let mut vm = fixture_vm();
    vm.scene.set_cohort(1);
    vm.scene.create("old", Kind::Image, HashMap::new());
    vm.scene.set_cohort(2);
    vm.scene.create("plate", Kind::Image, HashMap::new());

    vm.step_flip(&flip_command(1, 100, false, 0, &[]));
    assert_eq!(vm.scene.cohort(), 2);

    vm.finish_flip_wait();

    assert_eq!(vm.scene.cohort(), 3);
    assert!(vm.scene.names().iter().any(|name| name == "plate"));
    assert!(!vm.scene.names().iter().any(|name| name == "old"));
}

#[test]
fn stay_keeps_targets_and_delete_removes_only_the_back_surface() {
    let mut vm = fixture_vm();
    vm.scene.set_cohort(1);
    vm.scene.create("actor", Kind::Image, HashMap::new());
    vm.scene.create("background", Kind::Image, HashMap::new());
    vm.scene.set_cohort(2);
    vm.scene.create("fresh", Kind::Image, HashMap::new());
    vm.step_flip(&flip_command(1, 0, true, 0, &["actor"]));

    assert!(vm.scene.names().iter().any(|name| name == "actor"));
    assert!(vm.scene.names().iter().any(|name| name == "fresh"));
    assert!(vm.scene.get("background").is_none());
}

#[test]
fn delete_flip_removes_current_surface_objects_after_they_were_presented() {
    let mut vm = fixture_vm();
    vm.scene.set_cohort(1);
    vm.scene
        .create("recall_background", Kind::Image, HashMap::new());
    vm.scene.create("recall_menu", Kind::Menu, HashMap::new());
    vm.scene.set_cohort(2);
    vm.scene
        .create("CGサムネイル0", Kind::Image, HashMap::new());
    vm.scene
        .create("CGサムネイル1", Kind::Image, HashMap::new());
    vm.scene.mark_presented(&[
        "recall_background".into(),
        "recall_menu".into(),
        "CGサムネイル0".into(),
        "CGサムネイル1".into(),
    ]);

    vm.step_flip(&flip_command(1, 0, true, 0, &[]));

    assert!(vm.scene.get("recall_background").is_none());
    assert!(vm.scene.get("recall_menu").is_none());
    assert!(vm.scene.get("CGサムネイル0").is_none());
    assert!(vm.scene.get("CGサムネイル1").is_none());
}

#[test]
fn presented_tracking_preserves_message_and_control_menu_tree() {
    let mut vm = fixture_vm();
    vm.scene.create("message-root", Kind::Box, HashMap::new());
    vm.scene.create(
        "message",
        Kind::Message,
        HashMap::from([(Prop::PARENT, Value::Str("message-root".into()))]),
    );
    vm.scene.create(
        "message-controls",
        Kind::Menu,
        HashMap::from([(Prop::PARENT, Value::Str("message-root".into()))]),
    );
    vm.scene
        .create("scene-recall-menu", Kind::Menu, HashMap::new());
    vm.scene.mark_presented(&[
        "message-root".into(),
        "message".into(),
        "message-controls".into(),
        "scene-recall-menu".into(),
    ]);

    assert!(!vm.scene.was_presented("message-root"));
    assert!(!vm.scene.was_presented("message"));
    assert!(!vm.scene.was_presented("message-controls"));
    assert!(vm.scene.was_presented("scene-recall-menu"));
}

#[test]
fn targeted_flip_restores_archived_message_ui_without_deleting_it() {
    let mut vm = fixture_vm();
    vm.scene.set_cohort(1);
    vm.scene.create("message-root", Kind::Box, HashMap::new());
    vm.scene.create(
        "message",
        Kind::Message,
        HashMap::from([(Prop::PARENT, Value::Str("message-root".into()))]),
    );
    vm.scene.create(
        "message-controls",
        Kind::Menu,
        HashMap::from([(Prop::PARENT, Value::Str("message-root".into()))]),
    );
    vm.scene
        .create("selection-menu", Kind::Menu, HashMap::new());
    vm.scene.mark_back("message-root");
    vm.scene.set_cohort(2);

    vm.step_flip(&flip_command(0, 0, true, 0, &["selection-menu"]));

    assert!(vm.scene.get("selection-menu").is_none());
    for name in ["message-root", "message", "message-controls"] {
        assert!(vm.scene.get(name).is_some(), "{name} must survive the flip");
        assert!(
            !vm.scene.on_back(name),
            "{name} must be restored to the active surface"
        );
    }
}

#[test]
fn presented_surface_tracking_resets_after_each_flip_in_a_scene_chain() {
    let mut vm = fixture_vm();
    vm.scene.set_cohort(1);
    vm.scene.create("scene-a", Kind::Image, HashMap::new());
    vm.scene.mark_presented(&["scene-a".into()]);

    vm.scene.set_cohort(2);
    vm.scene.create("scene-b", Kind::Image, HashMap::new());
    vm.step_flip(&flip_command(1, 0, true, 0, &[]));
    assert!(vm.scene.get("scene-a").is_none());
    assert!(vm.scene.get("scene-b").is_some());
    assert!(!vm.scene.was_presented("scene-b"));

    vm.scene.mark_presented(&["scene-b".into()]);
    vm.scene.set_cohort(3);
    vm.scene.create("scene-c", Kind::Image, HashMap::new());
    vm.step_flip(&flip_command(1, 0, true, 0, &[]));
    assert!(vm.scene.get("scene-b").is_none());
    assert!(vm.scene.get("scene-c").is_some());
    assert!(!vm.scene.was_presented("scene-c"));

    vm.scene.mark_presented(&["scene-c".into()]);
    vm.scene.set_cohort(4);
    vm.scene.create("scene-d", Kind::Image, HashMap::new());
    vm.step_flip(&flip_command(1, 0, true, 0, &[]));
    assert!(vm.scene.get("scene-c").is_none());
    assert!(vm.scene.get("scene-d").is_some());
    assert!(!vm.scene.was_presented("scene-d"));
}

#[test]
fn restored_cabinet_objects_start_a_new_presented_surface() {
    let mut vm = fixture_vm();
    vm.scene.create("scene-a", Kind::Image, HashMap::new());
    vm.scene.mark_presented(&["scene-a".into()]);

    let saved = vm.scene.take("scene-a").unwrap();
    vm.scene.put(saved);

    assert!(!vm.scene.was_presented("scene-a"));
}

#[test]
fn out_moves_only_targets_and_reverse_does_not_change_surface_membership() {
    let mut vm = fixture_vm();
    vm.scene.set_cohort(1);
    vm.scene.create("actor", Kind::Image, HashMap::new());
    vm.scene.create("background", Kind::Image, HashMap::new());
    vm.scene.set_cohort(2);
    vm.scene.create("fresh", Kind::Image, HashMap::new());

    vm.step_flip(&flip_command_with_reverse(0, 0, false, 1, &["actor"], true));

    assert!(!vm.scene.names().iter().any(|name| name == "actor"));
    assert!(vm.scene.names().iter().any(|name| name == "background"));
    assert!(vm.scene.names().iter().any(|name| name == "fresh"));
}

#[test]
fn out_without_targets_restores_the_back_surface_and_keeps_the_front() {
    let mut vm = fixture_vm();
    vm.scene.set_cohort(1);
    vm.scene.create("old", Kind::Image, HashMap::new());
    vm.scene.set_cohort(2);
    vm.scene.create("current", Kind::Image, HashMap::new());
    vm.step_flip(&flip_command(1, 0, false, 0, &[]));

    vm.step_flip(&flip_command(0, 0, false, 0, &[]));

    assert!(vm.scene.names().iter().any(|name| name == "old"));
    assert!(vm.scene.names().iter().any(|name| name == "current"));
}

#[test]
fn enter_fade_lasts_the_whole_flip_time() {
    let mut vm = fixture_vm();
    vm.now = 1000.0;
    vm.scene.set_cohort(1);
    vm.scene.create("title", Kind::Image, HashMap::new());
    vm.scene.set_cohort(2);
    vm.scene.create("gallery", Kind::Image, HashMap::new());
    vm.scene.set_cohort(2);
    vm.step_flip(&flip_command(1, 500, true, 1, &[]));
    let fade = vm.wipe_fade.as_ref().unwrap();
    assert_eq!(fade.dur_ms, 500.0);
    assert!(fade.out.contains("title"));
    assert!(fade.inn.contains("gallery"));
    assert!(vm.scene.motions.is_empty());
    assert_eq!(vm.scene.get("gallery").unwrap().int(Prop::ALPHA, 255), 255);
    assert_eq!(vm.scene.get("title").unwrap().int(Prop::ALPHA, 255), 255);
}

#[test]
fn blind_flip_reveals_new_surface_in_strips_and_syncs_to_savescreen() {
    let mut vm = fixture_vm();
    vm.scene.set_cohort(1);
    vm.scene.create("old", Kind::Image, HashMap::new());
    vm.scene.set_cohort(2);
    vm.scene.create("plate", Kind::Image, HashMap::new());

    vm.step_flip(&flip_command_with_params(1, 200, false, 2, &[], &[20, 1]));
    let fade = vm.wipe_fade.as_ref().unwrap();
    assert!(matches!(
        fade.kind,
        super::wipe::WipeKind::Blind {
            horizontal: false,
            span: 20,
            appear_ms: 1.0,
            reverse: false,
        }
    ));
    assert_eq!(vm.wipe_alpha_factor("plate"), None);
    assert!(vm.wipe_blind_strips("plate", 800, 600).unwrap().is_empty());

    vm.now = 100.0;
    let strips = vm.wipe_blind_strips("plate", 800, 600).unwrap();
    assert_eq!(strips.len(), 40);
    assert!(strips.iter().all(|strip| strip.w <= 20 && strip.h == 600));

    let request = SaveScreenRequest {
        path: "mem:__wipe_capture.gal".into(),
        resample_w: 0,
        resample_h: 0,
    };
    assert!(vm.complete_savescreen(&request, 1, 1, vec![255, 0, 0, 255],));
    vm.update(120.0, 0);
    assert_eq!(vm.wipe_fade.as_ref().unwrap().started_at, 120.0);
}

#[test]
fn blindv_flip_reveals_horizontal_strips() {
    let mut vm = fixture_vm();
    vm.scene.set_cohort(1);
    vm.scene.create("old", Kind::Image, HashMap::new());
    vm.scene.set_cohort(2);
    vm.scene.create("plate", Kind::Image, HashMap::new());

    vm.step_flip(&flip_command_with_params(1, 200, false, 3, &[], &[20, 1]));
    let fade = vm.wipe_fade.as_ref().unwrap();
    assert!(matches!(
        fade.kind,
        super::wipe::WipeKind::Blind {
            horizontal: true,
            span: 20,
            appear_ms: 1.0,
            reverse: false,
        }
    ));

    vm.now = 100.0;
    let strips = vm.wipe_blind_strips("plate", 800, 600).unwrap();
    assert_eq!(strips.len(), 30);
    assert!(strips.iter().all(|strip| strip.w == 800 && strip.h <= 20));
}

#[test]
fn blindv_exit_clips_the_outgoing_menu_while_restoring_the_plate() {
    let mut vm = fixture_vm();
    vm.scene.set_cohort(1);
    vm.scene.create("saved_scene", Kind::Image, HashMap::new());
    vm.scene.set_cohort(2);
    vm.scene.create("menu_plate", Kind::Image, HashMap::new());

    vm.step_flip(&flip_command_with_params(1, 200, false, 2, &[], &[20, 1]));
    vm.finish_flip_wait();

    vm.step_flip(&flip_command_with_params(
        0,
        200,
        true,
        3,
        &["menu_plate"],
        &[20, 1],
    ));
    let fade = vm.wipe_fade.as_ref().unwrap();
    assert!(fade.out.contains("menu_plate"));
    assert!(fade.inn.contains("saved_scene"));

    assert_eq!(
        vm.wipe_blind_strips("menu_plate", 800, 600).unwrap().len(),
        30
    );
    vm.now = 100.0;
    let outgoing = vm.wipe_blind_strips("menu_plate", 800, 600).unwrap();
    let incoming = vm.wipe_blind_strips("saved_scene", 800, 600).unwrap();
    assert_eq!(outgoing.len(), 30);
    assert_eq!(incoming.len(), 30);
    assert!(outgoing.iter().all(|strip| strip.h < 20));
    assert!(incoming.iter().all(|strip| strip.h < 20));
    let area = |strips: &[super::wipe::Strip]| {
        strips
            .iter()
            .map(|strip| i64::from(strip.w) * i64::from(strip.h))
            .sum::<i64>()
    };
    assert_eq!(area(&outgoing) + area(&incoming), 800 * 600);

    vm.now = 200.0;
    assert!(
        vm.wipe_blind_strips("menu_plate", 800, 600)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        vm.wipe_blind_strips("saved_scene", 800, 600).unwrap().len(),
        30
    );
}

#[test]
fn blind_flip_clock_pauses_while_scene_assets_are_not_ready() {
    let mut vm = fixture_vm();
    vm.scene.set_cohort(1);
    vm.scene.create("old", Kind::Image, HashMap::new());
    vm.scene.set_cohort(2);
    vm.scene.create("plate", Kind::Image, HashMap::new());
    vm.step_flip(&flip_command_with_params(1, 200, false, 3, &[], &[20, 1]));
    vm.hold_wipe_until_scene_ready();

    vm.update(100.0, 0);
    assert_eq!(vm.wipe_fade.as_ref().unwrap().started_at, 100.0);
    assert!(matches!(vm.state, State::WaitUntil { until: 300.0, .. }));
    assert!(vm.wipe_blind_strips("plate", 800, 600).unwrap().is_empty());

    vm.hold_wipe_until_scene_ready();
    vm.update(150.0, 0);
    assert_eq!(vm.wipe_fade.as_ref().unwrap().started_at, 150.0);
    assert!(matches!(vm.state, State::WaitUntil { until: 350.0, .. }));

    vm.update(350.0, 0);
    assert!(vm.wipe_fade.is_none());
    assert!(matches!(vm.state, State::Running));
}

#[test]
fn delayed_flip_delete_preserves_children_created_after_its_plan() {
    let mut vm = fixture_vm();
    vm.scene.set_cohort(1);
    vm.scene.create("old_plate", Kind::Image, HashMap::new());
    let mut old_child_props = HashMap::new();
    old_child_props.insert(Prop::PARENT, Value::Str("old_plate".into()));
    vm.scene.create("old_child", Kind::Image, old_child_props);
    vm.scene.mark_back("old_plate");
    vm.scene.set_cohort(2);

    vm.step_flip(&flip_command(0, 100, true, 0, &[]));
    assert!(
        vm.flip_plan
            .as_ref()
            .is_some_and(|plan| plan.scrap.iter().any(|name| name == "old_plate"))
    );

    let mut menu_props = HashMap::new();
    menu_props.insert(Prop::PARENT, Value::Str("old_plate".into()));
    menu_props.insert(Prop::LEFT, Value::Int(20));
    menu_props.insert(Prop::TOP, Value::Int(30));
    vm.scene.create("menu", Kind::Menu, menu_props);
    let mut slider_props = HashMap::new();
    slider_props.insert(Prop::PARENT, Value::Str("menu".into()));
    slider_props.insert(Prop::LEFT, Value::Int(5));
    slider_props.insert(Prop::TOP, Value::Int(7));
    vm.scene.create("slider", Kind::Slider, slider_props);

    vm.finish_flip_wait();

    let menu = vm.scene.get("menu").expect("new menu survives Flip Delete");
    assert_eq!(menu.str(Prop::PARENT), "");
    assert_eq!(menu.int(Prop::LEFT, 0), 20);
    assert_eq!(menu.int(Prop::TOP, 0), 30);
    assert_eq!(
        vm.scene.abs_pos(vm.scene.get("slider").unwrap()),
        (25.0, 37.0)
    );
    assert!(vm.scene.get("old_plate").is_none());
    assert!(vm.scene.get("old_child").is_none());
}

#[test]
fn flip_delete_keeps_fresh_descendants_of_an_archived_surface() {
    let mut vm = fixture_vm();
    vm.scene.set_cohort(1);
    vm.scene.create("old_plate", Kind::Image, HashMap::new());
    vm.scene.mark_back("old_plate");
    vm.scene.set_cohort(2);
    let mut menu_props = HashMap::new();
    menu_props.insert(Prop::PARENT, Value::Str("old_plate".into()));
    vm.scene.create("menu", Kind::Menu, menu_props);
    let mut slider_props = HashMap::new();
    slider_props.insert(Prop::PARENT, Value::Str("menu".into()));
    vm.scene.create("slider", Kind::Slider, slider_props);

    vm.step_flip(&flip_command(0, 100, true, 0, &[]));
    let plan = vm.flip_plan.as_ref().unwrap();
    assert!(plan.fade_in.iter().any(|name| name == "menu"));
    assert!(plan.fade_in.iter().any(|name| name == "slider"));
    assert!(!plan.scrap.iter().any(|name| name == "menu"));
    assert!(!plan.scrap.iter().any(|name| name == "slider"));

    vm.finish_flip_wait();

    assert!(vm.scene.get("menu").is_some());
    assert!(vm.scene.get("slider").is_some());
    assert!(vm.scene.names().iter().any(|name| name == "menu"));
    assert!(vm.scene.names().iter().any(|name| name == "slider"));
    assert!(vm.scene.get("old_plate").is_none());
}

#[test]
fn cabinet_saves_only_back_surface_and_loads_it_back() {
    let mut vm = fixture_vm();
    vm.scene.create("front_menu", Kind::Menu, HashMap::new());
    vm.scene.create("front_plate", Kind::Image, HashMap::new());
    vm.scene.create("back_plate", Kind::Image, HashMap::new());
    vm.scene.create(
        "back_child",
        Kind::Image,
        HashMap::from([(Prop::PARENT, Value::Str("back_plate".into()))]),
    );
    vm.scene.mark_back("back_plate");

    let cabinet_command = |kind| Command {
        kind: kind as u8,
        indent: 0,
        mute: false,
        not_update: false,
        line_no: 0,
        fields: vec![
            (
                "components",
                crate::lsb::Field::Props(vec![(Prop::NAME, literal(Param::Str("system".into())))]),
            ),
            ("Act", crate::lsb::Field::Expr(literal(Param::Flag(true)))),
            ("Targets", crate::lsb::Field::Exprs(vec![])),
        ],
    };

    vm.step_cabinet(CmdType::SaveCabinet, &cabinet_command(CmdType::SaveCabinet));
    assert!(vm.scene.get("front_menu").is_some());
    assert!(vm.scene.get("front_plate").is_some());
    assert!(vm.scene.get("back_plate").is_none());
    assert!(vm.scene.get("back_child").is_none());
    assert!(
        vm.cabinets["system"]
            .iter()
            .any(|object| object.name == "back_plate")
    );
    assert!(
        vm.cabinets["system"]
            .iter()
            .any(|object| object.name == "back_child")
    );
    assert!(
        !vm.cabinets["system"]
            .iter()
            .any(|object| object.name == "front_menu" || object.name == "front_plate")
    );

    vm.step_cabinet(CmdType::LoadCabinet, &cabinet_command(CmdType::LoadCabinet));
    assert!(vm.scene.get("front_menu").is_some());
    assert!(vm.scene.get("front_plate").is_some());
    assert!(vm.scene.get("back_plate").is_some());
    assert!(vm.scene.get("back_child").is_some());
    assert!(
        vm.scene
            .back_names()
            .iter()
            .any(|name| name == "back_plate")
    );
    assert!(
        vm.scene
            .back_names()
            .iter()
            .any(|name| name == "back_child")
    );
    assert!(!vm.scene.names().iter().any(|name| name == "back_plate"));
    assert!(!vm.scene.names().iter().any(|name| name == "back_child"));
}

#[test]
fn named_flip_only_fades_or_deletes_its_targets() {
    let mut vm = fixture_vm();
    vm.scene.set_cohort(1);
    vm.scene.create("actor", Kind::Image, HashMap::new());
    let mut child = HashMap::new();
    child.insert(Prop::PARENT, Value::Str("actor".into()));
    vm.scene.create("face", Kind::Image, child);
    vm.scene.create("bg", Kind::Image, HashMap::new());
    vm.scene.set_cohort(2);
    vm.step_flip(&flip_command(0, 0, true, 0, &["actor"]));
    assert!(vm.scene.get("actor").is_none());
    assert!(vm.scene.get("face").is_none());
    assert!(vm.scene.names().iter().any(|name| name == "bg"));

    vm.scene.create("actor", Kind::Image, HashMap::new());
    vm.scene.set_cohort(4);
    vm.step_flip(&flip_command(0, 0, false, 0, &["actor"]));
    assert!(vm.scene.get("actor").is_some());
    assert!(!vm.scene.names().iter().any(|name| name == "actor"));
    assert_eq!(vm.scene.get("actor").unwrap().int(Prop::ALPHA, 255), 255);
}

/// MESOFF: Flip FL_OUT con targets → `back`. 再生中 solo SetProp VISIBLE=1
/// (sin Flip de entrada); debe desarchivar para que el cuadro vuelva a dibujarse.
#[test]
fn setprop_visible_true_unmarks_flip_back_surface() {
    let mut vm = fixture_vm();
    vm.scene.set_cohort(1);
    vm.scene.create("土台", Kind::Image, HashMap::new());
    vm.scene.set_cohort(2);
    vm.step_flip(&flip_command(0, 0, false, 0, &["土台"]));
    assert!(!vm.scene.names().iter().any(|name| name == "土台"));
    let cmd = Command {
        kind: CmdType::SetProp as u8,
        indent: 0,
        mute: false,
        not_update: false,
        line_no: 0,
        fields: vec![
            (
                "ObjName",
                crate::lsb::Field::Expr(literal(Param::Str("土台".into()))),
            ),
            ("ObjProp", crate::lsb::Field::Expr(literal(Param::Int(47)))),
            ("Value", crate::lsb::Field::Expr(literal(Param::Int(1)))),
        ],
    };
    let script = Lsb {
        name: "setprop.lsb".into(),
        version: 117,
        flags: 0,
        command_params: vec![],
        commands: vec![],
    };
    vm.step_set_prop(&script, 0, &cmd);
    assert!(vm.scene.names().iter().any(|name| name == "土台"));
}

/// FACE Flip archiva 土台 y メッセージボックス; 再生中 solo VISIBLE en el 土台.
#[test]
fn setprop_visible_unmarks_back_descendants() {
    let mut vm = fixture_vm();
    vm.scene.set_cohort(1);
    vm.scene.create("土台", Kind::Box, HashMap::new());
    let mut child = HashMap::new();
    child.insert(Prop::PARENT, Value::Str("土台".into()));
    vm.scene.create("メッセージボックス", Kind::Message, child);
    vm.scene.set_cohort(2);
    vm.step_flip(&flip_command(
        0,
        0,
        false,
        0,
        &["土台", "メッセージボックス"],
    ));
    assert!(!vm.scene.names().iter().any(|name| name == "土台"));
    assert!(
        !vm.scene
            .names()
            .iter()
            .any(|name| name == "メッセージボックス")
    );
    let cmd = Command {
        kind: CmdType::SetProp as u8,
        indent: 0,
        mute: false,
        not_update: false,
        line_no: 0,
        fields: vec![
            (
                "ObjName",
                crate::lsb::Field::Expr(literal(Param::Str("土台".into()))),
            ),
            ("ObjProp", crate::lsb::Field::Expr(literal(Param::Int(47)))),
            ("Value", crate::lsb::Field::Expr(literal(Param::Int(1)))),
        ],
    };
    let script = Lsb {
        name: "setprop.lsb".into(),
        version: 117,
        flags: 0,
        command_params: vec![],
        commands: vec![],
    };
    vm.step_set_prop(&script, 0, &cmd);
    assert!(vm.scene.names().iter().any(|name| name == "土台"));
    assert!(
        vm.scene
            .names()
            .iter()
            .any(|name| name == "メッセージボックス")
    );
}

#[test]
fn image_components_can_query_source_dimensions_while_their_props_are_evaluated() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
    let data = GameData::open(&path).unwrap();
    let assets = Arc::new(Assets::new(data, 2, 64));
    let mut vm = Vm::new(
        assets.clone(),
        "ゲームメイン.lsb",
        Options {
            screen: (800, 600),
            cinema_ms: 1000.0,
            text_cps: 30.0,
            lookahead: 0,
            trace: false,
            trace_conds: false,
            pick: None,
            show_text: false,
        },
    )
    .unwrap();
    let literal = |value: Param| LiveParser {
        entries: vec![crate::lsb::OpeData {
            op: crate::lsb::op::TO,
            name: String::new(),
            func: None,
            operands: vec![value],
        }],
    };
    let get_prop = |name: &str, property| LiveParser {
        entries: vec![crate::lsb::OpeData {
            op: crate::lsb::op::FUNC,
            name: String::new(),
            func: Some(Func::GETPROP),
            operands: vec![Param::Str(name.into()), Param::Int(property)],
        }],
    };

    let expected = match assets.get("bg.gal") {
        crate::assets::Asset::Image(image) => (image.w, image.h),
        _ => panic!("fixture bg.gal must be an image"),
    };
    for (kind, name) in [(CmdType::ImgNew, "image"), (CmdType::TileNew, "tile")] {
        let command = Command {
            kind: kind as u8,
            indent: 0,
            mute: false,
            not_update: false,
            line_no: 0,
            fields: vec![(
                "components",
                crate::lsb::Field::Props(vec![
                    (Prop::NAME, literal(Param::Str(name.into()))),
                    (Prop::SOURCE, literal(Param::Str("bg.gal".into()))),
                    (Prop::WIDTH, get_prop(name, 90)),
                    (Prop::HEIGHT, get_prop(name, 91)),
                ]),
            )],
        };
        let props = vm.props_of(&command);
        assert_eq!(props[&Prop::WIDTH].as_int(), i64::from(expected.0));
        assert_eq!(props[&Prop::HEIGHT].as_int(), i64::from(expected.1));
    }
}

#[test]
fn translates_component_captions_and_buttons_without_translating_edit_data() {
    let translations = HashMap::from([
        (
            crate::text_export::text_item_id("姉の職業は？（15文字まで）"),
            "¿Cuál es la profesión de tu hermana? (máx. 15 caracteres)".into(),
        ),
        (
            crate::text_export::text_item_id("姉の職業"),
            "Profesión de tu hermana".into(),
        ),
        (crate::text_export::text_item_id("OK"), "Aceptar".into()),
        (
            crate::text_export::text_item_id("キャンセル"),
            "Cancelar".into(),
        ),
    ]);

    assert_eq!(
        translate_component_text(
            CmdType::Caption,
            "姉の職業は？（15文字まで）",
            &translations
        ),
        Some("¿Cuál es la profesión de tu hermana? (máx. 15 caracteres)".into())
    );
    assert_eq!(
        translate_component_text(CmdType::Button, "OK", &translations),
        Some("Aceptar".into())
    );
    assert_eq!(
        translate_component_text(CmdType::Button, "キャンセル", &translations),
        Some("Cancelar".into())
    );
    assert_eq!(
        translate_component_text(CmdType::EditNew, "texto del usuario", &translations),
        None
    );
}

#[test]
fn game_level_variable_scopes_survive_script_returns() {
    assert!(variable_scope_is_persistent(0));
    assert!(variable_scope_is_persistent(3));
    assert!(!variable_scope_is_persistent(1));
    assert!(!variable_scope_is_persistent(2));
}

#[test]
fn reset_keeps_status_globals_and_clears_components() {
    // sequence.html「リセット」: componentes + vars (salvo status) + stack → Page.
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
    let data = GameData::open(&path).unwrap();
    let assets = Arc::new(Assets::new(data, 2, 64));
    let mut vm = Vm::new(
        assets,
        "ゲームメイン.lsb",
        Options {
            screen: (800, 600),
            cinema_ms: 0.0,
            text_cps: 0.0,
            lookahead: 0,
            trace: false,
            trace_conds: false,
            pick: None,
            show_text: false,
        },
    )
    .unwrap();
    vm.scene
        .create("tmp", crate::scene::Kind::Box, HashMap::new());
    vm.globals.insert("game_flag".into(), Value::Int(1));
    vm.global_scopes.insert("game_flag".into(), 0);
    vm.globals.insert("status_flag".into(), Value::Int(9));
    vm.global_scopes.insert("status_flag".into(), 3);
    let entry = vm.frames[0].script.name.clone();
    let mut detail = String::new();
    let cmd = Command {
        kind: CmdType::Reset as u8,
        indent: 0,
        mute: false,
        not_update: false,
        line_no: 0,
        fields: vec![
            (
                "Page",
                crate::lsb::Field::Page(crate::lsb::LabelRef {
                    page: entry,
                    label: 0,
                }),
            ),
            ("AllClear", crate::lsb::Field::Byte(1)),
        ],
    };
    assert!(vm.step_reset(&cmd, &mut detail));
    assert!(vm.scene.get("tmp").is_none());
    assert!(!vm.globals.contains_key("game_flag"));
    assert_eq!(vm.globals.get("status_flag"), Some(&Value::Int(9)));
    assert_eq!(vm.frames.len(), 1);
}

#[test]
fn pc_reset_all_clear_drops_call_stack() {
    // sequence.html「PCリセット」AllClear ON: stack vacío tras el salto.
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
    let data = GameData::open(&path).unwrap();
    let assets = Arc::new(Assets::new(data, 2, 64));
    let mut vm = Vm::new(
        assets,
        "ゲームメイン.lsb",
        Options {
            screen: (800, 600),
            cinema_ms: 0.0,
            text_cps: 0.0,
            lookahead: 0,
            trace: false,
            trace_conds: false,
            pick: None,
            show_text: false,
        },
    )
    .unwrap();
    let script = vm.frames[0].script.clone();
    let entry = script.name.clone();
    vm.frames.push(Frame {
        script,
        pc: 3,
        locals: HashMap::from([("temp".into(), Value::Int(1))]),
        result_var: String::new(),
        params: vec![],
        saved_state: None,
        sender: None,
        event_prop: None,
        auto_scrap_sender: false,
    });
    assert_eq!(vm.frames.len(), 2);
    let mut detail = String::new();
    let cmd = Command {
        kind: CmdType::PCReset as u8,
        indent: 0,
        mute: false,
        not_update: false,
        line_no: 0,
        fields: vec![
            (
                "Page",
                crate::lsb::Field::Page(crate::lsb::LabelRef {
                    page: entry,
                    label: 0,
                }),
            ),
            ("AllClear", crate::lsb::Field::Byte(1)),
        ],
    };
    assert!(vm.step_pc_reset(&cmd, &mut detail));
    assert_eq!(vm.frames.len(), 1);
    assert!(vm.frames[0].locals.is_empty());
}

#[test]
fn game_audio_volume_uses_thousand_point_scale() {
    assert_eq!(volume_gain(0.0), 0.0);
    assert_eq!(volume_gain(500.0), 0.5);
    assert_eq!(volume_gain(1000.0), 1.0);
    assert_eq!(volume_gain(1500.0), 1.0);
}

#[test]
fn timer_uses_onplayend_and_respects_repeat_pause_and_deletion() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
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
    vm.scene.create(
        "timer",
        Kind::Other(CmdType::Timer as u8),
        HashMap::from([
            (Prop::DELAY, Value::Int(50)),
            (Prop::REPEAT, Value::Bool(true)),
            (Prop::PAUSED, Value::Bool(false)),
            (Prop::ONPLAYEND, Value::Str("timer-tick.lsc".into())),
            (Prop::ONNOTIFY, Value::Str("wrong-event.lsc".into())),
        ]),
    );
    vm.now = 100.0;
    vm.timers.insert(
        "timer".into(),
        TimerState {
            due_at: 100.0,
            paused_at: None,
        },
    );

    vm.update_timers();

    assert_eq!(vm.queue.len(), 1);
    assert_eq!(vm.queue[0].prop, Prop::ONPLAYEND);
    assert_eq!(vm.queue[0].script, "timer-tick.lsc");
    assert_eq!(vm.timers.get("timer").unwrap().due_at, 150.0);

    vm.scene.set("timer", Prop::PAUSED, Value::Bool(true));
    vm.now = 120.0;
    vm.update_timers();
    assert_eq!(vm.timers.get("timer").unwrap().paused_at, Some(120.0));

    vm.now = 200.0;
    vm.update_timers();
    assert_eq!(vm.queue.len(), 1);
    assert_eq!(vm.timers.get("timer").unwrap().due_at, 150.0);

    vm.scene.set("timer", Prop::PAUSED, Value::Bool(false));
    vm.now = 300.0;
    vm.update_timers();
    assert_eq!(vm.queue.len(), 1);
    assert_eq!(vm.timers.get("timer").unwrap().due_at, 330.0);

    vm.now = 329.0;
    vm.update_timers();
    assert_eq!(vm.queue.len(), 1);

    vm.now = 330.0;
    vm.update_timers();
    assert_eq!(vm.queue.len(), 2);
    assert_eq!(vm.timers.get("timer").unwrap().due_at, 380.0);

    vm.enabled
        .insert(("timer".into(), MenuItemKey::Name("choice".into())), false);
    vm.visible
        .insert(("timer".into(), MenuItemKey::Index(1)), false);
    vm.selected
        .insert(("timer".into(), MenuItemKey::Index(1)), true);
    vm.scrap_component("timer");
    assert!(vm.timers.is_empty());
    assert!(vm.enabled.is_empty());
    assert!(vm.visible.is_empty());
    assert!(vm.selected.is_empty());

    vm.now = 400.0;
    vm.update_timers();
    assert!(vm.timers.is_empty());
    assert_eq!(vm.queue.len(), 2);
}

#[test]
fn timer_falls_back_to_onnotify_when_onplayend_is_absent() {
    // LiveNovel: un Timer puede definir solo ONNOTIFY.
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
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
    vm.scene.create(
        "tick-timer",
        Kind::Other(CmdType::Timer as u8),
        HashMap::from([
            (Prop::DELAY, Value::Int(0)),
            (Prop::ONNOTIFY, Value::Str("00000001.lsc:00000010".into())),
        ]),
    );
    vm.now = 10.0;
    vm.timers.insert(
        "tick-timer".into(),
        TimerState {
            due_at: 10.0,
            paused_at: None,
        },
    );

    vm.update_timers();

    assert_eq!(vm.queue.len(), 1);
    assert_eq!(vm.queue[0].prop, Prop::ONNOTIFY);
    assert_eq!(vm.queue[0].script, "00000001.lsc:00000010");
    assert!(vm.timers.is_empty());
}

#[test]
fn system_se_volume_uses_game_setting_or_default() {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
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

    assert_eq!(vm.system_var("StatusSEVolume"), Some(Value::Int(1000)));
    vm.globals.insert("SE音量".into(), Value::Int(425));
    assert_eq!(vm.system_var("StatusSEVolume"), Some(Value::Int(425)));
}

#[test]
fn nested_calls_resolve_nearest_caller_local_variable() {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
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
    vm.frames[0].locals.insert("SE音量".into(), Value::Int(80));
    let script = Arc::clone(&vm.frames[0].script);
    vm.frames.push(Frame {
        script,
        pc: 0,
        locals: HashMap::new(),
        result_var: String::new(),
        params: vec![],
        saved_state: None,
        sender: None,
        event_prop: None,
        auto_scrap_sender: false,
    });

    assert_eq!(vm.lookup("SE音量"), Some(&Value::Int(80)));
    vm.assign("SE音量", Value::Int(60));
    assert_eq!(vm.frames[0].locals.get("SE音量"), Some(&Value::Int(60)));

    vm.frames[1].locals.insert("SE音量".into(), Value::Int(20));
    assert_eq!(vm.lookup("SE音量"), Some(&Value::Int(20)));
    vm.assign("SE音量", Value::Int(10));
    assert_eq!(vm.frames[1].locals.get("SE音量"), Some(&Value::Int(10)));
}

#[test]
fn prevmenu_click_updates_the_selected_string_system_variable() {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
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
    vm.scene.create("scene-menu", Kind::Menu, HashMap::new());
    vm.prevmenus.insert(
        "scene-menu".into(),
        PrevMenu {
            buttons: vec![PmButton {
                x: 0,
                y: 0,
                w: 40,
                h: 40,
                src: String::new(),
                src_selected: String::new(),
                src_disabled: String::new(),
                name: "scene-a".into(),
                group: 0,
            }],
            hover: None,
        },
    );

    vm.click_at(10.0, 10.0, false);

    assert_eq!(vm.lookup("@SelStr"), Some(&Value::Str("scene-a".into())));
    assert_eq!(
        vm.call_engine(
            Func::GETSELECTED,
            &[
                Value::Str("scene-menu".into()),
                Value::Str("scene-a".into()),
            ],
            &[],
        ),
        Some(Value::Bool(true))
    );
    assert_eq!(
        vm.call_engine(
            Func::GETSELECTED,
            &[Value::Str("scene-menu".into()), Value::Str("0".into()),],
            &[],
        ),
        Some(Value::Bool(true))
    );
}

#[test]
fn editable_component_accepts_unicode_and_keyboard_editing() {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
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
        "edit",
        Kind::Edit,
        HashMap::from([
            (Prop::LEFT, Value::Int(10)),
            (Prop::TOP, Value::Int(10)),
            (Prop::WIDTH, Value::Int(200)),
            (Prop::HEIGHT, Value::Int(24)),
            (Prop::TEXT, Value::Str("猫".into())),
            (Prop::ONNOTIFY, Value::Str("ゲームメイン.lsb".into())),
        ]),
    );

    vm.assign("@ActiveCompo", Value::Str("edit".into()));
    assert_eq!(
        vm.system_var("@ActiveCompo"),
        Some(Value::Str("edit".into()))
    );
    assert!(vm.focus_edit_at(12.0, 12.0));
    assert!(vm.active_edit_name().is_some());
    assert!(vm.edit_key(37));
    assert!(vm.insert_edit_text("犬"));
    assert_eq!(vm.get_prop("edit", Prop::TEXT), Value::Str("犬猫".into()));
    assert!(vm.edit_key(8));
    assert_eq!(vm.get_prop("edit", Prop::TEXT), Value::Str("猫".into()));
    assert!(
        vm.queue
            .iter()
            .any(|event| event.obj == "edit" && event.params == vec![Value::Int(37)])
    );
}

#[test]
fn explicitly_active_invisible_edit_accepts_text_and_respects_hidden_parent() {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
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
    vm.scene.create("form", Kind::Box, HashMap::new());
    vm.scene.create(
        "edit",
        Kind::Edit,
        HashMap::from([
            (Prop::PARENT, Value::Str("form".into())),
            (Prop::VISIBLE, Value::Bool(false)),
            (Prop::TEXT, Value::Str(String::new())),
            (Prop::WIDTH, Value::Int(120)),
            (Prop::HEIGHT, Value::Int(16)),
        ]),
    );

    vm.assign("@ActiveCompo", Value::Str("edit".into()));
    assert_eq!(vm.active_edit_name(), Some("edit"));
    assert!(vm.insert_edit_text("entrada"));
    assert_eq!(
        vm.get_prop("edit", Prop::TEXT),
        Value::Str("entrada".into())
    );

    vm.scene.set("form", Prop::VISIBLE, Value::Bool(false));
    assert_eq!(vm.active_edit_name(), None);
}

#[test]
fn right_click_scraps_modal_menu_when_script_property_allows_it() {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
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
        "menu",
        Kind::Menu,
        HashMap::from([(Prop::RCLICKSCRAP, Value::Bool(true))]),
    );
    vm.state = State::Menu {
        name: "menu".into(),
        choices: vec!["choice".into()],
        values: vec!["choice".into()],
        var: String::new(),
    };

    vm.click_at(-1.0, -1.0, true);

    assert!(matches!(vm.state, State::Running));
    assert!(vm.scene.get("menu").is_none());
}

#[test]
fn right_click_scraps_after_menu_handler_finishes() {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
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
        "menu",
        Kind::Menu,
        HashMap::from([
            (Prop::RCLICKSCRAP, Value::Bool(true)),
            (Prop::ONRBTNUP, Value::Str("ゲームメイン.lsb".into())),
        ]),
    );
    vm.state = State::Menu {
        name: "menu".into(),
        choices: vec!["choice".into()],
        values: vec!["choice".into()],
        var: String::new(),
    };

    vm.click_at(-1.0, -1.0, true);
    assert!(vm.scene.get("menu").is_some());
    vm.dispatch_events();
    assert!(vm.frames.last().unwrap().auto_scrap_sender);
    vm.do_return(Value::Empty);

    assert!(vm.scene.get("menu").is_none());
}

#[test]
fn right_click_handler_on_modal_menu_parent_runs_without_dispatching_unrelated_events() {
    let mut vm = fixture_vm();
    let handler = Value::Str("ゲームメイン.lsb".into());
    vm.scene.create(
        "outside",
        Kind::Box,
        HashMap::from([(Prop::ONRBTNDOWN, handler.clone())]),
    );
    vm.scene.create(
        "plate",
        Kind::Box,
        HashMap::from([(Prop::ONRBTNDOWN, handler)]),
    );
    vm.scene.create(
        "menu",
        Kind::Menu,
        HashMap::from([(Prop::PARENT, Value::Str("plate".into()))]),
    );
    vm.push_modal_menu(State::Menu {
        name: "menu".into(),
        choices: vec!["choice".into()],
        values: vec!["choice".into()],
        var: String::new(),
    });

    vm.fire("outside", Prop::ONRBTNDOWN, vec![]);
    vm.fire("plate", Prop::ONRBTNDOWN, vec![]);
    vm.dispatch_events();

    assert_eq!(vm.frames.last().unwrap().sender.as_deref(), Some("plate"));
    assert!(matches!(
        vm.frames.last().unwrap().saved_state,
        Some(State::Menu { ref name, .. }) if name == "menu"
    ));
    assert_eq!(
        vm.queue.front().map(|event| event.obj.as_str()),
        Some("outside")
    );

    vm.do_return(Value::Empty);
    assert!(matches!(vm.state, State::Menu { ref name, .. } if name == "menu"));
    vm.dispatch_events();
    assert_eq!(vm.queue.len(), 1);
}

#[test]
fn choosing_auto_scrap_menu_without_handler_removes_it() {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
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
        "menu",
        Kind::Menu,
        HashMap::from([(Prop::AUTOSCRAP, Value::Bool(true))]),
    );
    vm.state = State::Menu {
        name: "menu".into(),
        choices: vec!["first".into()],
        values: vec!["first".into()],
        var: String::new(),
    };

    vm.choose(0);

    assert!(vm.scene.get("menu").is_none());
}

#[test]
fn auto_scrap_menu_is_removed_after_onselect_handler_returns() {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
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
        "menu",
        Kind::Menu,
        HashMap::from([
            (Prop::AUTOSCRAP, Value::Bool(true)),
            (Prop::ONSELECT, Value::Str("ゲームメイン.lsb".into())),
        ]),
    );
    vm.state = State::Menu {
        name: "menu".into(),
        choices: vec!["first".into()],
        values: vec!["first".into()],
        var: String::new(),
    };

    vm.choose(0);
    assert!(vm.scene.get("menu").is_some());
    vm.dispatch_events();

    assert!(vm.frames.last().unwrap().auto_scrap_sender);

    vm.do_return(Value::Empty);

    assert!(vm.scene.get("menu").is_none());
    assert!(vm.scene.was_deleted("menu"));
    assert!(matches!(vm.state, State::Running));
}

#[test]
fn stay_open_menu_restores_modal_after_onselect() {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
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
        "menu",
        Kind::Menu,
        HashMap::from([(Prop::ONSELECT, Value::Str("ゲームメイン.lsb".into()))]),
    );
    vm.state = State::Menu {
        name: "menu".into(),
        choices: vec!["a".into(), "b".into()],
        values: vec!["a".into(), "b".into()],
        var: "sel".into(),
    };

    vm.choose(1);
    assert!(matches!(vm.state, State::Menu { .. }));
    vm.dispatch_events();
    assert!(matches!(vm.state, State::Running));
    vm.do_return(Value::Empty);

    assert!(matches!(
        vm.state,
        State::Menu {
            name,
            ..
        } if name == "menu"
    ));
    assert!(vm.scene.get("menu").is_some());
}

#[test]
fn menu_select_runs_ahead_of_a_queued_play_end() {
    let mut vm = fixture_vm();
    vm.scene.create(
        "menu",
        Kind::Menu,
        HashMap::from([(Prop::ONSELECT, Value::Str("ゲームメイン.lsb".into()))]),
    );
    vm.state = State::Menu {
        name: "menu".into(),
        choices: vec!["a".into()],
        values: vec!["a".into()],
        var: String::new(),
    };
    vm.queue.push_back(ObjEvent {
        obj: "other".into(),
        prop: Prop::ONPLAYEND,
        script: "ゲームメイン.lsb".into(),
        params: vec![],
    });
    vm.choose(0);
    vm.dispatch_events();
    assert_eq!(vm.frames.last().unwrap().event_prop, Some(Prop::ONSELECT));
}

#[test]
fn menu_does_not_freeze_the_script_that_opened_it() {
    let mut vm = fixture_vm();
    vm.assign("hold", Value::Int(1));
    let calc = wait_then_flag(int_expr(0)).pop().unwrap();
    let wait = wait_then_flag(var_expr("hold")).remove(0);
    install_script(&mut vm, vec![calc, wait]);
    vm.scene.create("menu", Kind::Menu, HashMap::new());
    vm.push_modal_menu(State::Menu {
        name: "menu".into(),
        choices: vec!["a".into()],
        values: vec!["a".into()],
        var: String::new(),
    });
    vm.update(0.0, 32);
    assert_eq!(vm.var("done"), Some(&Value::Int(1)));
    assert!(matches!(vm.state, State::Menu { .. }));
    assert_eq!(vm.frames.last().map(|frame| frame.pc), Some(1));
}

#[test]
fn flip_wait_returns_to_the_open_menu() {
    let mut vm = fixture_vm();
    vm.assign("hold", Value::Int(1));
    install_script(&mut vm, wait_then_flag(var_expr("hold")));
    vm.scene.create("menu", Kind::Menu, HashMap::new());
    vm.push_modal_menu(State::Menu {
        name: "menu".into(),
        choices: vec!["a".into()],
        values: vec!["a".into()],
        var: String::new(),
    });
    vm.state = State::WaitUntil {
        until: 0.0,
        skippable: true,
    };
    vm.update(1.0, 16);
    assert!(matches!(vm.state, State::Menu { name, .. } if name == "menu"));
}

#[test]
fn higher_priority_button_wins_over_overlapping_prevmenu() {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
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
        "menu",
        Kind::Menu,
        HashMap::from([
            (Prop::PRIORITY, Value::Int(100)),
            (Prop::ONSELECT, Value::Str("menu.lsc".into())),
        ]),
    );
    vm.scene.create(
        "button",
        Kind::Button,
        HashMap::from([
            (Prop::LEFT, Value::Int(0)),
            (Prop::TOP, Value::Int(0)),
            (Prop::WIDTH, Value::Int(40)),
            (Prop::HEIGHT, Value::Int(40)),
            (Prop::PRIORITY, Value::Int(101)),
            (Prop::ONNOTIFY, Value::Str("button.lsc".into())),
        ]),
    );
    vm.prevmenus.insert(
        "menu".into(),
        PrevMenu {
            buttons: vec![PmButton {
                x: 0,
                y: 0,
                w: 40,
                h: 40,
                src: String::new(),
                src_selected: String::new(),
                src_disabled: String::new(),
                name: "item".into(),
                group: 0,
            }],
            hover: None,
        },
    );

    vm.click_at(10.0, 10.0, false);

    let event = vm.queue.pop_front().unwrap();
    assert_eq!(event.obj, "button");
    assert_eq!(event.script, "button.lsc");
    assert_eq!(vm.lookup("@SelStr"), Some(&Value::Str(String::new())));
}

#[test]
fn runs_fixture_to_end() {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
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
    let mut t = 0.0;
    for _ in 0..1000 {
        vm.update(t, 1000);
        if vm.ended() {
            break;
        }
        if vm.wants_click() {
            vm.click();
        }
        t += 50.0;
    }
    assert!(vm.ended(), "state {:?}", vm.state);
    assert_eq!(vm.globals.get("count"), Some(&Value::Int(2)));
    assert_eq!(vm.globals.get("name"), Some(&Value::Str("3".into())));
    assert_eq!(vm.globals.get("ret"), Some(&Value::Int(42)));
    assert_eq!(vm.globals.get("subran"), Some(&Value::Int(1)));
    assert!(vm.scene.get("bg").is_none());
    assert!(vm.scene.get("msg").is_some());
    assert!(vm.stats.is_empty());
    assert!(
        vm.events
            .iter()
            .any(|e| matches!(e, Event::PlaySound { file, .. } if file == "se.wav"))
    );
}

#[test]
fn applies_catalog_translation_to_static_textins() {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
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
    vm.set_translations(HashMap::from([(
        "00000001.lsb:line=13:command=12:field=Text".to_string(),
        "Hola\nmundo".to_string(),
    )]));

    let mut time = 0.0;
    let mut observed = String::new();
    for _ in 0..100 {
        vm.update(time, 1000);
        if let Some(message) = vm.scene.get("msg")
            && !message.text.is_empty()
        {
            observed = message
                .text
                .iter()
                .map(|item| match item {
                    TextItem::Char(character) => character.as_str(),
                    TextItem::Newline => "\n",
                    TextItem::Image { .. } => "",
                })
                .collect();
            break;
        }
        time += 50.0;
    }
    assert!(observed.starts_with("Hola"), "{observed:?}");
}

#[test]
fn translates_selected_menu_values_when_inserted_into_text() {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
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
    let source = "●断面を表示する";
    vm.set_translations(HashMap::from([(
        crate::text_export::menu_item_id(source),
        "●Mostrar cortes".to_string(),
    )]));
    vm.assign("selected", Value::Str(source.to_string()));
    vm.scene.create("msg", Kind::Message, HashMap::new());
    let text = crate::lsb::TpWord {
        body: vec![Glyph::Var {
            name: "selected".into(),
            hist: false,
        }],
        ..crate::lsb::TpWord::default()
    };

    vm.run_text("msg", &text, false, false, false);
    vm.update_plays();

    let observed = vm
        .scene
        .get("msg")
        .unwrap()
        .text
        .iter()
        .map(|item| match item {
            TextItem::Char(character) => character.as_str(),
            TextItem::Newline => "\n",
            TextItem::Image { .. } => "",
        })
        .collect::<String>();
    assert_eq!(observed, "●Mostrar cortes");
    assert_eq!(vm.lookup("selected").unwrap().as_str(), source);
}

#[test]
fn backward_jump_yields_the_rest_of_the_frame() {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
    let data = GameData::open(&p).unwrap();
    let assets = Arc::new(Assets::new(data, 2, 64));
    let mut vm = Vm::new(
        assets,
        "00000001.lsb",
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

    let steps = vm.update(0.0, 1_000);

    assert_eq!(steps, 6);
    assert_eq!(vm.location(), "00000001.lsb:2");
}

#[test]
fn button_notify_fires_on_click_not_mouse_hover() {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
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
        "hover",
        Kind::Button,
        HashMap::from([
            (Prop::LEFT, Value::Int(10)),
            (Prop::TOP, Value::Int(10)),
            (Prop::WIDTH, Value::Int(100)),
            (Prop::HEIGHT, Value::Int(100)),
            (Prop::ONNOTIFY, Value::Str("game.lsc:hover".into())),
        ]),
    );

    vm.mouse_move(20.0, 20.0);
    assert!(vm.queue.is_empty());
    vm.mouse_move(150.0, 20.0);
    assert!(vm.queue.is_empty());
    vm.click_at(20.0, 20.0, false);
    assert_eq!(vm.queue.len(), 1);
    assert_eq!(vm.queue.front().unwrap().prop, Prop::ONNOTIFY);
    vm.mouse_move(30.0, 30.0);
    assert_eq!(vm.queue.len(), 1);
    vm.click_at(150.0, 20.0, false);
    assert_eq!(vm.queue.len(), 1);
}

#[test]
fn do_event_dispatches_notifications_nested_in_an_event_handler() {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
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
        "button",
        Kind::Button,
        HashMap::from([(Prop::ONNOTIFY, Value::Str("ゲームメイン.lsb".into()))]),
    );
    vm.frames.last_mut().unwrap().saved_state = Some(State::Running);
    assert!(vm.fire("button", Prop::ONNOTIFY, vec![]));

    vm.dispatch_events();
    assert_eq!(vm.queue.len(), 1);
    assert_eq!(vm.frames.len(), 1);

    vm.dispatch_events_from_do_event();

    assert!(vm.queue.is_empty());
    assert_eq!(vm.frames.len(), 2);
    assert_eq!(vm.frames.last().unwrap().sender.as_deref(), Some("button"));
}

#[test]
fn wheel_and_arrow_inputs_are_exposed_to_scripts() {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
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

    vm.set_mouse_wheel(true, false);
    vm.set_key_push(38, true);
    vm.set_key_push(40, true);

    assert_eq!(vm.system_var("@WheelUp"), Some(Value::Bool(true)));
    assert_eq!(vm.system_var("@WheelDown"), Some(Value::Bool(false)));
    let Some(Value::Array(keys)) = vm.system_var("@KeyPush") else {
        panic!("@KeyPush should be an indexed array");
    };
    assert_eq!(keys[38], Value::Bool(true));
    assert_eq!(keys[40], Value::Bool(true));
}

fn fixture_vm() -> Vm {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
    let data = GameData::open(&path).unwrap();
    let assets = Arc::new(Assets::new(data, 1, 8));
    Vm::new(
        assets,
        "ゲームメイン.lsb",
        Options {
            screen: (800, 600),
            cinema_ms: 0.0,
            text_cps: 0.0,
            lookahead: 0,
            trace: false,
            trace_conds: false,
            pick: None,
            show_text: false,
        },
    )
    .unwrap()
}

#[test]
fn only_idle_handlers_return_without_yielding_the_frame() {
    let mut vm = fixture_vm();
    let script = vm.frames[0].script.clone();
    for (prop, expect_yield) in [(Prop::ONNOTIFY, true), (Prop::ONIDLE, false)] {
        vm.yielded = false;
        vm.frames.push(Frame {
            script: script.clone(),
            pc: 0,
            locals: HashMap::new(),
            result_var: String::new(),
            params: vec![],
            saved_state: Some(State::Running),
            sender: None,
            event_prop: Some(prop),
            auto_scrap_sender: false,
        });
        vm.do_return(Value::Empty);
        assert_eq!(vm.yielded, expect_yield, "{}", crate::lsb::prop_name(prop));
    }
}

fn int_expr(value: i32) -> LiveParser {
    LiveParser {
        entries: vec![crate::lsb::OpeData {
            op: crate::lsb::op::TO,
            name: String::new(),
            func: None,
            operands: vec![Param::Int(value)],
        }],
    }
}

fn var_expr(name: &str) -> LiveParser {
    LiveParser {
        entries: vec![crate::lsb::OpeData {
            op: crate::lsb::op::TO,
            name: String::new(),
            func: None,
            operands: vec![Param::Var(name.into())],
        }],
    }
}

fn install_script(vm: &mut Vm, commands: Vec<Command>) {
    let frame = vm.frames.last_mut().unwrap();
    frame.script = Arc::new(Lsb {
        name: "wait.lsb".into(),
        version: 117,
        flags: 0,
        command_params: vec![],
        commands,
    });
    frame.pc = 0;
}

fn wait_then_flag(calc: LiveParser) -> Vec<Command> {
    vec![
        Command {
            kind: CmdType::Wait as u8,
            indent: 0,
            mute: false,
            not_update: false,
            line_no: 1,
            fields: vec![
                ("Calc", crate::lsb::Field::Expr(calc)),
                ("Time", crate::lsb::Field::Expr(int_expr(0))),
                ("StopEvent", crate::lsb::Field::Expr(int_expr(0))),
            ],
        },
        Command {
            kind: CmdType::Calc as u8,
            indent: 0,
            mute: false,
            not_update: false,
            line_no: 2,
            fields: vec![(
                "Calc",
                crate::lsb::Field::Expr(LiveParser {
                    entries: vec![crate::lsb::OpeData {
                        op: crate::lsb::op::TO,
                        name: "done".into(),
                        func: None,
                        operands: vec![Param::Int(1)],
                    }],
                }),
            )],
        },
    ]
}

#[test]
fn zero_time_wait_stays_while_its_condition_holds() {
    let mut vm = fixture_vm();
    vm.assign("hold", Value::Int(1));
    install_script(&mut vm, wait_then_flag(var_expr("hold")));
    vm.update(0.0, 32);
    vm.update(16.0, 32);
    assert_eq!(vm.frames.last().map(|frame| frame.pc), Some(0));
    assert!(vm.var("done").is_none());
    vm.assign("hold", Value::Int(0));
    vm.update(32.0, 32);
    assert_eq!(vm.var("done"), Some(&Value::Int(1)));
}

#[test]
fn zero_time_literal_wait_advances_on_the_next_frame() {
    let mut vm = fixture_vm();
    install_script(&mut vm, wait_then_flag(int_expr(1)));
    vm.update(0.0, 32);
    assert_eq!(vm.frames.last().map(|frame| frame.pc), Some(1));
    assert!(vm.var("done").is_none());
    vm.update(16.0, 32);
    assert_eq!(vm.var("done"), Some(&Value::Int(1)));
}

#[test]
fn prevmenu_group_minus_one_toggles_and_reports_its_group() {
    let mut vm = fixture_vm();
    vm.scene.create("menu", Kind::Menu, HashMap::new());
    vm.prevmenus.insert(
        "menu".into(),
        PrevMenu {
            buttons: vec![PmButton {
                x: 0,
                y: 0,
                w: 40,
                h: 40,
                src: String::new(),
                src_selected: "down.gal".into(),
                src_disabled: String::new(),
                name: "オート".into(),
                group: -1,
            }],
            hover: None,
        },
    );

    vm.set_mouse_button(false, true);
    assert_eq!(vm.system_var("@LClick"), Some(Value::Bool(true)));
    vm.click_at(10.0, 10.0, false);
    assert_eq!(
        vm.system_var("@LClick"),
        Some(Value::Bool(false)),
        "PrevMenu ONSELECT must consume @LClick"
    );
    assert_eq!(
        vm.call_engine(
            Func::GETSELECTED,
            &[Value::Str("menu".into()), Value::Str("オート".into())],
            &[],
        ),
        Some(Value::Bool(true))
    );
    assert_eq!(
        vm.call_engine(
            Func::GETSELECTED,
            &[Value::Str("menu".into()), Value::Int(0)],
            &[],
        ),
        Some(Value::Bool(true))
    );
    assert_eq!(
        vm.call_engine(
            Func::GETGROUPINDEX,
            &[Value::Str("menu".into()), Value::Int(0)],
            &[],
        ),
        Some(Value::Int(-1))
    );
    // 許可設定: SetSelected por nombre apaga también el índice (dibujo + GetSelected).
    assert_eq!(
        vm.call_engine(
            Func::SETSELECTED,
            &[
                Value::Str("menu".into()),
                Value::Str("オート".into()),
                Value::Bool(false),
            ],
            &[],
        ),
        Some(Value::Bool(true))
    );
    assert!(!vm.prevmenu_item_selected("menu", 0, "オート"));
    assert_eq!(
        vm.call_engine(
            Func::GETSELECTED,
            &[Value::Str("menu".into()), Value::Int(0)],
            &[],
        ),
        Some(Value::Bool(false))
    );
    vm.click_at(10.0, 10.0, false);
    assert_eq!(
        vm.call_engine(
            Func::GETSELECTED,
            &[Value::Str("menu".into()), Value::Str("オート".into())],
            &[],
        ),
        Some(Value::Bool(true))
    );
    vm.click_at(10.0, 10.0, false);
    assert_eq!(
        vm.call_engine(
            Func::GETSELECTED,
            &[Value::Str("menu".into()), Value::Str("オート".into())],
            &[],
        ),
        Some(Value::Bool(false))
    );
}

#[test]
fn listcompo_without_parent_lists_all_live_components() {
    let mut vm = fixture_vm();
    vm.scene.create("a", Kind::Box, HashMap::new());
    vm.scene.create(
        "b",
        Kind::Box,
        HashMap::from([(Prop::PARENT, Value::Str("a".into()))]),
    );
    vm.scene.create("c", Kind::Image, HashMap::new());
    // ListCompo(ss): docs/livenovel/calc.html — sin padre enumera todos.
    let n = vm.call_engine(Func::LISTCOMPO, &[], &[Param::Var("ss".into())]);
    assert_eq!(n, Some(Value::Int(3)));
    let Some(Value::Array(names)) = vm.lookup("ss").cloned() else {
        panic!("ss");
    };
    let mut got: Vec<_> = names.iter().map(Value::as_str).collect();
    got.sort();
    assert_eq!(got, vec!["a", "b", "c"]);
}

#[test]
fn inprimary_checks_the_front_surface() {
    let mut vm = fixture_vm();
    vm.scene.create("front", Kind::Image, HashMap::new());
    vm.scene.create("back", Kind::Image, HashMap::new());
    vm.scene.mark_back("back");

    assert_eq!(
        vm.call_engine(Func::INPRIMARY, &[Value::Str("front".into())], &[]),
        Some(Value::Bool(true))
    );
    assert_eq!(
        vm.call_engine(Func::INPRIMARY, &[Value::Str("back".into())], &[]),
        Some(Value::Bool(false))
    );
}

#[test]
fn delete_stack_uses_zero_based_frame_indices_and_stack_count() {
    let mut vm = fixture_vm();
    let mut upper = vm.frames[0].clone();
    upper.script = Arc::new(Lsb {
        name: "upper.lsb".into(),
        version: 117,
        flags: 0,
        command_params: vec![],
        commands: vec![],
    });
    upper.pc = 7;
    vm.frames.push(upper);
    assert_eq!(vm.system_var("@StackCount"), Some(Value::Int(2)));

    assert_eq!(
        vm.call_engine(Func::DELETESTACK, &[Value::Int(0)], &[Param::Int(0)]),
        Some(Value::Bool(true))
    );
    assert_eq!(vm.system_var("@StackCount"), Some(Value::Int(1)));
    assert_eq!(vm.frames[0].script.name, "upper.lsb");
    assert_eq!(vm.frames[0].pc, 7);
    assert_eq!(
        vm.call_engine(Func::DELETESTACK, &[Value::Int(-1)], &[Param::Int(-1)]),
        Some(Value::Bool(false))
    );
    assert_eq!(vm.system_var("@StackCount"), Some(Value::Int(1)));
}

#[test]
fn mouse_and_key_edges_are_system_variables() {
    let mut vm = fixture_vm();
    vm.set_mouse_button(false, true);
    assert_eq!(vm.system_var("@LClick"), Some(Value::Bool(true)));
    assert_eq!(vm.system_var("@LPush"), Some(Value::Bool(true)));
    vm.set_mouse_button(false, false);
    assert_eq!(vm.system_var("@LPush"), Some(Value::Bool(false)));
    assert_eq!(vm.system_var("@LClick"), Some(Value::Bool(true)));
    vm.set_key_push(32, true);
    let Some(Value::Array(clicks)) = vm.system_var("@KeyClick") else {
        panic!("@KeyClick");
    };
    assert_eq!(clicks.len(), 512);
    assert_eq!(clicks[32], Value::Bool(true));
    assert_eq!(clicks[13], Value::Bool(false));
}

#[test]
fn already_read_text_reports_one_on_play_start() {
    let mut vm = fixture_vm();
    vm.scene.create(
        "msg",
        Kind::Message,
        HashMap::from([(Prop::ONPLAYSTART, Value::Str("probe.lsb".into()))]),
    );
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
    assert!(!vm.note_text_read("scene.lsb", 4));
    vm.run_text("msg", &text, false, false, false);
    vm.update_plays();
    assert_eq!(vm.queue[0].params, vec![Value::Int(0)]);
    vm.queue.clear();
    assert!(vm.note_text_read("scene.lsb", 4));
    vm.run_text("msg", &text, false, false, true);
    vm.update_plays();
    assert_eq!(vm.queue[0].params, vec![Value::Int(1)]);
}

#[test]
fn ctrl_does_not_interrupt_movie_waitplay_but_click_still_can() {
    let mut vm = fixture_vm();
    vm.scene.create("movie", Kind::Movie, HashMap::new());
    vm.scene.get_mut("movie").unwrap().anim = Some(crate::scene::Anim {
        source: "movie.avi".into(),
        started: Some(0.0),
    });
    vm.scene.create("msg", Kind::Message, HashMap::new());
    vm.scene.get_mut("msg").unwrap().play = Some(crate::scene::Playback {
        wait_for: Some(("movie".into(), 1_000.0)),
        click_skips_wait: true,
        ..Default::default()
    });
    vm.skip = true;
    vm.now = 100.0;

    vm.update_plays();
    assert_eq!(
        vm.play_of("msg").unwrap().wait_for,
        Some(("movie".into(), 1_000.0))
    );

    vm.click();
    assert!(vm.play_of("msg").unwrap().wait_for.is_none());
}

#[test]
fn load_text_file_stores_one_array_entry_per_line() {
    let dir = std::env::temp_dir().join(format!("livenyan-ltf-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/evt/終了待ち.lsb"),
        dir.join("entry.lsb"),
    )
    .unwrap();
    std::fs::write(dir.join("list.txt"), "a/1\r\n\r\nb/2\r\n").unwrap();
    let data = GameData::open(&dir).unwrap();
    let assets = Arc::new(Assets::new(data, 1, 8));
    let mut vm = Vm::new(
        assets,
        "entry.lsb",
        Options {
            screen: (800, 600),
            cinema_ms: 0.0,
            text_cps: 0.0,
            lookahead: 0,
            trace: false,
            trace_conds: false,
            pick: None,
            show_text: false,
        },
    )
    .unwrap();
    let loaded = vm
        .call_engine(
            Func::LOADTEXTFILE,
            &[Value::Empty, Value::Str("list.txt".into())],
            &[Param::Var("list".into()), Param::Str("list.txt".into())],
        )
        .unwrap();
    assert_eq!(loaded, Value::Bool(true));
    assert_eq!(
        vm.var("list"),
        Some(&Value::Array(vec![
            Value::Str("a/1".into()),
            Value::Str(String::new()),
            Value::Str("b/2".into()),
        ]))
    );
    let missing = vm
        .call_engine(
            Func::LOADTEXTFILE,
            &[Value::Empty, Value::Str("missing.txt".into())],
            &[
                Param::Var("missing".into()),
                Param::Str("missing.txt".into()),
            ],
        )
        .unwrap();
    assert_eq!(missing, Value::Bool(false));
    assert!(vm.var("missing").is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn options_menu_click_opens_the_speed_slider() {
    let path = std::path::PathBuf::from("/home/gemu/Desktop/test2/test2.exe");
    if !path.exists() {
        return;
    }
    let data = GameData::open(&path).unwrap();
    let assets = Arc::new(Assets::new(data, 1, 8));
    let mut vm = Vm::new(
        assets,
        "ノベルシステム\\■オプションメニュー.lsb",
        Options {
            screen: (800, 600),
            cinema_ms: 0.0,
            text_cps: 0.0,
            lookahead: 0,
            trace: false,
            trace_conds: false,
            pick: None,
            show_text: false,
        },
    )
    .unwrap();
    vm.assign(
        "システムメニュー項目名",
        Value::Array(vec![
            Value::Str("テキスト速度".into()),
            Value::Str("音量調節".into()),
        ]),
    );
    vm.assign(
        "システムメニュー項目値",
        Value::Array(vec![
            Value::Str("オプション_テキスト速度".into()),
            Value::Str("オプション_音量調節".into()),
        ]),
    );
    let mut now = 0.0;
    for _ in 0..8 {
        vm.update(now, 20_000);
        now += 1000.0;
        if vm.scene.get("システムオプションメニュー").is_some() {
            break;
        }
    }
    let menu = vm.scene.get("システムオプションメニュー");
    let onselect = menu.map(|o| o.str(Prop::ONSELECT)).unwrap_or_default();
    let scrap = menu.map(|o| o.int(Prop::RCLICKSCRAP, -1)).unwrap_or(-2);
    let top = menu.map(|o| o.int(Prop::TOP, -1)).unwrap_or(-2);
    let left = menu.map(|o| o.int(Prop::LEFT, -1)).unwrap_or(-2);
    let opened = vm.modal_menu();
    let index = opened
        .as_ref()
        .and_then(|(_, choices)| choices.iter().position(|c| c == "テキスト速度"))
        .unwrap_or(0);
    vm.choose(index);
    let queued: Vec<String> = vm
        .queue
        .iter()
        .map(|ev| format!("{}:{}", crate::lsb::prop_name(ev.prop), ev.script))
        .collect();
    vm.update(now, 20_000);
    let speed = vm.scene.get("スライダーテキスト速度").is_some();
    let bg = vm.scene.get("テキスト速度背景").is_some();
    let ignore = vm
        .scene
        .get("システムオプションメニュー")
        .map(|o| o.int(Prop::IGNOREMOUSE, -1))
        .unwrap_or(-2);
    assert!(
        speed,
        "onselect={onselect:?} scrap={scrap} left={left} top={top} opened={opened:?} queued={queued:?} bg={bg} ignore={ignore} state={:?}",
        vm.state
    );
}
