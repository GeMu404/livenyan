use super::*;

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

pub(super) fn can_crossfade_frames(
    image: &crate::gal::Image,
    from: usize,
    to: usize,
    object_alpha: u8,
) -> bool {
    object_alpha == u8::MAX && frame_is_opaque(image, from) && frame_is_opaque(image, to)
}

pub(super) fn interpolation_group_key(scene: &Scene, object: &Obj) -> String {
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

pub(super) fn add_rect_to_bounds(
    bounds: &mut Option<(f64, f64, f64, f64)>,
    rect: (f64, f64, f64, f64),
) {
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

pub(super) fn inferred_mask_group(
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

pub(super) fn should_interpolate_gal(
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

pub(super) fn image_for(
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

pub(super) fn picture_present_floor(priorities: impl IntoIterator<Item = i64>) -> Option<i64> {
    priorities.into_iter().min()
}

pub(super) fn waits_for_picture(priority: i64, floor: i64) -> bool {
    priority <= floor.saturating_add(PICTURE_PRESENT_BAND)
}

pub(super) fn scene_images_ready(assets: &Assets, vm: &Vm, ordered_objects: &[Obj]) -> bool {
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

pub(super) fn gal_animation_frame(delays: &[u32], elapsed_ms: f64) -> Option<usize> {
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
    text_scale: f32,
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
    let raster = outline_raster(font.is_pixel_font(), text_scale, px, width, height, filter);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (text.as_str(), px.to_bits(), color, raster.w, raster.h).hash(&mut hasher);
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
        let img = font.render(&items, raster.w, raster.h, raster.px, color, false);
        if let Some(texture) = upload(tc, img.w, img.h, &img.rgba, raster.filter) {
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
    text_scale: f32,
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
        text_scale.to_bits(),
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
                let raster = outline_raster(
                    renderer.is_pixel_font(),
                    text_scale,
                    px,
                    bounds.0,
                    bounds.1,
                    filter,
                );
                let image = renderer.render_aligned(
                    &items, raster.w, raster.h, raster.px, font_color, shadow, align,
                );
                (image, raster.filter)
            })
        };
        if let Some((rendered, text_filter)) = rendered
            && let Some(texture) = upload(tc, rendered.w, rendered.h, &rendered.rgba, text_filter)
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
    text_scale: f32,
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
        text_scale.to_bits(),
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
        let raster = outline_raster(renderer.is_pixel_font(), text_scale, px, w, h, filter);
        let img = renderer.render_aligned_with_images(
            &items,
            raster.w,
            raster.h,
            raster.px,
            color,
            shadow,
            shadow_color,
            border,
            border_color,
            0,
            &images,
        );
        if let Some(t) = upload(tc, img.w, img.h, &img.rgba, raster.filter) {
            gfx.texts.insert(o.name.clone(), (o.text_rev, extra, t));
        }
    }
    if let Some((_, _, texture)) = gfx.texts.get_mut(&o.name) {
        texture.set_alpha_mod(alpha);
        let _ = canvas.copy(
            texture,
            None,
            FRect::new(x as f32, y as f32, w as f32, h as f32),
        );
    }
    "drawn"
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_scene(
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
    text_scale: f32,
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
                        text_scale,
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
                    text_scale.to_bits(),
                )
                    .hash(&mut hasher);
                let style = hasher.finish() as usize;
                let key = format!("{}::lcm-text-{index}", o.name);
                let width = (movie_w as f32 - text.x.max(0.0)).ceil().max(1.0) as u32;
                let height = (movie_h as f32 - text.y.max(0.0))
                    .ceil()
                    .max(text.font_height as f32) as u32;
                let stale = gfx
                    .texts
                    .get(&key)
                    .is_none_or(|(revision, old_style, _)| *revision != 0 || *old_style != style);
                if stale {
                    let rendered = font.as_deref_mut().map(|renderer| {
                        let px = renderer.size_for_cell_height(text.font_height as f32);
                        let raster = outline_raster(
                            renderer.is_pixel_font(),
                            text_scale,
                            px,
                            width,
                            height,
                            filter,
                        );
                        let items: Vec<TextItem> = text
                            .value
                            .chars()
                            .map(|c| TextItem::Char(c.to_string()))
                            .collect();
                        let image = renderer.render_aligned(
                            &items,
                            raster.w,
                            raster.h,
                            raster.px,
                            tcolor(text.color as i64, 255),
                            false,
                            0,
                        );
                        (image, raster.filter)
                    });
                    if let Some((rendered, text_filter)) = rendered
                        && let Some(texture) =
                            upload(tc, rendered.w, rendered.h, &rendered.rgba, text_filter)
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
                            width as f32,
                            height as f32,
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
                // Un Button con TEXT dibuja la etiqueta centrada sobre la imagen.
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
                        let box_w = text_width.min(dw).max(1.0) as u32;
                        let box_h = text_height.min(dh).max(1.0) as u32;
                        let raster = outline_raster(
                            font.is_pixel_font(),
                            text_scale,
                            px,
                            box_w,
                            box_h,
                            filter,
                        );
                        let img = font
                            .render_aligned(&items, raster.w, raster.h, raster.px, color, false, 1);
                        if img.w > 0
                            && img.h > 0
                            && let Some(texture) =
                                upload(tc, img.w, img.h, &img.rgba, raster.filter)
                        {
                            let tx = x as f32 + (dw - box_w as f32) / 2.0;
                            let ty = y as f32 + (dh - box_h as f32) / 2.0;
                            let _ = canvas.copy(
                                &texture,
                                None,
                                FRect::new(tx, ty, box_w as f32, box_h as f32),
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
                    text_scale,
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
                    text_scale,
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
                    text_scale,
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
    // Tamaños intrínsecos para hit-testing y GetProp
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
