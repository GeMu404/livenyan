use super::*;

pub(super) fn point_in_logical_render(point: (f32, f32), logical: (u32, u32)) -> bool {
    point.0.is_finite()
        && point.1.is_finite()
        && point.0 >= 0.0
        && point.1 >= 0.0
        && point.0 < logical.0 as f32
        && point.1 < logical.1 as f32
}

pub(super) fn apply_present_scale_hint(canvas: &Canvas<Window>, present_scale: ScaleFilter) {
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

pub(super) fn apply_cached_texture_scale_modes(gfx: &mut Gfx, mode: ScaleMode) {
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

/// Letterbox del framebuffer lógico con nearest (líneas nítidas, sin suavizado).
pub(super) fn present_sharp_frame(
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
