//! Rasterizado de texto: TTF/OTF del sistema (ab_glyph) o LivePixelFont (`.lft`).

use crate::lft::{LivePixelFont, PixelFontCatalog};
use crate::scene::TextItem;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Rutas habituales si fontconfig no está disponible.
const FALLBACKS: &[&str] = &[
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto-cjk/NotoSerifCJK-Regular.ttc",
    "/usr/share/fonts/opentype/noto/NotoSerifCJK-Regular.ttc",
    "/usr/share/fonts/OTF/ipag.ttf",
    "/usr/share/fonts/TTF/ipag.ttf",
    "/usr/share/fonts/truetype/fonts-japanese-gothic.ttf",
    "/usr/share/fonts/adobe-source-han-sans/SourceHanSans.ttc",
    "C:\\Windows\\Fonts\\YuGothM.ttc",
    "C:\\Windows\\Fonts\\msgothic.ttc",
    "C:\\Windows\\Fonts\\meiryo.ttc",
];

/// Busca una fuente con soporte japonés: config > fontconfig > rutas conocidas.
pub fn find_font(cfg_path: Option<&Path>, cfg_index: u32) -> Option<(PathBuf, u32)> {
    if let Some(p) = cfg_path {
        return Some((p.to_path_buf(), cfg_index));
    }
    if let Ok(out) = std::process::Command::new("fc-match")
        .args(["-f", "%{file}\n%{index}", "sans-serif:lang=ja"])
        .output()
    {
        let s = String::from_utf8_lossy(&out.stdout);
        let mut it = s.lines();
        if let Some(f) = it.next().filter(|f| !f.is_empty() && Path::new(f).exists()) {
            let idx = it.next().and_then(|i| i.trim().parse().ok()).unwrap_or(0);
            return Some((PathBuf::from(f), idx));
        }
    }
    FALLBACKS
        .iter()
        .map(Path::new)
        .find(|p| p.exists())
        .map(|p| (p.to_path_buf(), 0))
}

pub fn find_font_family(family: &str) -> Option<(PathBuf, u32, f32)> {
    let run_match = |query: &str| -> Option<(String, PathBuf, u32)> {
        let out = std::process::Command::new("fc-match")
            .args(["-f", "%{family}\n%{file}\n%{index}", query])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let output = String::from_utf8_lossy(&out.stdout);
        let mut lines = output.lines();
        let matched_family = lines.next()?.to_string();
        let path = PathBuf::from(lines.next()?);
        let index = lines.next()?.trim().parse().ok()?;
        path.is_file().then_some((matched_family, path, index))
    };
    let query = format!("{family}:lang=ja");
    if let Some((matched, path, index)) = run_match(&query)
        && matched
            .split(',')
            .any(|name| normalize_font_family(name) == normalize_font_family(family))
    {
        return Some((path, index, 1.0));
    }
    let scale = outline_size_scale_for_fallback(family);
    run_match(font_family_fallback_query(family))
        .map(|(_, path, index)| (path, index, scale))
        .or_else(|| find_font(None, 0).map(|(path, index)| (path, index, 1.0)))
}

fn normalize_font_family(family: &str) -> String {
    family
        .chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Sustituto de contorno cuando no hay `.lft`.
///
/// Ume Gothic / Ume P Gothic (梅フォント) publican anchos compatibles con
/// MS Gothic / MS PGothic. Medido en `ume-tgo4.ttf`: `unitsPerEm` 4096,
/// hhea y usWin ascent+descent = 4096 (celda/em = 1), avance de あ = 1 em
/// y de A = 0.5 em. Con eso `FONTHEIGHT` px = ancho del cuadratín.
/// Noto Sans CJK tiene celda/em = 1.448; el mismo cálculo encoge el cuerpo
/// (22 px pedidos → 15.2 px de あ).
///
/// El em no se multiplica. `FONTHEIGHT` es la altura del script
/// (docs/livenovel/text.html, 「フォントの高さ」). Un factor encima de eso
/// aplasta el contorno cuando hay borde.
pub(crate) fn outline_size_scale_for_fallback(_family: &str) -> f32 {
    1.0
}

/// Sustituto de contorno para un nombre del motor. `None` si no está en la lista
/// (entonces puede usarse un `.lft` del juego, o Noto).
///
/// Nombres vistos en los scripts: `ＭＳ ゴシック`, `ＭＳ Ｐゴシック` y, en testgame,
/// `JKゴシックM` (`女台詞`, `00000071.lsb`). Ese último es el nombre interno de
/// `font0.lft`. El formato no está en el manual y el raster no se usa.
/// Ume publica el mismo cuadratín que MS Gothic y trae latino (`U+0020–U+007E`
/// y `U+00A0–U+017F`), no solo kana.
fn outline_family_query(family: &str) -> Option<&'static str> {
    match normalize_font_family(family).as_str() {
        "ｍｓゴシック" | "msgothic" | "jkゴシックm" | "jkgothicm" => {
            Some("Ume Gothic:style=Regular:lang=ja")
        }
        "ｍｓｐゴシック" | "mspgothic" => Some("Ume P Gothic:style=Regular:lang=ja"),
        "ｍｓ明朝" | "msmincho" => Some("Ume Mincho:style=Regular:lang=ja"),
        "ｍｓｐ明朝" | "mspmincho" => Some("Ume P Mincho:style=Regular:lang=ja"),
        "ｍｓｕｉゴシック" | "msuigothic" => Some("Ume UI Gothic:style=Regular:lang=ja"),
        _ => None,
    }
}

fn font_family_fallback_query(family: &str) -> &'static str {
    outline_family_query(family).unwrap_or("Noto Sans CJK JP:style=Medium:lang=ja")
}

enum FontBackend {
    Outline(ab_glyph::FontVec),
    Pixel(Arc<LivePixelFont>),
}

pub struct TextRenderer {
    backend: FontBackend,
    /// Etiqueta para logs (ruta TTF o `LivePixelFont:nombre`).
    face: String,
    /// Multiplica el em del contorno. `1` deja `FONTHEIGHT` tal cual.
    outline_size_scale: f32,
    cache: HashMap<(char, u32), Glyph>,
}

#[derive(Clone)]
struct Glyph {
    advance: f32,
    /// Desplazamiento del bitmap respecto a (pen_x, baseline).
    ox: i32,
    oy: i32,
    w: usize,
    h: usize,
    cov: Vec<u8>,
}

pub type Rendered = crate::rgba::RgbaImage;

impl TextRenderer {
    /// Carga perezosa (ab_glyph/ttf-parser): casi instantánea incluso con TTC CJK grandes.
    pub fn load(path: &Path, index: u32) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let font = ab_glyph::FontVec::try_from_vec_and_index(bytes, index)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Self {
            backend: FontBackend::Outline(font),
            face: path.display().to_string(),
            outline_size_scale: 1.0,
            cache: HashMap::new(),
        })
    }

    pub fn load_lft(font: Arc<LivePixelFont>) -> Self {
        let face = format!("LivePixelFont:{}", font.name);
        Self {
            backend: FontBackend::Pixel(font),
            face,
            outline_size_scale: 1.0,
            cache: HashMap::new(),
        }
    }

    pub fn face(&self) -> &str {
        &self.face
    }

    pub fn is_pixel_font(&self) -> bool {
        matches!(self.backend, FontBackend::Pixel(_))
    }

    /// Resuelve `FONTNAME`: primero catálogo `.lft`, luego TTF del sistema.
    pub fn load_family(
        family: &str,
        catalog: &PixelFontCatalog,
        outline_override: Option<(PathBuf, u32)>,
    ) -> Option<Self> {
        // Un nombre con sustituto de contorno no pasa por el `.lft` embebido.
        if outline_family_query(family).is_none()
            && !family.is_empty()
            && let Some(pixel) = catalog.get(family)
        {
            let renderer = Self::load_lft(pixel);
            log::info!("font family {family:?} using {}", renderer.face());
            return Some(renderer);
        }
        let outline = outline_override
            .map(|(path, index)| (path, index, 1.0))
            .or_else(|| {
                if family.is_empty() {
                    find_font(None, 0).map(|(path, index)| (path, index, 1.0))
                } else {
                    find_font_family(family)
                }
            })?;
        match Self::load(&outline.0, outline.1) {
            Ok(mut renderer) => {
                renderer.outline_size_scale = outline.2;
                log::info!(
                    "font family {family:?} using outline {} size {:.3}",
                    renderer.face(),
                    renderer.outline_size_scale
                );
                Some(renderer)
            }
            Err(error) => {
                log::warn!("cannot load font family {family:?}: {error}");
                None
            }
        }
    }

    pub fn size_for_cell_height(&self, height: f32) -> f32 {
        match &self.backend {
            FontBackend::Outline(font) => {
                use ab_glyph::{Font, ScaleFont};
                let scaled = font.as_scaled(1.0);
                em_size_for_cell_height(height, scaled.ascent() - scaled.descent())
                    * self.outline_size_scale
            }
            // Pixel: `px` = altura de celda pedida; el raster escala desde cell_height nativo.
            FontBackend::Pixel(_) => height,
        }
    }

    pub fn measure_text_width(&mut self, text: &str, px: f32) -> f32 {
        text.chars()
            .map(|character| self.glyph(character, px).advance)
            .sum()
    }

    /// Pluma al final del texto, con el mismo corte de línea que `render`.
    /// `(x, y)` es el borde izquierdo del siguiente glifo y el tope de su línea.
    pub fn pen_after(
        &mut self,
        items: &[TextItem],
        width: f32,
        px: f32,
        images: &HashMap<String, Arc<crate::gal::Image>>,
    ) -> (f32, f32) {
        let (_, line_h) = self.line_metrics(px);
        let w = width.max(1.0);
        let (mut x, mut y) = (0.0f32, 0.0f32);
        for item in items {
            match item {
                TextItem::Newline => {
                    x = 0.0;
                    y += line_h;
                }
                TextItem::Char(text) => {
                    for character in text.chars() {
                        let advance = self.glyph(character, px).advance;
                        if x + advance > w && x > 0.0 {
                            x = 0.0;
                            y += line_h;
                        }
                        x += advance;
                    }
                }
                TextItem::Image { source, .. } => {
                    let Some(image) = images.get(source) else {
                        continue;
                    };
                    let advance = inline_image_size(image, line_h).0;
                    if x + advance > w && x > 0.0 {
                        x = 0.0;
                        y += line_h;
                    }
                    x += advance;
                }
            }
        }
        (x, y)
    }

    fn line_metrics(&self, px: f32) -> (f32, f32) {
        match &self.backend {
            FontBackend::Outline(font) => {
                use ab_glyph::{Font, ScaleFont};
                let sf = font.as_scaled(px);
                let ascent = sf.ascent();
                let line_h = (sf.ascent() - sf.descent() + sf.line_gap()).max(px) * 1.1;
                (ascent, line_h)
            }
            FontBackend::Pixel(font) => {
                let scale = if font.cell_height > 0 {
                    px / f32::from(font.cell_height)
                } else {
                    1.0
                };
                let ascent = f32::from(font.ascent) * scale;
                (ascent, px * 1.1)
            }
        }
    }

    fn glyph(&mut self, c: char, px: f32) -> Glyph {
        let key = (c, (px * 4.0) as u32);
        if let Some(g) = self.cache.get(&key) {
            return g.clone();
        }
        let glyph = match &self.backend {
            FontBackend::Outline(font) => {
                use ab_glyph::{Font, ScaleFont};
                let sf = font.as_scaled(px);
                let id = font.glyph_id(c);
                let advance = sf.h_advance(id);
                let g = id.with_scale_and_position(px, ab_glyph::point(0.0, 0.0));
                match font.outline_glyph(g) {
                    Some(og) => {
                        let b = og.px_bounds();
                        let (w, h) = (b.width().ceil() as usize, b.height().ceil() as usize);
                        let mut cov = vec![0u8; w * h];
                        og.draw(|x, y, v| {
                            let i = y as usize * w + x as usize;
                            if i < cov.len() {
                                cov[i] = (v.clamp(0.0, 1.0) * 255.0) as u8;
                            }
                        });
                        Glyph {
                            advance,
                            ox: b.min.x.floor() as i32,
                            oy: b.min.y.floor() as i32,
                            w,
                            h,
                            cov,
                        }
                    }
                    None => Glyph {
                        advance,
                        ox: 0,
                        oy: 0,
                        w: 0,
                        h: 0,
                        cov: vec![],
                    },
                }
            }
            FontBackend::Pixel(font) => {
                let g = font.raster(c, px);
                Glyph {
                    advance: g.advance,
                    ox: g.ox,
                    oy: g.oy,
                    w: g.w,
                    h: g.h,
                    cov: g.cov,
                }
            }
        };
        self.cache.insert(key, glyph.clone());
        glyph
    }

    /// Dibuja `items` dentro de una caja w×h con ajuste de línea.
    pub fn render(
        &mut self,
        items: &[TextItem],
        w: u32,
        h: u32,
        px: f32,
        color: [u8; 4],
        shadow: bool,
    ) -> Rendered {
        self.render_aligned(items, w, h, px, color, shadow, 0)
    }

    /// Igual que `render`, alineando cada línea a la izquierda (0), centro (1) o derecha (2).
    pub fn render_aligned(
        &mut self,
        items: &[TextItem],
        w: u32,
        h: u32,
        px: f32,
        color: [u8; 4],
        shadow: bool,
        align: u8,
    ) -> Rendered {
        self.render_aligned_with_images(
            items,
            w,
            h,
            px,
            color,
            shadow,
            [0, 0, 0, color[3] / 2 + 64],
            false,
            [0, 0, 0, 255],
            align,
            &HashMap::new(),
        )
    }

    pub fn render_aligned_with_images(
        &mut self,
        items: &[TextItem],
        w: u32,
        h: u32,
        px: f32,
        color: [u8; 4],
        shadow: bool,
        shadow_color: [u8; 4],
        border: bool,
        border_color: [u8; 4],
        align: u8,
        images: &HashMap<String, Arc<crate::gal::Image>>,
    ) -> Rendered {
        let (w, h) = (w.max(1), h.max(1));
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        let (ascent, line_h) = self.line_metrics(px);
        let mut line_widths = vec![0.0f32];
        for item in items {
            match item {
                TextItem::Newline => line_widths.push(0.0),
                TextItem::Char(text) => {
                    for c in text.chars() {
                        let advance = self.glyph(c, px).advance;
                        let line = line_widths.last_mut().unwrap();
                        if *line + advance > w as f32 && *line > 0.0 {
                            line_widths.push(advance);
                        } else {
                            *line += advance;
                        }
                    }
                }
                TextItem::Image { source, .. } => {
                    if let Some(image) = images.get(source) {
                        let advance = inline_image_size(image, line_h).0;
                        let line = line_widths.last_mut().unwrap();
                        if *line + advance > w as f32 && *line > 0.0 {
                            line_widths.push(advance);
                        } else {
                            *line += advance;
                        }
                    }
                }
            }
        }
        let line_offset = |i: usize| {
            let remaining = (w as f32 - line_widths.get(i).copied().unwrap_or(0.0)).max(0.0);
            match align {
                1 => remaining / 2.0,
                2 => remaining,
                _ => 0.0,
            }
        };
        let (mut line, mut x, mut y) = (0usize, line_offset(0), 0.0f32);
        let blit = |rgba: &mut [u8], g: &Glyph, pen_x: f32, top: f32, col: [u8; 4]| {
            let gx = pen_x.round() as i32 + g.ox;
            let gy = (top + ascent).round() as i32 + g.oy;
            for row in 0..g.h {
                for colx in 0..g.w {
                    let cov = g.cov[row * g.w + colx] as u32;
                    if cov == 0 {
                        continue;
                    }
                    let (px_, py_) = (gx + colx as i32, gy + row as i32);
                    if px_ < 0 || py_ < 0 || px_ >= w as i32 || py_ >= h as i32 {
                        continue;
                    }
                    let o = ((py_ as u32 * w + px_ as u32) * 4) as usize;
                    let sa = cov * col[3] as u32 / 255;
                    let da = rgba[o + 3] as u32;
                    let oa = sa + da * (255 - sa) / 255;
                    if oa == 0 {
                        continue;
                    }
                    for c in 0..3 {
                        rgba[o + c] = ((col[c] as u32 * sa
                            + rgba[o + c] as u32 * da * (255 - sa) / 255)
                            / oa) as u8;
                    }
                    rgba[o + 3] = oa as u8;
                }
            }
        };
        'outer: for it in items {
            match it {
                TextItem::Newline => {
                    line += 1;
                    x = line_offset(line);
                    y += line_h;
                }
                TextItem::Char(s) => {
                    for c in s.chars() {
                        let g = self.glyph(c, px);
                        if x + g.advance > w as f32 && x > 0.0 {
                            line += 1;
                            x = line_offset(line);
                            y += line_h;
                        }
                        if y > h as f32 {
                            break 'outer;
                        }
                        if shadow {
                            blit(&mut rgba, &g, x + 2.0, y + 2.0, shadow_color);
                        }
                        if border {
                            for (dx, dy) in [
                                (-1.0, 0.0),
                                (1.0, 0.0),
                                (0.0, -1.0),
                                (0.0, 1.0),
                                (-1.0, -1.0),
                                (1.0, -1.0),
                                (-1.0, 1.0),
                                (1.0, 1.0),
                            ] {
                                blit(&mut rgba, &g, x + dx, y + dy, border_color);
                            }
                        }
                        blit(&mut rgba, &g, x, y, color);
                        x += g.advance;
                    }
                }
                TextItem::Image { source, .. } => {
                    let Some(image) = images.get(source) else {
                        continue;
                    };
                    let (image_w, image_h) = inline_image_size(image, line_h);
                    if x + image_w > w as f32 && x > 0.0 {
                        line += 1;
                        x = line_offset(line);
                        y += line_h;
                    }
                    if y > h as f32 {
                        break 'outer;
                    }
                    blit_inline_image(
                        &mut rgba,
                        (w, h),
                        image,
                        (
                            x.round() as i32,
                            (y + inline_image_vertical_offset(align, line_h, image_h)).round()
                                as i32,
                        ),
                        (
                            image_w.round().max(1.0) as u32,
                            image_h.round().max(1.0) as u32,
                        ),
                    );
                    x += image_w;
                }
            }
        }
        Rendered::new(w, h, rgba).expect("text rasterizer creates a valid RGBA image")
    }
}

fn inline_image_size(image: &crate::gal::Image, line_height: f32) -> (f32, f32) {
    if image.w == 0 || image.h == 0 {
        return (0.0, 0.0);
    }
    let scale = (line_height / image.h as f32).min(1.0);
    (image.w as f32 * scale, image.h as f32 * scale)
}

fn inline_image_vertical_offset(align: u8, line_height: f32, image_height: f32) -> f32 {
    match align {
        4 => 0.0,
        5 => (line_height - image_height).max(0.0),
        _ => (line_height - image_height).max(0.0) / 2.0,
    }
}

fn blit_inline_image(
    target: &mut [u8],
    target_size: (u32, u32),
    image: &crate::gal::Image,
    position: (i32, i32),
    size: (u32, u32),
) {
    if image.w == 0 || image.h == 0 || size.0 == 0 || size.1 == 0 {
        return;
    }
    for y in 0..size.1 {
        let target_y = position.1 + y as i32;
        if !(0..target_size.1 as i32).contains(&target_y) {
            continue;
        }
        let source_y = (y as u64 * image.h as u64 / size.1 as u64) as usize;
        for x in 0..size.0 {
            let target_x = position.0 + x as i32;
            if !(0..target_size.0 as i32).contains(&target_x) {
                continue;
            }
            let source_x = (x as u64 * image.w as u64 / size.0 as u64) as usize;
            let source_offset = (source_y * image.w as usize + source_x) * 4;
            let target_offset =
                (target_y as usize * target_size.0 as usize + target_x as usize) * 4;
            let source_alpha = image.rgba[source_offset + 3] as u32;
            if source_alpha == 0 {
                continue;
            }
            let target_alpha = target[target_offset + 3] as u32;
            let output_alpha = source_alpha + target_alpha * (255 - source_alpha) / 255;
            for channel in 0..3 {
                target[target_offset + channel] = ((image.rgba[source_offset + channel] as u32
                    * source_alpha
                    + target[target_offset + channel] as u32 * target_alpha * (255 - source_alpha)
                        / 255)
                    / output_alpha) as u8;
            }
            target[target_offset + 3] = output_alpha as u8;
        }
    }
}

/// TColor de Delphi (0x00BBGGRR) -> RGBA.
pub fn tcolor(v: i64, alpha: u8) -> [u8; 4] {
    let v = v as u32;
    [
        (v & 0xFF) as u8,
        ((v >> 8) & 0xFF) as u8,
        ((v >> 16) & 0xFF) as u8,
        alpha,
    ]
}

fn em_size_for_cell_height(height: f32, cell_height: f32) -> f32 {
    if height.is_finite() && height > 0.0 && cell_height.is_finite() && cell_height > 0.0 {
        height / cell_height
    } else {
        height
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TextRenderer, blit_inline_image, em_size_for_cell_height, font_family_fallback_query,
        inline_image_vertical_offset, outline_size_scale_for_fallback,
    };
    use crate::gal::Image;
    use crate::lft::{PixelFontCatalog, build_synthetic_lft};
    use crate::scene::TextItem;
    use std::collections::HashMap;
    use std::path::Path;

    #[test]
    fn font_size_matches_requested_cell_height() {
        assert!((em_size_for_cell_height(22.0, 1.1) - 20.0).abs() < f32::EPSILON);
        assert_eq!(em_size_for_cell_height(22.0, 0.0), 22.0);
    }

    #[test]
    fn ms_gothic_uses_a_metric_compatible_fallback() {
        assert_eq!(
            font_family_fallback_query("ＭＳ ゴシック"),
            "Ume Gothic:style=Regular:lang=ja"
        );
        assert_eq!(
            font_family_fallback_query("MS Gothic"),
            "Ume Gothic:style=Regular:lang=ja"
        );
        assert_eq!(
            font_family_fallback_query("ＭＳ Ｐゴシック"),
            "Ume P Gothic:style=Regular:lang=ja"
        );
        assert_eq!(
            font_family_fallback_query("MS PGothic"),
            "Ume P Gothic:style=Regular:lang=ja"
        );
        assert_eq!(
            font_family_fallback_query("JKゴシックM"),
            "Ume Gothic:style=Regular:lang=ja"
        );
        assert_eq!(
            font_family_fallback_query("ＭＳ　Ｐゴシック"),
            "Ume P Gothic:style=Regular:lang=ja"
        );
        assert_eq!(outline_size_scale_for_fallback("ＭＳ ゴシック"), 1.0);
        assert_eq!(outline_size_scale_for_fallback("Noto Sans CJK JP"), 1.0);
    }

    #[test]
    fn jk_gothic_skips_the_embedded_pixel_font() {
        let path = Path::new("/home/gemu/.local/share/fonts/ume/ume-tgo4.ttf");
        if !path.is_file() {
            return;
        }
        let (name, _, _) = encoding_rs::SHIFT_JIS.encode("JKゴシックM");
        let cov = [255u8; 16];
        let bytes = build_synthetic_lft(&name, &[(u16::from(b'A'), 4, &cov)]);
        let font = crate::lft::LivePixelFont::parse(&bytes).expect("synthetic lft");
        assert_eq!(font.name, "JKゴシックM");
        let mut catalog = PixelFontCatalog::default();
        catalog.insert_parsed(font);
        let mut renderer = TextRenderer::load_family("JKゴシックM", &catalog, None).expect("ume");
        assert!(!renderer.is_pixel_font());
        assert!(renderer.face().contains("ume-tgo4"));
        let latin = renderer.glyph('A', renderer.size_for_cell_height(16.0));
        let kana = renderer.glyph('あ', renderer.size_for_cell_height(16.0));
        assert!(latin.advance > 1.0);
        assert!(kana.advance > latin.advance);
    }

    #[test]
    fn ume_em_matches_the_script_height() {
        let path = Path::new("/home/gemu/.local/share/fonts/ume/ume-tgo4.ttf");
        if !path.is_file() {
            return;
        }
        let mut font = TextRenderer::load(path, 0).expect("ume");
        let height = 16.0;
        let glyph = font.glyph('大', font.size_for_cell_height(height));
        assert!((glyph.advance - height).abs() < 0.05);
    }

    #[test]
    fn pen_after_wraps_like_the_raster() {
        let path = Path::new("/home/gemu/.local/share/fonts/ume/ume-tgo4.ttf");
        if !path.is_file() {
            return;
        }
        let mut font = TextRenderer::load(path, 0).expect("ume");
        let px = 16.0;
        let advance = font.glyph('大', px).advance;
        let items = vec![TextItem::Char("大大大大".into())];
        let (x, y) = font.pen_after(&items, advance * 3.0, px, &HashMap::new());
        assert!((x - advance).abs() < 0.05);
        let (_, line_h) = font.line_metrics(px);
        assert!((y - line_h).abs() < 0.05);
    }

    #[test]
    fn inline_image_composites_rgba_pixels_into_text_surface() {
        let image = Image {
            w: 1,
            h: 1,
            rgba: vec![240, 40, 20, 255],
            frames: vec![],
            delays: vec![],
        };
        let mut target = vec![0; 2 * 2 * 4];

        blit_inline_image(&mut target, (2, 2), &image, (1, 0), (1, 1));

        assert_eq!(&target[4..8], &[240, 40, 20, 255]);
        assert_eq!(&target[0..4], &[0, 0, 0, 0]);
    }

    #[test]
    fn inline_image_vertical_alignment_supports_top_center_and_bottom() {
        assert_eq!(inline_image_vertical_offset(4, 20.0, 10.0), 0.0);
        assert_eq!(inline_image_vertical_offset(3, 20.0, 10.0), 5.0);
        assert_eq!(inline_image_vertical_offset(5, 20.0, 10.0), 10.0);
    }
}
