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

#[derive(Clone, Copy)]
enum ToolbarAction {
    Quit,
    Fullscreen,
    Options,
    Interpolation(InterpolationMode),
    ScaleFilter(ScaleFilter),
    ToggleFpsMeter,
    ExportTexts,
}

/// Edge strip that reveals the toolbar; keep small so it does not steal play space.
const TOOLBAR_REVEAL_HEIGHT: f32 = 6.0;
/// Drawn bar height. Open chrome stays hit-testable while hovered.
const TOOLBAR_BAR_HEIGHT: f32 = 48.0;
const OPTIONS_PANEL_WIDTH: f32 = 336.0;
const OPTIONS_PANEL_HEIGHT: f32 = 252.0;
const OPTIONS_PANEL_MARGIN: f32 = 14.0;

fn toolbar_button_rects(width: u32) -> [(ToolbarAction, FRect); 3] {
    let h = 30.0;
    let y = (TOOLBAR_BAR_HEIGHT - h) * 0.5;
    let gap = 8.0;
    let right = width as f32 - OPTIONS_PANEL_MARGIN;
    let options = FRect::new(right - 104.0, y, 104.0, h);
    let fullscreen = FRect::new(options.x - gap - 116.0, y, 116.0, h);
    let quit = FRect::new(fullscreen.x - gap - 76.0, y, 76.0, h);
    [
        (ToolbarAction::Quit, quit),
        (ToolbarAction::Fullscreen, fullscreen),
        (ToolbarAction::Options, options),
    ]
}

fn options_panel_rect(width: u32) -> FRect {
    FRect::new(
        width as f32 - OPTIONS_PANEL_MARGIN - OPTIONS_PANEL_WIDTH,
        TOOLBAR_BAR_HEIGHT,
        OPTIONS_PANEL_WIDTH,
        OPTIONS_PANEL_HEIGHT,
    )
}

fn fps_meter_option_rect(width: u32) -> FRect {
    let panel = options_panel_rect(width);
    FRect::new(
        panel.x + panel.w - 12.0 - 112.0,
        panel.y + 136.0,
        112.0,
        28.0,
    )
}

fn export_texts_option_rect(width: u32) -> FRect {
    let panel = options_panel_rect(width);
    FRect::new(panel.x + 12.0, panel.y + 196.0, panel.w - 24.0, 28.0)
}

fn scale_filter_option_rects(width: u32) -> Vec<(ScaleFilter, FRect)> {
    let panel = options_panel_rect(width);
    let y = panel.y + 96.0;
    let modes = [
        ScaleFilter::Linear,
        ScaleFilter::Nearest,
        ScaleFilter::Sharp,
    ];
    let w = 96.0;
    let h = 28.0;
    let gap = 8.0;
    let total_width = modes.len() as f32 * w + (modes.len() - 1) as f32 * gap;
    let x = panel.x + panel.w - 12.0 - total_width;
    modes
        .into_iter()
        .enumerate()
        .map(|(index, mode)| (mode, FRect::new(x + index as f32 * (w + gap), y, w, h)))
        .collect()
}

fn image_texture_cache_evictions(
    cache: &HashMap<String, ImageTextureCacheEntry>,
    required_bytes: usize,
    protected_source: &str,
    used_this_frame: &HashSet<String>,
) -> Vec<String> {
    let mut total_bytes = cache
        .values()
        .fold(0usize, |total, entry| total.saturating_add(entry.bytes));
    let mut candidates = cache
        .iter()
        .filter(|(source, _)| {
            source.as_str() != protected_source && !used_this_frame.contains(source.as_str())
        })
        .map(|(source, entry)| (source.clone(), entry.last_used, entry.bytes))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(_, last_used, _)| *last_used);

    let mut evictions = Vec::new();
    for (source, _, bytes) in candidates {
        if total_bytes.saturating_add(required_bytes) <= IMAGE_TEXTURE_CACHE_LIMIT_BYTES {
            break;
        }
        total_bytes = total_bytes.saturating_sub(bytes);
        evictions.push(source);
    }
    evictions
}

fn evict_image_texture_sources(gfx: &mut Gfx, sources: &[String]) {
    for source in sources {
        let frame_prefix = format!("{source}#");
        gfx.images
            .retain(|key, _| key != source && !key.starts_with(&frame_prefix));
        gfx.image_texture_cache.remove(source);
        gfx.animation_delays.remove(source);
        log::debug!("evicted cached image textures for {source}");
    }
}

fn make_image_texture_cache_room(gfx: &mut Gfx, required_bytes: usize, protected_source: &str) {
    let evictions = image_texture_cache_evictions(
        &gfx.image_texture_cache,
        required_bytes,
        protected_source,
        &gfx.image_texture_used_this_frame,
    );
    evict_image_texture_sources(gfx, &evictions);
}

fn touch_image_texture_source(gfx: &mut Gfx, source: &str) {
    gfx.image_texture_cache_clock = gfx.image_texture_cache_clock.saturating_add(1);
    gfx.image_texture_used_this_frame.insert(source.to_string());
    if let Some(entry) = gfx.image_texture_cache.get_mut(source) {
        entry.last_used = gfx.image_texture_cache_clock;
    }
}

fn estimated_texture_bytes(width: u32, height: u32) -> usize {
    (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(4)
}

fn texture_memory_stats(gfx: &Gfx, logical_size: (u32, u32)) -> TextureMemoryStats {
    let mut stats = TextureMemoryStats {
        image_bytes: gfx
            .image_texture_cache
            .values()
            .fold(0usize, |bytes, entry| bytes.saturating_add(entry.bytes)),
        image_count: gfx.images.len(),
        video_count: gfx.video_textures.len() + gfx.video_previous_textures.len(),
        ui_count: gfx.texts.len()
            + gfx.debug_labels.len()
            + gfx.toolbar_texts.len()
            + gfx
                .menu
                .as_ref()
                .map_or(0, |menu| menu.textures.iter().flatten().count()),
        render_target_bytes: estimated_texture_bytes(logical_size.0, logical_size.1),
        ..TextureMemoryStats::default()
    };

    stats.video_bytes = gfx
        .video_textures
        .values()
        .fold(0usize, |bytes, texture| {
            bytes.saturating_add(estimated_texture_bytes(texture.3, texture.4))
        })
        .saturating_add(
            gfx.video_previous_textures
                .values()
                .fold(0usize, |bytes, texture| {
                    bytes.saturating_add(estimated_texture_bytes(texture.3, texture.4))
                }),
        );
    stats.ui_bytes = gfx
        .texts
        .values()
        .map(|(_, _, texture)| texture)
        .chain(gfx.debug_labels.values().map(|(_, texture, _, _)| texture))
        .chain(gfx.toolbar_texts.values().map(|(_, texture)| texture))
        .chain(
            gfx.menu
                .iter()
                .flat_map(|menu| menu.textures.iter().flatten()),
        )
        .fold(0usize, |bytes, texture| {
            let query = texture.query();
            bytes.saturating_add(estimated_texture_bytes(query.width, query.height))
        });

    stats
}

fn toolbar_hit(width: u32, x: f32, y: f32, options_open: bool) -> Option<ToolbarAction> {
    for (action, rect) in toolbar_button_rects(width) {
        if x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h {
            return Some(action);
        }
    }
    if options_open {
        let fps_rect = fps_meter_option_rect(width);
        if x >= fps_rect.x
            && x < fps_rect.x + fps_rect.w
            && y >= fps_rect.y
            && y < fps_rect.y + fps_rect.h
        {
            return Some(ToolbarAction::ToggleFpsMeter);
        }
        let export_rect = export_texts_option_rect(width);
        if x >= export_rect.x
            && x < export_rect.x + export_rect.w
            && y >= export_rect.y
            && y < export_rect.y + export_rect.h
        {
            return Some(ToolbarAction::ExportTexts);
        }
        for (mode, rect) in interpolation_option_rects(width) {
            if x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h {
                return Some(ToolbarAction::Interpolation(mode));
            }
        }
        for (mode, rect) in scale_filter_option_rects(width) {
            if x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h {
                return Some(ToolbarAction::ScaleFilter(mode));
            }
        }
    }
    None
}

fn toolbar_contains(width: u32, x: f32, y: f32, options_open: bool) -> bool {
    let in_bar = (0.0..TOOLBAR_BAR_HEIGHT).contains(&y) && x >= 0.0 && x < width as f32;
    if in_bar {
        return true;
    }
    if !options_open {
        return false;
    }
    let panel = options_panel_rect(width);
    x >= panel.x && x < panel.x + panel.w && y >= panel.y && y < panel.y + panel.h
}

fn toolbar_in_reveal_strip(mouse_y: f32) -> bool {
    (0.0..TOOLBAR_REVEAL_HEIGHT).contains(&mouse_y)
}

/// Visible when latched open (hover chrome) or while the options panel is open.
fn is_toolbar_visible(toolbar_open: bool, options_open: bool) -> bool {
    toolbar_open || options_open
}

/// Open on the thin top strip; stay open while over bar/panel; close when leaving that chrome.
fn update_toolbar_open(
    toolbar_open: &mut bool,
    options_open: &mut bool,
    width: u32,
    mouse: (f32, f32),
) {
    let (x, y) = mouse;
    if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0 {
        *toolbar_open = false;
        *options_open = false;
        return;
    }
    if toolbar_in_reveal_strip(y) {
        *toolbar_open = true;
    } else if (*toolbar_open || *options_open) && !toolbar_contains(width, x, y, *options_open) {
        *toolbar_open = false;
        *options_open = false;
    }
}

fn point_in_logical_render(point: (f32, f32), logical: (u32, u32)) -> bool {
    point.0.is_finite()
        && point.1.is_finite()
        && point.0 >= 0.0
        && point.1 >= 0.0
        && point.0 < logical.0 as f32
        && point.1 < logical.1 as f32
}

fn interpolation_option_rects(width: u32) -> Vec<(InterpolationMode, FRect)> {
    let panel = options_panel_rect(width);
    let y = panel.y + 38.0;
    let modes = [InterpolationMode::Off, InterpolationMode::Crossfade];
    let w = 108.0;
    let h = 28.0;
    let gap = 8.0;
    let total_width = modes.len() as f32 * w + (modes.len() - 1) as f32 * gap;
    let x = panel.x + panel.w - 12.0 - total_width;
    modes
        .into_iter()
        .enumerate()
        .map(|(index, mode)| (mode, FRect::new(x + index as f32 * (w + gap), y, w, h)))
        .collect()
}

fn gal_frame_pixels(image: &crate::gal::Image, frame: usize) -> Option<&[u8]> {
    if frame == 0 {
        Some(&image.rgba)
    } else {
        image.frames.get(frame - 1).map(Vec::as_slice)
    }
}

fn frame_is_opaque(image: &crate::gal::Image, frame: usize) -> bool {
    gal_frame_pixels(image, frame).is_some_and(|pixels| {
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| pixel[3] == u8::MAX)
    })
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
    for (index, member) in members.iter().enumerate() {
        let key = if member.1.starts_with("object:") {
            format!("overlap:{}", find_group_root(&mut parents, index))
        } else {
            member.1.clone()
        };
        add_rect_to_bounds(group_bounds.entry(key.clone()).or_insert(None), member.2);
        object_groups.push((member.0.clone(), key));
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
    #[derive(Default)]
    struct UiPreferences {
        interpolation_mode: Option<String>,
        interpolate_animations: bool,
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
    draw_toolbar_text_styled(
        canvas,
        tc,
        gfx,
        font,
        text,
        x,
        y,
        width,
        height,
        [245, 245, 250, 255],
        0,
    );
}

fn draw_toolbar_text_styled(
    canvas: &mut Canvas<Window>,
    tc: &TextureCreator<WindowContext>,
    gfx: &mut Gfx,
    font: Option<&mut TextRenderer>,
    text: &str,
    x: f32,
    y: f32,
    width: u32,
    height: u32,
    color: [u8; 4],
    align: u8,
) {
    let Some(font) = font else {
        return;
    };
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (text, width, height, 16_u32, color, align).hash(&mut hasher);
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
        let image = font.render_aligned(&items, width, height, 16.0, color, false, align);
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

fn draw_toolbar_chip_label(
    canvas: &mut Canvas<Window>,
    tc: &TextureCreator<WindowContext>,
    gfx: &mut Gfx,
    font: Option<&mut TextRenderer>,
    rect: FRect,
    text: &str,
    color: [u8; 4],
) {
    let text_h = 22.0;
    draw_toolbar_text_styled(
        canvas,
        tc,
        gfx,
        font,
        text,
        rect.x,
        rect.y + (rect.h - text_h) / 2.0,
        rect.w.max(1.0) as u32,
        text_h as u32,
        color,
        1,
    );
}

fn language_picker_rects(
    (width, height): (u32, u32),
    language_count: usize,
    selection: usize,
) -> (FRect, Vec<(usize, FRect)>) {
    let total_rows = language_count + 1;
    let row_height = 30.0;
    let max_rows = ((height.saturating_sub(144)) / row_height as u32)
        .max(1)
        .min(total_rows as u32) as usize;
    let offset = selection
        .saturating_add(1)
        .saturating_sub(max_rows)
        .min(total_rows.saturating_sub(max_rows));
    let visible_rows = max_rows.min(total_rows);
    let panel_width = width.saturating_sub(32).min(500) as f32;
    let panel_height = 112.0 + visible_rows as f32 * row_height;
    let panel = FRect::new(
        (width as f32 - panel_width) / 2.0,
        (height as f32 - panel_height) / 2.0,
        panel_width,
        panel_height,
    );
    let rows = (offset..offset + visible_rows)
        .map(|index| {
            (
                index,
                FRect::new(
                    panel.x + 12.0,
                    panel.y + 60.0 + (index - offset) as f32 * row_height,
                    panel.w - 24.0,
                    row_height - 2.0,
                ),
            )
        })
        .collect();
    (panel, rows)
}

fn default_language_selection(language_count: usize) -> usize {
    usize::from(language_count > 0)
}

fn apply_language_selection(
    vm: &mut Vm,
    languages: &[crate::text_export::LanguageCatalog],
    selection: usize,
) {
    if let Some(language) = selection
        .checked_sub(1)
        .and_then(|index| languages.get(index))
    {
        vm.set_translations(language.translations.clone());
        log::info!(
            "selected translation catalog {} ({}) with {} entries",
            language.name,
            language.code,
            language.translations.len()
        );
    } else {
        vm.set_translations(HashMap::new());
        log::info!("selected original game text");
    }
}

fn draw_language_picker(
    canvas: &mut Canvas<Window>,
    tc: &TextureCreator<WindowContext>,
    gfx: &mut Gfx,
    font: Option<&mut TextRenderer>,
    dimensions: (u32, u32),
    languages: &[crate::text_export::LanguageCatalog],
    selection: usize,
    mouse: (f32, f32),
) {
    let (panel, rows) = language_picker_rects(dimensions, languages.len(), selection);
    let mut font = font;
    canvas.set_blend_mode(BlendMode::Blend);
    canvas.set_draw_color(Color::RGBA(0, 0, 0, 190));
    let _ = canvas.fill_rect(FRect::new(
        0.0,
        0.0,
        dimensions.0 as f32,
        dimensions.1 as f32,
    ));
    canvas.set_draw_color(Color::RGBA(16, 18, 26, 250));
    let _ = canvas.fill_rect(panel);
    canvas.set_draw_color(Color::RGBA(126, 137, 160, 255));
    let _ = canvas.draw_rect(panel);
    draw_toolbar_text(
        canvas,
        tc,
        gfx,
        font.as_deref_mut(),
        "Selecciona idioma",
        panel.x + 16.0,
        panel.y + 12.0,
        panel.w as u32 - 32,
        28,
    );
    for (index, rect) in rows {
        let hovered = mouse.0 >= rect.x
            && mouse.0 < rect.x + rect.w
            && mouse.1 >= rect.y
            && mouse.1 < rect.y + rect.h;
        canvas.set_draw_color(if index == selection {
            Color::RGBA(55, 112, 84, 255)
        } else if hovered {
            Color::RGBA(73, 99, 145, 255)
        } else {
            Color::RGBA(43, 47, 61, 255)
        });
        let _ = canvas.fill_rect(rect);
        canvas.set_draw_color(Color::RGBA(126, 137, 160, 255));
        let _ = canvas.draw_rect(rect);
        let label = if index == 0 {
            "Original (sin traducción)"
        } else {
            languages[index - 1].name.as_str()
        };
        draw_toolbar_text(
            canvas,
            tc,
            gfx,
            font.as_deref_mut(),
            label,
            rect.x + 10.0,
            rect.y,
            rect.w as u32 - 20,
            rect.h as u32,
        );
    }
    draw_toolbar_text(
        canvas,
        tc,
        gfx,
        font,
        "Flechas + Enter · Esc: original",
        panel.x + 16.0,
        panel.y + panel.h - 32.0,
        panel.w as u32 - 32,
        22,
    );
}

fn apply_present_scale_hint(canvas: &Canvas<Window>, present_scale: ScaleFilter) {
    let mode = if present_scale.present_nearest() {
        sdl3::sys::surface::SDL_SCALEMODE_NEAREST
    } else {
        sdl3::sys::surface::SDL_SCALEMODE_LINEAR
    };
    // SAFETY: canvas.raw() es el renderer vivo de `canvas`; la llamada no retiene el puntero.
    unsafe {
        let _ = sdl3::sys::render::SDL_SetDefaultTextureScaleMode(canvas.raw(), mode);
    }
}

fn apply_cached_texture_scale_modes(gfx: &mut Gfx, mode: ScaleMode) {
    for (texture, _, _) in gfx.images.values_mut() {
        texture.set_scale_mode(mode);
    }
    for (texture, _, _, _, _) in gfx.video_textures.values_mut() {
        texture.set_scale_mode(mode);
    }
    for (_, _, texture, _, _) in gfx.video_previous_textures.values_mut() {
        texture.set_scale_mode(mode);
    }
    for (_, _, texture) in gfx.texts.values_mut() {
        texture.set_scale_mode(mode);
    }
    for (_, texture) in gfx.toolbar_texts.values_mut() {
        texture.set_scale_mode(mode);
    }
    if let Some(menu) = gfx.menu.as_mut() {
        for texture in menu.textures.iter_mut().flatten() {
            texture.set_scale_mode(mode);
        }
    }
}

/// Letterbox del framebuffer lógico con nearest (líneas nítidas, sin IA).
fn present_sharp_frame(
    canvas: &mut Canvas<Window>,
    scene_frame: &mut Texture,
    logical: (u32, u32),
) -> Result<(), String> {
    let (lw, lh) = logical;
    let (pw, ph) = canvas.window().size_in_pixels();
    let (bx, by, cw, ch) = crate::util::letterbox((pw, ph), (lw, lh));
    scene_frame.set_scale_mode(ScaleMode::Nearest);
    canvas
        .set_logical_size(0, 0, sdl3::sys::render::SDL_LOGICAL_PRESENTATION_DISABLED)
        .map_err(|e| e.to_string())?;
    canvas.set_draw_color(Color::RGB(0, 0, 0));
    canvas.clear();
    canvas
        .copy(
            scene_frame,
            None,
            FRect::new(bx as f32, by as f32, cw as f32, ch as f32),
        )
        .map_err(|e| e.to_string())?;
    canvas.present();
    canvas
        .set_logical_size(
            lw,
            lh,
            sdl3::sys::render::SDL_LOGICAL_PRESENTATION_LETTERBOX,
        )
        .map_err(|e| e.to_string())?;
    Ok(())
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
    // en NUL (o NULL, que se comprueba) y se copia antes de salir.
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
    let mut present_scale = ScaleFilter::from_config(&cfg.window.scale_filter);
    apply_present_scale_hint(&canvas, present_scale);
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

        // con --snapshot-frame el reloj es simulado (60 fps) para que la captura sea reproducible
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

        // Keep the last complete scene until the picture layer of the next one is decoded.
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

fn draw_toolbar(
    canvas: &mut Canvas<Window>,
    tc: &TextureCreator<WindowContext>,
    gfx: &mut Gfx,
    font: Option<&mut TextRenderer>,
    (width, _height): (u32, u32),
    mouse: (f32, f32),
    options_open: bool,
    interpolation_mode: InterpolationMode,
    present_scale: ScaleFilter,
    vsync_active: bool,
    fps_meter_enabled: bool,
    export_status: Option<&str>,
) {
    let mut font = font;
    canvas.set_blend_mode(BlendMode::Blend);
    fill_vertical_fade(
        canvas,
        FRect::new(0.0, 0.0, width as f32, TOOLBAR_BAR_HEIGHT),
        [28, 22, 34, 244],
        [12, 10, 16, 230],
    );
    fill_vertical_fade(
        canvas,
        FRect::new(0.0, TOOLBAR_BAR_HEIGHT, width as f32, 8.0),
        [8, 6, 12, 72],
        [8, 6, 12, 0],
    );
    canvas.set_draw_color(Color::RGBA(214, 170, 104, 96));
    let _ = canvas.fill_rect(FRect::new(0.0, TOOLBAR_BAR_HEIGHT - 1.0, width as f32, 1.0));
    fill_round_rect(
        canvas,
        FRect::new(16.0, (TOOLBAR_BAR_HEIGHT - 8.0) * 0.5, 8.0, 8.0),
        [2.0; 4],
        Color::RGBA(214, 170, 104, 235),
    );
    draw_toolbar_text_styled(
        canvas,
        tc,
        gfx,
        font.as_deref_mut(),
        "livenyan",
        30.0,
        (TOOLBAR_BAR_HEIGHT - 22.0) * 0.5,
        140,
        22,
        [222, 190, 138, 255],
        0,
    );
    for (action, rect) in toolbar_button_rects(width) {
        let hovered = rect_contains(rect, mouse.0, mouse.1);
        let active = matches!(action, ToolbarAction::Options) && options_open;
        let (fill, rim, text_color) = chrome_chip_colors(if active {
            ChromeChip::On
        } else if hovered {
            ChromeChip::Hover
        } else {
            ChromeChip::Idle
        });
        fill_round_chip(canvas, rect, fill, rim);
        let label = match action {
            ToolbarAction::Quit => "Salir",
            ToolbarAction::Fullscreen => "Pantalla",
            ToolbarAction::Options => "Opciones",
            ToolbarAction::Interpolation(_)
            | ToolbarAction::ScaleFilter(_)
            | ToolbarAction::ToggleFpsMeter
            | ToolbarAction::ExportTexts => continue,
        };
        draw_toolbar_chip_label(
            canvas,
            tc,
            gfx,
            font.as_deref_mut(),
            rect,
            label,
            text_color,
        );
    }
    if options_open {
        let panel = options_panel_rect(width);
        let radius = 12.0;
        let label_w = (panel.w as u32).saturating_sub(32);
        let muted = [176, 164, 146, 255];
        fill_round_rect(
            canvas,
            FRect::new(panel.x + 3.0, panel.y + 6.0, panel.w, panel.h),
            [0.0, 0.0, radius, radius],
            Color::RGBA(0, 0, 0, 80),
        );
        fill_round_rect(
            canvas,
            panel,
            [0.0, 0.0, radius, radius],
            Color::RGBA(232, 206, 160, 88),
        );
        fill_round_rect(
            canvas,
            FRect::new(panel.x + 1.0, panel.y, panel.w - 2.0, panel.h - 1.0),
            [0.0, 0.0, (radius - 1.0).max(0.0), (radius - 1.0).max(0.0)],
            Color::RGBA(18, 15, 22, 248),
        );
        draw_toolbar_text_styled(
            canvas,
            tc,
            gfx,
            font.as_deref_mut(),
            "Fundido de GAL opacos grandes",
            panel.x + 16.0,
            panel.y + 12.0,
            label_w,
            22,
            muted,
            0,
        );
        for (mode, rect) in interpolation_option_rects(width) {
            let selected = mode == interpolation_mode;
            let (fill, rim, text_color) = chrome_chip_colors(if selected {
                ChromeChip::On
            } else if rect_contains(rect, mouse.0, mouse.1) {
                ChromeChip::Hover
            } else {
                ChromeChip::Idle
            });
            fill_round_chip(canvas, rect, fill, rim);
            let label = match mode {
                InterpolationMode::Off => "Off",
                InterpolationMode::Crossfade => "Fundido",
            };
            draw_toolbar_chip_label(
                canvas,
                tc,
                gfx,
                font.as_deref_mut(),
                rect,
                label,
                text_color,
            );
        }
        draw_toolbar_text_styled(
            canvas,
            tc,
            gfx,
            font.as_deref_mut(),
            "Escalado present",
            panel.x + 16.0,
            panel.y + 74.0,
            label_w,
            20,
            muted,
            0,
        );
        for (mode, rect) in scale_filter_option_rects(width) {
            let selected = mode == present_scale;
            let (fill, rim, text_color) = chrome_chip_colors(if selected {
                ChromeChip::On
            } else if rect_contains(rect, mouse.0, mouse.1) {
                ChromeChip::Hover
            } else {
                ChromeChip::Idle
            });
            fill_round_chip(canvas, rect, fill, rim);
            let label = match mode {
                ScaleFilter::Linear => "Linear",
                ScaleFilter::Nearest => "Nearest",
                ScaleFilter::Sharp => "Sharp",
            };
            draw_toolbar_chip_label(
                canvas,
                tc,
                gfx,
                font.as_deref_mut(),
                rect,
                label,
                text_color,
            );
        }
        let fps_rect = fps_meter_option_rect(width);
        let (fill, rim, text_color) = chrome_chip_colors(if fps_meter_enabled {
            ChromeChip::On
        } else if rect_contains(fps_rect, mouse.0, mouse.1) {
            ChromeChip::Hover
        } else {
            ChromeChip::Idle
        });
        fill_round_chip(canvas, fps_rect, fill, rim);
        draw_toolbar_text_styled(
            canvas,
            tc,
            gfx,
            font.as_deref_mut(),
            "Medidor",
            panel.x + 16.0,
            fps_rect.y + (fps_rect.h - 22.0) / 2.0,
            140,
            22,
            muted,
            0,
        );
        draw_toolbar_chip_label(
            canvas,
            tc,
            gfx,
            font.as_deref_mut(),
            fps_rect,
            if fps_meter_enabled {
                "FPS: On"
            } else {
                "FPS: Off"
            },
            text_color,
        );
        let detail = if vsync_active {
            "Solo GAL opacos grandes · refresco del monitor"
        } else {
            "Solo GAL opacos grandes · VSync no disponible"
        };
        draw_toolbar_text_styled(
            canvas,
            tc,
            gfx,
            font.as_deref_mut(),
            detail,
            panel.x + 16.0,
            panel.y + 168.0,
            label_w,
            20,
            muted,
            0,
        );
        let export_rect = export_texts_option_rect(width);
        let (fill, rim, text_color) =
            chrome_chip_colors(if rect_contains(export_rect, mouse.0, mouse.1) {
                ChromeChip::Hover
            } else {
                ChromeChip::Idle
            });
        fill_round_chip(canvas, export_rect, fill, rim);
        draw_toolbar_chip_label(
            canvas,
            tc,
            gfx,
            font.as_deref_mut(),
            export_rect,
            "Exportar textos…",
            text_color,
        );
        if let Some(status) = export_status {
            draw_toolbar_text_styled(
                canvas,
                tc,
                gfx,
                font,
                status,
                panel.x + 16.0,
                panel.y + 228.0,
                label_w,
                20,
                [222, 190, 138, 255],
                0,
            );
        }
    }
}

fn draw_fps_meter(
    canvas: &mut Canvas<Window>,
    tc: &TextureCreator<WindowContext>,
    gfx: &mut Gfx,
    mut font: Option<&mut TextRenderer>,
    fps: u32,
    refresh_rate: f32,
    vm_time_ms: f64,
    scene_time_ms: f64,
    present_time_ms: f64,
    vm_steps_per_frame: u64,
    vm_location: Option<&str>,
) {
    canvas.set_blend_mode(BlendMode::Blend);
    canvas.set_draw_color(Color::RGBA(16, 18, 26, 220));
    let _ = canvas.fill_rect(FRect::new(8.0, 52.0, 382.0, 96.0));
    canvas.set_draw_color(Color::RGBA(126, 137, 160, 255));
    let _ = canvas.draw_rect(FRect::new(8.0, 52.0, 382.0, 96.0));
    draw_toolbar_text(
        canvas,
        tc,
        gfx,
        font.as_deref_mut(),
        &format!("FPS {fps} / {refresh_rate:.0} Hz"),
        15.0,
        52.0,
        366,
        30,
    );
    draw_toolbar_text(
        canvas,
        tc,
        gfx,
        font.as_deref_mut(),
        &format!("VM {vm_time_ms:.2} ms ({vm_steps_per_frame} steps)"),
        15.0,
        78.0,
        366,
        24,
    );
    draw_toolbar_text(
        canvas,
        tc,
        gfx,
        font.as_deref_mut(),
        &format!("Draw {scene_time_ms:.2} ms | Present {present_time_ms:.2} ms"),
        15.0,
        96.0,
        366,
        24,
    );
    draw_toolbar_text(
        canvas,
        tc,
        gfx,
        font,
        &vm_location
            .map(|location| format!("At {location}"))
            .unwrap_or_else(|| "VM below step cap".to_string()),
        15.0,
        116.0,
        366,
        24,
    );
}

fn menu_geometry(choices: &[String], menu: Option<&Obj>, lw: u32, lh: u32) -> (FRect, Vec<FRect>) {
    menu_geometry_with_width(choices, menu, lw, lh, None)
}

fn menu_geometry_with_width(
    choices: &[String],
    menu: Option<&Obj>,
    lw: u32,
    lh: u32,
    measured_width: Option<f32>,
) -> (FRect, Vec<FRect>) {
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
        .and_then(|o| o.get(Prop::WIDTH).map(Value::as_int))
        .filter(|width| *width > 0)
        .map(|width| width as f32)
        .or_else(|| measured_width.map(|width| width.min(lw as f32)))
        .unwrap_or(default_w as f32)
        .max(0.0);
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

fn menu_content_rows(
    mut frame: FRect,
    rows: &[FRect],
    left_inset: f32,
    right_inset: f32,
    top_inset: f32,
    bottom_inset: f32,
    min_row_height: f32,
) -> (FRect, Vec<FRect>) {
    let (Some(first), Some(last)) = (rows.first(), rows.last()) else {
        return (frame, Vec::new());
    };
    let top = first.y.max(frame.y + top_inset.max(0.0));
    let original_bottom = (last.y + last.h).min(frame.y + frame.h - bottom_inset.max(0.0));
    let available_height = original_bottom - top;
    if !available_height.is_finite() {
        return (frame, rows.to_vec());
    }
    let min_content_height = min_row_height.max(1.0) * rows.len() as f32;
    let content_height = if available_height < min_content_height {
        frame.h = (top - frame.y + min_content_height + bottom_inset.max(0.0)).max(frame.h);
        min_content_height
    } else {
        available_height
    };
    let row_height = content_height / rows.len() as f32;
    let content_x = frame.x + left_inset.max(0.0);
    let content_width = (frame.w - left_inset.max(0.0) - right_inset.max(0.0)).max(0.0);
    let content_rows = rows
        .iter()
        .enumerate()
        .map(|(index, _)| {
            FRect::new(
                content_x,
                top + index as f32 * row_height,
                content_width,
                row_height,
            )
        })
        .collect();
    (frame, content_rows)
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct MenuScrollbarGeometry {
    x: f32,
    top: f32,
    button_size: f32,
    track_height: f32,
    thumb_offset: f32,
    thumb_height: f32,
}

fn menu_scrollbar_geometry(
    frame: FRect,
    track_top: f32,
    track_bottom: f32,
    right_inset: f32,
    source_cell: (u32, u32),
    choice_count: usize,
    row_count: i64,
    top_index: i64,
) -> Option<MenuScrollbarGeometry> {
    let visible = menu_visible_range(choice_count, row_count, top_index);
    if visible.len() >= choice_count || source_cell.0 == 0 || source_cell.1 == 0 {
        return None;
    }
    let button_size = source_cell.1 as f32;
    let track_height = (track_bottom - track_top - button_size * 2.0).max(0.0);
    if button_size > track_bottom - track_top || track_height <= 0.0 {
        return None;
    }
    let max_top = choice_count.saturating_sub(visible.len());
    let thumb_height = (track_height * visible.len() as f32 / choice_count as f32)
        .max(button_size.min(track_height));
    let max_offset = track_height - thumb_height;
    let progress = if max_top == 0 {
        0.0
    } else {
        visible.start as f32 / max_top as f32
    };
    Some(MenuScrollbarGeometry {
        x: frame.x + frame.w - right_inset - source_cell.0 as f32,
        top: track_top,
        button_size,
        track_height,
        thumb_offset: max_offset * progress,
        thumb_height,
    })
}

fn menu_hit(
    choices: &[String],
    menu: Option<&Obj>,
    lw: u32,
    lh: u32,
    x: f32,
    y: f32,
) -> Option<usize> {
    let range = menu_visible_range(
        choices.len(),
        menu.map(|object| object.int(Prop::ROWCOUNT, 0))
            .unwrap_or(0),
        menu.map(|object| object.int(Prop::TOPINDEX, 0))
            .unwrap_or(0),
    );
    let rows = menu_geometry(&choices[range.clone()], menu, lw, lh).1;
    menu_hit_rows(&rows, x, y).map(|index| range.start + index)
}

fn menu_hit_rows(rows: &[FRect], x: f32, y: f32) -> Option<usize> {
    rows.iter()
        .position(|r| x >= r.x && y >= r.y && x < r.x + r.w && y < r.y + r.h)
}

fn menu_hit_cached(
    gfx: &Gfx,
    name: &str,
    choices: &[String],
    lw: u32,
    lh: u32,
    x: f32,
    y: f32,
    menu: Option<&Obj>,
) -> Option<usize> {
    let range = menu_visible_range(
        choices.len(),
        menu.map(|object| object.int(Prop::ROWCOUNT, 0))
            .unwrap_or(0),
        menu.map(|object| object.int(Prop::TOPINDEX, 0))
            .unwrap_or(0),
    );
    let visible_choices = &choices[range.clone()];
    if let Some(cache) = gfx
        .menu
        .as_ref()
        .filter(|cache| cache.name == name && cache.choices == visible_choices)
    {
        menu_hit_rows(&cache.rows, x, y).map(|index| range.start + index)
    } else {
        menu_hit(choices, menu, lw, lh, x, y)
    }
}

fn keep_menu_selection_visible(vm: &mut Vm, name: &str, choice_count: usize, selection: usize) {
    let Some(menu) = vm.scene.get(name) else {
        return;
    };
    let row_count = menu.int(Prop::ROWCOUNT, 0);
    if row_count <= 0 || choice_count == 0 {
        return;
    }
    let current_top = menu.int(Prop::TOPINDEX, 0);
    let range = menu_visible_range(choice_count, row_count, current_top);
    let next_top = if selection < range.start {
        selection
    } else if selection >= range.end {
        selection.saturating_add(1).saturating_sub(range.len())
    } else {
        range.start
    };
    if i64::try_from(next_top).unwrap_or(i64::MAX) != current_top {
        vm.scene.set(
            name,
            Prop::TOPINDEX,
            Value::Int(i64::try_from(next_top).unwrap_or(i64::MAX)),
        );
    }
}

fn scroll_menu(vm: &mut Vm, name: &str, choice_count: usize, amount: i64, selection: &mut usize) {
    let Some(menu) = vm.scene.get(name) else {
        return;
    };
    let row_count = menu.int(Prop::ROWCOUNT, 0);
    if row_count <= 0 || choice_count == 0 {
        return;
    }
    let current = menu_visible_range(choice_count, row_count, menu.int(Prop::TOPINDEX, 0));
    if current.len() >= choice_count {
        return;
    }
    let max_top = choice_count.saturating_sub(current.len());
    let current_top = i64::try_from(current.start).unwrap_or(i64::MAX);
    let max_top = i64::try_from(max_top).unwrap_or(i64::MAX);
    let next_top = usize::try_from((current_top - amount).clamp(0, max_top)).unwrap_or(usize::MAX);
    if next_top == current.start {
        return;
    }
    if next_top > current.start {
        *selection = selection.saturating_add(next_top - current.start);
    } else {
        *selection = selection.saturating_sub(current.start - next_top);
    }
    vm.scene.set(
        name,
        Prop::TOPINDEX,
        Value::Int(i64::try_from(next_top).unwrap_or(i64::MAX)),
    );
}

/// Código de tecla de Windows (`@KeyPush` / `@KeyClick`, docs/livenovel/calc.html).
#[allow(clippy::just_underscores_and_digits)] // `_0`..`_9` son las variantes de Keycode de SDL
fn windows_vk(key: sdl3::keyboard::Keycode) -> Option<usize> {
    use sdl3::keyboard::Keycode::*;
    Some(match key {
        Backspace => 0x08,
        Tab => 0x09,
        Return | KpEnter => 0x0D,
        LShift | RShift => 0x10,
        LCtrl | RCtrl => 0x11,
        LAlt | RAlt => 0x12,
        Escape => 0x1B,
        Space => 0x20,
        PageUp => 0x21,
        PageDown => 0x22,
        End => 0x23,
        Home => 0x24,
        Left => 0x25,
        Up => 0x26,
        Right => 0x27,
        Down => 0x28,
        Insert => 0x2D,
        Delete => 0x2E,
        _0 => 0x30,
        _1 => 0x31,
        _2 => 0x32,
        _3 => 0x33,
        _4 => 0x34,
        _5 => 0x35,
        _6 => 0x36,
        _7 => 0x37,
        _8 => 0x38,
        _9 => 0x39,
        A => 0x41,
        B => 0x42,
        C => 0x43,
        D => 0x44,
        E => 0x45,
        F => 0x46,
        G => 0x47,
        H => 0x48,
        I => 0x49,
        J => 0x4A,
        K => 0x4B,
        L => 0x4C,
        M => 0x4D,
        N => 0x4E,
        O => 0x4F,
        P => 0x50,
        Q => 0x51,
        R => 0x52,
        S => 0x53,
        T => 0x54,
        U => 0x55,
        V => 0x56,
        W => 0x57,
        X => 0x58,
        Y => 0x59,
        Z => 0x5A,
        Kp0 => 0x60,
        Kp1 => 0x61,
        Kp2 => 0x62,
        Kp3 => 0x63,
        Kp4 => 0x64,
        Kp5 => 0x65,
        Kp6 => 0x66,
        Kp7 => 0x67,
        Kp8 => 0x68,
        Kp9 => 0x69,
        F1 => 0x70,
        F2 => 0x71,
        F3 => 0x72,
        F4 => 0x73,
        F5 => 0x74,
        F6 => 0x75,
        F7 => 0x76,
        F8 => 0x77,
        F9 => 0x78,
        F10 => 0x79,
        F11 => 0x7A,
        F12 => 0x7B,
        _ => return None,
    })
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
    vm: &Vm,
    gfx: &mut Gfx,
    mut font: Option<&mut TextRenderer>,
    choices: &[String],
    menu: Option<&Obj>,
    sel: usize,
    (lw, lh): (u32, u32),
    filter: ScaleMode,
    font_override: bool,
) {
    let visible_range = menu_visible_range(
        choices.len(),
        menu.map(|object| object.int(Prop::ROWCOUNT, 0))
            .unwrap_or(0),
        menu.map(|object| object.int(Prop::TOPINDEX, 0))
            .unwrap_or(0),
    );
    let visible_choices = &choices[visible_range.clone()];
    let visible_sel = sel
        .checked_sub(visible_range.start)
        .filter(|index| *index < visible_choices.len())
        .unwrap_or(usize::MAX);
    let (left_inset, right_inset, top_inset, bottom_inset) =
        menu_border_insets(tc, assets, vm, gfx, menu, filter);
    let scrollbar_key = if visible_choices.len() < choices.len() {
        menu.map(|object| object.str(Prop::SCROLLSOURCE))
            .filter(|source| !source.is_empty())
            .and_then(|source| image_for(tc, assets, vm, gfx, &source, filter))
    } else {
        None
    };
    let scrollbar_size = scrollbar_key
        .as_ref()
        .and_then(|key| gfx.images.get(key))
        .map(|(_, width, height)| (*width, *height));
    let scrollbar_cell = scrollbar_size
        .and_then(|(width, height)| (width >= 7 && height > 0).then_some((width / 7, height)));
    let scrollbar_width = scrollbar_cell.map(|(width, _)| width as f32).unwrap_or(0.0);
    let scrollbar_gap = if scrollbar_width > 0.0 { 4.0 } else { 0.0 };
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
    let prefer_pixel = !font_family.is_empty() && gfx.pixel_fonts.get(&font_family).is_some();
    let use_named = !font_family.is_empty() && (!font_override || prefer_pixel);
    if use_named && !gfx.menu_fonts.contains_key(&font_family) {
        let loaded = TextRenderer::load_family(&font_family, &gfx.pixel_fonts, None);
        if loaded.is_none() {
            log::warn!(
                "no font match for menu font family {font_family:?}; using configured fallback"
            );
        }
        gfx.menu_fonts.insert(font_family.clone(), loaded);
    }
    let font_size = if let Some(renderer) = use_named
        .then(|| {
            gfx.menu_fonts
                .get_mut(&font_family)
                .and_then(Option::as_mut)
        })
        .flatten()
    {
        Some(renderer.size_for_cell_height(font_height as f32))
    } else {
        font.as_deref_mut()
            .map(|renderer| renderer.size_for_cell_height(font_height as f32))
    };
    let measured_text_width = font_size.and_then(|font_size| {
        let renderer = if use_named {
            gfx.menu_fonts
                .get_mut(&font_family)
                .and_then(Option::as_mut)
        } else {
            None
        }
        .or(font.as_deref_mut())?;
        choices
            .iter()
            .map(|choice| renderer.measure_text_width(choice, font_size))
            .reduce(f32::max)
    });
    let natural_width = measured_text_width.map(|text_width| {
        (text_width
            + 2.0 * margin_x as f32
            + left_inset
            + right_inset
            + scrollbar_width
            + scrollbar_gap
            + font_height as f32 * 0.5)
            .min((lw as f32 - left_inset - right_inset).max(1.0))
    });
    let (frame, rows) = menu_geometry_with_width(visible_choices, menu, lw, lh, natural_width);
    let (frame, content_rows) = menu_content_rows(
        frame,
        &rows,
        left_inset,
        right_inset + scrollbar_width + scrollbar_gap,
        top_inset,
        bottom_inset,
        font_height as f32,
    );
    let style = vec![
        frame.w as i64,
        frame.h as i64,
        (left_inset * 100.0).round() as i64,
        (right_inset * 100.0).round() as i64,
        (top_inset * 100.0).round() as i64,
        (bottom_inset * 100.0).round() as i64,
        (font_height * 100.0).round() as i64,
        margin_x as i64,
        halign as i64,
        normal,
        hover,
        visible_sel as i64,
    ];
    if gfx.menu.as_ref().is_none_or(|cache| {
        cache.name != menu.map_or("", |object| object.name.as_str())
            || cache.choices != visible_choices
            || cache.font_family != font_family
            || cache.style != style
            || cache.textures.len() != visible_choices.len()
    }) {
        let selected_font = if use_named {
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
                visible_choices,
                &content_rows,
                margin_x,
                font_height,
                normal,
                hover,
                visible_sel,
                halign,
                filter,
            )
        } else if let Some(f) = font {
            render_menu_textures(
                f,
                tc,
                visible_choices,
                &content_rows,
                margin_x,
                font_height,
                normal,
                hover,
                visible_sel,
                halign,
                filter,
            )
        } else {
            Vec::new()
        };
        gfx.menu = Some(MenuCache {
            name: menu.map_or_else(String::new, |object| object.name.clone()),
            choices: visible_choices.to_vec(),
            font_family: font_family.clone(),
            style,
            textures,
            rows: content_rows.clone(),
        });
    } else if let Some(cache) = &mut gfx.menu {
        cache.rows.clone_from(&content_rows);
    }
    canvas.set_blend_mode(BlendMode::Blend);
    if fill >= 0 {
        let c = tcolor(fill, alpha);
        canvas.set_draw_color(Color::RGBA(c[0], c[1], c[2], c[3]));
        let _ = canvas.fill_rect(frame);
    }
    draw_menu_border(canvas, tc, assets, vm, gfx, menu, frame, alpha, filter);
    if hover_bg >= 0
        && let Some(row) = content_rows.get(visible_sel)
    {
        let c = tcolor(hover_bg, alpha);
        canvas.set_draw_color(Color::RGBA(c[0], c[1], c[2], c[3]));
        let _ = canvas.fill_rect(*row);
    }
    if let Some(cache) = &mut gfx.menu {
        for (row, texture) in content_rows.iter().zip(&mut cache.textures) {
            if let Some(texture) = texture {
                texture.set_alpha_mod(alpha);
                let query = texture.query();
                let _ = canvas.copy(
                    texture,
                    None,
                    FRect::new(
                        row.x + margin_x as f32,
                        row.y,
                        (row.w - margin_x as f32 * 2.0)
                            .max(0.0)
                            .min(query.width as f32),
                        row.h,
                    ),
                );
            }
        }
    }
    if let (Some(key), Some((cell_width, cell_height))) = (scrollbar_key, scrollbar_cell)
        && let Some(geometry) = menu_scrollbar_geometry(
            frame,
            content_rows
                .first()
                .map(|row| row.y)
                .unwrap_or(frame.y + top_inset),
            content_rows
                .last()
                .map(|row| row.y + row.h)
                .unwrap_or(frame.y + frame.h - bottom_inset),
            right_inset,
            (cell_width, cell_height),
            choices.len(),
            menu.map(|object| object.int(Prop::ROWCOUNT, 0))
                .unwrap_or(0),
            menu.map(|object| object.int(Prop::TOPINDEX, 0))
                .unwrap_or(0),
        )
        && let Some((texture, _, _)) = gfx.images.get_mut(&key)
    {
        texture.set_alpha_mod(alpha);
        let cell_source = |index: u32| {
            FRect::new(
                (index * cell_width) as f32,
                0.0,
                cell_width as f32,
                cell_height as f32,
            )
        };
        let copy_part =
            |canvas: &mut Canvas<Window>, texture: &mut Texture, index: u32, destination: FRect| {
                let _ = canvas.copy(texture, Some(cell_source(index)), destination);
            };
        let button = geometry.button_size;
        let track_top = geometry.top + button;
        let track_bottom = track_top + geometry.track_height;
        let track = FRect::new(
            geometry.x,
            track_top,
            cell_width as f32,
            geometry.track_height,
        );
        copy_part(canvas, texture, 4, track);
        copy_part(
            canvas,
            texture,
            0,
            FRect::new(geometry.x, geometry.top, button, button),
        );
        copy_part(
            canvas,
            texture,
            2,
            FRect::new(geometry.x, track_bottom, button, button),
        );
        copy_part(
            canvas,
            texture,
            6,
            FRect::new(
                geometry.x,
                track_top + geometry.thumb_offset,
                cell_width as f32,
                geometry.thumb_height,
            ),
        );
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
    vm: &Vm,
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
                .then(|| image_for(tc, assets, vm, gfx, &src, filter))
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
    vm: &Vm,
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
            image_for(tc, assets, vm, gfx, &src, filter)
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
    vm: &Vm,
    gfx: &mut Gfx,
    src: &str,
    filter: ScaleMode,
) -> Option<String> {
    // mem:/save: (SaveScreen / CopyFile) — fuera del archivo del juego.
    if let Some(img) = vm.vfs_image(src) {
        let name = src.to_string();
        if gfx.images.contains_key(&name) {
            touch_image_texture_source(gfx, &name);
            return Some(name);
        }
        return upload_decoded_image(tc, gfx, &name, &img, filter);
    }
    let name = assets.data().resolve(src, IMAGE_EXTS)?;
    if gfx.images.contains_key(&name) {
        touch_image_texture_source(gfx, &name);
        return Some(name);
    }

    let Some(Asset::Image(img)) = assets.try_get(&name) else {
        return None;
    };
    upload_decoded_image(tc, gfx, &name, &img, filter)
}

fn upload_decoded_image(
    tc: &TextureCreator<WindowContext>,
    gfx: &mut Gfx,
    name: &str,
    img: &crate::gal::Image,
    filter: ScaleMode,
) -> Option<String> {
    let key = name.to_string();
    let pixels_per_frame = (img.w as usize)
        .saturating_mul(img.h as usize)
        .saturating_mul(4);
    let estimated_bytes = pixels_per_frame.saturating_mul(img.frames.len().saturating_add(1));
    make_image_texture_cache_room(gfx, estimated_bytes, &key);
    gfx.animation_delays.insert(key.clone(), img.delays.clone());
    if log::log_enabled!(log::Level::Debug) {
        let n = (img.w * img.h).max(1) as u64;
        let asum: u64 = img
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| p[3] as u64)
            .sum();
        let rgbsum: u64 = img
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| p[0] as u64 + p[1] as u64 + p[2] as u64)
            .sum();
        log::debug!(
            "texture {key}: {}x{} avg alpha {} avg rgb {} first px {:?}",
            img.w,
            img.h,
            asum / n,
            rgbsum / (3 * n),
            img.rgba.get(0..4)
        );
    }
    let t = upload(tc, img.w, img.h, &img.rgba, filter)?;
    let mut uploaded_bytes = pixels_per_frame;
    gfx.images.insert(key.clone(), (t, img.w, img.h));
    for (i, frame) in img.frames.iter().enumerate() {
        match upload_with_error(tc, img.w, img.h, frame, filter) {
            Ok(texture) => {
                gfx.images
                    .insert(format!("{key}#{}", i + 1), (texture, img.w, img.h));
                uploaded_bytes = uploaded_bytes.saturating_add(pixels_per_frame);
            }
            Err(error) => {
                log::warn!("cannot cache GAL texture {key} frame {}: {error}", i + 1);
            }
        }
    }
    gfx.image_texture_cache_clock = gfx.image_texture_cache_clock.saturating_add(1);
    let last_used = gfx.image_texture_cache_clock;
    gfx.image_texture_cache.insert(
        key.clone(),
        ImageTextureCacheEntry {
            bytes: uploaded_bytes,
            last_used,
        },
    );
    gfx.image_texture_used_this_frame.insert(key.clone());
    if uploaded_bytes > IMAGE_TEXTURE_CACHE_LIMIT_BYTES {
        log::warn!(
            "image texture set {key} uses {} MiB, exceeding the {} MiB GPU cache budget",
            uploaded_bytes / (1024 * 1024),
            IMAGE_TEXTURE_CACHE_LIMIT_BYTES / (1024 * 1024)
        );
    }
    if !img.frames.is_empty() {
        log::debug!(
            "texture {key}: {} animation frames, delays {:?}",
            img.frames.len() + 1,
            img.delays
        );
    }
    Some(key)
}

/// Prioridades por encima de la imagen de fondo (y lo pegado a ella) no frenan el present.
/// 32 cubre fondo y clip (p. ej. 95 y 100) y deja fuera overlays y el mapa (500+).
const PICTURE_PRESENT_BAND: i64 = 32;

fn picture_present_floor(priorities: impl IntoIterator<Item = i64>) -> Option<i64> {
    priorities.into_iter().min()
}

fn waits_for_picture(priority: i64, floor: i64) -> bool {
    priority <= floor.saturating_add(PICTURE_PRESENT_BAND)
}

fn scene_images_ready(assets: &Assets, vm: &Vm, ordered_objects: &[Obj]) -> bool {
    let image_ready = |name: &str| {
        assets
            .data()
            .resolve(name, IMAGE_EXTS)
            .is_none_or(|resolved| assets.try_get(&resolved).is_some())
    };
    let picture_floor = picture_present_floor(ordered_objects.iter().filter_map(|object| {
        if !vm.scene.effective_visible(object)
            || !matches!(
                object.kind,
                Kind::Image | Kind::Button | Kind::Tile | Kind::Slider
            )
        {
            return None;
        }
        let sourced = !object.str(Prop::SOURCE).is_empty()
            || !object.str(Prop::THUMBSOURCE).is_empty()
            || !object.str(Prop::BUTTONSOURCE).is_empty()
            || !object.str(Prop::HOVERSOURCE).is_empty()
            || !object.str(Prop::PRESSEDSOURCE).is_empty();
        sourced.then_some(object.int(Prop::PRIORITY, 0))
    }));
    let in_picture = |priority: i64| match picture_floor {
        Some(floor) => waits_for_picture(priority, floor),
        None => true,
    };
    for object in ordered_objects {
        if !vm.scene.effective_visible(object) {
            continue;
        }
        // Con una imagen de escena, el texto inline y el mapa no retienen el cuadro anterior.
        if picture_floor.is_none() {
            for source in object.text.iter().filter_map(|item| match item {
                TextItem::Image { source, .. } => Some(source.as_str()),
                _ => None,
            }) {
                if !image_ready(source) {
                    return false;
                }
            }
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
        ) && in_picture(object.int(Prop::PRIORITY, 0))
        {
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
        if picture_floor.is_none()
            && object.kind == Kind::Menu
            && let Some(menu) = vm.prevmenus.get(&object.name)
        {
            for (index, button) in menu.buttons.iter().enumerate() {
                if !vm.prevmenu_item_visible(&object.name, index, &button.name) {
                    continue;
                }
                let source = vm.prevmenu_button_image(
                    &object.name,
                    index,
                    button,
                    menu.hover == Some(index),
                );
                if !source.is_empty() && !image_ready(source) {
                    return false;
                }
            }
        }
    }
    if picture_floor.is_none() {
        for movie in vm.cinema_playbacks() {
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
    font: Option<&mut TextRenderer>,
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
        let loaded = TextRenderer::load_family(&family, &gfx.pixel_fonts, None);
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
            let selected = selected.or(font);
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
    assets: &Assets,
    gfx: &mut Gfx,
    font: Option<&mut TextRenderer>,
    o: &Obj,
    alpha: u8,
    (x, y): (f64, f64),
    (lw, lh): (u32, u32),
    cfg: &Config,
    filter: ScaleMode,
    debug_layout: bool,
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
    let family = o.str(Prop::FONTNAME);
    if !family.is_empty() && !gfx.menu_fonts.contains_key(&family) {
        let loaded = TextRenderer::load_family(&family, &gfx.pixel_fonts, None);
        if loaded.is_none() {
            log::warn!("no font match for message font {family:?}; using configured fallback");
        }
        gfx.menu_fonts.insert(family.clone(), loaded);
    }
    let font_height = o
        .get(Prop::FONTHEIGHT)
        .map(|v| v.as_f64() as f32)
        .filter(|v| *v > 4.0)
        .unwrap_or(cfg.text.size);
    let mut color = o
        .get(Prop::FONTCOLOR)
        .map(|v| tcolor(v.as_int(), 255))
        .unwrap_or(cfg.text.color);
    let shadow = o
        .get(Prop::FONTSHADOW)
        .map(Value::truthy)
        .unwrap_or(cfg.text.shadow);
    let mut shadow_color = o
        .get(Prop::FONTSHADOWCOLOR)
        .map(|v| tcolor(v.as_int(), 255))
        .unwrap_or([0, 0, 0, color[3] / 2 + 64]);
    let border = o.get(Prop::FONTBORDER).is_some_and(Value::truthy);
    let mut border_color = o
        .get(Prop::FONTBORDERCOLOR)
        .map(|v| tcolor(v.as_int(), 255))
        .unwrap_or([0, 0, 0, 255]);
    // --debug-layout: fondo negro; FONTCOLOR=0 del novel system queda invisible.
    if debug_layout {
        let lum = (u16::from(color[0]) + u16::from(color[1]) + u16::from(color[2])) / 3;
        if lum < 40 {
            color = [180, 255, 180, 255];
            shadow_color = [0, 0, 0, 180];
            border_color = [40, 80, 40, 255];
        }
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (
        family.as_str(),
        font_height.to_bits(),
        color,
        shadow,
        shadow_color,
        border,
        border_color,
    )
        .hash(&mut hasher);
    let extra = hasher.finish() as usize;
    let stale = gfx
        .texts
        .get(&o.name)
        .is_none_or(|(rev, ex, _)| *rev != o.text_rev || *ex != extra);
    if stale {
        let images = items
            .iter()
            .filter_map(|item| match item {
                TextItem::Image { source, .. } => match assets.try_get(source) {
                    Some(Asset::Image(image)) => Some((source.clone(), image)),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        let selected = if !family.is_empty() {
            gfx.menu_fonts.get_mut(&family).and_then(Option::as_mut)
        } else {
            None
        };
        let renderer = selected.unwrap_or(font);
        // FONTHEIGHT = "フォントの高さ" (docs/livenovel/text.html); unidades exactas no confirmadas.
        let px = renderer.size_for_cell_height(font_height);
        let img = renderer.render_aligned_with_images(
            &items,
            w,
            h,
            px,
            color,
            shadow,
            shadow_color,
            border,
            border_color,
            0,
            &images,
        );
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
    ordered_objects: &[Obj],
    gfx: &mut Gfx,
    mut font: Option<&mut TextRenderer>,
    cfg: &Config,
    filter: ScaleMode,
    (lw, lh): (u32, u32),
    debug_layout: bool,
    interpolation_mode: InterpolationMode,
) -> Vec<Drawn> {
    let order = ordered_objects;
    gfx.image_texture_used_this_frame.clear();
    let interpolation_areas = if interpolation_mode == InterpolationMode::Off {
        HashMap::new()
    } else {
        interpolation_group_areas(&vm.scene, order, gfx, assets)
    };
    let mut sizes: Vec<(String, u32, u32)> = vec![];
    let mut active_menu_animations = HashSet::new();
    let mut active_crossfade_pairs = HashSet::new();
    let mut active_videos = HashSet::new();
    let mut drawn: Vec<Drawn> = Vec::with_capacity(order.len());
    for (z, o) in order.iter().enumerate() {
        let (x0, y0) = vm.scene.abs_pos(o);
        // NAME vacío en QUAKE = todos los objetos (script.html).
        let (qx, qy) = vm.quake_draw_offset(&o.name);
        let (x, y) = (x0 + qx, y0 + qy);
        let mut alpha = o.int(Prop::ALPHA, 255).clamp(0, 255) as u8;
        if let Some(factor) = vm.wipe_alpha_factor(&o.name) {
            alpha = (f64::from(alpha) * factor).round().clamp(0.0, 255.0) as u8;
        }
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
        let active_edit =
            o.kind == Kind::Edit && vm.active_edit_name().is_some_and(|name| name == o.name);
        if !vm.scene.effective_visible(o) && !active_edit {
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
                    if !vm.prevmenu_item_visible(&o.name, i, &button.name) {
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
                    && let Some(key) = image_for(tc, assets, vm, gfx, &src, filter)
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
            if let Some(movie) = vm.cinema(&o.name) {
                if w <= 0 {
                    w = movie.movie.width as i64;
                }
                if h <= 0 {
                    h = movie.movie.height as i64;
                }
            }
            if w > 0 && h > 0 {
                let (dx, dy, dw, dh) = if matches!(o.kind, Kind::Image | Kind::Button) {
                    let (vx, vy, vw, vh) = vm.scene.placed_rect(o, w as f64, h as f64);
                    (vx as f32, vy as f32, vw as f32, vh as f32)
                } else {
                    (x as f32, y as f32, w as f32, h as f32)
                };
                debug_outline(
                    canvas,
                    tc,
                    gfx,
                    font.as_deref_mut(),
                    &o.name,
                    &format!("{} [{}x{}]", o.name, dw as i64, dh as i64),
                    FRect::new(dx, dy, dw, dh),
                    color,
                    filter,
                );
                if o.kind == Kind::Message {
                    let _ = draw_message_text(
                        canvas,
                        tc,
                        assets,
                        gfx,
                        font.as_deref_mut(),
                        o,
                        alpha,
                        (x, y),
                        (lw, lh),
                        cfg,
                        filter,
                        true,
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
        if let Some((sprites, texts, movie_w, movie_h)) = vm.cinema(&o.name).map(|playback| {
            (
                playback.sprites_at(vm.now),
                playback.texts_at(vm.now),
                playback.movie.width,
                playback.movie.height,
            )
        }) {
            let mut rendered = 0;
            for sprite in sprites {
                let Some(key) = image_for(tc, assets, vm, gfx, &sprite.media, filter) else {
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
            for (index, text) in texts.into_iter().enumerate() {
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                (
                    text.value.as_str(),
                    text.font_height,
                    text.font_style,
                    text.color,
                    movie_w,
                    movie_h,
                )
                    .hash(&mut hasher);
                let style = hasher.finish() as usize;
                let key = format!("{}::lcm-text-{index}", o.name);
                let stale = gfx
                    .texts
                    .get(&key)
                    .is_none_or(|(revision, old_style, _)| *revision != 0 || *old_style != style);
                if stale {
                    let width = (movie_w as f32 - text.x.max(0.0)).ceil().max(1.0) as u32;
                    let height = (movie_h as f32 - text.y.max(0.0))
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
                let Some(key) = image_for(tc, assets, vm, gfx, &src, filter) else {
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
                let base_w = if ow > 0 { ow as f64 } else { f64::from(iw) };
                let base_h = if oh > 0 { oh as f64 } else { f64::from(ih) };
                // Tile: el zoom escala el mosaico, no la superficie (graphic.html).
                let (x, y, dw, dh) = if o.kind == Kind::Tile {
                    (x, y, base_w as f32, base_h as f32)
                } else {
                    let (vx, vy, vw, vh) = vm.scene.placed_rect(o, base_w, base_h);
                    (vx, vy, vw as f32, vh as f32)
                };
                drawn[di].rect = (x as f32, y as f32, dw, dh);
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
                        .entry(crossfade_key.clone())
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
                if crossfade_active {
                    active_crossfade_pairs.insert(crossfade_key.clone());
                }
                let (brightness_rgb, brightness_white) =
                    crate::scene::brightness_draw(o.int(Prop::BRIGHTNESS, 0));
                if let Some((texture, _, _)) = gfx.images.get_mut(&current_key) {
                    texture.set_color_mod(brightness_rgb, brightness_rgb, brightness_rgb);
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
                    texture.set_color_mod(255, 255, 255);
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
                        texture.set_color_mod(brightness_rgb, brightness_rgb, brightness_rgb);
                        texture.set_alpha_mod((f64::from(alpha) * progress).round() as u8);
                        let _ = canvas.copy(texture, None, FRect::new(x as f32, y as f32, dw, dh));
                        texture.set_color_mod(255, 255, 255);
                    }
                }
                if brightness_white > 0 {
                    let white_alpha = (u16::from(alpha) * u16::from(brightness_white) / 255) as u8;
                    canvas.set_blend_mode(BlendMode::Blend);
                    canvas.set_draw_color(Color::RGBA(255, 255, 255, white_alpha));
                    let _ = canvas.fill_rect(FRect::new(x as f32, y as f32, dw, dh));
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
                // Buttons with TEXT render a label centred over the image
                if o.kind == Kind::Button {
                    let text = o.str(Prop::TEXT);
                    if !text.is_empty()
                        && let Some(font) = font.as_deref_mut()
                    {
                        let px =
                            o.get(Prop::FONTHEIGHT)
                                .map(Value::as_f64)
                                .filter(|v| *v > 0.0)
                                .unwrap_or(cfg.text.size as f64) as f32;
                        let color = o
                            .get(Prop::FONTCOLOR)
                            .map(|v| tcolor(v.as_int(), 255))
                            .unwrap_or(cfg.text.color);
                        let items: Vec<TextItem> = text
                            .chars()
                            .map(|c| TextItem::Char(c.to_string()))
                            .collect();
                        let text_width = font.measure_text_width(&text, px).ceil().max(1.0);
                        let text_height = (px * 1.5).ceil().max(1.0);
                        let img = font.render_aligned(
                            &items,
                            text_width.min(dw).max(1.0) as u32,
                            text_height.min(dh).max(1.0) as u32,
                            px,
                            color,
                            false,
                            1,
                        );
                        if img.w > 0
                            && img.h > 0
                            && let Some(texture) = upload(tc, img.w, img.h, &img.rgba, filter)
                        {
                            let tx = x as f32 + (dw - img.w as f32) / 2.0;
                            let ty = y as f32 + (dh - img.h as f32) / 2.0;
                            let _ = canvas.copy(
                                &texture,
                                None,
                                FRect::new(tx, ty, img.w as f32, img.h as f32),
                            );
                        }
                    }
                }
            }
            Kind::Slider => {
                let horizontal = o.int(Prop::ORIENTATION, 0) == 0;
                let bar_src = o.str(Prop::SOURCE);
                let thumb_src = o.str(Prop::THUMBSOURCE);
                let button_src = o.str(Prop::BUTTONSOURCE);
                let bar = if bar_src.is_empty() {
                    None
                } else {
                    image_for(tc, assets, vm, gfx, &bar_src, filter)
                };
                let thumb = if thumb_src.is_empty() {
                    None
                } else {
                    image_for(tc, assets, vm, gfx, &thumb_src, filter)
                };
                let buttons = if button_src.is_empty() {
                    None
                } else {
                    image_for(tc, assets, vm, gfx, &button_src, filter)
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
                // explícito (no clNone/-1) y tamaño.
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
                    // Deshabilitado se dibuja igual (docs/livenovel/selimg.html).
                    // El clic lo filtra prevmenu_hit.
                    if !vm.prevmenu_item_visible(&o.name, i, &b.name) {
                        continue;
                    }
                    let src = vm.prevmenu_button_image(&o.name, i, b, pm.hover == Some(i));
                    let Some(key) = image_for(tc, assets, vm, gfx, src, filter) else {
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
                    assets,
                    gfx,
                    font.as_deref_mut(),
                    o,
                    alpha,
                    (x, y),
                    (lw, lh),
                    cfg,
                    filter,
                    false,
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
                active_videos.insert(o.name.clone());
                let Some(video_frame) = vm.video_frame_pair(&o.name, vm.now) else {
                    st!() = "video loading";
                    continue;
                };
                let generation = video_frame.generation;
                let sequence = video_frame.sequence;
                let pixels = video_frame.current;
                let video_width = video_frame.width;
                let video_height = video_frame.height;
                let (display_pixels, display_sequence) = if video_frame.progress < 1.0 {
                    video_frame
                        .previous
                        .map(|(pixels, _, _, sequence)| (pixels, sequence))
                        .unwrap_or((pixels, sequence))
                } else {
                    (pixels, sequence)
                };
                let recreate =
                    gfx.video_textures
                        .get(&o.name)
                        .is_none_or(|(_, old_generation, _, w, h)| {
                            *old_generation != generation || *w != video_width || *h != video_height
                        });
                if recreate {
                    let Some(texture) =
                        upload(tc, video_width, video_height, display_pixels, filter)
                    else {
                        log::error!("cannot create video texture for {:?}", o.name);
                        st!() = "video texture unavailable";
                        continue;
                    };
                    gfx.video_textures.insert(
                        o.name.clone(),
                        (
                            texture,
                            generation,
                            display_sequence,
                            video_width,
                            video_height,
                        ),
                    );
                }
                let Some((texture, _, current_sequence, _, _)) =
                    gfx.video_textures.get_mut(&o.name)
                else {
                    st!() = "video texture unavailable";
                    continue;
                };
                if *current_sequence != display_sequence {
                    if let Err(e) = texture.update(None, display_pixels, (video_width * 4) as usize)
                    {
                        log::error!("cannot update video texture for {:?}: {e}", o.name);
                        st!() = "video texture update failed";
                        continue;
                    }
                    *current_sequence = display_sequence;
                }
                // Misma regla que Image: base = fuente (o WIDTH de prop), zoom/OFFSET
                // vía placed_rect (`media.html` / `graphic.html`). Sin fallback a pantalla.
                let base_w = if ow > 0 {
                    ow as f64
                } else {
                    f64::from(video_width)
                };
                let base_h = if oh > 0 {
                    oh as f64
                } else {
                    f64::from(video_height)
                };
                let (x, y, dw, dh) = {
                    let (vx, vy, vw, vh) = vm.scene.placed_rect(o, base_w, base_h);
                    (vx, vy, vw as f32, vh as f32)
                };
                drawn[di].rect = (x as f32, y as f32, dw, dh);
                if let Some((texture, _, _, _, _)) = gfx.video_textures.get_mut(&o.name) {
                    texture.set_alpha_mod(alpha);
                    let _ = canvas.copy(texture, None, FRect::new(x as f32, y as f32, dw, dh));
                }
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
        vm.scene.contains(k)
            || vm
                .cinema_named()
                .any(|(name, _)| k.starts_with(&format!("{name}::lcm-text-")))
    });
    gfx.video_textures
        .retain(|name, _| vm.scene.get(name).is_some_and(|o| o.kind == Kind::Movie));
    gfx.video_previous_textures
        .retain(|name, _| active_videos.contains(name));
    gfx.crossfade_safety
        .retain(|key, _| active_crossfade_pairs.contains(key));
    gfx.menu_animation_starts
        .retain(|key, _| active_menu_animations.contains(key));
    // Flip WP_WHITE / WP_BLACK / WP_FLASH (**interpretación** I7).
    if let Some((r, g, b, a)) = vm.wipe_flash_rgba()
        && a > 0
    {
        canvas.set_blend_mode(BlendMode::Blend);
        canvas.set_draw_color(Color::RGBA(r, g, b, a));
        let _ = canvas.fill_rect(FRect::new(0.0, 0.0, lw as f32, lh as f32));
    }
    drawn
}

#[cfg(test)]
mod tests {
    use super::{
        FRect, ImageTextureCacheEntry, add_rect_to_bounds, default_language_selection,
        edit_key_code, estimated_texture_bytes, gal_animation_frame, image_texture_cache_evictions,
        inferred_mask_group, interpolation_group_key, is_toolbar_visible, menu_content_rows,
        menu_geometry, menu_geometry_with_width, menu_hit, menu_hit_rows, menu_scrollbar_geometry,
        picture_present_floor, should_interpolate_gal, toolbar_contains, toolbar_hit,
        toolbar_in_reveal_strip, update_toolbar_open, waits_for_picture,
    };
    use crate::interpolation::InterpolationMode;
    use crate::lsb::{Prop, expr::Value};
    use crate::scene::{Kind, Scene};
    use std::collections::{HashMap, HashSet};

    #[test]
    fn scene_cut_waits_for_the_picture_not_the_map() {
        let floor = picture_present_floor([95, 100, 505, 1102]).unwrap();
        assert_eq!(floor, 95);
        assert!(waits_for_picture(95, floor));
        assert!(waits_for_picture(100, floor));
        assert!(!waits_for_picture(505, floor));
        assert!(!waits_for_picture(1102, floor));
    }

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
        // Los puntos se toman del centro de cada rect para seguir al layout real.
        let mid = |r: FRect| (r.x + r.w / 2.0, r.y + r.h / 2.0);
        let (ox, oy) = mid(super::toolbar_button_rects(800)
            .into_iter()
            .find(|(a, _)| matches!(a, super::ToolbarAction::Options))
            .expect("botón Options")
            .1);
        assert!(matches!(
            toolbar_hit(800, ox, oy, false),
            Some(super::ToolbarAction::Options)
        ));
        let (ix, iy) = mid(super::interpolation_option_rects(800)[1].1);
        assert!(matches!(
            toolbar_hit(800, ix, iy, true),
            Some(super::ToolbarAction::Interpolation(
                InterpolationMode::Crossfade
            ))
        ));
        let (fx, fy) = mid(super::fps_meter_option_rect(800));
        assert!(matches!(
            toolbar_hit(800, fx, fy, true),
            Some(super::ToolbarAction::ToggleFpsMeter)
        ));
        let (ex, ey) = mid(super::export_texts_option_rect(800));
        assert!(matches!(
            toolbar_hit(800, ex, ey, true),
            Some(super::ToolbarAction::ExportTexts)
        ));
        let (sx, sy) = mid(super::scale_filter_option_rects(800)[0].1);
        assert!(matches!(
            toolbar_hit(800, sx, sy, true),
            Some(super::ToolbarAction::ScaleFilter(_))
        ));
        assert!(
            toolbar_hit(800, fx, fy, false).is_none(),
            "opciones cerradas: sin acciones del panel"
        );
        assert!(toolbar_in_reveal_strip(0.0));
        assert!(toolbar_in_reveal_strip(5.0));
        assert!(!toolbar_in_reveal_strip(6.0));
        assert!(!is_toolbar_visible(false, false));
        assert!(is_toolbar_visible(true, false));
        assert!(is_toolbar_visible(false, true));
        assert!(toolbar_contains(800, 400.0, 20.0, false));
        assert!(toolbar_contains(800, 400.0, 40.0, false));
        assert!(toolbar_contains(
            800,
            400.0,
            super::TOOLBAR_BAR_HEIGHT - 1.0,
            false
        ));
        assert!(!toolbar_contains(
            800,
            400.0,
            super::TOOLBAR_BAR_HEIGHT + 1.0,
            false
        ));
        assert!(toolbar_contains(800, 700.0, 100.0, true));
        let mut toolbar_open = false;
        let mut options_open = false;
        update_toolbar_open(&mut toolbar_open, &mut options_open, 800, (400.0, 3.0));
        assert!(toolbar_open);
        update_toolbar_open(&mut toolbar_open, &mut options_open, 800, (400.0, 20.0));
        assert!(toolbar_open);
        update_toolbar_open(&mut toolbar_open, &mut options_open, 800, (400.0, 80.0));
        assert!(!toolbar_open);
        assert!(!super::point_in_logical_render((-1.0, 300.0), (800, 600)));
        assert!(!super::point_in_logical_render((400.0, 600.0), (800, 600)));
        assert!(super::point_in_logical_render((799.0, 599.0), (800, 600)));
    }

    #[test]
    fn language_picker_defaults_to_first_translation_when_available() {
        assert_eq!(default_language_selection(0), 0);
        assert_eq!(default_language_selection(1), 1);
        assert_eq!(default_language_selection(3), 1);
    }

    #[test]
    fn image_texture_cache_evicts_lru_sources_but_protects_used_sources() {
        let cache = HashMap::from([
            (
                "old".to_string(),
                ImageTextureCacheEntry {
                    bytes: 200 * 1024 * 1024,
                    last_used: 1,
                },
            ),
            (
                "recent".to_string(),
                ImageTextureCacheEntry {
                    bytes: 20 * 1024 * 1024,
                    last_used: 2,
                },
            ),
            (
                "current".to_string(),
                ImageTextureCacheEntry {
                    bytes: 20 * 1024 * 1024,
                    last_used: 3,
                },
            ),
        ]);
        let used_this_frame = HashSet::from(["recent".to_string()]);

        assert_eq!(
            image_texture_cache_evictions(&cache, 100 * 1024 * 1024, "current", &used_this_frame),
            vec!["old"]
        );
    }

    #[test]
    fn texture_memory_estimate_uses_rgba_bytes_and_saturates() {
        assert_eq!(estimated_texture_bytes(16, 8), 16 * 8 * 4);
        assert_eq!(estimated_texture_bytes(u32::MAX, u32::MAX), usize::MAX);
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
    fn native_menu_hit_testing_maps_visible_rows_to_full_choice_indices() {
        let choices: Vec<_> = (0..30).map(|index| format!("slot {index}")).collect();
        let mut scene = Scene::default();
        scene.create(
            "menu",
            Kind::Menu,
            HashMap::from([
                (Prop::LEFT, Value::Int(20)),
                (Prop::TOP, Value::Int(30)),
                (Prop::WIDTH, Value::Int(200)),
                (Prop::FONTHEIGHT, Value::Int(20)),
                (Prop::ROWCOUNT, Value::Int(10)),
                (Prop::TOPINDEX, Value::Int(12)),
            ]),
        );
        let menu = scene.get("menu");

        assert_eq!(menu_hit(&choices, menu, 800, 600, 40.0, 31.0), Some(12));
        assert_eq!(menu_hit(&choices, menu, 800, 600, 40.0, 230.0), None);
    }

    #[test]
    fn native_menu_uses_measured_width_unless_script_sets_width() {
        let choices = vec!["short".to_string(), "a much longer option".to_string()];
        let mut scene = Scene::default();
        scene.create(
            "menu",
            Kind::Menu,
            HashMap::from([(Prop::TOP, Value::Int(30))]),
        );

        let (frame, _) =
            menu_geometry_with_width(&choices, scene.get("menu"), 800, 600, Some(240.0));
        assert_eq!(frame.w, 240.0);
        assert_eq!(frame.x, 280.0);

        scene.set("menu", Prop::WIDTH, Value::Int(0));
        let (frame, _) =
            menu_geometry_with_width(&choices, scene.get("menu"), 800, 600, Some(240.0));
        assert_eq!(frame.w, 240.0);
        assert_eq!(frame.x, 280.0);

        scene.set("menu", Prop::ROWCOUNT, Value::Int(10));
        scene.set("menu", Prop::LEFT, Value::Int(400));
        let (frame, _) =
            menu_geometry_with_width(&choices, scene.get("menu"), 800, 600, Some(240.0));
        assert_eq!(frame.w, 240.0);
        assert_eq!(frame.x, 400.0);

        scene.set("menu", Prop::WIDTH, Value::Int(0));
        let (frame, _) =
            menu_geometry_with_width(&choices, scene.get("menu"), 800, 600, Some(240.0));
        assert_eq!(frame.w, 240.0);
        assert_eq!(frame.x, 400.0);

        scene.set("menu", Prop::WIDTH, Value::Int(200));
        let (frame, _) =
            menu_geometry_with_width(&choices, scene.get("menu"), 800, 600, Some(240.0));
        assert_eq!(frame.w, 200.0);
        assert_eq!(frame.x, 400.0);
    }

    #[test]
    fn menu_scrollbar_is_vertical_on_the_right_and_tracks_top_index() {
        let frame = FRect::new(100.0, 100.0, 480.0, 300.0);
        let top = menu_scrollbar_geometry(frame, 120.0, 380.0, 8.0, (20, 20), 30, 10, 0)
            .expect("scrollbar should be visible when choices overflow");
        assert_eq!(top.x, 552.0);
        assert_eq!(top.top, 120.0);
        assert_eq!(top.button_size, 20.0);
        assert_eq!(top.track_height, 220.0);
        assert_eq!(top.thumb_offset, 0.0);
        assert!((top.thumb_height - 220.0 / 3.0).abs() < f32::EPSILON);

        let bottom = menu_scrollbar_geometry(frame, 120.0, 380.0, 8.0, (20, 20), 30, 10, 20)
            .expect("scrollbar should be visible at the last page");
        assert_eq!(bottom.x, 552.0);
        assert!(
            (bottom.thumb_offset - (bottom.track_height - bottom.thumb_height)).abs()
                < f32::EPSILON
        );
        assert!(menu_scrollbar_geometry(frame, 120.0, 380.0, 8.0, (20, 20), 10, 10, 0).is_none());
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

    #[test]
    fn native_menu_border_insets_reflow_all_rows_without_hiding_options() {
        let choices = vec![
            "first".to_string(),
            "second".to_string(),
            "third".to_string(),
        ];
        let mut scene = Scene::default();
        scene.create(
            "menu",
            Kind::Menu,
            HashMap::from([
                (Prop::LEFT, Value::Int(20)),
                (Prop::TOP, Value::Int(30)),
                (Prop::FONTHEIGHT, Value::Int(22)),
                (Prop::LINESPACE, Value::Int(4)),
                (Prop::MARGINY, Value::Int(0)),
            ]),
        );

        let (frame, rows) = menu_geometry(&choices, scene.get("menu"), 800, 600);
        let (frame, content_rows) = menu_content_rows(frame, &rows, 3.0, 4.0, 5.0, 7.0, 22.0);

        assert_eq!(content_rows.len(), choices.len());
        assert_eq!(content_rows[0].y, frame.y + 5.0);
        assert_eq!(content_rows[0].h, 22.0);
        assert_eq!(content_rows[1].h, content_rows[0].h);
        assert_eq!(content_rows[2].h, content_rows[0].h);
        assert_eq!(content_rows[0].x, frame.x + 3.0);
        assert_eq!(content_rows[0].w, frame.w - 7.0);
        assert!(content_rows[0].y >= frame.y);
        assert!(content_rows[2].y + content_rows[2].h <= frame.y + frame.h - 7.0);
        assert_eq!(
            menu_hit_rows(&content_rows, 30.0, content_rows[0].y + 1.0),
            Some(0)
        );
        assert_eq!(
            menu_hit_rows(&content_rows, 30.0, content_rows[1].y + 1.0),
            Some(1)
        );
        assert_eq!(
            menu_hit_rows(&content_rows, 30.0, content_rows[2].y + 1.0),
            Some(2)
        );
    }

    #[test]
    fn native_menu_with_short_suboptions_expands_without_clipping_rows() {
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
                (Prop::MARGINY, Value::Int(0)),
            ]),
        );

        let (frame, rows) = menu_geometry(&choices, scene.get("menu"), 800, 600);
        let (frame, content_rows) = menu_content_rows(frame, &rows, 10.0, 10.0, 30.0, 30.0, 22.0);

        assert_eq!(content_rows.len(), choices.len());
        assert_eq!(frame.h, 104.0);
        assert_eq!(content_rows[0].h, 22.0);
        assert_eq!(content_rows[1].h, 22.0);
        assert!(content_rows[0].y >= frame.y + 30.0);
        assert!(content_rows[1].y + content_rows[1].h <= frame.y + frame.h - 30.0);
        assert_eq!(
            menu_hit_rows(&content_rows, 30.0, content_rows[0].y + 1.0),
            Some(0)
        );
        assert_eq!(
            menu_hit_rows(&content_rows, 30.0, content_rows[1].y + 1.0),
            Some(1)
        );
    }
}
