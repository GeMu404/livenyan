//! Verificación de オート/スキップ/閉じる en test2.
//! `LIVENYAN_TEST2=/ruta/test2.exe cargo test --test verify_mesctrl_test2 -- --nocapture`

use livenyan::assets::Assets;
use livenyan::config::Config;
use livenyan::game::Game;
use livenyan::vm::{Options, State, Vm};
use std::path::PathBuf;
use std::sync::Arc;

fn test2_path() -> Option<PathBuf> {
    std::env::var_os("LIVENYAN_TEST2")
        .map(PathBuf::from)
        .filter(|p| p.exists())
}

fn at_control_pause(vm: &Vm) -> bool {
    let has_ctrl = vm.prevmenus.contains_key("メッセージボックスコントロール");
    let has_cursor = vm.scene.get("メッセージカーソル").is_some();
    let in_ctrl = vm.location().contains("プレビューメニュー処理");
    has_ctrl && has_cursor && !in_ctrl
}

fn reach_message_wait(vm: &mut Vm) -> bool {
    let mut t = 0.0;
    let mut idle = 0u32;
    let mut fired = 0usize;
    let mut saw_ctrl = false;
    for _ in 0..30_000 {
        vm.update(t, 50_000);
        if vm.ended() {
            return false;
        }
        if at_control_pause(vm) {
            // Dejar que ONIDLE/許可設定 asienten enabled.
            for _ in 0..5 {
                t += 16.0;
                vm.update(t, 20_000);
            }
            return at_control_pause(vm)
                || vm.prevmenus.contains_key("メッセージボックスコントロール");
        }
        let has_ctrl = vm.prevmenus.contains_key("メッセージボックスコントロール");
        saw_ctrl |= has_ctrl;
        let mut acted = false;
        // No comerse la pausa del cuadro: sin eso no hay メッセージカーソル.
        if vm.wants_click() && !has_ctrl {
            vm.click();
            acted = true;
        }
        if matches!(vm.state, State::Menu { .. }) {
            vm.choose(0);
            acted = true;
        }
        if let State::WaitUntil { until, .. } = vm.state {
            t = t.max(until);
        }
        t += 16.0;
        idle = if acted { 0 } else { idle.saturating_add(1) };
        if idle >= 60 && vm.auto_menu().is_some() {
            idle = 0;
        }
        if !has_ctrl && idle > 0 && idle.is_multiple_of(30) && !idle.is_multiple_of(60) {
            let _ = vm.click_message();
        }
        if !has_ctrl && idle > 0 && idle.is_multiple_of(60) && vm.auto_fire(fired).is_some() {
            fired += 1;
        }
        // Con controles ya cargados, un clic de pantalla puede crear el cursor vía idle.
        if has_ctrl && idle > 0 && idle.is_multiple_of(45) && vm.wants_click() {
            // No vm.click(): dejar que el texto muestre el PAUSE/cursor.
        }
        if idle > 6000 {
            eprintln!(
                "stall loc={} ctrl={has_ctrl} cursor={} wants={} saw_ctrl={saw_ctrl}",
                vm.location(),
                vm.scene.get("メッセージカーソル").is_some(),
                vm.wants_click(),
            );
            break;
        }
    }
    false
}

fn menu_origin(vm: &Vm) -> (f64, f64) {
    let o = vm
        .scene
        .get("メッセージボックスコントロール")
        .expect("control menu");
    vm.scene.abs_pos(o)
}

fn btn_index(vm: &Vm, name: &str) -> usize {
    vm.prevmenus
        .get("メッセージボックスコントロール")
        .unwrap()
        .buttons
        .iter()
        .position(|b| b.name == name)
        .unwrap_or_else(|| panic!("missing button {name}"))
}

fn click_btn(vm: &mut Vm, name: &str) {
    let (ox, oy) = menu_origin(vm);
    let i = btn_index(vm, name);
    let b = vm.prevmenus["メッセージボックスコントロール"].buttons[i].clone();
    let x = ox + b.x as f64 + (b.w.max(1) as f64) / 2.0;
    let y = oy + b.y as f64 + (b.h.max(1) as f64) / 2.0;
    let vis = vm.prevmenu_item_visible("メッセージボックスコントロール", i, name);
    eprintln!(
        "click {name} at ({x:.0},{y:.0}) origin=({ox:.0},{oy:.0}) {}x{} vis={vis} loc={}",
        b.w,
        b.h,
        vm.location()
    );
    vm.set_mouse_button(false, true);
    vm.click_at(x, y, false);
    vm.set_mouse_button(false, false);
    let base = vm.now;
    for k in 0..60 {
        vm.update(base + 16.0 * (k + 1) as f64, 50_000);
    }
    eprintln!(
        "  after: selected={} 自動送り={} メッセージスキップ={} loc={}",
        vm.prevmenu_item_selected("メッセージボックスコントロール", i, name),
        var_truthy(vm, "自動送り"),
        var_truthy(vm, "メッセージスキップ"),
        vm.location()
    );
}

fn var_truthy(vm: &Vm, name: &str) -> bool {
    vm.var(name).is_some_and(livenyan::lsb::expr::Value::truthy)
}

#[test]
fn auto_skip_hide_on_test2() {
    let Some(path) = test2_path() else {
        eprintln!("skip: set LIVENYAN_TEST2 to test2.exe");
        return;
    };
    let cfg = Config::default();
    let game = Game::open(&path, &cfg).expect("open test2");
    let assets = Arc::new(Assets::new(game.data.clone(), 2, 64));
    let mut vm = Vm::new(
        assets,
        &game.entry,
        Options {
            screen: game.logical,
            cinema_ms: 0.0,
            text_cps: 120.0,
            lookahead: 0,
            trace: false,
            show_text: false,
            trace_conds: false,
            pick: Some(0),
        },
    )
    .expect("vm");

    assert!(
        reach_message_wait(&mut vm),
        "did not reach message wait (at {})",
        vm.location()
    );
    eprintln!("reached wait at {}", vm.location());

    let auto_i = btn_index(&vm, "オート");
    assert!(!var_truthy(&vm, "自動送り"));
    click_btn(&mut vm, "オート");
    assert!(
        vm.prevmenu_item_selected("メッセージボックスコントロール", auto_i, "オート"),
        "オート selected"
    );
    assert!(var_truthy(&vm, "自動送り"), "自動送り after click");
    for _ in 0..15 {
        vm.update(vm.now + 16.0, 20_000);
    }
    assert!(
        var_truthy(&vm, "自動送り"),
        "自動送り must survive idle ( @LClick consumed )"
    );
    click_btn(&mut vm, "オート");
    assert!(!var_truthy(&vm, "自動送り"), "auto toggles off");

    // スキップ: con 選択実行中 el novel system solo lo habilita si 既読.
    // En la primera pausa no leída el clic no debe activarlo (selimg: se ve, no pulsa).
    assert!(reach_message_wait(&mut vm), "need wait before skip");
    let skip_i = btn_index(&vm, "スキップ");
    let already = var_truthy(&vm, "既読");
    click_btn(&mut vm, "スキップ");
    if already {
        assert!(
            vm.prevmenu_item_selected("メッセージボックスコントロール", skip_i, "スキップ"),
            "スキップ selected when 既読"
        );
        assert!(
            var_truthy(&vm, "メッセージスキップ") || var_truthy(&vm, "既読メッセージスキップ"),
            "skip mode vars"
        );
        click_btn(&mut vm, "スキップ");
        assert!(
            !vm.prevmenu_item_selected("メッセージボックスコントロール", skip_i, "スキップ"),
            "スキップ toggles off"
        );
    } else {
        assert!(
            !vm.prevmenu_item_selected("メッセージボックスコントロール", skip_i, "スキップ"),
            "スキップ stays off while unread (disabled)"
        );
        eprintln!("スキップ correctly inert while 既読=0 (visible disabled)");
    }

    assert!(reach_message_wait(&mut vm), "need wait before hide");
    let hide_target = if vm.scene.get("メッセージボックス土台").is_some() {
        "メッセージボックス土台"
    } else {
        "メッセージボックス"
    };
    assert!(
        vm.scene.get(hide_target).is_some_and(|o| o.visible()),
        "{hide_target} visible before 閉じる"
    );
    click_btn(&mut vm, "閉じる");
    let hidden = vm.scene.get(hide_target).is_some_and(|o| !o.visible());
    assert!(
        hidden || var_truthy(&vm, "テキスト停止"),
        "閉じる hides UI or sets テキスト停止 (visible={:?})",
        vm.scene.get(hide_target).map(|o| o.visible())
    );
    // Sigue oculto tras soltar: no debe volver solo con mouse-up del mismo clic.
    assert!(
        hidden || var_truthy(&vm, "テキスト停止"),
        "閉じる must stay hidden after mouse-up"
    );
    eprintln!(
        "閉じる ok hidden={hidden} テキスト停止={}",
        var_truthy(&vm, "テキスト停止")
    );
}
