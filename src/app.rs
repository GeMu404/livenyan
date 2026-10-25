//! Bucle principal con SDL3: ventana redimensionable, resolución lógica fija con
//! letterbox, render de la escena y entrada.

use crate::assets::{Asset, Assets, IMAGE_EXTS};
use crate::audio::Audio;
use crate::config::Config;
use crate::game::Game;
use crate::input::{Action, Keymap};
use crate::interpolation::InterpolationMode;
use crate::lsb::{Prop, expr::Value};
use crate::scene::{Kind, Obj, Scene, TextItem, menu_line_height, native_menu_size};
use crate::slider::{SliderPart, slider_size};
use crate::text::{TextRenderer, tcolor};
use crate::vm::{Event as VmEvent, State, Vm};
use sdl3::event::{Event, WindowEvent};
use sdl3::keyboard::Mod;
use sdl3::mouse::{MouseButton, MouseWheelDirection};
use sdl3::pixels::{Color, PixelFormat};

use sdl3::render::{BlendMode, Canvas, FRect, ScaleMode, Texture, TextureCreator};
use sdl3::video::{Window, WindowContext};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Instant;

struct Gfx {
    images: HashMap<String, (Texture, u32, u32)>,
    video_textures: HashMap<String, (Texture, u64, u64, u32, u32)>,
    texts: HashMap<String, (u64, usize, Texture)>,
    debug_labels: HashMap<String, (String, Texture, u32, u32)>,
    menu_animation_starts: HashMap<String, f64>,
    menu_fonts: HashMap<String, Option<TextRenderer>>,
    toolbar_texts: HashMap<String, (usize, Texture)>,
    crossfade_safety: HashMap<String, bool>,
    animation_delays: HashMap<String, Vec<u32>>,
    menu: Option<MenuCache>,
}

struct MenuCache {
    choices: Vec<String>,
    font_family: String,
    style: Vec<i64>,
    textures: Vec<Option<Texture>>,
}

#[derive(Clone, Copy)]
enum ToolbarAction {
    Quit,
    Fullscreen,
    Options,
    Interpolation(InterpolationMode),
}

fn toolbar_button_rects(width: u32) -> [(ToolbarAction, FRect); 3] {
    let y = 3.0;
    let h = 24.0;
    let gap = 5.0;
    let right = width as f32 - 6.0;
    let options = FRect::new(right - 76.0, y, 76.0, h);
    let fullscreen = FRect::new(options.x - gap - 104.0, y, 104.0, h);
    let quit = FRect::new(fullscreen.x - gap - 54.0, y, 54.0, h);
    [
        (ToolbarAction::Quit, quit),
        (ToolbarAction::Fullscreen, fullscreen),
        (ToolbarAction::Options, options),
    ]
}

fn toolbar_hit(width: u32, x: f32, y: f32, options_open: bool) -> Option<ToolbarAction> {
    for (action, rect) in toolbar_button_rects(width) {
        if x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h {
            return Some(action);
        }
    }
    if options_open {
        for (mode, rect) in interpolation_option_rects(width) {
            if x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h {
                return Some(ToolbarAction::Interpolation(mode));
            }
        }
    }
    None
}

fn toolbar_contains(width: u32, x: f32, y: f32, options_open: bool) -> bool {
    (y >= 0.0 && y < 32.0 && x >= 0.0 && x < width as f32)
        || (options_open
            && x >= width as f32 - 340.0
            && x < width as f32 - 6.0
            && (32.0..138.0).contains(&y))
}

fn point_in_logical_render(point: (f32, f32), logical: (u32, u32)) -> bool {
    point.0.is_finite()
        && point.1.is_finite()
        && point.0 >= 0.0
        && point.1 >= 0.0
        && point.0 < logical.0 as f32
        && point.1 < logical.1 as f32
}

fn interpolation_option_rects(width: u32) -> [(InterpolationMode, FRect); 2] {
    let x = width as f32 - 212.0;
    let y = 77.0;
    let w = 96.0;
    let h = 27.0;
    let gap = 8.0;
    [
        (InterpolationMode::Off, FRect::new(x, y, w, h)),
        (
            InterpolationMode::Crossfade,
            FRect::new(x + w + gap, y, w, h),
        ),
    ]
}

fn gal_frame_pixels(image: &crate::gal::Image, frame: usize) -> Option<&[u8]> {
    if frame == 0 {
        Some(&image.rgba)
    } else {
        image.frames.get(frame - 1).map(Vec::as_slice)
    }
}

fn frame_is_opaque(image: &crate::gal::Image, frame: usize) -> bool {
    gal_frame_pixels(image, frame)
        .is_some_and(|pixels| pixels.chunks_exact(4).all(|pixel| pixel[3] == u8::MAX))
}

fn can_crossfade_frames(
    image: &crate::gal::Image,
    from: usize,
    to: usize,
    object_alpha: u8,
) -> bool {
    object_alpha == u8::MAX && frame_is_opaque(image, from) && frame_is_opaque(image, to)
}

fn interpolation_group_key(scene: &Scene, object: &Obj) -> String {
    if let Some(tag) = object.get(Prop::TAG) {
        return format!("tag:{tag:?}");
    }
    let parent = object.str(Prop::PARENT);
    let owner = if !parent.is_empty() && parent != crate::vm::ROOT && scene.get(&parent).is_some() {
        parent
    } else if !scene.children(&object.name, false).is_empty() {
        object.name.clone()
    } else {
        return format!("object:{}", object.name);
    };
    format!("parent:{owner}")
}

fn add_rect_to_bounds(bounds: &mut Option<(f64, f64, f64, f64)>, rect: (f64, f64, f64, f64)) {
    *bounds = Some(match *bounds {
        Some((left, top, right, bottom)) => (
            left.min(rect.0),
            top.min(rect.1),
            right.max(rect.2),
            bottom.max(rect.3),
        ),
        None => rect,
    });
}

fn inferred_mask_group(
    first: (f64, f64, f64, f64),
    first_delays: &[u32],
    second: (f64, f64, f64, f64),
    second_delays: &[u32],
) -> bool {
    if first_delays.len() < 2 || first_delays != second_delays {
        return false;
    }
    let first_area = (first.2 - first.0).max(0.0) * (first.3 - first.1).max(0.0);
    let second_area = (second.2 - second.0).max(0.0) * (second.3 - second.1).max(0.0);
    let smaller_area = first_area.min(second_area);
    if smaller_area <= 0.0 {
        return false;
    }
    let overlap_width = (first.2.min(second.2) - first.0.max(second.0)).max(0.0);
    let overlap_height = (first.3.min(second.3) - first.1.max(second.1)).max(0.0);
    overlap_width * overlap_height >= smaller_area * 0.9
}

fn interpolation_group_areas(
    scene: &Scene,
    objects: &[Obj],
    gfx: &Gfx,
    assets: &Assets,
) -> HashMap<String, f64> {
    let mut members = Vec::new();
    for candidate in objects
        .iter()
        .filter(|candidate| candidate.kind == Kind::Image && scene.effective_visible(candidate))
    {
        let (x, y) = scene.abs_pos(candidate);
        let source = candidate.str(Prop::SOURCE);
        let intrinsic_size = assets
            .data()
            .resolve(&source, IMAGE_EXTS)
            .and_then(|resolved| gfx.images.get(&resolved))
            .map(|(_, width, height)| (f64::from(*width), f64::from(*height)));
        let width = candidate.int(Prop::WIDTH, 0);
        let height = candidate.int(Prop::HEIGHT, 0);
        let Some((intrinsic_width, intrinsic_height)) = intrinsic_size else {
            continue;
        };
        let width = if width > 0 {
            width as f64
        } else {
            intrinsic_width
        };
        let height = if height > 0 {
            height as f64
        } else {
            intrinsic_height
        };
        if width <= 0.0 || height <= 0.0 {
            continue;
        }
        let Some(resolved) = assets.data().resolve(&source, IMAGE_EXTS) else {
            continue;
        };
        let rect = (x, y, x + width, y + height);
        let delays = gfx
            .animation_delays
            .get(&resolved)
            .filter(|delays| delays.len() > 1)
            .cloned();
        members.push((
            candidate.name.clone(),
            interpolation_group_key(scene, candidate),
            rect,
            delays,
        ));
    }

    let mut parents: Vec<usize> = (0..members.len()).collect();
    for first in 0..members.len() {
        if !members[first].1.starts_with("object:") {
            continue;
        }
        let Some(first_delays) = members[first].3.as_deref() else {
            continue;
        };
        for second in first + 1..members.len() {
            if !members[second].1.starts_with("object:") {
                continue;
            }
            let Some(second_delays) = members[second].3.as_deref() else {
                continue;
            };
            if inferred_mask_group(
                members[first].2,
                first_delays,
                members[second].2,
                second_delays,
            ) {
                let first_root = find_group_root(&mut parents, first);
                let second_root = find_group_root(&mut parents, second);
                if first_root != second_root {
                    parents[second_root] = first_root;
                }
            }
        }
    }

    let mut group_bounds = HashMap::new();
    let mut object_groups = Vec::with_capacity(members.len());
    for index in 0..members.len() {
        let key = if members[index].1.starts_with("object:") {
            format!("overlap:{}", find_group_root(&mut parents, index))
        } else {
            members[index].1.clone()
        };
        add_rect_to_bounds(
            group_bounds.entry(key.clone()).or_insert(None),
            members[index].2,
        );
        object_groups.push((members[index].0.clone(), key));
    }
    let group_areas: HashMap<String, f64> = group_bounds
        .into_iter()
        .map(|(key, bounds)| {
            let area = bounds.map_or(0.0, |(left, top, right, bottom)| {
                (right - left) * (bottom - top)
            });
            (key, area)
        })
        .collect();
    object_groups
        .into_iter()
        .filter_map(|(name, key)| group_areas.get(&key).map(|area| (name, *area)))
        .collect()
}

fn find_group_root(parents: &mut [usize], mut index: usize) -> usize {
    while parents[index] != index {
        let parent = parents[index];
        parents[index] = parents[parent];
        index = parents[index];
    }
    index
}

fn should_interpolate_gal(
    mode: InterpolationMode,
    kind: Kind,
    object_area: f64,
    screen_area: f64,
) -> bool {
    mode != InterpolationMode::Off
        && kind == Kind::Image
        && object_area.is_finite()
        && screen_area.is_finite()
        && object_area >= screen_area / 3.0
}

fn load_interpolation_preference(default: InterpolationMode) -> InterpolationMode {
    #[derive(serde::Deserialize)]
    #[serde(default)]
    struct UiPreferences {
        interpolation_mode: Option<String>,
        interpolate_animations: bool,
    }
    impl Default for UiPreferences {
        fn default() -> Self {
            Self {
                interpolation_mode: None,
                interpolate_animations: false,
            }
        }
    }
    let path = crate::util::config_dir().join("ui-preferences.toml");
    match std::fs::read_to_string(&path) {
        Ok(contents) => match toml::from_str::<UiPreferences>(&contents) {
            Ok(preferences) => preferences
                .interpolation_mode
                .as_deref()
                .and_then(InterpolationMode::parse)
                .unwrap_or(if preferences.interpolate_animations {
                    InterpolationMode::Crossfade
                } else {
                    default
                }),
            Err(error) => {
                log::warn!("cannot read UI preferences {}: {error}", path.display());
                default
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => default,
        Err(error) => {
            log::warn!("cannot read UI preferences {}: {error}", path.display());
            default
        }
    }
}

fn save_interpolation_preference(mode: InterpolationMode) -> Result<(), String> {
    let path = crate::util::config_dir().join("ui-preferences.toml");
    let parent = path
        .parent()
        .ok_or_else(|| format!("invalid UI preferences path {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    let contents = format!("interpolation_mode = \"{}\"\n", mode.as_str());
    std::fs::write(&path, contents)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

fn draw_toolbar_text(
    canvas: &mut Canvas<Window>,
    tc: &TextureCreator<WindowContext>,
    gfx: &mut Gfx,
    font: Option<&mut TextRenderer>,
    text: &str,
    x: f32,
    y: f32,
    width: u32,
    height: u32,
) {
    let Some(font) = font else {
        return;
    };
    let color = [245, 245, 250, 255];
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (text, width, height, 16_u32).hash(&mut hasher);
    let style = hasher.finish() as usize;
    let stale = gfx
        .toolbar_texts
        .get(text)
        .is_none_or(|(old_style, _)| *old_style != style);
    if stale {
        let items = text
            .chars()
            .map(|character| TextItem::Char(character.to_string()))
            .collect::<Vec<_>>();
        let image = font.render(&items, width, height, 16.0, color, false);
        if let Some(texture) = upload(tc, image.w, image.h, &image.rgba, ScaleMode::Linear) {
            gfx.toolbar_texts.insert(text.to_string(), (style, texture));
        }
    }
    if let Some((_, texture)) = gfx.toolbar_texts.get_mut(text) {
        let texture_height = texture.query().height as f32;
        let _ = canvas.copy(
            texture,
            None,
            FRect::new(
                x,
                y + (height as f32 - texture_height) / 2.0,
                width as f32,
                texture_height,
            ),
        );
    }
}

fn upload(
    tc: &TextureCreator<WindowContext>,
    w: u32,
    h: u32,
    rgba: &[u8],
    filter: ScaleMode,
) -> Option<Texture> {
    let mut t = tc.create_texture_static(PixelFormat::RGBA32, w, h).ok()?;
    t.update(None, rgba, (w * 4) as usize).ok()?;
    t.set_blend_mode(BlendMode::Blend);
    t.set_scale_mode(filter);
    Some(t)
}

/// Estado de dibujo de un objeto (para el registro de depuración).
struct Drawn {
    name: String,
    z: usize,
    rect: (f32, f32, f32, f32),
    alpha: u8,
    status: &'static str,
    extra: String,
}

pub fn run(
    game: Game,
    cfg: Config,
    assets: Arc<Assets>,
    mut vm: Vm,
    t0: Instant,
    snapshot: Option<(u64, std::path::PathBuf)>,
    debug_layout: bool,
) -> Result<(), String> {
    let mut interpolation_mode =
        load_interpolation_preference(if cfg.window.interpolate_animations {
            InterpolationMode::Crossfade
        } else {
            InterpolationMode::Off
        });
    let sdl = sdl3::init().map_err(|e| e.to_string())?;
    let video = sdl.video().map_err(|e| e.to_string())?;
    log::info!("video driver: {}", video.current_video_driver());
    let (lw, lh) = game.logical;
    let (ww, wh) = if cfg.window.width > 0 && cfg.window.height > 0 {
        (cfg.window.width, cfg.window.height)
    } else {
        (lw, lh)
    };
    let (ww, wh) = if snapshot.is_some() {
        (lw, lh)
    } else {
        (ww, wh)
    };
    let mut wb = video.window(&format!("livenyan — {}", game.title), ww, wh);
    wb.resizable();
    if snapshot.is_some() {
        wb.hidden();
    } else {
        wb.high_pixel_density();
    }
    let mut window = wb.build().map_err(|e| e.to_string())?;
    if cfg.window.fullscreen {
        let _ = window.set_fullscreen(true);
    }
    if !cfg.window.renderer.is_empty() {
        // probar el driver antes: si no funciona en este sistema, avisar y usar el automático
        let name = std::ffi::CString::new(cfg.window.renderer.as_str()).unwrap_or_default();
        let ok = unsafe {
            let r = sdl3::sys::render::SDL_CreateRenderer(window.raw(), name.as_ptr());
            if r.is_null() {
                false
            } else {
                sdl3::sys::render::SDL_DestroyRenderer(r);
                true
            }
        };
        if ok {
            sdl3::hint::set("SDL_RENDER_DRIVER", &cfg.window.renderer);
        } else {
            log::warn!(
                "render driver {:?} unavailable ({}); using automatic",
                cfg.window.renderer,
                sdl3::get_error()
            );
        }
    }
    let mut canvas: Canvas<Window> = window.into_canvas();
    let rname = unsafe {
        let p = sdl3::sys::render::SDL_GetRendererName(canvas.raw());
        if p.is_null() {
            "?".to_string()
        } else {
            std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    };
    log::info!(
        "render driver: {rname} (requested {:?})",
        if cfg.window.renderer.is_empty() {
            "auto"
        } else {
            cfg.window.renderer.as_str()
        }
    );
    canvas
        .set_logical_size(
            lw,
            lh,
            sdl3::sys::render::SDL_LOGICAL_PRESENTATION_LETTERBOX,
        )
        .map_err(|e| e.to_string())?;
    let mut vsync_active = unsafe {
        sdl3::sys::render::SDL_SetRenderVSync(
            canvas.raw(),
            if cfg.window.vsync || interpolation_mode != InterpolationMode::Off {
                1
            } else {
                0
            },
        )
    };
    let refresh_rate = canvas
        .window()
        .get_display()
        .ok()
        .and_then(|display| display.get_mode().ok())
        .map(|mode| mode.refresh_rate)
        .filter(|rate| rate.is_finite() && *rate >= 30.0)
        .unwrap_or(60.0)
        .clamp(30.0, 360.0);
    let frame_period = std::time::Duration::from_secs_f64(1.0 / f64::from(refresh_rate));
    if (cfg.window.vsync || interpolation_mode != InterpolationMode::Off) && !vsync_active {
        log::warn!(
            "renderer could not enable VSync; pacing fallback to detected display rate {refresh_rate:.2} Hz"
        );
    }
    let requested_vsync = cfg.window.vsync || interpolation_mode != InterpolationMode::Off;
    let mut frame_pacer = requested_vsync && !vsync_active;
    log::info!("display refresh rate: {refresh_rate:.2} Hz");
    let mut frame_deadline = Instant::now();
    let tc = canvas.texture_creator();
    let mut scene_frame = tc
        .create_texture_target(PixelFormat::RGBA32, lw, lh)
        .map_err(|e| format!("cannot create persistent scene frame: {e}"))?;
    scene_frame.set_blend_mode(BlendMode::None);
    let mut scene_frame_ready = false;
    let filter = if cfg.window.scale_filter == "nearest" {
        ScaleMode::Nearest
    } else {
        ScaleMode::Linear
    };

    // primer frame cuanto antes
    canvas.set_draw_color(Color::RGB(0, 0, 0));
    canvas.clear();
    canvas.present();
    log::info!(
        "first frame after {:.1} ms",
        t0.elapsed().as_secs_f64() * 1000.0
    );

    // fuente en segundo plano
    let (ftx, frx) = mpsc::channel();
    {
        let (fp, fi) = (cfg.text.font.clone(), cfg.text.font_index);
        std::thread::spawn(move || {
            let t = Instant::now();
            let r = crate::text::find_font(fp.as_deref(), fi)
                .ok_or_else(|| {
                    "no Japanese-capable font found (set text.font in config)".to_string()
                })
                .and_then(|(p, i)| TextRenderer::load(&p, i).map(|r| (r, p)));
            if let Ok((_, p)) = &r {
                log::info!(
                    "font {} loaded in {:.1} ms",
                    p.display(),
                    t.elapsed().as_secs_f64() * 1000.0
                );
            }
            let _ = ftx.send(r.map(|(r, _)| r));
        });
    }
    let mut font: Option<TextRenderer> = None;

    let mut audio = if cfg.audio.enabled {
        match sdl
            .audio()
            .map_err(|e| e.to_string())
            .and_then(|a| Audio::new(a, cfg.audio.volume))
        {
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
    let text_input = video.text_input();
    let mut text_input_active = false;
    let mut pump = sdl.event_pump().map_err(|e| e.to_string())?;
    let mut gfx = Gfx {
        images: HashMap::new(),
        video_textures: HashMap::new(),
        texts: HashMap::new(),
        debug_labels: HashMap::new(),
        menu_animation_starts: HashMap::new(),
        menu_fonts: HashMap::new(),
        toolbar_texts: HashMap::new(),
        crossfade_safety: HashMap::new(),
        animation_delays: HashMap::new(),
        menu: None,
    };
    let start = Instant::now();
    let mut menu_sel = 0usize;
    let mut mouse = (-1.0f32, -1.0f32);
    let mut first_scene_logged = false;
    let mut frame: u64 = 0;
    let mut last_dbg = -1000.0f64;
    let mut options_open = false;
    let mut missing_logged: std::collections::HashSet<String> = Default::default();

    'main: loop {
        for ev in pump.poll_iter() {
            let ev = ev.get_converted_coords(&canvas).unwrap_or(ev);
            log::trace!("{ev:?}");
            match ev {
                Event::Quit { .. } => break 'main,
                Event::Window {
                    win_event: WindowEvent::PixelSizeChanged(..) | WindowEvent::Resized(..),
                    ..
                } => {
                    // la presentación lógica de SDL ya recalcula el letterbox
                }
                Event::Window {
                    win_event: WindowEvent::MouseLeave,
                    ..
                } => {
                    mouse = (-1.0, -1.0);
                    vm.mouse_move(-1.0, -1.0);
                }
                Event::KeyDown {
                    keycode: Some(k),
                    keymod,
                    repeat,
                    ..
                } => {
                    if options_open && k == sdl3::keyboard::Keycode::Escape {
                        options_open = false;
                        continue;
                    }
                    if k == sdl3::keyboard::Keycode::Up {
                        vm.set_key_push(38, true);
                    } else if k == sdl3::keyboard::Keycode::Down {
                        vm.set_key_push(40, true);
                    }
                    if vm.active_edit_name().is_some() {
                        if let Some(code) = edit_key_code(&k.name()) {
                            vm.edit_key(code);
                        }
                        if matches!(
                            k.name().as_str(),
                            "Return"
                                | "KP Enter"
                                | "Escape"
                                | "Tab"
                                | "Backspace"
                                | "Delete"
                                | "Left"
                                | "Right"
                                | "Home"
                                | "End"
                                | "Up"
                                | "Down"
                        ) {
                            continue;
                        }
                    }
                    if !repeat
                        && vm.active_edit_name().is_none()
                        && let Some(code) = edit_key_code(&k.name())
                    {
                        vm.slider_keydown(code);
                    }
                    let alt_enter = (keymod.intersects(Mod::LALTMOD | Mod::RALTMOD))
                        && k == sdl3::keyboard::Keycode::Return;
                    let act = if alt_enter {
                        Some(Action::Fullscreen)
                    } else {
                        keys.action(&k.name())
                    };
                    match act {
                        Some(Action::Fullscreen) if !repeat => {
                            let w = canvas.window_mut();
                            let fs =
                                !matches!(w.fullscreen_state(), sdl3::video::FullscreenType::Off);
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
                Event::KeyUp {
                    keycode: Some(k), ..
                } => {
                    if k == sdl3::keyboard::Keycode::Up {
                        vm.set_key_push(38, false);
                    } else if k == sdl3::keyboard::Keycode::Down {
                        vm.set_key_push(40, false);
                    }
                    if keys.action(&k.name()) == Some(Action::Skip) {
                        vm.skip = false;
                    }
                }
                Event::MouseMotion { x, y, .. } => {
                    if point_in_logical_render((x, y), (lw, lh)) {
                        mouse = (x, y);
                        vm.mouse_move(x as f64, y as f64);
                    } else {
                        mouse = (-1.0, -1.0);
                        vm.mouse_move(-1.0, -1.0);
                    }
                }
                Event::MouseButtonDown {
                    mouse_btn: MouseButton::Left,
                    x,
                    y,
                    ..
                } => {
                    if !point_in_logical_render((x, y), (lw, lh)) {
                        mouse = (-1.0, -1.0);
                        vm.mouse_move(-1.0, -1.0);
                        continue;
                    }
                    log::debug!("click at logical ({x:.0},{y:.0})");
                    mouse = (x, y);
                    if let Some(action) = toolbar_hit(lw, x, y, options_open) {
                        match action {
                            ToolbarAction::Quit => break 'main,
                            ToolbarAction::Fullscreen => {
                                let window = canvas.window_mut();
                                let fullscreen = !matches!(
                                    window.fullscreen_state(),
                                    sdl3::video::FullscreenType::Off
                                );
                                if let Err(error) = window.set_fullscreen(!fullscreen) {
                                    log::warn!("cannot change fullscreen state: {error}");
                                }
                            }
                            ToolbarAction::Options => options_open = !options_open,
                            ToolbarAction::Interpolation(mode) => {
                                interpolation_mode = mode;
                                if let Err(error) =
                                    save_interpolation_preference(interpolation_mode)
                                {
                                    log::error!("{error}");
                                }
                                let requested_vsync = cfg.window.vsync
                                    || interpolation_mode != InterpolationMode::Off;
                                vsync_active = unsafe {
                                    sdl3::sys::render::SDL_SetRenderVSync(
                                        canvas.raw(),
                                        if requested_vsync { 1 } else { 0 },
                                    )
                                };
                                frame_pacer = requested_vsync && !vsync_active;
                                frame_deadline = Instant::now();
                                if requested_vsync && !vsync_active {
                                    log::warn!(
                                        "renderer could not enable VSync for animation interpolation; fallback pacing uses {refresh_rate:.2} Hz"
                                    );
                                }
                            }
                        }
                    } else if toolbar_contains(lw, x, y, options_open) {
                        continue;
                    } else {
                        options_open = false;
                        if let State::Menu { name, choices, .. } = &vm.state {
                            if let Some(i) = menu_hit(choices, vm.scene.get(name), lw, lh, x, y) {
                                vm.choose(i);
                            }
                        } else {
                            vm.click_at(x as f64, y as f64, false);
                        }
                    }
                }
                Event::MouseButtonDown {
                    mouse_btn: MouseButton::Right,
                    x,
                    y,
                    ..
                } => {
                    if !point_in_logical_render((x, y), (lw, lh)) {
                        mouse = (-1.0, -1.0);
                        vm.mouse_move(-1.0, -1.0);
                        continue;
                    }
                    vm.click_at(x as f64, y as f64, true);
                }
                Event::MouseButtonUp {
                    mouse_btn: MouseButton::Left,
                    ..
                } => {
                    vm.end_slider_drag();
                }
                Event::TextInput { text, .. } => {
                    vm.insert_edit_text(&text);
                }
                Event::MouseWheel { y, direction, .. } => {
                    if !point_in_logical_render((mouse.0, mouse.1), (lw, lh)) {
                        continue;
                    }
                    let y = if direction == MouseWheelDirection::Flipped {
                        -y
                    } else {
                        y
                    };
                    vm.set_mouse_wheel(y > 0.0, y < 0.0);
                }
                _ => {}
            }
        }
        if let State::Menu { name, choices, .. } = &vm.state {
            menu_sel = menu_sel.min(choices.len().saturating_sub(1));
            if let Some(i) = menu_hit(choices, vm.scene.get(name), lw, lh, mouse.0, mouse.1) {
                menu_sel = i;
            }
        }
        if font.is_none()
            && let Ok(r) = frx.try_recv()
        {
            match r {
                Ok(f) => font = Some(f),
                Err(e) => log::error!("{e}"),
            }
        }

        // con --snapshot-frame el reloj es simulado (60 fps) para que la captura sea reproducible
        let now = if snapshot.is_some() {
            frame as f64 * 1000.0 / 60.0
        } else {
            start.elapsed().as_secs_f64() * 1000.0
        };
        vm.update(now, 20_000);
        let wants_text_input = vm.active_edit_name().is_some();
        if wants_text_input != text_input_active {
            if wants_text_input {
                text_input.start(canvas.window());
            } else {
                text_input.stop(canvas.window());
            }
            text_input_active = wants_text_input;
        }
        for e in vm.events.drain(..) {
            match e {
                VmEvent::PlaySound {
                    channel,
                    file,
                    volume,
                    repeat,
                } => {
                    if let Some(a) = audio.as_mut() {
                        a.play(channel, file, volume, repeat)
                    }
                }
                VmEvent::StopSound { channel } => {
                    if let Some(a) = audio.as_mut() {
                        a.stop(&channel)
                    }
                }
            }
        }
        if let Some(a) = audio.as_mut() {
            a.update(&assets);
        }
        if vm.ended() {
            log::info!("script ended");
            break;
        }

        // Keep the last complete scene visible while newly referenced images decode.
        let mut drawn = Vec::new();
        if scene_images_ready(&assets, &vm) {
            canvas
                .with_texture_canvas(&mut scene_frame, |scene_canvas| {
                    scene_canvas.set_draw_color(Color::RGB(0, 0, 0));
                    scene_canvas.clear();
                    drawn = draw_scene(
                        scene_canvas,
                        &tc,
                        &assets,
                        &mut vm,
                        &mut gfx,
                        font.as_mut(),
                        &cfg,
                        filter,
                        (lw, lh),
                        debug_layout,
                        interpolation_mode,
                    );
                    if let State::Menu { name, choices, .. } = &vm.state {
                        draw_menu(
                            scene_canvas,
                            &tc,
                            &assets,
                            &mut gfx,
                            font.as_mut(),
                            choices,
                            vm.scene.get(name),
                            menu_sel,
                            (lw, lh),
                            filter,
                            cfg.text.font.is_some(),
                        );
                    } else {
                        gfx.menu = None;
                    }
                })
                .map_err(|e| format!("cannot render scene frame: {e}"))?;
            scene_frame_ready = true;
        }
        if scene_frame_ready {
            canvas
                .copy(
                    &scene_frame,
                    None,
                    FRect::new(0.0, 0.0, lw as f32, lh as f32),
                )
                .map_err(|e| e.to_string())?;
        } else {
            canvas.set_draw_color(Color::RGB(0, 0, 0));
            canvas.clear();
        }
        let toolbar_visible =
            snapshot.is_none() && (options_open || (mouse.1 >= 0.0 && mouse.1 < 8.0));
        if toolbar_visible {
            draw_toolbar(
                &mut canvas,
                &tc,
                &mut gfx,
                font.as_mut(),
                (lw, lh),
                (mouse.0, mouse.1),
                options_open,
                interpolation_mode,
                vsync_active,
            );
        }
        frame += 1;
        if log::log_enabled!(log::Level::Debug) {
            for d in drawn.iter().filter(|d| d.status.starts_with("missing")) {
                if missing_logged.insert(d.name.clone()) {
                    log::debug!("render: {} {}", d.name, d.status);
                }
            }
            if now - last_dbg >= 1000.0 {
                last_dbg = now;
                let n = drawn.iter().filter(|d| d.status == "drawn").count();
                let (ww, wh) = canvas.window().size();
                // tamaño real en píxeles, consultado en cada log (output_size() puede venir
                // afectado por la presentación lógica)
                let (pw, ph) = canvas.window().size_in_pixels();
                let (rw, rh) = canvas.output_size().unwrap_or((0, 0));
                let vp = canvas.viewport();
                log::debug!(
                    "render frame {frame}: {} objects, {n} drawn | window {ww}x{wh} output {pw}x{ph} logical {lw}x{lh} viewport ({},{} {}x{}) scale {:?}",
                    drawn.len(),
                    vp.x(),
                    vp.y(),
                    vp.width(),
                    vp.height(),
                    canvas.scale()
                );
                // dónde cae el lienzo lógico en píxeles reales (letterbox: escala uniforme, centrado)
                let (bx, by, cw, chh) = crate::util::letterbox((pw, ph), (lw, lh));
                let k = cw / lw as f64;
                log::debug!(
                    "canvas in window pixels {pw}x{ph} (render output_size {rw}x{rh}): ({bx:.0},{by:.0} {cw:.0}x{chh:.0}) scale {k:.3}; pixel density {:.2} (pixels/window {:.2}x{:.2})",
                    canvas.window().pixel_density(),
                    pw as f64 / ww.max(1) as f64,
                    ph as f64 / wh.max(1) as f64
                );
                for d in &drawn {
                    log::debug!(
                        "  z{:<3} {:<24} rect=({:.0},{:.0} {:.0}x{:.0}) alpha={} {} {}",
                        d.z,
                        d.name,
                        d.rect.0,
                        d.rect.1,
                        d.rect.2,
                        d.rect.3,
                        d.alpha,
                        d.status,
                        d.extra
                    );
                }
            }
        }
        if let Some((n, path)) = &snapshot
            && frame >= *n
        {
            let shot = canvas.read_pixels(None).map_err(|e| e.to_string())?;
            let shot = shot
                .convert_format(PixelFormat::RGBA32)
                .map_err(|e| e.to_string())?;
            let (w, h, pitch) = (shot.width(), shot.height(), shot.pitch() as usize);
            let mut rgba = Vec::with_capacity((w * h * 4) as usize);
            shot.with_lock(|px: &[u8]| {
                for y in 0..h as usize {
                    rgba.extend_from_slice(&px[y * pitch..y * pitch + w as usize * 4]);
                }
            });
            image::save_buffer(path, &rgba, w, h, image::ColorType::Rgba8)
                .map_err(|e| e.to_string())?;
            log::info!(
                "snapshot of frame {frame} ({w}x{h}) written to {}",
                path.display()
            );
            println!(
                "snapshot: frame {frame}, {w}x{h}, {} objects ({} drawn) -> {}",
                drawn.len(),
                drawn.iter().filter(|d| d.status == "drawn").count(),
                path.display()
            );
            break;
        }
        canvas.present();
        if !first_scene_logged && !vm.scene.objs.is_empty() {
            first_scene_logged = true;
            log::info!(
                "first scene frame after {:.1} ms",
                t0.elapsed().as_secs_f64() * 1000.0
            );
        }
        if frame_pacer {
            frame_deadline += frame_period;
            let now = Instant::now();
            if frame_deadline > now {
                std::thread::sleep(frame_deadline - now);
            } else {
                frame_deadline = now;
            }
        } else if !cfg.window.vsync {
            std::thread::sleep(std::time::Duration::from_millis(4));
        }
    }
    Ok(())
}

fn draw_toolbar(
    canvas: &mut Canvas<Window>,
    tc: &TextureCreator<WindowContext>,
    gfx: &mut Gfx,
    font: Option<&mut TextRenderer>,
    (width, _height): (u32, u32),
    mouse: (f32, f32),
    options_open: bool,
    interpolation_mode: InterpolationMode,
    vsync_active: bool,
) {
    let mut font = font;
    canvas.set_blend_mode(BlendMode::Blend);
    canvas.set_draw_color(Color::RGBA(16, 18, 26, 238));
    let _ = canvas.fill_rect(FRect::new(0.0, 0.0, width as f32, 46.0));
    for (action, rect) in toolbar_button_rects(width) {
        let hovered = mouse.0 >= rect.x
            && mouse.0 < rect.x + rect.w
            && mouse.1 >= rect.y
            && mouse.1 < rect.y + rect.h;
        let active = matches!(action, ToolbarAction::Options) && options_open;
        let color = if hovered || active {
            Color::RGBA(73, 99, 145, 255)
        } else {
            Color::RGBA(43, 47, 61, 255)
        };
        canvas.set_draw_color(color);
        let _ = canvas.fill_rect(rect);
        canvas.set_draw_color(Color::RGBA(126, 137, 160, 255));
        let _ = canvas.draw_rect(rect);
        let label = match action {
            ToolbarAction::Quit => "Salir",
            ToolbarAction::Fullscreen => "Pantalla",
            ToolbarAction::Options => "Opciones",
            ToolbarAction::Interpolation(_) => continue,
        };
        let label_width = rect.w as u32 - 16;
        draw_toolbar_text(
            canvas,
            tc,
            gfx,
            font.as_deref_mut(),
            label,
            rect.x + 8.0,
            rect.y,
            label_width,
            rect.h as u32,
        );
    }
    if options_open {
        let panel = FRect::new(width as f32 - 340.0, 32.0, 334.0, 106.0);
        canvas.set_draw_color(Color::RGBA(16, 18, 26, 248));
        let _ = canvas.fill_rect(panel);
        canvas.set_draw_color(Color::RGBA(126, 137, 160, 255));
        let _ = canvas.draw_rect(panel);
        draw_toolbar_text(
            canvas,
            tc,
            gfx,
            font.as_deref_mut(),
            "Fundido de GAL opacos grandes",
            panel.x + 12.0,
            42.0,
            310,
            24,
        );
        for (mode, rect) in interpolation_option_rects(width) {
            let selected = mode == interpolation_mode;
            let hovered = mouse.0 >= rect.x
                && mouse.0 < rect.x + rect.w
                && mouse.1 >= rect.y
                && mouse.1 < rect.y + rect.h;
            canvas.set_draw_color(if selected {
                Color::RGBA(55, 112, 84, 255)
            } else if hovered {
                Color::RGBA(73, 99, 145, 255)
            } else {
                Color::RGBA(43, 47, 61, 255)
            });
            let _ = canvas.fill_rect(rect);
            canvas.set_draw_color(Color::RGBA(126, 137, 160, 255));
            let _ = canvas.draw_rect(rect);
            let label = match mode {
                InterpolationMode::Off => "Off",
                InterpolationMode::Crossfade => "Fundido",
            };
            draw_toolbar_text(
                canvas,
                tc,
                gfx,
                font.as_deref_mut(),
                label,
                rect.x + 5.0,
                rect.y,
                rect.w as u32 - 10,
                rect.h as u32,
            );
        }
        let detail = if vsync_active {
            "Solo GAL opacos grandes · refresco del monitor"
        } else {
            "Solo GAL opacos grandes · VSync no disponible"
        };
        draw_toolbar_text(
            canvas,
            tc,
            gfx,
            font.as_deref_mut(),
            detail,
            panel.x + 16.0,
            108.0,
            310,
            20,
        );
    }
}

fn menu_geometry(choices: &[String], menu: Option<&Obj>, lw: u32, lh: u32) -> (FRect, Vec<FRect>) {
    let font_height = menu
        .and_then(|o| o.get(Prop::FONTHEIGHT))
        .map(|v| v.as_f64())
        .filter(|v| *v > 0.0)
        .unwrap_or(24.0);
    let line_space = menu
        .and_then(|o| o.get(Prop::LINESPACE))
        .map(|v| v.as_f64())
        .unwrap_or(0.0);
    let margin_y = menu.map(|o| o.int(Prop::MARGINY, 0)).unwrap_or(0).max(0) as f32;
    let (default_w, default_h) =
        native_menu_size(choices.len(), lw, font_height, line_space, margin_y as i64);
    let width = menu
        .map(|o| o.int(Prop::WIDTH, default_w))
        .unwrap_or(default_w)
        .max(0) as f32;
    let height = menu
        .map(|o| o.int(Prop::HEIGHT, default_h))
        .unwrap_or(default_h)
        .max(0) as f32;
    let default_x = (lw as f64 - width as f64) / 2.0;
    let default_y = (lh as f64 - height as f64) / 2.0;
    let x = menu
        .map(|o| o.get(Prop::LEFT).map(|v| v.as_f64()).unwrap_or(default_x))
        .unwrap_or(default_x) as f32;
    let y = menu
        .map(|o| o.get(Prop::TOP).map(|v| v.as_f64()).unwrap_or(default_y))
        .unwrap_or(default_y) as f32;
    let frame = FRect::new(x, y, width, height);
    let row_height = menu_line_height(font_height, line_space) as f32;
    let rows = (0..choices.len())
        .map(|i| FRect::new(x, y + margin_y + i as f32 * row_height, width, row_height))
        .collect();
    (frame, rows)
}

fn menu_hit(
    choices: &[String],
    menu: Option<&Obj>,
    lw: u32,
    lh: u32,
    x: f32,
    y: f32,
) -> Option<usize> {
    menu_geometry(choices, menu, lw, lh)
        .1
        .iter()
        .position(|r| x >= r.x && y >= r.y && x < r.x + r.w && y < r.y + r.h)
}

fn edit_key_code(name: &str) -> Option<i64> {
    Some(match name {
        "Return" | "KP Enter" => 13,
        "Escape" => 27,
        "Tab" => 9,
        "Backspace" => 8,
        "Delete" => 46,
        "Left" => 37,
        "Up" => 38,
        "Right" => 39,
        "Down" => 40,
        "Home" => 36,
        "End" => 35,
        "PageUp" => 33,
        "PageDown" => 34,
        _ => {
            let mut chars = name.chars();
            let c = chars.next()?;
            if chars.next().is_some() || !c.is_ascii_alphanumeric() {
                return None;
            }
            c.to_ascii_uppercase() as i64
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn draw_menu(
    canvas: &mut Canvas<Window>,
    tc: &TextureCreator<WindowContext>,
    assets: &Assets,
    gfx: &mut Gfx,
    mut font: Option<&mut TextRenderer>,
    choices: &[String],
    menu: Option<&Obj>,
    sel: usize,
    (lw, lh): (u32, u32),
    filter: ScaleMode,
    font_override: bool,
) {
    let (frame, rows) = menu_geometry(choices, menu, lw, lh);
    let alpha = menu
        .map(|o| o.int(Prop::ALPHA, 255).clamp(0, 255) as u8)
        .unwrap_or(255);
    let font_height = menu
        .and_then(|o| o.get(Prop::FONTHEIGHT))
        .map(|v| v.as_f64())
        .filter(|v| *v > 0.0)
        .unwrap_or(24.0);
    let margin_x = menu.map(|o| o.int(Prop::MARGINX, 0)).unwrap_or(0).max(0) as u32;
    let halign = menu
        .map(|o| o.int(Prop::HALIGN, 0).clamp(0, 2) as u8)
        .unwrap_or(0);
    let font_family = menu.map(|o| o.str(Prop::FONTNAME)).unwrap_or_default();
    let normal = menu
        .map(|o| o.int(Prop::FONTCOLOR, 0x00FF_FFFF))
        .unwrap_or(0x00FF_FFFF);
    let hover = menu
        .map(|o| o.int(Prop::FONTHOVERCOLOR, normal))
        .unwrap_or(normal);
    let hover_bg = menu.map(|o| o.int(Prop::HOVERCOLOR, -1)).unwrap_or(-1);
    let fill = menu.map(|o| o.int(Prop::COLOR, -1)).unwrap_or(-1);
    let style = vec![
        frame.w as i64,
        frame.h as i64,
        (font_height * 100.0).round() as i64,
        margin_x as i64,
        halign as i64,
        normal,
        hover,
        sel as i64,
    ];
    if !font_override && !font_family.is_empty() && !gfx.menu_fonts.contains_key(&font_family) {
        let loaded = crate::text::find_font_family(&font_family).and_then(|(path, index)| {
            match TextRenderer::load(&path, index) {
                Ok(renderer) => {
                    log::info!("menu font {font_family:?} loaded from {}", path.display());
                    Some(renderer)
                }
                Err(error) => {
                    log::warn!("cannot load menu font {font_family:?}: {error}");
                    None
                }
            }
        });
        if loaded.is_none() {
            log::warn!(
                "no font match for menu font family {font_family:?}; using configured fallback"
            );
        }
        gfx.menu_fonts.insert(font_family.clone(), loaded);
    }
    if gfx.menu.as_ref().is_none_or(|cache| {
        cache.choices != choices || cache.font_family != font_family || cache.style != style
    }) {
        let selected_font = if !font_override && !font_family.is_empty() {
            gfx.menu_fonts
                .get_mut(&font_family)
                .and_then(Option::as_mut)
        } else {
            None
        };
        let textures = if let Some(f) = selected_font {
            render_menu_textures(
                f,
                tc,
                choices,
                &rows,
                margin_x,
                font_height,
                normal,
                hover,
                sel,
                halign,
                filter,
            )
        } else if let Some(f) = font.as_deref_mut() {
            render_menu_textures(
                f,
                tc,
                choices,
                &rows,
                margin_x,
                font_height,
                normal,
                hover,
                sel,
                halign,
                filter,
            )
        } else {
            Vec::new()
        };
        if !textures.is_empty() {
            gfx.menu = Some(MenuCache {
                choices: choices.to_vec(),
                font_family: font_family.clone(),
                style,
                textures,
            });
        }
    }
    canvas.set_blend_mode(BlendMode::Blend);
    if fill >= 0 {
        let c = tcolor(fill, alpha);
        canvas.set_draw_color(Color::RGBA(c[0], c[1], c[2], c[3]));
        let _ = canvas.fill_rect(frame);
    }
    let (left_inset, _, _, _) = menu_border_insets(tc, assets, gfx, menu, filter);
    draw_menu_border(canvas, tc, assets, gfx, menu, frame, alpha, filter);
    if hover_bg >= 0
        && let Some(row) = rows.get(sel)
    {
        let c = tcolor(hover_bg, alpha);
        canvas.set_draw_color(Color::RGBA(c[0], c[1], c[2], c[3]));
        let _ = canvas.fill_rect(*row);
    }
    if let Some(cache) = &mut gfx.menu {
        for (row, texture) in rows.iter().zip(&mut cache.textures) {
            if let Some(texture) = texture {
                texture.set_alpha_mod(alpha);
                let query = texture.query();
                let _ = canvas.copy(
                    texture,
                    None,
                    FRect::new(
                        row.x + left_inset + margin_x as f32,
                        row.y,
                        row.w.min(query.width as f32),
                        row.h,
                    ),
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_menu_textures(
    font: &mut TextRenderer,
    tc: &TextureCreator<WindowContext>,
    choices: &[String],
    rows: &[FRect],
    margin_x: u32,
    font_height: f64,
    normal: i64,
    hover: i64,
    sel: usize,
    halign: u8,
    filter: ScaleMode,
) -> Vec<Option<Texture>> {
    choices
        .iter()
        .zip(rows)
        .enumerate()
        .map(|(i, (choice, row))| {
            let items: Vec<TextItem> = choice
                .chars()
                .map(|ch| TextItem::Char(ch.to_string()))
                .collect();
            let color = tcolor(if i == sel { hover } else { normal }, 255);
            let text_w = (row.w as u32)
                .saturating_sub(margin_x.saturating_mul(2))
                .max(1);
            let font_size = font.size_for_cell_height(font_height as f32);
            let img = font.render_aligned(
                &items,
                text_w,
                row.h as u32,
                font_size,
                color,
                false,
                halign,
            );
            upload(tc, img.w, img.h, &img.rgba, filter)
        })
        .collect()
}

fn menu_border_insets(
    tc: &TextureCreator<WindowContext>,
    assets: &Assets,
    gfx: &mut Gfx,
    menu: Option<&Obj>,
    filter: ScaleMode,
) -> (f32, f32, f32, f32) {
    let Some(menu) = menu else {
        return (0.0, 0.0, 0.0, 0.0);
    };
    let props = [
        Prop::BORDERSOURCETL,
        Prop::BORDERSOURCETC,
        Prop::BORDERSOURCETR,
        Prop::BORDERSOURCECL,
        Prop::BORDERSOURCECC,
        Prop::BORDERSOURCECR,
        Prop::BORDERSOURCEBL,
        Prop::BORDERSOURCEBC,
        Prop::BORDERSOURCEBR,
    ];
    let keys: Vec<_> = props
        .iter()
        .map(|prop| {
            let src = menu.str(*prop);
            (!src.is_empty())
                .then(|| image_for(tc, assets, gfx, &src, filter))
                .flatten()
        })
        .collect();
    let size = |i: usize| {
        keys.get(i)
            .and_then(Option::as_ref)
            .and_then(|key| gfx.images.get(key))
            .map(|(_, w, h)| (*w as f32, *h as f32))
            .unwrap_or((0.0, 0.0))
    };
    (
        size(0).0.max(size(3).0).max(size(6).0),
        size(2).0.max(size(5).0).max(size(8).0),
        size(0).1.max(size(1).1).max(size(2).1),
        size(6).1.max(size(7).1).max(size(8).1),
    )
}

fn draw_menu_border(
    canvas: &mut Canvas<Window>,
    tc: &TextureCreator<WindowContext>,
    assets: &Assets,
    gfx: &mut Gfx,
    menu: Option<&Obj>,
    frame: FRect,
    alpha: u8,
    filter: ScaleMode,
) {
    let Some(menu) = menu else { return };
    let props = [
        Prop::BORDERSOURCETL,
        Prop::BORDERSOURCETC,
        Prop::BORDERSOURCETR,
        Prop::BORDERSOURCECL,
        Prop::BORDERSOURCECC,
        Prop::BORDERSOURCECR,
        Prop::BORDERSOURCEBL,
        Prop::BORDERSOURCEBC,
        Prop::BORDERSOURCEBR,
    ];
    let mut keys = Vec::with_capacity(props.len());
    for prop in props {
        let src = menu.str(prop);
        keys.push(if src.is_empty() {
            None
        } else {
            image_for(tc, assets, gfx, &src, filter)
        });
    }
    let size = |i: usize| {
        keys.get(i)
            .and_then(Option::as_ref)
            .and_then(|key| gfx.images.get(key))
            .map(|(_, w, h)| (*w as f32, *h as f32))
            .unwrap_or((0.0, 0.0))
    };
    let (left, right, top, bottom) = (
        size(0).0.max(size(3).0).max(size(6).0),
        size(2).0.max(size(5).0).max(size(8).0),
        size(0).1.max(size(1).1).max(size(2).1),
        size(6).1.max(size(7).1).max(size(8).1),
    );
    let xmid = (frame.w - left - right).max(0.0);
    let ymid = (frame.h - top - bottom).max(0.0);
    let rects = [
        FRect::new(frame.x, frame.y, left, top),
        FRect::new(frame.x + left, frame.y, xmid, top),
        FRect::new(frame.x + frame.w - right, frame.y, right, top),
        FRect::new(frame.x, frame.y + top, left, ymid),
        FRect::new(frame.x + left, frame.y + top, xmid, ymid),
        FRect::new(frame.x + frame.w - right, frame.y + top, right, ymid),
        FRect::new(frame.x, frame.y + frame.h - bottom, left, bottom),
        FRect::new(frame.x + left, frame.y + frame.h - bottom, xmid, bottom),
        FRect::new(
            frame.x + frame.w - right,
            frame.y + frame.h - bottom,
            right,
            bottom,
        ),
    ];
    for (key, rect) in keys.into_iter().zip(rects) {
        if rect.w <= 0.0 || rect.h <= 0.0 {
            continue;
        }
        if let Some(key) = key
            && let Some((texture, _, _)) = gfx.images.get_mut(&key)
        {
            texture.set_alpha_mod(alpha);
            let _ = canvas.copy(texture, None, rect);
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
            gfx.animation_delays
                .insert(name.clone(), img.delays.clone());
            if log::log_enabled!(log::Level::Debug) {
                let n = (img.w * img.h).max(1) as u64;
                let asum: u64 = img.rgba.chunks_exact(4).map(|p| p[3] as u64).sum();
                let rgbsum: u64 = img
                    .rgba
                    .chunks_exact(4)
                    .map(|p| p[0] as u64 + p[1] as u64 + p[2] as u64)
                    .sum();
                log::debug!(
                    "texture {name}: {}x{} avg alpha {} avg rgb {} first px {:?}",
                    img.w,
                    img.h,
                    asum / n,
                    rgbsum / (3 * n),
                    img.rgba.get(0..4)
                );
            }
            let t = upload(tc, img.w, img.h, &img.rgba, filter)?;
            gfx.images.insert(name.clone(), (t, img.w, img.h));
            // frames de animación GAL como texturas "nombre#i"
            for (i, f) in img.frames.iter().enumerate() {
                if let Some(t) = upload(tc, img.w, img.h, f, filter) {
                    gfx.images
                        .insert(format!("{name}#{}", i + 1), (t, img.w, img.h));
                }
            }
            if !img.frames.is_empty() {
                log::debug!(
                    "texture {name}: {} animation frames, delays {:?}",
                    img.frames.len() + 1,
                    img.delays
                );
            }
        } else {
            return None;
        }
    }
    Some(name)
}

fn scene_images_ready(assets: &Assets, vm: &Vm) -> bool {
    let image_ready = |name: &str| {
        assets
            .data()
            .resolve(name, IMAGE_EXTS)
            .is_none_or(|resolved| assets.try_get(&resolved).is_some())
    };
    for object in vm.scene.draw_order() {
        if !vm.scene.effective_visible(object) {
            continue;
        }
        if object.kind == Kind::Movie
            && object.get(Prop::PLAYING).is_some_and(Value::truthy)
            && vm.video_frame(&object.name).is_none()
        {
            return false;
        }
        if matches!(
            object.kind,
            Kind::Image | Kind::Button | Kind::Tile | Kind::Slider
        ) {
            let mut sources = vec![object.str(Prop::SOURCE)];
            if object.kind == Kind::Slider {
                sources.extend([
                    object.str(Prop::THUMBSOURCE),
                    object.str(Prop::BUTTONSOURCE),
                ]);
            } else if object.kind == Kind::Button {
                sources.extend([
                    object.str(Prop::HOVERSOURCE),
                    object.str(Prop::PRESSEDSOURCE),
                ]);
            }
            for source in sources.into_iter().filter(|source| !source.is_empty()) {
                if !image_ready(&source) {
                    return false;
                }
            }
        }
        if object.kind == Kind::Menu
            && let Some(menu) = vm.prevmenus.get(&object.name)
        {
            for (index, button) in menu.buttons.iter().enumerate() {
                if !vm.menu_item_visible(&object.name, index) {
                    continue;
                }
                let source = if menu.hover == Some(index) && !button.src_selected.is_empty() {
                    &button.src_selected
                } else {
                    &button.src
                };
                if !source.is_empty() && !image_ready(source) {
                    return false;
                }
            }
        }
    }
    for movie in vm.cinema_movies.values() {
        for sprite in movie.sprites_at(vm.now) {
            if !image_ready(&sprite.media) {
                return false;
            }
        }
    }
    if let State::Menu { name, .. } = &vm.state
        && let Some(menu) = vm.scene.get(name)
    {
        for prop in [
            Prop::BORDERSOURCETL,
            Prop::BORDERSOURCETC,
            Prop::BORDERSOURCETR,
            Prop::BORDERSOURCECL,
            Prop::BORDERSOURCECC,
            Prop::BORDERSOURCECR,
            Prop::BORDERSOURCEBL,
            Prop::BORDERSOURCEBC,
            Prop::BORDERSOURCEBR,
        ] {
            let source = menu.str(prop);
            if !source.is_empty() && !image_ready(&source) {
                return false;
            }
        }
    }
    true
}

fn gal_animation_frame(delays: &[u32], elapsed_ms: f64) -> Option<usize> {
    if delays.len() < 2 {
        return None;
    }
    let total: f64 = delays.iter().map(|delay| *delay as f64).sum();
    if total <= 0.0 {
        return None;
    }
    let position = elapsed_ms.max(0.0).rem_euclid(total);
    let mut elapsed = 0.0;
    delays.iter().position(|delay| {
        elapsed += *delay as f64;
        position < elapsed
    })
}

fn debug_outline(
    canvas: &mut Canvas<Window>,
    tc: &TextureCreator<WindowContext>,
    gfx: &mut Gfx,
    font: Option<&mut TextRenderer>,
    key: &str,
    label: &str,
    rect: FRect,
    color: Color,
    filter: ScaleMode,
) {
    if rect.w <= 0.0 || rect.h <= 0.0 {
        return;
    }
    canvas.set_draw_color(color);
    let _ = canvas.draw_rect(rect);
    if let Some(font) = font
        && !label.is_empty()
    {
        let label_w = (label.chars().count() as u32 * 16).clamp(1, 400);
        let label_h = 20;
        let stale = gfx
            .debug_labels
            .get(key)
            .is_none_or(|(old, _, _, _)| old != label);
        if stale {
            let rendered = font.render(
                &[TextItem::Char(label.to_string())],
                label_w,
                label_h,
                14.0,
                [255, 255, 255, 255],
                false,
            );
            if let Some(texture) = upload(tc, rendered.w, rendered.h, &rendered.rgba, filter) {
                gfx.debug_labels.insert(
                    key.to_string(),
                    (label.to_string(), texture, rendered.w, rendered.h),
                );
            }
        }
        if let Some((_, texture, w, h)) = gfx.debug_labels.get_mut(key) {
            canvas.set_draw_color(Color::RGBA(0, 0, 0, 210));
            let bg = FRect::new(
                rect.x,
                (rect.y - *h as f32).max(0.0),
                *w as f32 + 4.0,
                *h as f32,
            );
            let _ = canvas.fill_rect(bg);
            let _ = canvas.copy(
                texture,
                None,
                FRect::new(bg.x + 2.0, bg.y, *w as f32, *h as f32),
            );
        }
    }
}

fn draw_edit_text(
    canvas: &mut Canvas<Window>,
    tc: &TextureCreator<WindowContext>,
    gfx: &mut Gfx,
    font: Option<&mut TextRenderer>,
    o: &Obj,
    alpha: u8,
    (x, y): (f64, f64),
    focused: bool,
    cfg: &Config,
    filter: ScaleMode,
) -> &'static str {
    let Some(font) = font else {
        return "font not loaded yet";
    };
    let width = o.int(Prop::WIDTH, 0).max(1) as u32;
    let height = o.int(Prop::HEIGHT, 0).max(1) as u32;
    let text = o.str(Prop::TEXT);
    let px = o
        .get(Prop::FONTHEIGHT)
        .map(Value::as_f64)
        .filter(|v| *v > 0.0)
        .unwrap_or(cfg.text.size as f64) as f32;
    let color = o
        .get(Prop::FONTCOLOR)
        .map(|v| tcolor(v.as_int(), 255))
        .unwrap_or(cfg.text.color);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (text.as_str(), px.to_bits(), color).hash(&mut hasher);
    let style = hasher.finish() as usize;
    let stale = gfx
        .texts
        .get(&o.name)
        .is_none_or(|(rev, extra, _)| *rev != 0 || *extra != style);
    if stale {
        let items: Vec<TextItem> = text
            .chars()
            .map(|c| TextItem::Char(c.to_string()))
            .collect();
        let img = font.render(&items, width, height, px, color, false);
        if let Some(texture) = upload(tc, img.w, img.h, &img.rgba, filter) {
            gfx.texts.insert(o.name.clone(), (0, style, texture));
        }
    }
    if let Some((_, _, texture)) = gfx.texts.get_mut(&o.name) {
        texture.set_alpha_mod(alpha);
        let _ = canvas.copy(
            texture,
            None,
            FRect::new(x as f32, y as f32, width as f32, height as f32),
        );
    }
    if focused {
        let caret_x = x as f32 + o.int(Prop::CARETX, 0).clamp(0, width as i64) as f32;
        canvas.set_draw_color(Color::RGBA(color[0], color[1], color[2], alpha));
        let _ = canvas.fill_rect(FRect::new(caret_x, y as f32, 1.0, height as f32));
    }
    "edit"
}

fn draw_caption_text(
    canvas: &mut Canvas<Window>,
    tc: &TextureCreator<WindowContext>,
    gfx: &mut Gfx,
    mut font: Option<&mut TextRenderer>,
    o: &Obj,
    alpha: u8,
    (x, y): (f64, f64),
    (lw, _lh): (u32, u32),
    filter: ScaleMode,
) -> &'static str {
    let text = o.str(Prop::TEXT);
    if text.is_empty() {
        return "empty caption";
    }
    let family = o.str(Prop::FONTNAME);
    if !family.is_empty() && !gfx.menu_fonts.contains_key(&family) {
        let loaded =
            crate::text::find_font_family(&family).and_then(
                |(path, index)| match TextRenderer::load(&path, index) {
                    Ok(renderer) => {
                        log::info!("caption font {family:?} loaded from {}", path.display());
                        Some(renderer)
                    }
                    Err(error) => {
                        log::warn!("cannot load caption font {family:?}: {error}");
                        None
                    }
                },
            );
        if loaded.is_none() {
            log::warn!(
                "no font match for caption font family {family:?}; using configured fallback"
            );
        }
        gfx.menu_fonts.insert(family.clone(), loaded);
    }
    let width = o.int(Prop::WIDTH, 0).max(0) as u32;
    let height = o.int(Prop::HEIGHT, 0).max(0) as u32;
    let font_height = o
        .get(Prop::FONTHEIGHT)
        .map(Value::as_f64)
        .filter(|v| *v > 0.0)
        .unwrap_or(24.0) as f32;
    let bounds = (
        if width > 0 {
            width
        } else {
            lw.saturating_sub(x.max(0.0) as u32).max(1)
        },
        if height > 0 {
            height
        } else {
            font_height.ceil().max(1.0) as u32
        },
    );
    let font_color = tcolor(o.int(Prop::FONTCOLOR, 0x00FF_FFFF), alpha);
    let align = o.int(Prop::HALIGN, 0).clamp(0, 2) as u8;
    let shadow = o.get(Prop::FONTSHADOW).is_some_and(Value::truthy);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (
        text.as_str(),
        family.as_str(),
        bounds,
        font_height.to_bits(),
        font_color,
        align,
        shadow,
        o.int(Prop::FONTSTYLE, 0),
    )
        .hash(&mut hasher);
    let style = hasher.finish() as usize;
    let stale = gfx
        .texts
        .get(&o.name)
        .is_none_or(|(rev, extra, _)| *rev != o.text_rev || *extra != style);
    if stale {
        let items: Vec<TextItem> = text
            .chars()
            .map(|c| {
                if c == '\n' {
                    TextItem::Newline
                } else {
                    TextItem::Char(c.to_string())
                }
            })
            .collect();
        let rendered = {
            let selected = if !family.is_empty() {
                gfx.menu_fonts.get_mut(&family).and_then(Option::as_mut)
            } else {
                None
            };
            let selected = selected.or_else(|| font.as_deref_mut());
            selected.map(|renderer| {
                let px = renderer.size_for_cell_height(font_height);
                renderer.render_aligned(&items, bounds.0, bounds.1, px, font_color, shadow, align)
            })
        };
        if let Some(rendered) = rendered
            && let Some(texture) = upload(tc, rendered.w, rendered.h, &rendered.rgba, filter)
        {
            gfx.texts
                .insert(o.name.clone(), (o.text_rev, style, texture));
        }
    }
    if let Some((_, _, texture)) = gfx.texts.get_mut(&o.name) {
        texture.set_alpha_mod(alpha);
        let _ = canvas.copy(
            texture,
            None,
            FRect::new(x as f32, y as f32, bounds.0 as f32, bounds.1 as f32),
        );
    }
    "caption"
}

fn draw_message_text(
    canvas: &mut Canvas<Window>,
    tc: &TextureCreator<WindowContext>,
    gfx: &mut Gfx,
    font: Option<&mut TextRenderer>,
    o: &Obj,
    alpha: u8,
    (x, y): (f64, f64),
    (lw, lh): (u32, u32),
    cfg: &Config,
    filter: ScaleMode,
) -> &'static str {
    let Some(font) = font else {
        return "font not loaded yet";
    };
    let w = if o.int(Prop::WIDTH, 0) > 0 {
        o.int(Prop::WIDTH, 0) as u32
    } else {
        lw.saturating_sub(x.max(0.0) as u32)
    };
    let h = if o.int(Prop::HEIGHT, 0) > 0 {
        o.int(Prop::HEIGHT, 0) as u32
    } else {
        lh.saturating_sub(y.max(0.0) as u32)
    };
    let items = o.text.clone();
    if items.is_empty() {
        return "empty text";
    }
    let extra = 0;
    let stale = gfx
        .texts
        .get(&o.name)
        .is_none_or(|(rev, ex, _)| *rev != o.text_rev || *ex != extra);
    if stale {
        let px = o
            .get(Prop::FONTHEIGHT)
            .map(|v| v.as_f64() as f32)
            .filter(|v| *v > 4.0)
            .unwrap_or(cfg.text.size);
        let color = o
            .get(Prop::FONTCOLOR)
            .map(|v| tcolor(v.as_int(), 255))
            .unwrap_or(cfg.text.color);
        let img = font.render(&items, w, h, px, color, cfg.text.shadow);
        if let Some(t) = upload(tc, img.w, img.h, &img.rgba, filter) {
            gfx.texts.insert(o.name.clone(), (o.text_rev, extra, t));
        }
    }
    if let Some((_, _, texture)) = gfx.texts.get_mut(&o.name) {
        texture.set_alpha_mod(alpha);
        let q = texture.query();
        let _ = canvas.copy(
            texture,
            None,
            FRect::new(x as f32, y as f32, q.width as f32, q.height as f32),
        );
    }
    "drawn"
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
    debug_layout: bool,
    interpolation_mode: InterpolationMode,
) -> Vec<Drawn> {
    let order: Vec<Obj> = vm.scene.draw_order().into_iter().cloned().collect();
    let interpolation_areas = interpolation_group_areas(&vm.scene, &order, gfx, assets);
    let mut sizes: Vec<(String, u32, u32)> = vec![];
    let mut active_menu_animations = HashSet::new();
    let mut drawn: Vec<Drawn> = Vec::with_capacity(order.len());
    for (z, o) in order.iter().enumerate() {
        let (x, y) = vm.scene.abs_pos(o);
        let alpha = o.int(Prop::ALPHA, 255).clamp(0, 255) as u8;
        let (ow, oh) = (o.int(Prop::WIDTH, 0), o.int(Prop::HEIGHT, 0));
        drawn.push(Drawn {
            name: o.name.clone(),
            z,
            rect: (x as f32, y as f32, ow as f32, oh as f32),
            alpha,
            status: "drawn",
            extra: String::new(),
        });
        let di = drawn.len() - 1;
        macro_rules! st {
            () => {
                drawn[di].status
            };
        }
        if !vm.scene.effective_visible(o) {
            st!() = "hidden";
            continue;
        }
        if alpha == 0 {
            st!() = "alpha=0";
            continue;
        }
        if debug_layout {
            let color = match o.kind {
                Kind::Image | Kind::Movie | Kind::Tile => Color::RGB(70, 220, 255),
                Kind::Button | Kind::Slider => Color::RGB(255, 190, 40),
                Kind::Menu => Color::RGB(255, 70, 210),
                Kind::Message | Kind::Edit | Kind::Caption => Color::RGB(100, 255, 120),
                Kind::Box => Color::RGB(255, 90, 80),
                Kind::Other(_) => Color::RGB(220, 220, 220),
            };
            if o.kind == Kind::Menu
                && let Some(pm) = vm.prevmenus.get(&o.name)
            {
                for (i, button) in pm.buttons.iter().enumerate() {
                    if !vm.menu_item_visible(&o.name, i) {
                        continue;
                    }
                    let key = format!("{}:{i}", o.name);
                    let label = format!("{} / {}", o.name, button.name);
                    debug_outline(
                        canvas,
                        tc,
                        gfx,
                        font.as_deref_mut(),
                        &key,
                        &label,
                        FRect::new(
                            (x + button.x as f64) as f32,
                            (y + button.y as f64) as f32,
                            button.w as f32,
                            button.h as f32,
                        ),
                        color,
                        filter,
                    );
                }
                drawn[di].status = "debug layout";
                continue;
            }
            let mut w = ow;
            let mut h = oh;
            if matches!(
                o.kind,
                Kind::Image | Kind::Button | Kind::Tile | Kind::Slider
            ) {
                let src = o.str(Prop::SOURCE);
                if !src.is_empty()
                    && let Some(key) = image_for(tc, assets, gfx, &src, filter)
                    && let Some((_, iw, ih)) = gfx.images.get(&key)
                {
                    if w <= 0 {
                        w = *iw as i64;
                    }
                    if h <= 0 {
                        h = *ih as i64;
                    }
                }
            }
            if o.kind == Kind::Message {
                if w <= 0 {
                    w = lw.saturating_sub(x.max(0.0) as u32) as i64;
                }
                if h <= 0 {
                    h = lh.saturating_sub(y.max(0.0) as u32) as i64;
                }
            }
            if let Some(movie) = vm.cinema_movies.get(&o.name) {
                if w <= 0 {
                    w = movie.movie.width as i64;
                }
                if h <= 0 {
                    h = movie.movie.height as i64;
                }
            }
            if w > 0 && h > 0 {
                debug_outline(
                    canvas,
                    tc,
                    gfx,
                    font.as_deref_mut(),
                    &o.name,
                    &format!("{} [{}x{}]", o.name, w, h),
                    FRect::new(x as f32, y as f32, w as f32, h as f32),
                    color,
                    filter,
                );
                if o.kind == Kind::Message {
                    let _ = draw_message_text(
                        canvas,
                        tc,
                        gfx,
                        font.as_deref_mut(),
                        o,
                        alpha,
                        (x, y),
                        (lw, lh),
                        cfg,
                        filter,
                    );
                }
                if (ow <= 0 || oh <= 0)
                    && matches!(
                        o.kind,
                        Kind::Image | Kind::Button | Kind::Tile | Kind::Slider
                    )
                {
                    sizes.push((o.name.clone(), w as u32, h as u32));
                }
            }
            drawn[di].status = "debug layout";
            continue;
        }
        if let Some(playback) = vm.cinema_movies.get(&o.name) {
            let mut rendered = 0;
            let elapsed = vm.now - playback.started_at;
            for sprite in playback.movie.sprites_at(elapsed) {
                let Some(key) = image_for(tc, assets, gfx, &sprite.media, filter) else {
                    continue;
                };
                let Some((texture, iw, ih)) = gfx.images.get_mut(&key) else {
                    continue;
                };
                let (iw, ih) = (*iw, *ih);
                texture.set_alpha_mod(((alpha as u16 * sprite.alpha as u16) / 255) as u8);
                let width = iw as f32 * sprite.scale_x;
                let height = ih as f32 * sprite.scale_y;
                if width > 0.0 && height > 0.0 {
                    let _ = canvas.copy(
                        texture,
                        None,
                        FRect::new(x as f32 + sprite.x, y as f32 + sprite.y, width, height),
                    );
                    rendered += 1;
                }
            }
            let mut rendered_text = 0;
            for (index, text) in playback.movie.texts_at(elapsed).into_iter().enumerate() {
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                (
                    text.value.as_str(),
                    text.font_height,
                    text.font_style,
                    text.color,
                    playback.movie.width,
                    playback.movie.height,
                )
                    .hash(&mut hasher);
                let style = hasher.finish() as usize;
                let key = format!("{}::lcm-text-{index}", o.name);
                let stale = gfx
                    .texts
                    .get(&key)
                    .is_none_or(|(revision, old_style, _)| *revision != 0 || *old_style != style);
                if stale {
                    let width = (playback.movie.width as f32 - text.x.max(0.0))
                        .ceil()
                        .max(1.0) as u32;
                    let height = (playback.movie.height as f32 - text.y.max(0.0))
                        .ceil()
                        .max(text.font_height as f32) as u32;
                    let rendered = font.as_deref_mut().map(|renderer| {
                        let px = renderer.size_for_cell_height(text.font_height as f32);
                        let items: Vec<TextItem> = text
                            .value
                            .chars()
                            .map(|c| TextItem::Char(c.to_string()))
                            .collect();
                        renderer.render_aligned(
                            &items,
                            width,
                            height,
                            px,
                            tcolor(text.color as i64, 255),
                            false,
                            0,
                        )
                    });
                    if let Some(rendered) = rendered
                        && let Some(texture) =
                            upload(tc, rendered.w, rendered.h, &rendered.rgba, filter)
                    {
                        gfx.texts.insert(key.clone(), (0, style, texture));
                    }
                }
                if let Some((_, _, texture)) = gfx.texts.get_mut(&key) {
                    texture.set_alpha_mod(((alpha as u16 * text.alpha as u16) / 255) as u8);
                    let _ = canvas.copy(
                        texture,
                        None,
                        FRect::new(
                            x as f32 + text.x,
                            y as f32 + text.y,
                            texture.query().width as f32,
                            texture.query().height as f32,
                        ),
                    );
                    rendered_text += 1;
                }
            }
            drawn[di].extra = format!("LiveCinema {rendered} image, {rendered_text} text tracks");
            continue;
        }
        match o.kind {
            Kind::Image | Kind::Button | Kind::Tile => {
                let src = if o.kind == Kind::Button {
                    let hover = o
                        .get(Prop::HOVERSOURCE)
                        .map(Value::as_str)
                        .unwrap_or_default();
                    if !hover.is_empty() && vm.is_hovered(&o.name) {
                        hover
                    } else {
                        o.str(Prop::SOURCE)
                    }
                } else {
                    o.str(Prop::SOURCE)
                };
                if src.is_empty() {
                    st!() = "no source";
                    continue;
                }
                if assets.data().resolve(&src, IMAGE_EXTS).is_none() {
                    st!() = "missing: source not in archive";
                    continue;
                }
                let Some(key) = image_for(tc, assets, gfx, &src, filter) else {
                    st!() = "loading (texture not ready)";
                    continue;
                };
                let fkey = format!("{key}#{}", o.frame);
                let has_frame = o.frame == 0 || gfx.images.contains_key(&fkey);
                let current_key = if o.frame > 0 && has_frame {
                    fkey
                } else {
                    key.clone()
                };
                let (iw, ih) = gfx
                    .images
                    .get(&current_key)
                    .map(|(_, w, h)| (*w, *h))
                    .unwrap();
                if ow <= 0 || oh <= 0 {
                    sizes.push((o.name.clone(), iw, ih));
                }
                let (dw, dh) = (
                    if ow > 0 { ow as f32 } else { iw as f32 },
                    if oh > 0 { oh as f32 } else { ih as f32 },
                );
                let interpolation_area = interpolation_areas
                    .get(&o.name)
                    .copied()
                    .unwrap_or(f64::from(dw) * f64::from(dh));
                let blend = if should_interpolate_gal(
                    interpolation_mode,
                    o.kind,
                    interpolation_area,
                    f64::from(lw) * f64::from(lh),
                ) {
                    match assets.try_get(&key) {
                        Some(Asset::Image(image)) => {
                            vm.animation_frame_blend(&o.name, &image.delays)
                        }
                        _ => None,
                    }
                } else {
                    None
                };
                let (current_frame, next_frame, progress) =
                    blend.unwrap_or((o.frame, o.frame, 0.0));
                let current_key = if current_frame == 0 {
                    key.clone()
                } else {
                    format!("{key}#{current_frame}")
                };
                let crossfade_key = format!("{key}:{current_frame}:{next_frame}");
                let crossfade_active = interpolation_mode == InterpolationMode::Crossfade
                    && progress > 0.0
                    && next_frame != current_frame;
                let crossfade_safe = crossfade_active
                    && alpha == u8::MAX
                    && *gfx
                        .crossfade_safety
                        .entry(crossfade_key)
                        .or_insert_with(|| {
                            matches!(
                                assets.try_get(&key),
                                Some(Asset::Image(image))
                                    if can_crossfade_frames(
                                        &image,
                                        current_frame,
                                        next_frame,
                                        alpha,
                                    )
                            )
                        });
                if let Some((texture, _, _)) = gfx.images.get_mut(&current_key) {
                    if crossfade_safe {
                        texture.set_blend_mode(BlendMode::None);
                        texture.set_alpha_mod(u8::MAX);
                    } else {
                        texture.set_blend_mode(BlendMode::Blend);
                        texture.set_alpha_mod(alpha);
                    }
                    if o.kind == Kind::Tile {
                        let mut ty = 0.0;
                        while ty < dh {
                            let mut tx = 0.0;
                            while tx < dw {
                                let cw = (iw as f32).min(dw - tx);
                                let ch = (ih as f32).min(dh - ty);
                                let _ = canvas.copy(
                                    texture,
                                    FRect::new(0.0, 0.0, cw, ch),
                                    FRect::new(x as f32 + tx, y as f32 + ty, cw, ch),
                                );
                                tx += iw as f32;
                            }
                            ty += ih as f32;
                        }
                    } else {
                        let _ = canvas.copy(texture, None, FRect::new(x as f32, y as f32, dw, dh));
                    }
                    if crossfade_safe {
                        texture.set_blend_mode(BlendMode::Blend);
                    }
                }
                if crossfade_safe {
                    let next_key = if next_frame == 0 {
                        key.clone()
                    } else {
                        format!("{key}#{next_frame}")
                    };
                    if let Some((texture, _, _)) = gfx.images.get_mut(&next_key) {
                        texture.set_alpha_mod((f64::from(alpha) * progress).round() as u8);
                        let _ = canvas.copy(texture, None, FRect::new(x as f32, y as f32, dw, dh));
                    }
                }
                drawn[di].extra = format!(
                    "tex {iw}x{ih} frame {current_frame}{}{} draw ({x},{y} {}x{})",
                    if has_frame {
                        ""
                    } else {
                        " (texture for this frame MISSING -> frame 0)"
                    },
                    if crossfade_safe {
                        format!(" blend->{next_frame} {progress:.2}")
                    } else {
                        String::new()
                    },
                    dw as i64,
                    dh as i64
                );
            }
            Kind::Slider => {
                let horizontal = o.int(Prop::ORIENTATION, 0) == 0;
                let bar_src = o.str(Prop::SOURCE);
                let thumb_src = o.str(Prop::THUMBSOURCE);
                let button_src = o.str(Prop::BUTTONSOURCE);
                let bar = if bar_src.is_empty() {
                    None
                } else {
                    image_for(tc, assets, gfx, &bar_src, filter)
                };
                let thumb = if thumb_src.is_empty() {
                    None
                } else {
                    image_for(tc, assets, gfx, &thumb_src, filter)
                };
                let buttons = if button_src.is_empty() {
                    None
                } else {
                    image_for(tc, assets, gfx, &button_src, filter)
                };
                let bar_size = bar
                    .as_ref()
                    .and_then(|key| gfx.images.get(key))
                    .map(|(_, w, h)| (*w, *h));
                let thumb_size = thumb
                    .as_ref()
                    .and_then(|key| gfx.images.get(key))
                    .map(|(_, w, h)| (*w, *h));
                let button_size = buttons
                    .as_ref()
                    .and_then(|key| gfx.images.get(key))
                    .map(|(_, w, h)| (*w, *h));
                let bar_part = SliderPart::new(bar_size, 2, horizontal);
                let thumb_part = SliderPart::new(thumb_size, 3, horizontal);
                let button_part = SliderPart::new(button_size, 6, horizontal);
                let size = slider_size(ow, oh, horizontal, bar_part, thumb_part, button_part);
                let (width, height) = (size.width, size.height);
                if width == 0 || height == 0 {
                    st!() = "slider has no visual source";
                    continue;
                }
                let state = vm.slider_visual_state(&o.name);
                let min = o.int(Prop::MIN, 0) as f64;
                let max = o.int(Prop::MAX, 100) as f64;
                let value = o.int(Prop::VALUE, min as i64) as f64;
                let fraction = if max != min {
                    ((value - min) / (max - min)).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                if let Some(key) = &bar
                    && let Some((texture, _, _)) = gfx.images.get_mut(key)
                    && let Some(part) = bar_part
                {
                    texture.set_alpha_mod(alpha);
                    let (bar_w, bar_h) = part.cell_size;
                    let source = part.source_rect(0);
                    let destination = if horizontal {
                        FRect::new(
                            x as f32,
                            y as f32 + (height.saturating_sub(bar_h) / 2) as f32,
                            width as f32,
                            bar_h.min(height) as f32,
                        )
                    } else {
                        FRect::new(
                            x as f32 + (width.saturating_sub(bar_w) / 2) as f32,
                            y as f32,
                            bar_w.min(width) as f32,
                            height as f32,
                        )
                    };
                    let source = sdl3::rect::Rect::new(source.0, source.1, source.2, source.3);
                    let _ = canvas.copy(texture, source, destination);
                    if part.frames > 1 && fraction > 0.0 {
                        let filled = part.source_rect(1);
                        let fill_destination = if horizontal {
                            FRect::new(
                                x as f32,
                                destination.y,
                                width as f32 * fraction as f32,
                                destination.h,
                            )
                        } else {
                            let fill_height = height as f32 * fraction as f32;
                            FRect::new(
                                destination.x,
                                y as f32 + height as f32 - fill_height,
                                destination.w,
                                fill_height,
                            )
                        };
                        let filled = sdl3::rect::Rect::new(filled.0, filled.1, filled.2, filled.3);
                        let _ = canvas.copy(texture, filled, fill_destination);
                    }
                }
                if let Some(key) = &buttons
                    && let Some((texture, _, _)) = gfx.images.get_mut(key)
                    && let Some(part) = button_part
                {
                    texture.set_alpha_mod(alpha);
                    let states = (part.frames / 2).max(1);
                    let state = state.min(states - 1);
                    let second_state = if part.frames >= 2 {
                        states + state
                    } else {
                        state
                    };
                    let first_src = part.source_rect(state);
                    let second_src = part.source_rect(second_state);
                    let (cell_w, cell_h) = part.cell_size;
                    if horizontal {
                        let by = y as f32 + (height.saturating_sub(cell_h) / 2) as f32;
                        let first_rect = FRect::new(x as f32, by, cell_w as f32, cell_h as f32);
                        let second_rect = FRect::new(
                            x as f32 + width.saturating_sub(cell_w) as f32,
                            by,
                            cell_w as f32,
                            cell_h as f32,
                        );
                        let first_src = sdl3::rect::Rect::new(
                            first_src.0,
                            first_src.1,
                            first_src.2,
                            first_src.3,
                        );
                        let second_src = sdl3::rect::Rect::new(
                            second_src.0,
                            second_src.1,
                            second_src.2,
                            second_src.3,
                        );
                        let _ = canvas.copy(texture, first_src, first_rect);
                        let _ = canvas.copy(texture, second_src, second_rect);
                    } else {
                        let bx = x as f32 + (width.saturating_sub(cell_w) / 2) as f32;
                        let first_rect = FRect::new(bx, y as f32, cell_w as f32, cell_h as f32);
                        let second_rect = FRect::new(
                            bx,
                            y as f32 + height.saturating_sub(cell_h) as f32,
                            cell_w as f32,
                            cell_h as f32,
                        );
                        let first_src = sdl3::rect::Rect::new(
                            first_src.0,
                            first_src.1,
                            first_src.2,
                            first_src.3,
                        );
                        let second_src = sdl3::rect::Rect::new(
                            second_src.0,
                            second_src.1,
                            second_src.2,
                            second_src.3,
                        );
                        let _ = canvas.copy(texture, first_src, first_rect);
                        let _ = canvas.copy(texture, second_src, second_rect);
                    }
                }
                if let Some(key) = &thumb
                    && let Some((texture, _, _)) = gfx.images.get_mut(key)
                    && let Some(part) = thumb_part
                {
                    texture.set_alpha_mod(alpha);
                    let source = part.source_rect(state.min(part.frames - 1));
                    let source = sdl3::rect::Rect::new(source.0, source.1, source.2, source.3);
                    let (tw, th) = part.cell_size;
                    if horizontal {
                        let button_extent = button_part.map(|part| part.cell_size.0).unwrap_or(0);
                        let travel = width
                            .saturating_sub(button_extent.saturating_mul(2).saturating_add(tw));
                        let thumb_x =
                            x as f32 + button_extent as f32 + (fraction * travel as f64) as f32;
                        let thumb_y = y as f32 + (height.saturating_sub(th) / 2) as f32;
                        let _ = canvas.copy(
                            texture,
                            source,
                            FRect::new(thumb_x, thumb_y, tw as f32, th as f32),
                        );
                    } else {
                        let button_extent = button_part.map(|part| part.cell_size.1).unwrap_or(0);
                        let travel = height
                            .saturating_sub(button_extent.saturating_mul(2).saturating_add(th));
                        let thumb_x = x as f32 + (width.saturating_sub(tw) / 2) as f32;
                        let thumb_y =
                            y as f32 + button_extent as f32 + (fraction * travel as f64) as f32;
                        let _ = canvas.copy(
                            texture,
                            source,
                            FRect::new(thumb_x, thumb_y, tw as f32, th as f32),
                        );
                    }
                }
                if ow <= 0 || oh <= 0 {
                    sizes.push((o.name.clone(), width, height));
                }
                drawn[di].extra = format!(
                    "slider value {} range {}..{}",
                    o.int(Prop::VALUE, 0),
                    o.int(Prop::MIN, 0),
                    o.int(Prop::MAX, 100)
                );
            }
            Kind::Box => {
                // Un Box es ante todo un contenedor ("土台"): solo se rellena si tiene COLOR
                // explícito (no clNone/-1) y tamaño. Antes un Box sin COLOR ni tamaño se
                // pintaba negro a pantalla completa y tapaba todo.
                let col = o.get(Prop::COLOR).map(|v| v.as_int());
                let fill = matches!(col, Some(c) if c >= 0 && c != 0x1FFF_FFFF);
                if !fill || ow <= 0 || oh <= 0 {
                    st!() = "container";
                    continue;
                }
                let c = tcolor(col.unwrap(), alpha);
                drawn[di].extra = format!(
                    "box fill TColor=0x{:06X} -> rgb({},{},{})",
                    col.unwrap(),
                    c[0],
                    c[1],
                    c[2]
                );
                canvas.set_blend_mode(BlendMode::Blend);
                canvas.set_draw_color(Color::RGBA(c[0], c[1], c[2], c[3]));
                let _ = canvas.fill_rect(FRect::new(x as f32, y as f32, ow as f32, oh as f32));
            }
            Kind::Menu if vm.prevmenus.contains_key(&o.name) => {
                let pm = vm.prevmenus[&o.name].clone();
                let mut n = 0;
                for (i, b) in pm.buttons.iter().enumerate() {
                    if !vm.menu_item_visible(&o.name, i) {
                        continue;
                    }
                    let src = if pm.hover == Some(i) && !b.src_selected.is_empty() {
                        &b.src_selected
                    } else {
                        &b.src
                    };
                    let Some(key) = image_for(tc, assets, gfx, src, filter) else {
                        continue;
                    };
                    let animation_id = format!("{}:{i}", o.name);
                    let started = *gfx
                        .menu_animation_starts
                        .entry(animation_id.clone())
                        .or_insert(vm.now);
                    active_menu_animations.insert(animation_id);
                    let frame = match assets.try_get(&key) {
                        Some(Asset::Image(image)) => {
                            gal_animation_frame(&image.delays, vm.now - started)
                        }
                        _ => None,
                    };
                    let frame_key = frame
                        .filter(|frame| *frame > 0)
                        .map(|frame| format!("{key}#{frame}"))
                        .filter(|key| gfx.images.contains_key(key))
                        .unwrap_or(key);
                    let (t, iw, ih) = gfx.images.get_mut(&frame_key).unwrap();
                    t.set_alpha_mod(alpha);
                    let _ = canvas.copy(
                        t,
                        None,
                        FRect::new(
                            (x + b.x as f64) as f32,
                            (y + b.y as f64) as f32,
                            *iw as f32,
                            *ih as f32,
                        ),
                    );
                    n += 1;
                }
                drawn[di].extra = format!(
                    "prevmenu {n}/{} buttons drawn, hover {:?}",
                    pm.buttons.len(),
                    pm.hover
                );
            }
            Kind::Message => {
                st!() = draw_message_text(
                    canvas,
                    tc,
                    gfx,
                    font.as_deref_mut(),
                    o,
                    alpha,
                    (x, y),
                    (lw, lh),
                    cfg,
                    filter,
                );
            }
            Kind::Caption => {
                st!() = draw_caption_text(
                    canvas,
                    tc,
                    gfx,
                    font.as_deref_mut(),
                    o,
                    alpha,
                    (x, y),
                    (lw, lh),
                    filter,
                );
            }
            Kind::Edit => {
                st!() = draw_edit_text(
                    canvas,
                    tc,
                    gfx,
                    font.as_deref_mut(),
                    o,
                    alpha,
                    (x, y),
                    vm.active_edit_name() == Some(o.name.as_str()),
                    cfg,
                    filter,
                );
            }
            Kind::Movie => {
                let Some((generation, sequence, pixels, video_width, video_height)) =
                    vm.video_frame(&o.name)
                else {
                    st!() = "video loading";
                    continue;
                };
                let recreate =
                    gfx.video_textures
                        .get(&o.name)
                        .is_none_or(|(_, old_generation, _, w, h)| {
                            *old_generation != generation || *w != video_width || *h != video_height
                        });
                if recreate {
                    let Some(texture) = upload(tc, video_width, video_height, pixels, filter)
                    else {
                        log::error!("cannot create video texture for {:?}", o.name);
                        st!() = "video texture unavailable";
                        continue;
                    };
                    gfx.video_textures.insert(
                        o.name.clone(),
                        (texture, generation, sequence, video_width, video_height),
                    );
                }
                let Some((texture, _, current_sequence, _, _)) =
                    gfx.video_textures.get_mut(&o.name)
                else {
                    st!() = "video texture unavailable";
                    continue;
                };
                if *current_sequence != sequence {
                    if let Err(e) = texture.update(None, pixels, (video_width * 4) as usize) {
                        log::error!("cannot update video texture for {:?}: {e}", o.name);
                        st!() = "video texture update failed";
                        continue;
                    }
                    *current_sequence = sequence;
                }
                texture.set_alpha_mod(alpha);
                let dw = if ow > 0 { ow as f32 } else { lw as f32 };
                let dh = if oh > 0 { oh as f32 } else { lh as f32 };
                let _ = canvas.copy(texture, None, FRect::new(x as f32, y as f32, dw, dh));
                drawn[di].extra = format!("video frame {sequence}");
            }
            _ => st!() = "not rendered (kind)",
        }
    }
    // tamaños intrínsecos para hit-testing y GetProp
    for (n, w, h) in sizes {
        if let Some(o) = vm.scene.get_mut(&n) {
            o.props
                .insert(Prop::WIDTH, crate::lsb::expr::Value::Int(w as i64));
            o.props
                .insert(Prop::HEIGHT, crate::lsb::expr::Value::Int(h as i64));
        }
    }
    gfx.texts.retain(|k, _| {
        vm.scene.objs.contains_key(k)
            || vm
                .cinema_movies
                .keys()
                .any(|name| k.starts_with(&format!("{name}::lcm-text-")))
    });
    gfx.video_textures
        .retain(|name, _| vm.scene.get(name).is_some_and(|o| o.kind == Kind::Movie));
    gfx.menu_animation_starts
        .retain(|key, _| active_menu_animations.contains(key));
    drawn
}

#[cfg(test)]
mod tests {
    use super::{
        add_rect_to_bounds, edit_key_code, gal_animation_frame, inferred_mask_group,
        interpolation_group_key, menu_geometry, menu_hit, should_interpolate_gal, toolbar_hit,
    };
    use crate::interpolation::InterpolationMode;
    use crate::lsb::{Prop, expr::Value};
    use crate::scene::{Kind, Scene};
    use std::collections::HashMap;

    #[test]
    fn interpolation_is_limited_to_large_image_gals_and_toolbar_hit_targets() {
        assert!(should_interpolate_gal(
            InterpolationMode::Crossfade,
            Kind::Image,
            200_000.0,
            600_000.0
        ));
        assert!(!should_interpolate_gal(
            InterpolationMode::Crossfade,
            Kind::Image,
            199_999.0,
            600_000.0
        ));
        assert!(!should_interpolate_gal(
            InterpolationMode::Crossfade,
            Kind::Button,
            600_000.0,
            600_000.0
        ));
        assert!(!should_interpolate_gal(
            InterpolationMode::Off,
            Kind::Image,
            600_000.0,
            600_000.0
        ));
        assert!(matches!(
            toolbar_hit(800, 730.0, 10.0, false),
            Some(super::ToolbarAction::Options)
        ));
        assert!(matches!(
            toolbar_hit(800, 700.0, 90.0, true),
            Some(super::ToolbarAction::Interpolation(
                InterpolationMode::Crossfade
            ))
        ));
        assert!(!super::point_in_logical_render((-1.0, 300.0), (800, 600)));
        assert!(!super::point_in_logical_render((400.0, 600.0), (800, 600)));
        assert!(super::point_in_logical_render((799.0, 599.0), (800, 600)));
    }

    #[test]
    fn crossfade_requires_opaque_frames_and_an_opaque_object() {
        let image = crate::gal::Image {
            w: 1,
            h: 1,
            rgba: vec![10, 20, 30, 255],
            frames: vec![vec![30, 20, 10, 255], vec![30, 20, 10, 0]],
            delays: vec![100, 100, 100],
        };

        assert!(super::can_crossfade_frames(&image, 0, 1, 255));
        assert!(!super::can_crossfade_frames(&image, 0, 2, 255));
        assert!(!super::can_crossfade_frames(&image, 0, 1, 128));
    }

    #[test]
    fn interpolation_groups_use_explicit_tags_or_non_root_parents() {
        let mut scene = Scene::default();
        scene.create("container", Kind::Box, HashMap::new());
        scene.create(
            "base",
            Kind::Image,
            HashMap::from([
                (Prop::PARENT, Value::Str("container".into())),
                (Prop::TAG, Value::Int(7)),
            ]),
        );
        scene.create(
            "mask",
            Kind::Image,
            HashMap::from([
                (Prop::PARENT, Value::Str("container".into())),
                (Prop::TAG, Value::Int(7)),
            ]),
        );
        scene.create(
            "unrelated",
            Kind::Image,
            HashMap::from([
                (Prop::PARENT, Value::Str("container".into())),
                (Prop::TAG, Value::Int(8)),
            ]),
        );

        assert_eq!(
            interpolation_group_key(&scene, scene.get("base").unwrap()),
            interpolation_group_key(&scene, scene.get("mask").unwrap())
        );
        assert_ne!(
            interpolation_group_key(&scene, scene.get("base").unwrap()),
            interpolation_group_key(&scene, scene.get("unrelated").unwrap())
        );

        let mut untagged = Scene::default();
        untagged.create("container", Kind::Box, HashMap::new());
        untagged.create(
            "base",
            Kind::Image,
            HashMap::from([(Prop::PARENT, Value::Str("container".into()))]),
        );
        untagged.create(
            "mask",
            Kind::Image,
            HashMap::from([(Prop::PARENT, Value::Str("container".into()))]),
        );
        untagged.create(
            "unrelated",
            Kind::Image,
            HashMap::from([(Prop::PARENT, Value::Str(crate::vm::ROOT.into()))]),
        );
        assert_eq!(
            interpolation_group_key(&untagged, untagged.get("base").unwrap()),
            interpolation_group_key(&untagged, untagged.get("mask").unwrap())
        );
        assert_ne!(
            interpolation_group_key(&untagged, untagged.get("unrelated").unwrap()),
            interpolation_group_key(&untagged, untagged.get("base").unwrap())
        );
    }

    #[test]
    fn interpolation_group_bounds_cover_related_sprite_union() {
        let mut bounds = None;
        add_rect_to_bounds(&mut bounds, (20.0, 30.0, 120.0, 130.0));
        add_rect_to_bounds(&mut bounds, (90.0, 60.0, 150.0, 90.0));

        let (left, top, right, bottom) = bounds.unwrap();
        assert_eq!((right - left) * (bottom - top), 13_000.0);
    }

    #[test]
    fn inferred_mask_groups_require_matching_animation_timing_and_overlap() {
        let delays = [67, 67, 133];
        assert!(inferred_mask_group(
            (0.0, 0.0, 800.0, 600.0),
            &delays,
            (300.0, 200.0, 500.0, 400.0),
            &delays
        ));
        assert!(!inferred_mask_group(
            (0.0, 0.0, 800.0, 600.0),
            &delays,
            (300.0, 200.0, 500.0, 400.0),
            &[67, 50, 133]
        ));
        assert!(!inferred_mask_group(
            (0.0, 0.0, 800.0, 600.0),
            &delays,
            (790.0, 590.0, 890.0, 690.0),
            &delays
        ));
    }

    #[test]
    fn gal_menu_frames_loop_by_their_declared_delays() {
        let delays = [100, 200, 300];
        assert_eq!(gal_animation_frame(&delays, 0.0), Some(0));
        assert_eq!(gal_animation_frame(&delays, 100.0), Some(1));
        assert_eq!(gal_animation_frame(&delays, 299.0), Some(1));
        assert_eq!(gal_animation_frame(&delays, 300.0), Some(2));
        assert_eq!(gal_animation_frame(&delays, 600.0), Some(0));
    }

    #[test]
    fn edit_keys_use_original_script_virtual_key_codes() {
        assert_eq!(edit_key_code("Return"), Some(13));
        assert_eq!(edit_key_code("Escape"), Some(27));
        assert_eq!(edit_key_code("Tab"), Some(9));
        assert_eq!(edit_key_code("a"), Some(65));
        assert_eq!(edit_key_code("F1"), None);
    }

    #[test]
    fn native_menu_hit_testing_uses_script_position() {
        let choices = vec!["first".to_string(), "second".to_string()];
        let mut scene = Scene::default();
        scene.create(
            "menu",
            Kind::Menu,
            HashMap::from([
                (Prop::LEFT, Value::Int(20)),
                (Prop::TOP, Value::Int(30)),
                (Prop::WIDTH, Value::Int(200)),
            ]),
        );
        let menu = scene.get("menu");

        assert_eq!(menu_hit(&choices, menu, 800, 600, 40.0, 40.0), Some(0));
        assert_eq!(menu_hit(&choices, menu, 800, 600, 400.0, 300.0), None);
    }

    #[test]
    fn native_menu_rows_follow_script_font_and_spacing_properties() {
        let choices = vec!["first".to_string(), "second".to_string()];
        let mut scene = Scene::default();
        scene.create(
            "menu",
            Kind::Menu,
            HashMap::from([
                (Prop::LEFT, Value::Int(20)),
                (Prop::TOP, Value::Int(30)),
                (Prop::FONTHEIGHT, Value::Int(22)),
                (Prop::LINESPACE, Value::Int(4)),
                (Prop::MARGINY, Value::Int(2)),
            ]),
        );

        let (frame, rows) = menu_geometry(&choices, scene.get("menu"), 800, 600);

        assert_eq!(frame.h, 56.0);
        assert_eq!(rows[0].y, 32.0);
        assert_eq!(rows[0].h, 26.0);
        assert_eq!(rows[1].y, 58.0);
    }
}
