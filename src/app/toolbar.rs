use super::*;

#[derive(Clone, Copy)]
pub(super) enum ToolbarAction {
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
pub(super) const TOOLBAR_BAR_HEIGHT: f32 = 48.0;

const OPTIONS_PANEL_WIDTH: f32 = 336.0;

const OPTIONS_PANEL_HEIGHT: f32 = 252.0;

const OPTIONS_PANEL_MARGIN: f32 = 14.0;

pub(super) fn toolbar_button_rects(width: u32) -> [(ToolbarAction, FRect); 3] {
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

pub(super) fn fps_meter_option_rect(width: u32) -> FRect {
    let panel = options_panel_rect(width);
    FRect::new(
        panel.x + panel.w - 12.0 - 112.0,
        panel.y + 136.0,
        112.0,
        28.0,
    )
}

pub(super) fn export_texts_option_rect(width: u32) -> FRect {
    let panel = options_panel_rect(width);
    FRect::new(panel.x + 12.0, panel.y + 196.0, panel.w - 24.0, 28.0)
}

pub(super) fn scale_filter_option_rects(width: u32) -> Vec<(ScaleFilter, FRect)> {
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

pub(super) fn image_texture_cache_evictions(
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

pub(super) fn make_image_texture_cache_room(
    gfx: &mut Gfx,
    required_bytes: usize,
    protected_source: &str,
) {
    let evictions = image_texture_cache_evictions(
        &gfx.image_texture_cache,
        required_bytes,
        protected_source,
        &gfx.image_texture_used_this_frame,
    );
    evict_image_texture_sources(gfx, &evictions);
}

pub(super) fn touch_image_texture_source(gfx: &mut Gfx, source: &str) {
    gfx.image_texture_cache_clock = gfx.image_texture_cache_clock.saturating_add(1);
    gfx.image_texture_used_this_frame.insert(source.to_string());
    if let Some(entry) = gfx.image_texture_cache.get_mut(source) {
        entry.last_used = gfx.image_texture_cache_clock;
    }
}

pub(super) fn estimated_texture_bytes(width: u32, height: u32) -> usize {
    (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(4)
}

pub(super) fn texture_memory_stats(gfx: &Gfx, logical_size: (u32, u32)) -> TextureMemoryStats {
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

pub(super) fn toolbar_hit(width: u32, x: f32, y: f32, options_open: bool) -> Option<ToolbarAction> {
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

pub(super) fn toolbar_contains(width: u32, x: f32, y: f32, options_open: bool) -> bool {
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

pub(super) fn toolbar_in_reveal_strip(mouse_y: f32) -> bool {
    (0.0..TOOLBAR_REVEAL_HEIGHT).contains(&mouse_y)
}

/// Visible when latched open (hover chrome) or while the options panel is open.
pub(super) fn is_toolbar_visible(toolbar_open: bool, options_open: bool) -> bool {
    toolbar_open || options_open
}

/// Open on the thin top strip; stay open while over bar/panel; close when leaving that chrome.
pub(super) fn update_toolbar_open(
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

pub(super) fn interpolation_option_rects(width: u32) -> Vec<(InterpolationMode, FRect)> {
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

pub(super) fn load_interpolation_preference(default: InterpolationMode) -> InterpolationMode {
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

pub(super) fn save_interpolation_preference(mode: InterpolationMode) -> Result<(), String> {
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

pub(super) fn language_picker_rects(
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

pub(super) fn default_language_selection(language_count: usize) -> usize {
    usize::from(language_count > 0)
}

pub(super) fn apply_language_selection(
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

pub(super) fn draw_language_picker(
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

pub(super) fn draw_toolbar(
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

pub(super) fn draw_fps_meter(
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
