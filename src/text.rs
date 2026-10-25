//! Rasterizado de texto con una fuente CJK del sistema (fontdue, Rust puro).

use crate::scene::TextItem;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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

pub fn find_font_family(family: &str) -> Option<(PathBuf, u32)> {
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
    let normalized = |s: &str| {
        s.chars()
            .filter(|c| !c.is_whitespace())
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    let query = format!("{family}:lang=ja");
    if let Some((matched, path, index)) = run_match(&query)
        && matched
            .split(',')
            .any(|name| normalized(name) == normalized(family))
    {
        return Some((path, index));
    }
    run_match("Noto Sans CJK JP:style=Medium:lang=ja")
        .map(|(_, path, index)| (path, index))
        .or_else(|| find_font(None, 0))
}

pub struct TextRenderer {
    font: ab_glyph::FontVec,
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

pub struct Rendered {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

impl TextRenderer {
    /// Carga perezosa (ab_glyph/ttf-parser): casi instantánea incluso con TTC CJK grandes.
    pub fn load(path: &Path, index: u32) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let font = ab_glyph::FontVec::try_from_vec_and_index(bytes, index)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Self {
            font,
            cache: HashMap::new(),
        })
    }

    pub fn size_for_cell_height(&self, height: f32) -> f32 {
        use ab_glyph::{Font, ScaleFont};
        let scaled = self.font.as_scaled(1.0);
        em_size_for_cell_height(height, scaled.ascent() - scaled.descent())
    }

    fn glyph(&mut self, c: char, px: f32) -> Glyph {
        use ab_glyph::{Font, ScaleFont};
        let key = (c, (px * 4.0) as u32);
        if let Some(g) = self.cache.get(&key) {
            return g.clone();
        }
        let sf = self.font.as_scaled(px);
        let id = self.font.glyph_id(c);
        let advance = sf.h_advance(id);
        let g = id.with_scale_and_position(px, ab_glyph::point(0.0, 0.0));
        let glyph = match self.font.outline_glyph(g) {
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
        use ab_glyph::{Font, ScaleFont};
        let (w, h) = (w.max(1), h.max(1));
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        let sf = self.font.as_scaled(px);
        let ascent = sf.ascent();
        let line_h = (sf.ascent() - sf.descent() + sf.line_gap()).max(px) * 1.1;
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
                            blit(
                                &mut rgba,
                                &g,
                                x + 2.0,
                                y + 2.0,
                                [0, 0, 0, color[3] / 2 + 64],
                            );
                        }
                        blit(&mut rgba, &g, x, y, color);
                        x += g.advance;
                    }
                }
            }
        }
        Rendered { w, h, rgba }
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
    use super::em_size_for_cell_height;

    #[test]
    fn font_size_matches_requested_cell_height() {
        assert!((em_size_for_cell_height(22.0, 1.1) - 20.0).abs() < f32::EPSILON);
        assert_eq!(em_size_for_cell_height(22.0, 0.0), 22.0);
    }
}
