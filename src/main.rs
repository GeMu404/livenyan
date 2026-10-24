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
    /// Ejecuta el VM sin ventana registrando los comandos.
    #[arg(long)]
    trace: bool,
    /// Pasos máximos para --trace.
    #[arg(long, default_value_t = 5000)]
    steps: u64,
    /// Incluye el texto del juego en la salida (por defecto se oculta).
    #[arg(long)]
    show_text: bool,
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
    Ok((w.parse().map_err(|_| "bad width")?, h.parse().map_err(|_| "bad height")?))
}

fn main() {
    let t0 = Instant::now();
    env_logger::Builder::from_env(env_logger::Env::new().filter_or("LIVENYAN_LOG", "warn")).format_timestamp_millis().init();
    let cli = Cli::parse();
    if let Err(e) = real_main(cli, t0) {
        eprintln!("livenyan: {e}");
        std::process::exit(1);
    }
}

fn real_main(cli: Cli, t0: Instant) -> Result<(), String> {
    let mut cfg = Config::load(cli.config.as_deref())?;
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
    let assets = Arc::new(Assets::new(game.data.clone(), cfg.threads(), cfg.assets.cache_mb));

    if cli.info {
        return info(&game, &assets, &cfg, t0);
    }
    if cli.dump_lsb {
        let name = cli.script.as_deref().unwrap_or(&game.entry);
        return dump(&game, name, cli.show_text);
    }
    let opts = Options {
        text_cps: if cli.trace { 0.0 } else { cfg.text.speed_cps },
        lookahead: cfg.assets.prefetch_lookahead,
        trace: cli.trace,
        show_text: cli.show_text,
    };
    let vm = Vm::new(assets.clone(), &game.entry, opts)?;
    if cli.trace {
        return trace(vm, cli.steps, t0);
    }
    if let Err(e) = std::fs::create_dir_all(game.save_dir()) {
        log::warn!("cannot create save dir {}: {e}", game.save_dir().display());
    }
    livenyan::app::run(game, cfg, assets, vm, t0)
}

fn info(game: &Game, assets: &Assets, cfg: &Config, t0: Instant) -> Result<(), String> {
    let names = game.data.names();
    let mut exts: HashMap<String, usize> = HashMap::new();
    for n in names {
        let e = n.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
        *exts.entry(e).or_default() += 1;
    }
    let mut exts: Vec<_> = exts.into_iter().collect();
    exts.sort_by(|a, b| b.1.cmp(&a.1));
    println!("archive:     {}", game.data.describe());
    println!("entries:     {}", names.len());
    println!("by type:     {}", exts.iter().map(|(e, n)| format!(".{e} {n}")).collect::<Vec<_>>().join(", "));
    if let Some(p) = &game.project {
        println!("project:     lpb v{} init={:?} exit={:?} settings={}", p.version, p.init_lsb, p.exit_lsb, p.settings.len());
    } else {
        println!("project:     (no .lpb)");
    }
    println!("entry:       {}", game.entry);
    println!("resolution:  {}x{} (source: {})", game.logical.0, game.logical.1, game.logical_source);
    println!("save dir:    {}", game.save_dir().display());
    println!("open time:   {:.1} ms", t0.elapsed().as_secs_f64() * 1000.0);
    // parseo de todos los LSB en paralelo
    let t = Instant::now();
    let lsbs: Vec<String> = names.iter().filter(|n| n.to_lowercase().ends_with(".lsb")).cloned().collect();
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
    println!("lsb:         {ok}/{} parsed in {:.1} ms ({} threads)", lsbs.len(), t.elapsed().as_secs_f64() * 1000.0, cfg.threads());
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
    let n = livenyan::game::resolve_lsb(&game.data, name).ok_or_else(|| format!("{name}: not found"))?;
    let raw = game.data.read(&n).map_err(|e| e.to_string())?;
    let l = Lsb::parse(&n, &raw).map_err(|e| e.0)?;
    println!("# {} version={} commands={}", l.name, l.version, l.commands.len());
    for (i, c) in l.commands.iter().enumerate() {
        let mut parts = vec![];
        for (k, f) in &c.fields {
            let s = match f {
                Field::Expr(e) if e.is_empty() => continue,
                // nombres de objeto / destinos no son texto del juego
                Field::Expr(e) => e.render(!show && !matches!(*k, "Target" | "Name" | "ObjName" | "ObjProp")),
                Field::Exprs(es) => format!("[{}]", es.iter().map(|e| e.render(!show)).collect::<Vec<_>>().join(", ")),
                Field::Str(s) if *k == "Name" || *k == "Result" || *k == "VarName" || *k == "Page" => format!("{s:?}"),
                Field::Str(s) => if show { format!("{s:?}") } else { format!("<{} chars>", s.chars().count()) },
                Field::U32(v) => v.to_string(),
                Field::Byte(v) => v.to_string(),
                Field::Page(p) => format!("{}:{}", p.page, p.label),
                Field::Text(t) => {
                    if show { format!("{:?}", t.plain_text()) } else { format!("<TpWord v{} {} chars>", t.version, t.char_count()) }
                }
                Field::Props(ps) => ps
                    .iter()
                    .filter(|(_, e)| !e.is_empty())
                    .map(|(p, e)| format!("{}={}", livenyan::lsb::prop_name(*p), e.render(!show_prop(*p, show))))
                    .collect::<Vec<_>>()
                    .join(" "),
            };
            parts.push(format!("{k}: {s}"));
        }
        println!("{i:5} L{:<5} {}{} {}", c.line_no, "  ".repeat(c.indent.min(16) as usize), c.name(), parts.join("; "));
    }
    Ok(())
}

/// Propiedades cuyo contenido no es texto del juego (nombres, rutas, números).
fn show_prop(p: u8, show: bool) -> bool {
    use livenyan::lsb::Prop;
    show || matches!(p, Prop::NAME | Prop::SOURCE | Prop::PARENT) || p != Prop::TEXT && p != Prop::CAPTUREMASK
}

fn trace(mut vm: Vm, steps: u64, t0: Instant) -> Result<(), String> {
    let mut t = 0.0;
    let mut clicks = 0;
    let mut menus = 0;
    let mut idle = 0;
    while vm.steps < steps && !vm.ended() {
        let before = vm.steps;
        vm.update(t, (steps - vm.steps) as usize);
        match &vm.state {
            State::WaitClick { .. } | State::Text { .. } => {
                clicks += 1;
                vm.click();
            }
            State::Menu { .. } => {
                menus += 1;
                vm.choose(0);
            }
            State::WaitUntil { until, .. } => t = t.max(*until),
            _ => {}
        }
        t += 16.0;
        idle = if vm.steps == before { idle + 1 } else { 0 };
        if idle > 1000 {
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
    println!("# counts: {}", vm.counts_sorted().iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" "));
    Ok(())
}
