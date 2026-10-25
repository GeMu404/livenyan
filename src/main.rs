use clap::Parser;
use livenyan::assets::Assets;
use livenyan::config::Config;
use livenyan::game::Game;
use livenyan::lsb::{Field, Lsb};
use livenyan::vm::{Options, State, Vm};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// Reproductor nativo de juegos LiveMaker.
#[derive(Parser, Debug)]
#[command(name = "livenyan", version, about)]
struct Cli {
    /// Juego: .exe con archivo VF, .dat/.ext, o directorio (del juego o extraído).
    game: PathBuf,
    /// Muestra información del archivo/juego y sale.
    #[arg(long)]
    info: bool,
    /// Script LSB para --dump-lsb (p. ej. 00000001.lsb; la extensión es opcional).
    script: Option<String>,
    /// Lista los comandos del script LSB indicado y sale.
    #[arg(long)]
    dump_lsb: bool,
    /// Exporta un catálogo de textos para traducción en la carpeta indicada.
    #[arg(long, value_name = "DIR")]
    dump_texts: Option<PathBuf>,
    /// Ejecuta el VM sin ventana registrando los comandos.
    #[arg(long)]
    trace: bool,
    /// Pasos máximos para --trace.
    #[arg(long, default_value_t = 5000)]
    steps: u64,
    /// Decodifica una imagen GAL del archivo y muestra solo estadísticas (sin imagen).
    #[arg(long, value_name = "ARCHIVE_PATH")]
    gal_stats: Option<String>,
    /// Driver de render de SDL (opengl, vulkan, software, gpu...; predeterminado: vulkan).
    #[arg(long)]
    renderer: Option<String>,
    /// Interpola cuadros de animaciones GAL de pantalla completa con NVIDIA Optical Flow
    /// (requiere compilar con --features nvidia-optical-flow).
    #[arg(long)]
    #[cfg(feature = "nvidia-optical-flow")]
    optical_flow: bool,
    /// Muestra el medidor de FPS (también se puede alternar con F3).
    #[arg(long)]
    fps_meter: bool,
    /// Con --trace: ítem que se elige en los menús (0 = primero habilitado).
    #[arg(long)]
    pick: Option<i64>,
    /// Con --trace: imprime valores de condiciones If/Elseif/While en scripts de escenario.
    #[arg(long)]
    trace_conds: bool,
    /// Renderiza (oculto) hasta el frame N y guarda una captura PNG; luego sale.
    #[arg(long, num_args = 2, value_names = ["N", "OUT_PNG"])]
    snapshot_frame: Option<Vec<String>>,
    /// Incluye el texto del juego en la salida (por defecto se oculta).
    #[arg(long)]
    show_text: bool,
    /// Dibuja solo contornos y etiquetas de objetos, sin sprites ni animaciones.
    #[arg(long)]
    debug_layout: bool,
    /// Archivo de configuración (por defecto $XDG_CONFIG_HOME/livenyan/config.toml).
    #[arg(long)]
    config: Option<PathBuf>,
    /// Fuerza el script de entrada.
    #[arg(long)]
    entry: Option<String>,
    /// Fuerza la resolución lógica, p. ej. 1024x768.
    #[arg(long, value_parser = parse_res)]
    res: Option<(u32, u32)>,
    /// Ruta de la fuente (TTF/OTF/TTC).
    #[arg(long)]
    font: Option<PathBuf>,
    /// Hilos de decodificación (0 = auto).
    #[arg(long)]
    threads: Option<usize>,
    /// Arranca en pantalla completa.
    #[arg(long)]
    fullscreen: bool,
    /// Sin audio.
    #[arg(long)]
    mute: bool,
}

fn parse_res(s: &str) -> Result<(u32, u32), String> {
    let (w, h) = s.split_once(['x', 'X']).ok_or("expected WxH")?;
    Ok((
        w.parse().map_err(|_| "bad width")?,
        h.parse().map_err(|_| "bad height")?,
    ))
}

fn main() {
    let t0 = Instant::now();
    env_logger::Builder::from_env(env_logger::Env::new().filter_or("LIVENYAN_LOG", "warn"))
        .format_timestamp_millis()
        .init();
    let cli = Cli::parse();
    if let Err(e) = real_main(cli, t0) {
        eprintln!("livenyan: {e}");
        std::process::exit(1);
    }
}

fn real_main(cli: Cli, t0: Instant) -> Result<(), String> {
    let mut cfg = Config::load(cli.config.as_deref())?;
    if let Some(r) = &cli.renderer {
        cfg.window.renderer = r.clone();
    }
    #[cfg(feature = "nvidia-optical-flow")]
    if cli.optical_flow {
        cfg.window.optical_flow = true;
    }
    cfg.window.fps_meter |= cli.fps_meter;
    if let Some((w, h)) = cli.res {
        cfg.game.logical_width = w;
        cfg.game.logical_height = h;
    }
    if cli.entry.is_some() {
        cfg.game.entry = cli.entry.clone();
    }
    if cli.font.is_some() {
        cfg.text.font = cli.font.clone();
    }
    if let Some(t) = cli.threads {
        cfg.assets.threads = t;
    }
    cfg.window.fullscreen |= cli.fullscreen;
    if cli.mute {
        cfg.audio.enabled = false;
    }
    let game = Game::open(&cli.game, &cfg)?;
    log::info!(
        "opened {} in {:.1} ms: {} entries, entry={}, {}x{} ({})",
        game.data.describe(),
        t0.elapsed().as_secs_f64() * 1000.0,
        game.data.len(),
        game.entry,
        game.logical.0,
        game.logical.1,
        game.logical_source
    );
    if let Some(directory) = &cli.dump_texts {
        let summary = livenyan::text_export::export(&game.data, directory)?;
        println!(
            "translation catalog: {} entries ({} dialogues, {} menu options, {} LCM files) -> {}",
            summary.entries,
            summary.dialogues,
            summary.menu_items,
            summary.movies,
            directory.join("texts.toml").display()
        );
        return Ok(());
    }
    let assets = Arc::new(Assets::new(
        game.data.clone(),
        cfg.threads(),
        cfg.assets.cache_mb,
    ));

    if let Some(name) = &cli.gal_stats {
        let n = match game.data.resolve(name, livenyan::assets::IMAGE_EXTS) {
            Some(n) => n,
            None => {
                // por sufijo / nombre base (sin carpetas, con o sin extensión)
                let want = name.replace('/', "\\").to_lowercase();
                let stem = want
                    .rsplit_once('.')
                    .map(|(a, _)| a.to_string())
                    .unwrap_or(want.clone());
                let c: Vec<&String> = game
                    .data
                    .names()
                    .iter()
                    .filter(|f| {
                        let l = f.to_lowercase();
                        let ls = l.rsplit_once('.').map(|(a, _)| a).unwrap_or(&l);
                        let tail = |x: &str, w: &str| x == w || x.ends_with(&format!("\\{w}"));
                        tail(&l, &want) || (l.ends_with(".gal") && tail(ls, &stem))
                    })
                    .collect();
                match c.len() {
                    0 => return Err(format!("not in archive: {name}").into()),
                    1 => c[0].clone(),
                    _ => {
                        eprintln!("ambiguous: {name} matches {} files:", c.len());
                        for x in &c {
                            eprintln!("  {x}");
                        }
                        return Err(
                            "ambiguous --gal-stats name; pass one of the paths above".into()
                        );
                    }
                }
            }
        };
        let d = game.data.read(&n).map_err(|e| e.to_string())?;
        println!("{n}: {} bytes", d.len());
        match livenyan::gal::stats(&d) {
            Ok(s) => print!("{s}"),
            Err(e) => println!("stats error: {e}"),
        }
        print!("{}", livenyan::gal::dump(&d));
        return Ok(());
    }
    if cli.info {
        return info(&game, &assets, &cfg, t0, cli.show_text);
    }
    if cli.dump_lsb {
        let name = cli.script.as_deref().unwrap_or(&game.entry);
        return dump(&game, name, cli.show_text);
    }
    let opts = Options {
        screen: game.logical,
        cinema_ms: cfg.game.cinema_placeholder_ms,
        text_cps: if cli.trace { 0.0 } else { cfg.text.speed_cps },
        lookahead: cfg.assets.prefetch_lookahead,
        trace: cli.trace,
        show_text: cli.show_text,
        trace_conds: cli.trace_conds,
        pick: cli.pick,
    };
    let vm = Vm::new(assets.clone(), &game.entry, opts)?;
    if cli.trace {
        return trace(vm, cli.steps, t0);
    }
    if let Err(e) = std::fs::create_dir_all(game.save_dir()) {
        log::warn!("cannot create save dir {}: {e}", game.save_dir().display());
    }
    let snap = match cli.snapshot_frame.as_deref() {
        Some([n, p]) => Some((
            n.parse::<u64>()
                .map_err(|_| format!("bad frame number {n:?}"))?,
            PathBuf::from(p),
        )),
        _ => None,
    };
    let languages = if snap.is_some() {
        Vec::new()
    } else {
        livenyan::text_export::load_languages(&game.translations_dir())?
    };
    if languages.is_empty() {
        log::info!("no translated language catalogs found");
    } else {
        log::info!(
            "found {} translated language catalog(s) in {}",
            languages.len(),
            game.translations_dir().display()
        );
    }
    let r = livenyan::app::run(game, cfg, assets, vm, languages, t0, snap, cli.debug_layout);
    // salir de verdad aunque queden hilos de decodificación/audio vivos
    if let Err(e) = &r {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
    log::info!("exiting");
    std::process::exit(0)
}

fn info(
    game: &Game,
    assets: &Assets,
    cfg: &Config,
    t0: Instant,
    show_text: bool,
) -> Result<(), String> {
    let names = game.data.names();
    let mut exts: HashMap<String, usize> = HashMap::new();
    for n in names {
        let e = n
            .rsplit_once('.')
            .map(|(_, e)| e.to_lowercase())
            .unwrap_or_default();
        *exts.entry(e).or_default() += 1;
    }
    let mut exts: Vec<_> = exts.into_iter().collect();
    exts.sort_by(|a, b| b.1.cmp(&a.1));
    println!("archive:     {}", game.data.describe());
    println!("entries:     {}", names.len());
    println!(
        "by type:     {}",
        exts.iter()
            .map(|(e, n)| format!(".{e} {n}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    if let Some(p) = &game.project {
        println!(
            "project:     lpb v{} init={:?} exit={:?} settings={}",
            p.version,
            p.init_lsb,
            p.exit_lsb,
            p.settings.len()
        );
        // claves y valores numéricos (las cadenas solo como longitud salvo --show-text): sirve
        // para localizar dónde guarda el .lpb el tamaño de pantalla sin filtrar texto del juego
        println!("  lpb unk:   {:?}", p.unk);
        for (k, v) in &p.settings {
            let shown = if v.trim().parse::<f64>().is_ok() || show_text {
                v.clone()
            } else {
                format!("<{} chars>", v.chars().count())
            };
            println!("  setting {k} = {shown}");
        }
    } else {
        println!("project:     (no .lpb)");
    }
    for f in livenyan::game::exe_forms(&game.path) {
        println!(
            "  exe form {} {}x{}{}",
            f.class,
            f.w,
            f.h,
            if f.main {
                " (largest/main-like; not used)"
            } else {
                ""
            }
        );
    }
    println!("entry:       {}", game.entry);
    // autocomprobación del decodificador GAL: primeras 30 imágenes
    {
        let gals: Vec<&String> = names
            .iter()
            .filter(|n| n.to_lowercase().ends_with(".gal"))
            .take(30)
            .collect();
        let mut c: std::collections::BTreeMap<&str, u32> = Default::default();
        let mut bad = vec![];
        for n in &gals {
            let k = game
                .data
                .read(n)
                .map(|d| livenyan::gal::quick_class(&d))
                .unwrap_or_else(|e| format!("error: read {e}"));
            let class = if k.starts_with("ok") {
                "ok"
            } else if k.starts_with("error") {
                "error"
            } else if k == "all-black" {
                "all-black"
            } else {
                "all-transparent"
            };
            *c.entry(class).or_default() += 1;
            if k != "ok" {
                bad.push(format!("{n} ({k})"));
            }
        }
        println!(
            "gal check:   {} decoded: {}",
            gals.len(),
            c.iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        for b in bad.iter().take(30) {
            println!("  note: {b}");
        }
    }
    println!(
        "resolution:  {}x{} (source: {})",
        game.logical.0, game.logical.1, game.logical_source
    );
    println!("save dir:    {}", game.save_dir().display());
    println!("open time:   {:.1} ms", t0.elapsed().as_secs_f64() * 1000.0);
    // parseo de todos los LSB en paralelo
    let t = Instant::now();
    let lsbs: Vec<String> = names
        .iter()
        .filter(|n| n.to_lowercase().ends_with(".lsb"))
        .cloned()
        .collect();
    for n in &lsbs {
        assets.prefetch(n);
    }
    let mut counts: HashMap<&'static str, usize> = HashMap::new();
    let mut ok = 0;
    let mut failed = vec![];
    for n in &lsbs {
        match assets.script(n) {
            Some(s) => {
                ok += 1;
                for c in &s.commands {
                    *counts.entry(c.name()).or_default() += 1;
                }
            }
            None => failed.push(n.clone()),
        }
    }
    println!(
        "lsb:         {ok}/{} parsed in {:.1} ms ({} threads)",
        lsbs.len(),
        t.elapsed().as_secs_f64() * 1000.0,
        cfg.threads()
    );
    for f in &failed {
        println!("  FAILED {f}");
    }
    let mut counts: Vec<_> = counts.into_iter().collect();
    counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    println!("commands:    {} types", counts.len());
    for (n, c) in counts {
        println!("  {n:<14} {c}");
    }
    Ok(())
}

fn dump(game: &Game, name: &str, show: bool) -> Result<(), String> {
    let n = livenyan::game::resolve_lsb(&game.data, name)
        .ok_or_else(|| format!("{name}: not found"))?;
    let raw = game.data.read(&n).map_err(|e| e.to_string())?;
    let l = Lsb::parse(&n, &raw).map_err(|e| e.0)?;
    println!(
        "# {} version={} commands={}",
        l.name,
        l.version,
        l.commands.len()
    );
    for (i, c) in l.commands.iter().enumerate() {
        let mut parts = vec![];
        for (k, f) in &c.fields {
            let s = match f {
                Field::Expr(e) if e.is_empty() => continue,
                // nombres de objeto / destinos no son texto del juego
                Field::Expr(e) => {
                    e.render(!show && !matches!(*k, "Target" | "Name" | "ObjName" | "ObjProp"))
                }
                Field::Exprs(es) => format!(
                    "[{}]",
                    es.iter()
                        .map(|e| e.render(!show))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                Field::Str(s)
                    if *k == "Name" || *k == "Result" || *k == "VarName" || *k == "Page" =>
                {
                    format!("{s:?}")
                }
                Field::Str(s) => {
                    if show {
                        format!("{s:?}")
                    } else {
                        format!("<{} chars>", s.chars().count())
                    }
                }
                Field::U32(v) => v.to_string(),
                Field::Byte(v) => v.to_string(),
                Field::Page(p) => format!("{}:{}", p.page, p.label),
                Field::Text(t) => {
                    if show {
                        format!("{:?}", t.plain_text())
                    } else {
                        format!("<TpWord v{} {} chars>", t.version, t.char_count())
                    }
                }
                Field::Props(ps) => ps
                    .iter()
                    .filter(|(_, e)| !e.is_empty())
                    .map(|(p, e)| {
                        format!(
                            "{}={}",
                            livenyan::lsb::prop_name(*p),
                            e.render(!show_prop(*p, show))
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            };
            parts.push(format!("{k}: {s}"));
        }
        println!(
            "{i:5} L{:<5} {}{} {}",
            c.line_no,
            "  ".repeat(c.indent.min(16) as usize),
            c.name(),
            parts.join("; ")
        );
    }
    Ok(())
}

/// Propiedades cuyo contenido no es texto del juego (nombres, rutas, números).
fn show_prop(p: u8, show: bool) -> bool {
    use livenyan::lsb::Prop;
    show || matches!(p, Prop::NAME | Prop::SOURCE | Prop::PARENT)
        || p != Prop::TEXT && p != Prop::CAPTUREMASK
}

fn trace(mut vm: Vm, steps: u64, t0: Instant) -> Result<(), String> {
    let mut t = 0.0;
    let mut clicks = 0;
    let mut menus = 0;
    let mut fired = 0;
    let mut idle = 0;
    let mut cov_idle = 0u64;
    let mut last_sig = (String::new(), 0u64);
    while vm.steps < steps && !vm.ended() {
        vm.update(t, (steps - vm.steps).min(100_000) as usize);
        let mut acted = false;
        if vm.wants_click() {
            clicks += 1;
            vm.click();
            acted = true;
        }
        match &vm.state {
            State::Menu { .. } => {
                menus += 1;
                vm.choose(0);
                acted = true;
            }
            State::WaitUntil { until, .. } => t = t.max(*until),
            _ => {}
        }
        t += 16.0;
        let sig = vm.progress_sig();
        // idle: sin comandos nuevos ni clics -> el script espera algo (botón, etc.)
        // cov_idle: sin comandos nuevos aunque haya clics de texto -> p.ej. el título en bucle
        let new_code = sig != last_sig;
        idle = if new_code || acted { 0 } else { idle + 1 };
        cov_idle = if new_code { 0 } else { cov_idle + 1 };
        last_sig = sig;
        // a partir de 60 frames sin código nuevo, intentar cada frame hasta que haya un menú
        // (en un bucle de título el menú puede existir solo en algunos frames del ciclo)
        if cov_idle >= 60 {
            if let Some(m) = vm.auto_menu() {
                menus += 1;
                cov_idle = 0;
                println!("[{t:>8.0}ms] auto-select menu {m}");
            }
        }
        // el script espera y no hay texto pendiente: clic en la caja de mensajes
        if idle > 0 && idle % 30 == 0 && idle % 60 != 0 {
            if let Some(m) = vm.click_message() {
                clicks += 1;
                println!("[{t:>8.0}ms] auto-click on screen -> {m}");
            }
        }
        if idle > 0 && idle % 60 == 0 {
            if let Some(o) = vm.auto_fire(fired) {
                fired += 1;
                println!("[{t:>8.0}ms] auto-click on object {o}");
            }
        }
        if idle > 3000 {
            println!("# stalled at {}", vm.location());
            break;
        }
    }
    println!(
        "# trace end: {} steps, {:.0} ms simulated, {} clicks, {} menus, state={}, wall {:.1} ms",
        vm.steps,
        t,
        clicks,
        menus,
        match vm.state {
            State::Ended => "ended",
            State::Running => "running",
            _ => "waiting",
        },
        t0.elapsed().as_secs_f64() * 1000.0
    );
    if vm.unimpl.is_empty() {
        println!("# unimplemented: none");
    } else {
        println!("# unimplemented ({} unique):", vm.unimpl.len());
        for (k, n) in &vm.unimpl {
            println!("#   {k} x{n}");
        }
    }
    println!("# auto-clicks on objects: {fired}");
    let mut uniq: Vec<&String> = vm
        .script_log
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    uniq.sort();
    println!(
        "# scripts entered: {} ({} distinct)",
        vm.script_log.len(),
        uniq.len()
    );
    match vm.cycle() {
        Some((p, reps)) => {
            let tail = &vm.script_log[vm.script_log.len() - p..];
            println!(
                "# cycling: last {p} script entries repeated {reps}x: {}",
                tail.join(" > ")
            );
        }
        None => println!("# cycling: no"),
    }
    let mut edges: Vec<(&String, &u64)> = vm.edges.iter().collect();
    edges.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    println!(
        "# jump/call chain ({} distinct edges, most used first):",
        edges.len()
    );
    for (e, n) in edges.iter().take(40) {
        println!("#   {e} x{n}");
    }
    println!(
        "# counts: {}",
        vm.counts_sorted()
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    Ok(())
}
