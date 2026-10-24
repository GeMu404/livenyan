//! Bucle principal con SDL3: ventana redimensionable, resolución lógica fija con
//! letterbox, render de la escena y entrada.

use crate::assets::{Asset, Assets, IMAGE_EXTS};
use crate::audio::Audio;
use crate::config::Config;
use crate::game::Game;
use crate::input::{Action, Keymap};
use crate::lsb::Prop;
use crate::scene::{Kind, Obj, TextItem};
use crate::text::{tcolor, TextRenderer};
use crate::vm::{Event as VmEvent, State, Vm};
use sdl3::event::{Event, WindowEvent};
use sdl3::keyboard::Mod;
use sdl3::mouse::MouseButton;
use sdl3::pixels::{Color, PixelFormat};

use sdl3::render::{BlendMode, Canvas, FRect, ScaleMode, Texture, TextureCreator};
use sdl3::video::{Window, WindowContext};
use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Instant;

struct Gfx {
    images: HashMap<String, (Texture, u32, u32)>,
    texts: HashMap<String, (u64, usize, Texture)>,
    menu: Option<(Vec<String>, Vec<Texture>)>,
}

fn upload(tc: &TextureCreator<WindowContext>, w: u32, h: u32, rgba: &[u8], filter: ScaleMode) -> Option<Texture> {
    let mut t = tc.create_texture_static(PixelFormat::RGBA32, w, h).ok()?;
    t.update(None, rgba, (w * 4) as usize).ok()?;
    t.set_blend_mode(BlendMode::Blend);
    t.set_scale_mode(filter);
    Some(t)
}

pub fn run(game: Game, cfg: Config, assets: Arc<Assets>, mut vm: Vm, t0: Instant) -> Result<(), String> {
    let sdl = sdl3::init().map_err(|e| e.to_string())?;
    let video = sdl.video().map_err(|e| e.to_string())?;
    log::info!("video driver: {}", video.current_video_driver());
    let (lw, lh) = game.logical;
    let (ww, wh) = if cfg.window.width > 0 && cfg.window.height > 0 { (cfg.window.width, cfg.window.height) } else { (lw, lh) };
    let mut window = video
        .window(&format!("livenyan — {}", game.title), ww, wh)
        .resizable()
        .high_pixel_density()
        .build()
        .map_err(|e| e.to_string())?;
    if cfg.window.fullscreen {
        let _ = window.set_fullscreen(true);
    }
    let mut canvas: Canvas<Window> = window.into_canvas();
    canvas
        .set_logical_size(lw, lh, sdl3::sys::render::SDL_LOGICAL_PRESENTATION_LETTERBOX)
        .map_err(|e| e.to_string())?;
    unsafe {
        sdl3::sys::render::SDL_SetRenderVSync(canvas.raw(), if cfg.window.vsync { 1 } else { 0 });
    }
    let tc = canvas.texture_creator();
    let filter = if cfg.window.scale_filter == "nearest" { ScaleMode::Nearest } else { ScaleMode::Linear };

    // primer frame cuanto antes
    canvas.set_draw_color(Color::RGB(0, 0, 0));
    canvas.clear();
    canvas.present();
    log::info!("first frame after {:.1} ms", t0.elapsed().as_secs_f64() * 1000.0);

    // fuente en segundo plano
    let (ftx, frx) = mpsc::channel();
    {
        let (fp, fi) = (cfg.text.font.clone(), cfg.text.font_index);
        std::thread::spawn(move || {
            let t = Instant::now();
            let r = crate::text::find_font(fp.as_deref(), fi)
                .ok_or_else(|| "no Japanese-capable font found (set text.font in config)".to_string())
                .and_then(|(p, i)| TextRenderer::load(&p, i).map(|r| (r, p)));
            if let Ok((_, p)) = &r {
                log::info!("font {} loaded in {:.1} ms", p.display(), t.elapsed().as_secs_f64() * 1000.0);
            }
            let _ = ftx.send(r.map(|(r, _)| r));
        });
    }
    let mut font: Option<TextRenderer> = None;

    let mut audio = if cfg.audio.enabled {
        match sdl.audio().map_err(|e| e.to_string()).and_then(|a| Audio::new(a, cfg.audio.volume)) {
            Ok(a) => Some(a),
            Err(e) => {
                log::warn!("audio disabled: {e}");
                None
            }
        }
    } else {
        None
    };

    let keys = Keymap::new(&cfg.keys);
    let mut pump = sdl.event_pump().map_err(|e| e.to_string())?;
    let mut gfx = Gfx { images: HashMap::new(), texts: HashMap::new(), menu: None };
    let start = Instant::now();
    let mut menu_sel = 0usize;
    let mut mouse = (0.0f32, 0.0f32);
    let mut first_scene_logged = false;

    'main: loop {
        for ev in pump.poll_iter() {
            let ev = ev.get_converted_coords(&canvas).unwrap_or(ev);
            log::trace!("{ev:?}");
            match ev {
                Event::Quit { .. } => break 'main,
                Event::Window { win_event: WindowEvent::PixelSizeChanged(..) | WindowEvent::Resized(..), .. } => {
                    // la presentación lógica de SDL ya recalcula el letterbox
                }
                Event::KeyDown { keycode: Some(k), keymod, repeat, .. } => {
                    let alt_enter = (keymod.intersects(Mod::LALTMOD | Mod::RALTMOD)) && k == sdl3::keyboard::Keycode::Return;
                    let act = if alt_enter { Some(Action::Fullscreen) } else { keys.action(&k.name()) };
                    match act {
                        Some(Action::Fullscreen) if !repeat => {
                            let w = canvas.window_mut();
                            let fs = !matches!(w.fullscreen_state(), sdl3::video::FullscreenType::Off);
                            let _ = w.set_fullscreen(!fs);
                        }
                        Some(Action::Quit) => break 'main,
                        Some(Action::Skip) => vm.skip = true,
                        Some(Action::MenuUp) => menu_sel = menu_sel.saturating_sub(1),
                        Some(Action::MenuDown) => menu_sel += 1,
                        Some(Action::Advance) => {
                            if let State::Menu { choices, .. } = &vm.state {
                                let i = menu_sel.min(choices.len().saturating_sub(1));
                                vm.choose(i);
                            } else {
                                vm.click();
                            }
                        }
                        _ => {}
                    }
                }
                Event::KeyUp { keycode: Some(k), .. } => {
                    if keys.action(&k.name()) == Some(Action::Skip) {
                        vm.skip = false;
                    }
                }
                Event::MouseMotion { x, y, .. } => mouse = (x, y),
                Event::MouseButtonDown { mouse_btn: MouseButton::Left, x, y, .. } => {
                    log::debug!("click at logical ({x:.0},{y:.0})");
                    mouse = (x, y);
                    if let State::Menu { choices, .. } = &vm.state {
                        if let Some(i) = menu_hit(choices.len(), lw, lh, x, y) {
                            vm.choose(i);
                        }
                    } else {
                        vm.click_at(x as f64, y as f64);
                    }
                }
                Event::MouseWheel { y, .. } if y < 0.0 => vm.click(),
                _ => {}
            }
        }
        if let State::Menu { choices, .. } = &vm.state {
            menu_sel = menu_sel.min(choices.len().saturating_sub(1));
            if let Some(i) = menu_hit(choices.len(), lw, lh, mouse.0, mouse.1) {
                menu_sel = i;
            }
        }
        if font.is_none() {
            if let Ok(r) = frx.try_recv() {
                match r {
                    Ok(f) => font = Some(f),
                    Err(e) => log::error!("{e}"),
                }
            }
        }

        let now = start.elapsed().as_secs_f64() * 1000.0;
        vm.update(now, 20_000);
        for e in vm.events.drain(..) {
            match e {
                VmEvent::PlaySound { channel, file, volume, repeat } => {
                    if let Some(a) = audio.as_mut() {
                        a.play(channel, file, volume, repeat)
                    }
                }
                VmEvent::StopSound { channel } => {
                    if let Some(a) = audio.as_mut() {
                        a.stop(&channel)
                    }
                }
                VmEvent::MovieSkipped { .. } => {}
            }
        }
        if let Some(a) = audio.as_mut() {
            a.update(&assets);
        }
        if vm.ended() {
            log::info!("script ended");
            break;
        }

        // ---- render ----
        canvas.set_draw_color(Color::RGB(0, 0, 0));
        canvas.clear();
        draw_scene(&mut canvas, &tc, &assets, &mut vm, &mut gfx, font.as_mut(), &cfg, filter, (lw, lh));
        if let State::Menu { choices, .. } = &vm.state {
            draw_menu(&mut canvas, &tc, &mut gfx, font.as_mut(), choices, menu_sel, (lw, lh), filter);
        } else {
            gfx.menu = None;
        }
        canvas.present();
        if !first_scene_logged && !vm.scene.objs.is_empty() {
            first_scene_logged = true;
            log::info!("first scene frame after {:.1} ms", t0.elapsed().as_secs_f64() * 1000.0);
        }
        if !cfg.window.vsync {
            std::thread::sleep(std::time::Duration::from_millis(4));
        }
    }
    Ok(())
}

fn menu_rects(n: usize, lw: u32, lh: u32) -> Vec<FRect> {
    let (bw, bh, gap) = (lw as f32 * 0.6, 40.0f32, 12.0f32);
    let total = n as f32 * (bh + gap) - gap;
    let y0 = (lh as f32 - total) / 2.0;
    (0..n).map(|i| FRect::new((lw as f32 - bw) / 2.0, y0 + i as f32 * (bh + gap), bw, bh)).collect()
}

fn menu_hit(n: usize, lw: u32, lh: u32, x: f32, y: f32) -> Option<usize> {
    menu_rects(n, lw, lh).iter().position(|r| x >= r.x && y >= r.y && x < r.x + r.w && y < r.y + r.h)
}

#[allow(clippy::too_many_arguments)]
fn draw_menu(
    canvas: &mut Canvas<Window>,
    tc: &TextureCreator<WindowContext>,
    gfx: &mut Gfx,
    font: Option<&mut TextRenderer>,
    choices: &[String],
    sel: usize,
    (lw, lh): (u32, u32),
    filter: ScaleMode,
) {
    let rects = menu_rects(choices.len(), lw, lh);
    if gfx.menu.as_ref().is_none_or(|(c, _)| c != choices) {
        if let Some(f) = font {
            let texs = choices
                .iter()
                .zip(&rects)
                .filter_map(|(c, r)| {
                    let items: Vec<TextItem> = c.chars().map(|ch| TextItem::Char(ch.to_string())).collect();
                    let img = f.render(&items, r.w as u32 - 16, r.h as u32, 24.0, [255, 255, 255, 255], false);
                    upload(tc, img.w, img.h, &img.rgba, filter)
                })
                .collect();
            gfx.menu = Some((choices.to_vec(), texs));
        }
    }
    canvas.set_blend_mode(BlendMode::Blend);
    for (i, r) in rects.iter().enumerate() {
        canvas.set_draw_color(if i == sel { Color::RGBA(60, 80, 160, 220) } else { Color::RGBA(0, 0, 0, 180) });
        let _ = canvas.fill_rect(*r);
        if let Some((_, texs)) = &gfx.menu {
            if let Some(t) = texs.get(i) {
                let q = t.query();
                let _ = canvas.copy(t, None, FRect::new(r.x + 8.0, r.y + 6.0, q.width as f32, q.height as f32));
            }
        }
    }
}

fn image_for(
    tc: &TextureCreator<WindowContext>,
    assets: &Assets,
    gfx: &mut Gfx,
    src: &str,
    filter: ScaleMode,
) -> Option<String> {
    let name = assets.data().resolve(src, IMAGE_EXTS)?;
    if !gfx.images.contains_key(&name) {
        if let Some(Asset::Image(img)) = assets.try_get(&name) {
            let t = upload(tc, img.w, img.h, &img.rgba, filter)?;
            gfx.images.insert(name.clone(), (t, img.w, img.h));
        } else {
            return None;
        }
    }
    Some(name)
}

#[allow(clippy::too_many_arguments)]
fn draw_scene(
    canvas: &mut Canvas<Window>,
    tc: &TextureCreator<WindowContext>,
    assets: &Assets,
    vm: &mut Vm,
    gfx: &mut Gfx,
    mut font: Option<&mut TextRenderer>,
    cfg: &Config,
    filter: ScaleMode,
    (lw, lh): (u32, u32),
) {
    let partial = vm.partial_text().map(|(t, items)| (t.to_string(), items.to_vec()));
    let order: Vec<Obj> = vm.scene.draw_order().into_iter().cloned().collect();
    let mut sizes: Vec<(String, u32, u32)> = vec![];
    for o in &order {
        if !vm.scene.effective_visible(o) {
            continue;
        }
        let (x, y) = vm.scene.abs_pos(o);
        let alpha = o.int(Prop::ALPHA, 255).clamp(0, 255) as u8;
        let (ow, oh) = (o.int(Prop::WIDTH, 0), o.int(Prop::HEIGHT, 0));
        match o.kind {
            Kind::Image | Kind::Button | Kind::Tile | Kind::Slider => {
                let src = o.str(Prop::SOURCE);
                if src.is_empty() {
                    continue;
                }
                let Some(key) = image_for(tc, assets, gfx, &src, filter) else { continue };
                let (t, iw, ih) = gfx.images.get_mut(&key).unwrap();
                let (iw, ih) = (*iw, *ih);
                if ow <= 0 || oh <= 0 {
                    sizes.push((o.name.clone(), iw, ih));
                }
                t.set_alpha_mod(alpha);
                let (dw, dh) = (if ow > 0 { ow as f32 } else { iw as f32 }, if oh > 0 { oh as f32 } else { ih as f32 });
                if o.kind == Kind::Tile {
                    let mut ty = 0.0;
                    while ty < dh {
                        let mut tx = 0.0;
                        while tx < dw {
                            let cw = (iw as f32).min(dw - tx);
                            let ch = (ih as f32).min(dh - ty);
                            let _ = canvas.copy(t, FRect::new(0.0, 0.0, cw, ch), FRect::new(x as f32 + tx, y as f32 + ty, cw, ch));
                            tx += iw as f32;
                        }
                        ty += ih as f32;
                    }
                } else {
                    let _ = canvas.copy(t, None, FRect::new(x as f32, y as f32, dw, dh));
                }
            }
            Kind::Box => {
                let c = tcolor(o.int(Prop::COLOR, 0), alpha);
                canvas.set_blend_mode(BlendMode::Blend);
                canvas.set_draw_color(Color::RGBA(c[0], c[1], c[2], c[3]));
                let w = if ow > 0 { ow as f32 } else { lw as f32 };
                let h = if oh > 0 { oh as f32 } else { lh as f32 };
                let _ = canvas.fill_rect(FRect::new(x as f32, y as f32, w, h));
            }
            Kind::Message => {
                let Some(f) = font.as_deref_mut() else { continue };
                let w = if ow > 0 { ow as u32 } else { lw.saturating_sub(x.max(0.0) as u32) };
                let h = if oh > 0 { oh as u32 } else { lh.saturating_sub(y.max(0.0) as u32) };
                let mut items = o.text.clone();
                let mut extra = 0;
                if let Some((t, p)) = &partial {
                    if *t == o.name {
                        extra = p.len();
                        items.extend(p.iter().cloned());
                    }
                }
                if items.is_empty() {
                    continue;
                }
                let stale = gfx.texts.get(&o.name).is_none_or(|(rev, ex, _)| *rev != o.text_rev || *ex != extra);
                if stale {
                    let px = o.get(Prop::FONTHEIGHT).map(|v| v.as_f64() as f32).filter(|v| *v > 4.0).unwrap_or(cfg.text.size);
                    let color = o.get(Prop::FONTCOLOR).map(|v| tcolor(v.as_int(), 255)).unwrap_or(cfg.text.color);
                    let img = f.render(&items, w, h, px, color, cfg.text.shadow);
                    if let Some(t) = upload(tc, img.w, img.h, &img.rgba, filter) {
                        gfx.texts.insert(o.name.clone(), (o.text_rev, extra, t));
                    }
                }
                if let Some((_, _, t)) = gfx.texts.get_mut(&o.name) {
                    t.set_alpha_mod(alpha);
                    let q = t.query();
                    let _ = canvas.copy(t, None, FRect::new(x as f32, y as f32, q.width as f32, q.height as f32));
                }
            }
            _ => {}
        }
    }
    // tamaños intrínsecos para hit-testing y GetProp
    for (n, w, h) in sizes {
        if let Some(o) = vm.scene.get_mut(&n) {
            o.props.insert(Prop::WIDTH, crate::lsb::expr::Value::Int(w as i64));
            o.props.insert(Prop::HEIGHT, crate::lsb::expr::Value::Int(h as i64));
        }
    }
    gfx.texts.retain(|k, _| vm.scene.objs.contains_key(k));
}
