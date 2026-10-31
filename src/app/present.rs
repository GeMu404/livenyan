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

/// Píxeles del target de escena. Native se queda en el lógico (I2b).
/// Si no, es el rectángulo del letterbox: ahí se estiran las imágenes y
/// el texto de contorno se rasteriza 1:1 (I14).
pub(super) fn scene_target_size(
    external_upscale: bool,
    pixels: (u32, u32),
    logical: (u32, u32),
) -> (u32, u32) {
    let (lw, lh) = (logical.0.max(1), logical.1.max(1));
    if external_upscale {
        return (lw, lh);
    }
    let (_, _, cw, ch) = crate::util::letterbox(pixels, (lw, lh));
    (cw.round().max(1.0) as u32, ch.round().max(1.0) as u32)
}

/// `target_w / lógico`. 1 en Native y cuando la ventana ya cabe en el lógico.
pub(super) fn outline_text_scale(target: (u32, u32), logical: (u32, u32)) -> f32 {
    target.0 as f32 / logical.0.max(1) as f32
}

pub(super) struct OutlineRaster {
    pub px: f32,
    pub w: u32,
    pub h: u32,
    pub filter: ScaleMode,
}

/// Caja de raster. El destino sigue en coordenadas lógicas; el target estira
/// esa caja hasta estos píxeles. Una fuente de píxel no gana detalle (I14).
pub(super) fn outline_raster(
    pixel_font: bool,
    scale: f32,
    px: f32,
    w: u32,
    h: u32,
    filter: ScaleMode,
) -> OutlineRaster {
    let w = w.max(1);
    let h = h.max(1);
    let scaled_w = ((w as f32) * scale).round().max(1.0) as u32;
    let scaled_h = ((h as f32) * scale).round().max(1.0) as u32;
    if pixel_font || !scale.is_finite() || (scaled_w == w && scaled_h == h) {
        return OutlineRaster { px, w, h, filter };
    }
    OutlineRaster {
        px: px * scale,
        w: scaled_w,
        h: scaled_h,
        filter: ScaleMode::Linear,
    }
}

/// Copia el target al letterbox en píxeles de ventana, sin un segundo escalado
/// si el target ya es ese rectángulo. No presenta: la barra se dibuja después.
pub(super) fn blit_scene_frame(
    canvas: &mut Canvas<Window>,
    scene_frame: &mut Texture,
    logical: (u32, u32),
) -> Result<(), String> {
    let (lw, lh) = logical;
    let (pw, ph) = canvas.window().size_in_pixels();
    let (bx, by, cw, ch) = crate::util::letterbox((pw, ph), (lw, lh));
    let content_w = cw.round().max(1.0) as u32;
    let content_h = ch.round().max(1.0) as u32;
    let query = scene_frame.query();
    let (x, y, w, h) = if query.width == content_w && query.height == content_h {
        scene_frame.set_scale_mode(ScaleMode::Nearest);
        (
            (pw as f32 - query.width as f32) / 2.0,
            (ph as f32 - query.height as f32) / 2.0,
            query.width as f32,
            query.height as f32,
        )
    } else {
        (bx as f32, by as f32, cw as f32, ch as f32)
    };
    canvas
        .set_logical_size(0, 0, sdl3::sys::render::SDL_LOGICAL_PRESENTATION_DISABLED)
        .map_err(|e| e.to_string())?;
    canvas.set_draw_color(Color::RGB(0, 0, 0));
    canvas.clear();
    let copied = canvas.copy(scene_frame, None, FRect::new(x, y, w, h));
    canvas
        .set_logical_size(
            lw,
            lh,
            sdl3::sys::render::SDL_LOGICAL_PRESENTATION_LETTERBOX,
        )
        .map_err(|e| e.to_string())?;
    copied.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::{outline_raster, outline_text_scale, scene_target_size};

    #[test]
    fn native_target_stays_logical() {
        assert_eq!(
            scene_target_size(true, (1920, 1080), (800, 600)),
            (800, 600)
        );
        assert_eq!(outline_text_scale((800, 600), (800, 600)), 1.0);
    }

    #[test]
    fn enlarged_window_target_is_the_letterbox() {
        assert_eq!(
            scene_target_size(false, (1600, 1200), (800, 600)),
            (1600, 1200)
        );
        assert_eq!(outline_text_scale((1600, 1200), (800, 600)), 2.0);
        let wide = scene_target_size(false, (2000, 1000), (800, 600));
        assert_eq!(wide.1, 1000);
        assert_eq!(wide.0, (1000.0_f64 * 800.0 / 600.0).round() as u32);
    }

    #[test]
    fn outline_raster_grows_with_the_window_and_pixel_fonts_do_not() {
        let grown = outline_raster(false, 2.0, 16.0, 400, 80, super::ScaleMode::Nearest);
        assert_eq!((grown.px, grown.w, grown.h), (32.0, 800, 160));
        assert_eq!(grown.filter, super::ScaleMode::Linear);
        let pixel = outline_raster(true, 2.0, 16.0, 400, 80, super::ScaleMode::Nearest);
        assert_eq!((pixel.px, pixel.w, pixel.h), (16.0, 400, 80));
        assert_eq!(pixel.filter, super::ScaleMode::Nearest);
        let same = outline_raster(false, 1.0, 16.0, 400, 80, super::ScaleMode::Nearest);
        assert_eq!((same.px, same.w, same.h), (16.0, 400, 80));
    }
}
