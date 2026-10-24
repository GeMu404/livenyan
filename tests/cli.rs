//! Pruebas de la CLI sin pantalla (--info, --dump-lsb, --trace) sobre fixtures sintéticos.
use std::process::Command;

fn run(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_livenyan")).args(args).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

fn fx(n: &str) -> String {
    format!("{}/tests/fixtures/{n}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn info_counts() {
    let s = run(&["--info", &fx("game.exe")]);
    assert!(s.contains("entries:     5"));
    assert!(s.contains("lsb:         3/3"));
    assert!(s.contains("entry:       ゲームメイン.lsb"));
}

#[test]
fn trace_ends_and_hides_text() {
    let s = run(&["--trace", &fx("game.exe")]);
    assert!(s.contains("state=ended"), "{s}");
    assert!(!s.contains("こんにちは"), "text leaked without --show-text");
    let s = run(&["--trace", "--show-text", &fx("game.dat")]);
    assert!(s.contains("こんにちは"));
}

#[test]
fn dump_hides_text() {
    let s = run(&["--dump-lsb", &fx("game.exe"), "00000001"]);
    assert!(s.contains("TextIns"));
    assert!(!s.contains("こんにちは"));
}

/// Regresión v0.1: TextIns a un objeto aún inexistente + MesNew con ONPLAYEND + While de espera
/// (patrón de 終了待ち.lsb de LiveNovel). El manejador debe dispararse y el bucle terminar.
#[test]
fn onplayend_releases_wait_loop() {
    let s = run(&["--trace", "--steps", "5000", &fx("evt")]);
    assert!(s.contains("event mbox.ONPLAYEND -> end.lsc"), "{s}");
    assert!(s.contains("state=ended"), "{s}");
    assert!(!s.contains("stalled"), "{s}");
}

/// Regresión: Exit con Calc falso no debe salir; TextIns vacío igual dispara ONPLAYEND.
#[test]
fn conditional_exit_and_empty_textins() {
    let s = run(&["--trace", "--steps", "5000", &fx("evt2")]);
    assert!(s.contains("Exit (not taken)"), "{s}");
    assert!(s.contains("Calc reached = 1"), "{s}");
    assert!(s.contains("event mbox.ONPLAYEND -> end.lsc"), "{s}");
    assert!(s.contains("state=ended"), "{s}");
}

/// Regresión: Call Params -> @ParamStr[i]/@ParamCount por marco, restaurados tras un Call anidado.
#[test]
fn paramstr_per_frame() {
    let s = run(&["--trace", "--steps", "5000", &fx("evt3")]);
    assert!(s.contains("p.lsb:0    L1     If -> true"), "{s}");
    assert!(s.contains("p.lsb:4    L5     If -> true"), "{s}");
    assert!(s.contains("restored_ok = 1"), "{s}");
    assert!(s.contains("state=ended"), "{s}");
}

/// Regresión: While con End = índice tras el WhileLoop + If/Elseif dentro; ListCompo;
/// SetProp con índice numérico base 0 (3=LEFT, 4=TOP); IsDelimiter; Timer.
#[test]
fn while_end_listcompo_props_timer() {
    let s = run(&["--trace", "--steps", "5000", &fx("evt4")]);
    for want in ["SetProp \"c1\".LEFT", "SetProp \"c2\".TOP", "While -> false", "Calc after = 1", "Calc delim = 1", "event tm.ONPLAYEND -> tick.lsc", "state=ended", "# unimplemented: none"] {
        assert!(s.contains(want), "missing {want:?}\n{s}");
    }
    assert_eq!(s.matches("While -> true").count() >= 2, true);
}

/// El detector de estancamiento de --trace debe atrapar bucles que repiten SetProp.
#[test]
fn trace_detects_busy_loop() {
    let s = run(&["--trace", "--steps", "10000000", &fx("evt5")]);
    assert!(s.contains("# stalled at"), "{s}");
}

/// Cinema temporizado dispara ONPLAYEND; --trace elige un ítem habilitado del PrevMenu.
#[test]
fn cinema_and_prevmenu_autoselect() {
    let s = run(&["--trace", "--steps", "20000", &fx("evt6")]);
    for want in ["event mv.ONPLAYEND -> mvend.lsc", "auto-select menu title[1]", "event title.ONSELECT -> sel.lsc", "Calc primary = 1", "state=ended"] {
        assert!(s.contains(want), "missing {want:?}\n{s}");
    }
}

/// --trace informa de scripts distintos y de ciclos de escenas.
#[test]
fn trace_reports_cycle() {
    let s = run(&["--trace", "--steps", "3000", &fx("evt7")]);
    assert!(s.contains("(3 distinct)"), "{s}");
    assert!(s.contains("# cycling: last 3 script entries repeated"), "{s}");
    let t = run(&["--trace", &fx("evt6")]);
    assert!(t.contains("# cycling: no"), "{t}");
}

/// Render fuera de pantalla: un Box contenedor (sin COLOR, PRIORITY alta) no debe tapar a su
/// imagen hija. Usa el driver "offscreen" de SDL; si no está disponible, se omite.
#[test]
fn snapshot_renders_child_image() {
    let out = std::env::temp_dir().join(format!("livenyan-snap-{}.png", std::process::id()));
    let r = Command::new(env!("CARGO_BIN_EXE_livenyan"))
        .env("SDL_VIDEODRIVER", "offscreen")
        .args(["--mute", "--snapshot-frame", "20", out.to_str().unwrap(), &fx("render")])
        .output()
        .unwrap();
    if !r.status.success() {
        eprintln!("skipping: offscreen video unavailable: {}", String::from_utf8_lossy(&r.stderr));
        return;
    }
    let img = image::open(&out).unwrap().to_rgba8();
    let _ = std::fs::remove_file(&out);
    assert_eq!(img.dimensions(), (800, 600));
    assert_eq!(img.get_pixel(15, 12).0, [255, 0, 0, 255], "child image not visible");
    assert_eq!(img.get_pixel(5, 5).0, [0, 0, 0, 255]);
}

/// --trace-conds muestra condiciones con valores y el resumen la cadena de saltos.
#[test]
fn trace_conds_and_jump_chain() {
    let s = run(&["--trace", "--trace-conds", "--steps", "3000", &fx("evt7")]);
    assert!(s.contains("# jump/call chain"), "{s}");
    assert!(s.contains("Jump ゲームメイン.lsb:3 -> ゲームメイン.lsb:0 x"), "{s}");
    let t = run(&["--trace", "--trace-conds", &fx("evt4")]);
    assert!(t.contains("cond ゲームメイン.lsb:6 While"), "{t}");
    assert!(t.contains("i=Int(0)"), "{t}");
    assert!(t.matches("cond ゲームメイン.lsb:6 While").count() <= 4, "{t}");
}

/// Capa raíz "\x01" (PARENT y SetProp), COLOR como TColor BGR, GetProp de PARENT ausente = "".
#[test]
fn root_layer_tcolor_and_getprop_defaults() {
    let t = run(&["--trace", "--steps", "50", &fx("render2")]);
    assert!(t.contains("Calc pr_empty = 1"), "{t}");
    assert!(!t.contains("SetProp on missing"), "{t}");
    let out = std::env::temp_dir().join(format!("livenyan-snap2-{}.png", std::process::id()));
    let r = Command::new(env!("CARGO_BIN_EXE_livenyan"))
        .env("SDL_VIDEODRIVER", "offscreen")
        .args(["--mute", "--renderer", "software", "--snapshot-frame", "10", out.to_str().unwrap(), &fx("render2")])
        .output()
        .unwrap();
    if !r.status.success() {
        eprintln!("skipping: offscreen video unavailable");
        return;
    }
    let img = image::open(&out).unwrap().to_rgba8();
    let _ = std::fs::remove_file(&out);
    assert_eq!(img.get_pixel(5, 5).0, [255, 0, 0, 255], "TColor 0x0000FF must be red");
    assert_eq!(img.get_pixel(105, 103).0, [0, 0, 255, 255], "image under root layer must draw above the box");
}

/// Regresión: un título en bucle que pide clics de texto cada vuelta. --trace debe seguir
/// haciendo clic (no estancarse) y acabar eligiendo el menú; --pick decide el ítem.
#[test]
fn title_loop_clicks_and_menu_pick() {
    let s = run(&["--trace", "--pick", "1", "--steps", "5000", &fx("evt8")]);
    assert!(s.contains("auto-select menu title[1]"), "{s}");
    assert!(s.contains("Calc started = 1"), "{s}");
    assert!(s.contains("state=ended"), "{s}");
    let clicks: u32 = s.split(" clicks").next().unwrap().rsplit(' ').next().unwrap().parse().unwrap();
    assert!(clicks > 10, "text clicks must keep advancing: {clicks}");
    let p0 = run(&["--trace", "--pick", "0", "--steps", "3000", &fx("evt8")]);
    assert!(p0.contains("auto-select menu title[0]") && !p0.contains("started = 1") && !p0.contains("# stalled"), "{p0}");
}

/// --gal-stats solo imprime estadísticas.
#[test]
fn gal_stats_output() {
    let s = run(&["--gal-stats", "pic.gal", &fx("render2")]);
    assert!(s.contains("version Gale106 size 16x8 bpp 24 frames 1"), "{s}");
    assert!(s.contains("avg rgb (0,0,255) avg alpha 255"), "{s}");
    let i = run(&["--info", &fx("game.exe")]);
    assert!(i.contains("gal check:   1 decoded: ok=1"), "{i}");
}

/// Regresión del flujo real: TextIns vacío + espera tipo 終了待ち que solo se libera con
/// un clic en la caja de mensajes; --trace debe hacer ese clic y avanzar 5 veces.
#[test]
fn trace_clicks_message_box_repeatedly() {
    let s = run(&["--trace", "--steps", "20000", &fx("evt9")]);
    assert!(s.contains("Calc done5 = 1"), "{s}");
    assert!(s.contains("auto-click on screen -> mb.ONLBTNUP"), "{s}");
    assert!(!s.contains("# stalled"), "{s}");
}

/// GAL animado de 3 frames (con bytes intermedios): se leen todos y el ImgNew dispara
/// ONPLAYEND al terminar la animación.
#[test]
fn gal_animation_plays_and_ends() {
    let g = run(&["--gal-stats", "anim.gal", &fx("evt10")]);
    assert!(g.contains("frames 3 (parsed 3)"), "{g}");
    let s = run(&["--trace", "--steps", "5000", &fx("evt10")]);
    assert!(s.contains("event logo.ONPLAYEND -> ae.lsc"), "{s}");
    assert!(s.contains("state=ended"), "{s}");
}

/// Clic de pantalla en --trace: libera texto retenido y llega a una caja OCULTA con ONLBTNUP.
#[test]
fn screen_click_reaches_hidden_box() {
    let s = run(&["--trace", "--steps", "5000", &fx("evt11")]);
    assert!(s.contains("auto-click on screen -> advance text (released ghost)"), "{s}");
    assert!(s.contains("auto-click on screen -> mb.ONLBTNUP"), "{s}");
    assert!(s.contains("Calc released = 1"), "{s}");
}

/// Caja borrada en cascada (ObjDel del padre) y luego TextIns con <EVENT>: el sustituto
/// hereda las propiedades y el evento va a su ONNOTIFY.
#[test]
fn deleted_box_standin_inherits_onnotify() {
    let s = run(&["--trace", "--steps", "5000", &fx("evt12")]);
    assert!(s.contains("event mb.ONNOTIFY -> ev.lsc"), "{s}");
    assert!(s.contains("Calc released = 1"), "{s}");
}

/// {CREATECG name $RRGGBB ...} en el texto crea un fondo de color visible.
#[test]
fn createcg_text_event_draws_color_background() {
    let out = std::env::temp_dir().join(format!("livenyan-cg-{}.png", std::process::id()));
    let r = Command::new(env!("CARGO_BIN_EXE_livenyan"))
        .env("SDL_VIDEODRIVER", "offscreen")
        .args(["--mute", "--res", "800x600", "--snapshot-frame", "20", out.to_str().unwrap(), &fx("evt13")])
        .output()
        .unwrap();
    if !r.status.success() {
        eprintln!("skipping: offscreen video unavailable: {}", String::from_utf8_lossy(&r.stderr));
        return;
    }
    let img = image::open(&out).unwrap().to_rgba8();
    let _ = std::fs::remove_file(&out);
    assert_eq!(img.get_pixel(400, 300).0, [0, 128, 0, 255]);
}
