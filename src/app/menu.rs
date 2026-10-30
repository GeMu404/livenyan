use super::*;

pub(super) fn menu_geometry(
    choices: &[String],
    menu: Option<&Obj>,
    lw: u32,
    lh: u32,
) -> (FRect, Vec<FRect>) {
    menu_geometry_with_width(choices, menu, lw, lh, None)
}

pub(super) fn menu_geometry_with_width(
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

pub(super) fn menu_content_rows(
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
pub(super) struct MenuScrollbarGeometry {
    pub(super) x: f32,
    pub(super) top: f32,
    pub(super) button_size: f32,
    pub(super) track_height: f32,
    pub(super) thumb_offset: f32,
    pub(super) thumb_height: f32,
}

pub(super) fn menu_scrollbar_geometry(
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

pub(super) fn menu_hit(
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

pub(super) fn menu_hit_rows(rows: &[FRect], x: f32, y: f32) -> Option<usize> {
    rows.iter()
        .position(|r| x >= r.x && y >= r.y && x < r.x + r.w && y < r.y + r.h)
}

pub(super) fn menu_hit_cached(
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

pub(super) fn keep_menu_selection_visible(
    vm: &mut Vm,
    name: &str,
    choice_count: usize,
    selection: usize,
) {
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

pub(super) fn scroll_menu(
    vm: &mut Vm,
    name: &str,
    choice_count: usize,
    amount: i64,
    selection: &mut usize,
) {
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

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_menu(
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
