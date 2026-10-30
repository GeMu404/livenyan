//! Bucle principal con SDL3: ventana redimensionable, resolución lógica fija con
//! letterbox, render de la escena y entrada.

use crate::assets::{Asset, Assets, IMAGE_EXTS};
use crate::audio::Audio;
use crate::config::Config;
use crate::game::Game;
use crate::input::{Action, Keymap};
use crate::interpolation::InterpolationMode;
use crate::lft::PixelFontCatalog;
use crate::lsb::{Prop, expr::Value};
use crate::scale_filter::ScaleFilter;
use crate::scene::{
    Kind, Obj, Scene, TextItem, menu_line_height, menu_visible_range, native_menu_size,
};
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

mod input;
mod menu;
mod present;
mod scene;
mod toolbar;
use input::*;
use menu::*;
use present::*;
use scene::*;
use toolbar::*;

const IMAGE_TEXTURE_CACHE_LIMIT_BYTES: usize = 256 * 1024 * 1024;

struct Gfx {
    images: HashMap<String, (Texture, u32, u32)>,
    image_texture_cache: HashMap<String, ImageTextureCacheEntry>,
    image_texture_cache_clock: u64,
    image_texture_used_this_frame: HashSet<String>,
    video_textures: HashMap<String, (Texture, u64, u64, u32, u32)>,
    video_previous_textures: HashMap<String, (u64, u64, Texture, u32, u32)>,
    texts: HashMap<String, (u64, usize, Texture)>,
    debug_labels: HashMap<String, (String, Texture, u32, u32)>,
    menu_animation_starts: HashMap<String, f64>,
    menu_fonts: HashMap<String, Option<TextRenderer>>,
    pixel_fonts: Arc<PixelFontCatalog>,
    toolbar_texts: HashMap<String, (usize, Texture)>,
    crossfade_safety: HashMap<String, bool>,
    animation_delays: HashMap<String, Vec<u32>>,
    menu: Option<MenuCache>,
}

struct ImageTextureCacheEntry {
    bytes: usize,
    last_used: u64,
}

#[derive(Default)]
struct TextureMemoryStats {
    image_bytes: usize,
    video_bytes: usize,
    ui_bytes: usize,
    render_target_bytes: usize,
    image_count: usize,
    video_count: usize,
    ui_count: usize,
}

struct MenuCache {
    name: String,
    choices: Vec<String>,
    font_family: String,
    style: Vec<i64>,
    textures: Vec<Option<Texture>>,
    rows: Vec<FRect>,
}

fn upload(
    tc: &TextureCreator<WindowContext>,
    w: u32,
    h: u32,
    rgba: &[u8],
    filter: ScaleMode,
) -> Option<Texture> {
    upload_with_error(tc, w, h, rgba, filter).ok()
}

fn upload_with_error(
    tc: &TextureCreator<WindowContext>,
    w: u32,
    h: u32,
    rgba: &[u8],
    filter: ScaleMode,
) -> Result<Texture, String> {
    let mut t = tc
        .create_texture_static(PixelFormat::RGBA32, w, h)
        .map_err(|error| format!("cannot create {w}x{h} texture: {error}"))?;
    t.update(None, rgba, (w * 4) as usize)
        .map_err(|error| format!("cannot upload {w}x{h} RGBA pixels: {error}"))?;
    t.set_blend_mode(BlendMode::Blend);
    t.set_scale_mode(filter);
    Ok(t)
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
    languages: Vec<crate::text_export::LanguageCatalog>,
    t0: Instant,
    snapshot: Option<(u64, std::path::PathBuf)>,
    debug_layout: bool,
) -> Result<(), String> {
    let default_interpolation = if cfg.window.interpolate_animations {
        InterpolationMode::Crossfade
    } else {
        InterpolationMode::Off
    };
    let mut interpolation_mode = load_interpolation_preference(default_interpolation);
    let sdl = sdl3::init().map_err(|e| e.to_string())?;
    let video = sdl.video().map_err(|e| e.to_string())?;
    log::info!("video driver: {}", video.current_video_driver());
    let (lw, lh) = game.logical;
    let present_scale_boot = ScaleFilter::from_config(&cfg.window.scale_filter);
    let native_present = present_scale_boot.external_upscale() && snapshot.is_none();
    // Native (Lossless Scaling / Magpie / Gamescope): ventana = lógico, sin HiDPI ni
    // letterbox interno — un solo escalado externo (**interpretación** I2b).
    let (ww, wh) = if snapshot.is_some() || native_present {
        if native_present
            && cfg.window.width > 0
            && (cfg.window.width != lw || cfg.window.height != lh)
        {
            log::warn!(
                "scale_filter=native: ignoring window {}x{}, using logical {lw}x{lh}",
                cfg.window.width,
                cfg.window.height
            );
        }
        (lw, lh)
    } else if cfg.window.width > 0 && cfg.window.height > 0 {
        (cfg.window.width, cfg.window.height)
    } else {
        (lw, lh)
    };
    let mut wb = video.window(&format!("livenyan — {}", game.title), ww, wh);
    if !native_present {
        wb.resizable();
    }
    if snapshot.is_some() {
        wb.hidden();
    } else if !native_present {
        // HiDPI duplicaría píxeles antes de un upscaler externo.
        wb.high_pixel_density();
    }
    let mut window = wb.build().map_err(|e| e.to_string())?;
    if cfg.window.fullscreen {
        if native_present {
            log::warn!(
                "scale_filter=native: ignoring fullscreen (use the external scaler instead)"
            );
        } else {
            let _ = window.set_fullscreen(true);
        }
    }
    if !cfg.window.renderer.is_empty() {
        // Probar el driver antes: si no funciona en este sistema, avisar y usar el automático
        let name = std::ffi::CString::new(cfg.window.renderer.as_str()).unwrap_or_default();
        // SAFETY: `window` sigue vivo y `name` es un CString válido durante la llamada; el
        // renderer de prueba se destruye aquí mismo y no escapa del bloque.
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
    // SAFETY: el renderer de `canvas` está vivo; SDL devuelve una cadena estática terminada
    // En NUL (o NULL, que se comprueba) y se copia antes de salir.
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
    // SAFETY: canvas.raw() es el renderer vivo de `canvas`; no se retiene el puntero.
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
    let mut present_scale = present_scale_boot;
    apply_present_scale_hint(&canvas, present_scale);
    if native_present {
        log::info!(
            "present native {lw}x{lh}: 1:1 for external upscalers (Lossless Scaling / Magpie / Gamescope)"
        );
    }
    let mut filter = if present_scale.asset_nearest() {
        ScaleMode::Nearest
    } else {
        ScaleMode::Linear
    };
    scene_frame.set_scale_mode(if present_scale.present_nearest() {
        ScaleMode::Nearest
    } else {
        ScaleMode::Linear
    });
    let mut sharp_compose: Option<Texture> = None;

    // Primer frame cuanto antes
    canvas.set_draw_color(Color::RGB(0, 0, 0));
    canvas.clear();
    canvas.present();
    log::info!(
        "first frame after {:.1} ms",
        t0.elapsed().as_secs_f64() * 1000.0
    );

    // Fuente en segundo plano
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
        image_texture_cache: HashMap::new(),
        image_texture_cache_clock: 0,
        image_texture_used_this_frame: HashSet::new(),
        video_textures: HashMap::new(),
        video_previous_textures: HashMap::new(),
        texts: HashMap::new(),
        debug_labels: HashMap::new(),
        menu_animation_starts: HashMap::new(),
        menu_fonts: HashMap::new(),
        pixel_fonts: vm.pixel_fonts(),
        toolbar_texts: HashMap::new(),
        crossfade_safety: HashMap::new(),
        animation_delays: HashMap::new(),
        menu: None,
    };
    let start = Instant::now();
    let mut menu_sel = 0usize;
    let mut active_menu: Option<String> = None;
    let mut mouse = (-1.0f32, -1.0f32);
    let mut first_scene_logged = false;
    let mut frame: u64 = 0;
    let mut last_dbg = -1000.0f64;
    let mut options_open = false;
    let mut toolbar_open = false;
    let mut language_picker_open = !languages.is_empty() && snapshot.is_none();
    let mut language_selection = default_language_selection(languages.len());
    let mut export_status: Option<String> = None;
    let mut fps_meter_enabled = cfg.window.fps_meter;
    let mut fps_epoch = Instant::now();
    let mut texture_memory_log = Instant::now();
    let mut fps_present_count = 0_u32;
    let mut fps_value = 0_u32;
    let mut vm_time_sum_ms = 0.0_f64;
    let mut scene_time_sum_ms = 0.0_f64;
    let mut present_time_sum_ms = 0.0_f64;
    let mut vm_time_ms = 0.0_f64;
    let mut scene_time_ms = 0.0_f64;
    let mut present_time_ms = 0.0_f64;
    let mut vm_steps_sum = 0_u64;
    let mut vm_steps_per_frame = 0_u64;
    let mut missing_logged: std::collections::HashSet<String> = Default::default();
    let (export_tx, export_rx) = mpsc::channel::<Result<std::path::PathBuf, String>>();

    'main: loop {
        while let Ok(result) = export_rx.try_recv() {
            match result {
                Ok(directory) => match crate::text_export::export(&game.data, &directory) {
                    Ok(summary) => {
                        let catalog = directory.join("texts.toml");
                        log::info!(
                            "exported {} entries ({} dialogues, {} menu options, {} LCM files) to {}",
                            summary.entries,
                            summary.dialogues,
                            summary.menu_items,
                            summary.movies,
                            catalog.display()
                        );
                        println!(
                            "translation catalog: {} entries -> {}",
                            summary.entries,
                            catalog.display()
                        );
                        export_status = Some("Catálogo exportado (ver consola)".into());
                    }
                    Err(error) => {
                        log::error!("cannot export translation catalog: {error}");
                        eprintln!("livenyan: cannot export translation catalog: {error}");
                        export_status = Some("Error al exportar (ver consola)".into());
                    }
                },
                Err(error) => {
                    log::error!("translation folder dialog failed: {error}");
                    eprintln!("livenyan: translation folder dialog failed: {error}");
                    export_status = Some("Error al abrir carpeta (ver consola)".into());
                }
            }
        }
        let mut mouse_moved_this_frame = false;
        for ev in pump.poll_iter() {
            let ev = ev.get_converted_coords(&canvas).unwrap_or(ev);
            log::trace!("{ev:?}");
            match ev {
                Event::Quit { .. } => break 'main,
                Event::Window {
                    win_event: WindowEvent::PixelSizeChanged(..) | WindowEvent::Resized(..),
                    ..
                } => {
                    if present_scale.external_upscale() {
                        // Mantener 1:1: si el SO/compositor cambia el tamaño, volver al lógico.
                        let _ = canvas.window_mut().set_size(lw, lh);
                    }
                    // Con letterbox SDL, el resto se recalcula solo.
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
                    if language_picker_open {
                        match k {
                            sdl3::keyboard::Keycode::Up => {
                                language_selection = language_selection.saturating_sub(1);
                            }
                            sdl3::keyboard::Keycode::Down => {
                                language_selection = (language_selection + 1).min(languages.len());
                            }
                            sdl3::keyboard::Keycode::Return | sdl3::keyboard::Keycode::KpEnter => {
                                apply_language_selection(&mut vm, &languages, language_selection);
                                language_picker_open = false;
                            }
                            sdl3::keyboard::Keycode::Escape => {
                                language_selection = 0;
                                language_picker_open = false;
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if !repeat && k == sdl3::keyboard::Keycode::F3 {
                        fps_meter_enabled = !fps_meter_enabled;
                        continue;
                    }
                    if options_open && k == sdl3::keyboard::Keycode::Escape {
                        options_open = false;
                        if !toolbar_contains(lw, mouse.0, mouse.1, false) {
                            toolbar_open = false;
                        }
                        continue;
                    }
                    if let Some(vk) = windows_vk(k) {
                        if repeat {
                            vm.set_key_repeat(vk);
                        } else {
                            vm.set_key_push(vk, true);
                        }
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
                    if let Some(vk) = windows_vk(k) {
                        vm.set_key_push(vk, false);
                    }
                    if keys.action(&k.name()) == Some(Action::Skip) {
                        vm.skip = false;
                    }
                }
                Event::MouseMotion { x, y, .. } => {
                    mouse_moved_this_frame = true;
                    if point_in_logical_render((x, y), (lw, lh)) {
                        mouse = (x, y);
                        vm.mouse_move(x as f64, y as f64);
                        if !language_picker_open && snapshot.is_none() {
                            update_toolbar_open(&mut toolbar_open, &mut options_open, lw, mouse);
                        }
                    } else {
                        mouse = (-1.0, -1.0);
                        vm.mouse_move(-1.0, -1.0);
                        toolbar_open = false;
                        options_open = false;
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
                    if language_picker_open {
                        let (_, rows) =
                            language_picker_rects((lw, lh), languages.len(), language_selection);
                        if let Some(index) = rows.iter().find_map(|(index, rect)| {
                            (x >= rect.x
                                && x < rect.x + rect.w
                                && y >= rect.y
                                && y < rect.y + rect.h)
                                .then_some(*index)
                        }) {
                            apply_language_selection(&mut vm, &languages, index);
                            language_selection = index;
                            language_picker_open = false;
                        }
                        continue;
                    }
                    if is_toolbar_visible(toolbar_open, options_open)
                        && let Some(action) = toolbar_hit(lw, x, y, options_open)
                    {
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
                            ToolbarAction::Options => {
                                options_open = !options_open;
                                if options_open {
                                    toolbar_open = true;
                                }
                            }
                            ToolbarAction::ToggleFpsMeter => {
                                fps_meter_enabled = !fps_meter_enabled;
                            }
                            ToolbarAction::ExportTexts => {
                                let directory = game.translations_dir();
                                match std::fs::create_dir_all(&directory) {
                                    Ok(()) => {
                                        let sender = export_tx.clone();
                                        sdl3::dialog::show_open_folder_dialog(
                                            Some(directory),
                                            false,
                                            canvas.window(),
                                            Box::new(move |result, _| match result {
                                                Ok(mut paths) => {
                                                    if let Some(path) = paths.pop() {
                                                        let _ = sender.send(Ok(path));
                                                    }
                                                }
                                                Err(sdl3::dialog::DialogError::Canceled) => {}
                                                Err(error) => {
                                                    let _ = sender.send(Err(error.to_string()));
                                                }
                                            }),
                                        );
                                    }
                                    Err(error) => {
                                        let message = format!(
                                            "cannot create translation directory {}: {error}",
                                            directory.display()
                                        );
                                        log::error!("{message}");
                                        eprintln!("livenyan: {message}");
                                        export_status =
                                            Some("Error al crear carpeta (ver consola)".into());
                                    }
                                }
                            }
                            ToolbarAction::ScaleFilter(mode) => {
                                let was_native = present_scale.external_upscale();
                                present_scale = mode;
                                filter = if present_scale.asset_nearest() {
                                    ScaleMode::Nearest
                                } else {
                                    ScaleMode::Linear
                                };
                                scene_frame.set_scale_mode(if present_scale.present_nearest() {
                                    ScaleMode::Nearest
                                } else {
                                    ScaleMode::Linear
                                });
                                apply_cached_texture_scale_modes(&mut gfx, filter);
                                apply_present_scale_hint(&canvas, present_scale);
                                if present_scale.external_upscale() {
                                    let _ = canvas.window_mut().set_fullscreen(false);
                                    let _ = canvas.window_mut().set_size(lw, lh);
                                    // set_resizable: si la API falla, el snap en Resized basta.
                                    log::info!(
                                        "present native {lw}x{lh}: external upscaler owns scaling"
                                    );
                                } else if was_native {
                                    log::info!(
                                        "left native present; resize/letterbox enabled again (restart clears HiDPI limits)"
                                    );
                                }
                                log::info!(
                                    "scale_filter → {} (asset={:?}, present_nearest={})",
                                    present_scale.as_str(),
                                    filter,
                                    present_scale.present_nearest()
                                );
                            }
                            ToolbarAction::Interpolation(mode) => {
                                interpolation_mode = mode;
                                if let Err(error) =
                                    save_interpolation_preference(interpolation_mode)
                                {
                                    log::error!("{error}");
                                }
                                let requested_vsync = cfg.window.vsync
                                    || interpolation_mode != InterpolationMode::Off;
                                // SAFETY: igual que el SDL_SetRenderVSync inicial.
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
                    } else if is_toolbar_visible(toolbar_open, options_open)
                        && toolbar_contains(lw, x, y, options_open)
                    {
                        continue;
                    } else {
                        options_open = false;
                        toolbar_open = false;
                        vm.set_mouse_button(false, true);
                        if let State::Menu { name, choices, .. } = &vm.state {
                            if let Some(i) = menu_hit_cached(
                                &gfx,
                                name,
                                choices,
                                lw,
                                lh,
                                x,
                                y,
                                vm.scene.get(name),
                            ) {
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
                    vm.set_mouse_button(true, true);
                    vm.click_at(x as f64, y as f64, true);
                }
                Event::MouseButtonUp { mouse_btn, .. } => match mouse_btn {
                    MouseButton::Left => {
                        vm.set_mouse_button(false, false);
                        vm.end_slider_drag();
                    }
                    MouseButton::Right => vm.set_mouse_button(true, false),
                    _ => {}
                },
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
                    if y != 0.0 {
                        let menu_context = match &vm.state {
                            State::Menu { name, choices, .. } => {
                                Some((name.clone(), choices.clone()))
                            }
                            _ => None,
                        };
                        if let Some((name, choices)) = menu_context
                            && menu_hit_cached(
                                &gfx,
                                &name,
                                &choices,
                                lw,
                                lh,
                                mouse.0,
                                mouse.1,
                                vm.scene.get(&name),
                            )
                            .is_some()
                        {
                            scroll_menu(
                                &mut vm,
                                &name,
                                choices.len(),
                                y.round() as i64,
                                &mut menu_sel,
                            );
                        }
                    }
                }
                _ => {}
            }
        }
        let menu_selection_context = match &vm.state {
            State::Menu { name, choices, .. } => Some((name.clone(), choices.len())),
            _ => None,
        };
        if let State::Menu { name, choices, .. } = &vm.state {
            if active_menu.as_deref() != Some(name.as_str()) {
                active_menu = Some(name.clone());
                menu_sel = vm
                    .scene
                    .get(name)
                    .map(|menu| {
                        menu_visible_range(
                            choices.len(),
                            menu.int(Prop::ROWCOUNT, 0),
                            menu.int(Prop::TOPINDEX, 0),
                        )
                        .start
                    })
                    .unwrap_or(0);
            }
            menu_sel = menu_sel.min(choices.len().saturating_sub(1));
            if mouse_moved_this_frame
                && let Some(i) = menu_hit_cached(
                    &gfx,
                    name,
                    choices,
                    lw,
                    lh,
                    mouse.0,
                    mouse.1,
                    vm.scene.get(name),
                )
            {
                menu_sel = i;
            }
        } else {
            active_menu = None;
        }
        if let Some((name, choice_count)) = menu_selection_context {
            keep_menu_selection_visible(&mut vm, &name, choice_count, menu_sel);
        }
        if font.is_none()
            && let Ok(r) = frx.try_recv()
        {
            match r {
                Ok(f) => font = Some(f),
                Err(e) => log::error!("{e}"),
            }
        }

        // Con --snapshot-frame el reloj es simulado (60 fps) para que la captura sea reproducible
        let now = if snapshot.is_some() {
            frame as f64 * 1000.0 / 60.0
        } else {
            start.elapsed().as_secs_f64() * 1000.0
        };
        let vm_update_started = Instant::now();
        let vm_steps = if language_picker_open {
            0
        } else {
            vm.update(now, 20_000)
        };
        let vm_update_ms = vm_update_started.elapsed().as_secs_f64() * 1000.0;
        let menu_selection_context = match &vm.state {
            State::Menu { name, choices, .. } => Some((name.clone(), choices.len())),
            _ => None,
        };
        if let State::Menu { name, choices, .. } = &vm.state {
            if active_menu.as_deref() != Some(name.as_str()) {
                active_menu = Some(name.clone());
                menu_sel = vm
                    .scene
                    .get(name)
                    .map(|menu| {
                        menu_visible_range(
                            choices.len(),
                            menu.int(Prop::ROWCOUNT, 0),
                            menu.int(Prop::TOPINDEX, 0),
                        )
                        .start
                    })
                    .unwrap_or(0);
            }
        } else {
            active_menu = None;
        }
        if let Some((name, choice_count)) = menu_selection_context {
            keep_menu_selection_visible(&mut vm, &name, choice_count, menu_sel);
        }
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
                VmEvent::FadeSound {
                    channel,
                    duration_ms,
                } => {
                    if let Some(a) = audio.as_mut() {
                        a.fade_out(&channel, duration_ms)
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

        // Se mantiene la última escena completa hasta decodificar la capa de imagen de la siguiente (I13).
        let scene_render_started = Instant::now();
        let mut drawn = Vec::new();
        let mut ordered_objects: Vec<Obj> = vm.scene.draw_order().into_iter().cloned().collect();
        vm.append_wipe_incoming(&mut ordered_objects);
        let savescreen_req = vm.take_pending_savescreen();
        if scene_images_ready(&assets, &vm, &ordered_objects) {
            let mut captured_rgba: Option<Vec<u8>> = None;
            let need_capture = savescreen_req.is_some();
            canvas
                .with_texture_canvas(&mut scene_frame, |scene_canvas| {
                    scene_canvas.set_draw_color(Color::RGB(0, 0, 0));
                    scene_canvas.clear();
                    drawn = draw_scene(
                        scene_canvas,
                        &tc,
                        &assets,
                        &mut vm,
                        &ordered_objects,
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
                            &vm,
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
                    // SaveScreen: un solo readback GPU→CPU cuando hay pedido (no cada frame).
                    if need_capture
                        && let Ok(shot) = scene_canvas.read_pixels(None)
                        && let Ok(shot) = shot.convert_format(PixelFormat::RGBA32)
                    {
                        let (w, h, pitch) = (shot.width(), shot.height(), shot.pitch() as usize);
                        if w == lw && h == lh {
                            let mut rgba = Vec::with_capacity((w * h * 4) as usize);
                            shot.with_lock(|px: &[u8]| {
                                for y in 0..h as usize {
                                    rgba.extend_from_slice(
                                        &px[y * pitch..y * pitch + w as usize * 4],
                                    );
                                }
                            });
                            captured_rgba = Some(rgba);
                        }
                    }
                })
                .map_err(|e| format!("cannot render scene frame: {e}"))?;
            if let (Some(req), Some(rgba)) = (savescreen_req, captured_rgba) {
                let path = req.path.clone();
                if !vm.complete_savescreen(&req, lw, lh, rgba) {
                    log::warn!("SaveScreen: failed to write {path:?}");
                }
            }
            scene_frame_ready = true;
        } else if let Some(req) = savescreen_req {
            // Escena aún sin texturas: reencolar para el próximo frame listo.
            vm.pending_savescreen = Some(req);
        }
        let scene_render_ms = scene_render_started.elapsed().as_secs_f64() * 1000.0;
        let toolbar_visible = snapshot.is_none()
            && !language_picker_open
            && is_toolbar_visible(toolbar_open, options_open);
        let use_sharp_present = present_scale.present_nearest() && scene_frame_ready;
        let draw_lang = language_picker_open && snapshot.is_none();
        let draw_fps = fps_meter_enabled && snapshot.is_none() && !language_picker_open;
        let sharp_needs_overlay = use_sharp_present && (draw_lang || toolbar_visible || draw_fps);
        if sharp_needs_overlay {
            if sharp_compose.as_ref().is_none_or(|texture| {
                let q = texture.query();
                q.width != lw || q.height != lh
            }) {
                let mut texture = tc
                    .create_texture_target(PixelFormat::RGBA32, lw, lh)
                    .map_err(|e| format!("cannot create sharp compose target: {e}"))?;
                texture.set_blend_mode(BlendMode::None);
                texture.set_scale_mode(ScaleMode::Nearest);
                sharp_compose = Some(texture);
            }
            let compose = sharp_compose
                .as_mut()
                .ok_or_else(|| "sharp compose target missing".to_string())?;
            canvas
                .with_texture_canvas(compose, |overlay| {
                    overlay.set_draw_color(Color::RGB(0, 0, 0));
                    overlay.clear();
                    let _ = overlay.copy(
                        &scene_frame,
                        None,
                        FRect::new(0.0, 0.0, lw as f32, lh as f32),
                    );
                    if draw_lang {
                        draw_language_picker(
                            overlay,
                            &tc,
                            &mut gfx,
                            font.as_mut(),
                            (lw, lh),
                            &languages,
                            language_selection,
                            (mouse.0, mouse.1),
                        );
                    }
                    if toolbar_visible {
                        draw_toolbar(
                            overlay,
                            &tc,
                            &mut gfx,
                            font.as_mut(),
                            (lw, lh),
                            (mouse.0, mouse.1),
                            options_open,
                            interpolation_mode,
                            present_scale,
                            vsync_active,
                            fps_meter_enabled,
                            export_status.as_deref(),
                        );
                    }
                    if draw_fps {
                        draw_fps_meter(
                            overlay,
                            &tc,
                            &mut gfx,
                            font.as_mut(),
                            fps_value,
                            refresh_rate,
                            vm_time_ms,
                            scene_time_ms,
                            present_time_ms,
                            vm_steps_per_frame,
                            vm.budget_location(),
                        );
                    }
                })
                .map_err(|e| format!("cannot composite sharp overlays: {e}"))?;
        } else if !use_sharp_present {
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
            if language_picker_open && snapshot.is_none() {
                draw_language_picker(
                    &mut canvas,
                    &tc,
                    &mut gfx,
                    font.as_mut(),
                    (lw, lh),
                    &languages,
                    language_selection,
                    (mouse.0, mouse.1),
                );
            }
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
                    present_scale,
                    vsync_active,
                    fps_meter_enabled,
                    export_status.as_deref(),
                );
            }
            if fps_meter_enabled && snapshot.is_none() && !language_picker_open {
                draw_fps_meter(
                    &mut canvas,
                    &tc,
                    &mut gfx,
                    font.as_mut(),
                    fps_value,
                    refresh_rate,
                    vm_time_ms,
                    scene_time_ms,
                    present_time_ms,
                    vm_steps_per_frame,
                    vm.budget_location(),
                );
            }
        }
        if texture_memory_log.elapsed() >= std::time::Duration::from_secs(5) {
            texture_memory_log = Instant::now();
            let stats = texture_memory_stats(&gfx, (lw, lh));
            let total_bytes = stats
                .image_bytes
                .saturating_add(stats.video_bytes)
                .saturating_add(stats.ui_bytes)
                .saturating_add(stats.render_target_bytes);
            let mib = |bytes: usize| bytes as f64 / (1024.0 * 1024.0);
            log::info!(
                "estimated GPU textures: total {:.1} MiB | GAL/images {:.1} MiB ({} textures) | video {:.1} MiB ({} textures) | UI/text {:.1} MiB ({} textures) | scene target {:.1} MiB",
                mib(total_bytes),
                mib(stats.image_bytes),
                stats.image_count,
                mib(stats.video_bytes),
                stats.video_count,
                mib(stats.ui_bytes),
                stats.ui_count,
                mib(stats.render_target_bytes)
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
                // Tamaño real en píxeles, consultado en cada log (output_size() puede venir
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
                // Dónde cae el lienzo lógico en píxeles reales (letterbox: escala uniforme, centrado)
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
        let present_started = Instant::now();
        if use_sharp_present {
            let source = if sharp_needs_overlay {
                sharp_compose.as_mut().unwrap_or(&mut scene_frame)
            } else {
                &mut scene_frame
            };
            if let Err(error) = present_sharp_frame(&mut canvas, source, (lw, lh)) {
                log::warn!("sharp present failed; using SDL letterbox: {error}");
                canvas.present();
            }
        } else {
            canvas.present();
        }
        let present_ms = present_started.elapsed().as_secs_f64() * 1000.0;
        fps_present_count = fps_present_count.saturating_add(1);
        vm_time_sum_ms += vm_update_ms;
        scene_time_sum_ms += scene_render_ms;
        present_time_sum_ms += present_ms;
        vm_steps_sum = vm_steps_sum.saturating_add(vm_steps as u64);
        let fps_elapsed = fps_epoch.elapsed();
        if fps_elapsed >= std::time::Duration::from_millis(500) {
            let samples = f64::from(fps_present_count);
            fps_value = (samples / fps_elapsed.as_secs_f64()).round() as u32;
            vm_time_ms = vm_time_sum_ms / samples;
            scene_time_ms = scene_time_sum_ms / samples;
            present_time_ms = present_time_sum_ms / samples;
            vm_steps_per_frame = (vm_steps_sum as f64 / samples).round() as u64;
            fps_present_count = 0;
            vm_time_sum_ms = 0.0;
            scene_time_sum_ms = 0.0;
            present_time_sum_ms = 0.0;
            vm_steps_sum = 0;
            fps_epoch = Instant::now();
        }
        if !first_scene_logged && !vm.scene.is_empty() {
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
        crate::log_dedup::poll();
    }
    if vm.unimpl.is_empty() {
        log::warn!("# unimplemented: none");
    } else {
        log::warn!("# unimplemented ({} unique):", vm.unimpl.len());
        for (k, n) in &vm.unimpl {
            log::warn!("#   {k} x{n}");
        }
    }
    Ok(())
}

fn corner_inset(distance: f32, radius: f32) -> f32 {
    if radius <= 0.5 || distance >= radius {
        return 0.0;
    }
    let opposite = radius - distance;
    radius - (radius * radius - opposite * opposite).max(0.0).sqrt()
}

/// `radii` is top-left, top-right, bottom-right, bottom-left.
fn fill_round_rect(canvas: &mut Canvas<Window>, rect: FRect, radii: [f32; 4], color: Color) {
    if rect.w <= 0.0 || rect.h <= 0.0 {
        return;
    }
    let max_radius = (rect.w * 0.5).min(rect.h * 0.5);
    let radii = radii.map(|radius| radius.clamp(0.0, max_radius));
    canvas.set_draw_color(color);
    let rows = rect.h.ceil() as i32;
    for row in 0..rows {
        let y = (row as f32 + 0.5).min(rect.h);
        let left = if y < radii[0] {
            corner_inset(y, radii[0])
        } else {
            corner_inset(rect.h - y, radii[3])
        };
        let right = if y < radii[1] {
            corner_inset(y, radii[1])
        } else {
            corner_inset(rect.h - y, radii[2])
        };
        let width = rect.w - left - right;
        if width <= 0.0 {
            continue;
        }
        let _ = canvas.fill_rect(FRect::new(rect.x + left, rect.y + row as f32, width, 1.0));
    }
}

fn fill_round_chip(canvas: &mut Canvas<Window>, rect: FRect, fill: Color, rim: Color) {
    let radius = rect.h * 0.5;
    fill_round_rect(canvas, rect, [radius; 4], rim);
    let inner_radius = (radius - 1.0).max(0.0);
    fill_round_rect(
        canvas,
        FRect::new(
            rect.x + 1.0,
            rect.y + 1.0,
            (rect.w - 2.0).max(1.0),
            (rect.h - 2.0).max(1.0),
        ),
        [inner_radius; 4],
        fill,
    );
}

fn fill_vertical_fade(canvas: &mut Canvas<Window>, rect: FRect, top: [u8; 4], bottom: [u8; 4]) {
    if rect.w <= 0.0 || rect.h <= 0.0 {
        return;
    }
    let rows = rect.h.ceil() as i32;
    let denom = (rows - 1).max(1) as f32;
    for row in 0..rows {
        let t = row as f32 / denom;
        let sample = |index: usize| {
            (top[index] as f32 + (bottom[index] as f32 - top[index] as f32) * t).round() as u8
        };
        canvas.set_draw_color(Color::RGBA(sample(0), sample(1), sample(2), sample(3)));
        let _ = canvas.fill_rect(FRect::new(rect.x, rect.y + row as f32, rect.w, 1.0));
    }
}

fn rect_contains(rect: FRect, x: f32, y: f32) -> bool {
    x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h
}

enum ChromeChip {
    Idle,
    Hover,
    On,
}

fn chrome_chip_colors(kind: ChromeChip) -> (Color, Color, [u8; 4]) {
    match kind {
        ChromeChip::On => (
            Color::RGBA(196, 148, 78, 242),
            Color::RGBA(236, 208, 156, 255),
            [42, 30, 18, 255],
        ),
        ChromeChip::Hover => (
            Color::RGBA(214, 176, 112, 70),
            Color::RGBA(222, 186, 128, 190),
            [248, 242, 230, 255],
        ),
        ChromeChip::Idle => (
            Color::RGBA(255, 246, 232, 22),
            Color::RGBA(255, 246, 232, 52),
            [244, 238, 228, 255],
        ),
    }
}

#[cfg(test)]
mod tests;
