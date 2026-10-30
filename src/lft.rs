//! LiveMaker LivePixelFont (`.lft`) — parser y raster de glifos.
//!
//! El formato **no** está documentado en `docs/livenovel/` ni en pylivemaker.
//! Layout inferido de un `.lft` real + uso de `FONTNAME` = nombre interno (COMPATIBILITY H20).
//! Campos no usados se marcan **no confirmado**.

use crate::archive::GameData;
use flate2::read::ZlibDecoder;
use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;

const MAGIC: &[u8] = b"LivePixelFont\0";
/// Tamaño del bloque de métricas tras el nombre (H20: cabe exacto con dir+bitmaps).
const METRICS_LEN: usize = 232;
const NAME_LEN: usize = 64;
const GLYPH_RECORD_LEN: usize = 18;

#[derive(Debug, Clone)]
pub struct LivePixelFont {
    pub name: String,
    /// Altura de celda en píxeles nativos (cabecera).
    pub cell_height: u16,
    /// **no confirmado**: interpretado como ascent GDI-like desde el bloque de métricas.
    pub ascent: i16,
    /// **no confirmado**: descent del mismo bloque.
    pub descent: i16,
    /// **no confirmado**: `tmAveCharWidth`-like; usado solo para sintetizar U+0020 ausente.
    pub ave_char_width: u16,
    glyphs: HashMap<u16, GlyphInfo>,
    bitmaps: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
struct GlyphInfo {
    width: u16,
    height: u16,
    bearing_x: i16,
    /// Distancia baseline → top del bitmap (positivo hacia arriba en el atlas).
    bearing_y: i16,
    advance: u16,
    offset: u32,
}

#[derive(Debug, Clone)]
pub struct RasterGlyph {
    pub advance: f32,
    pub ox: i32,
    pub oy: i32,
    pub w: usize,
    pub h: usize,
    pub cov: Vec<u8>,
}

#[derive(Debug, Default, Clone)]
pub struct PixelFontCatalog {
    by_name: HashMap<String, Arc<LivePixelFont>>,
}

impl PixelFontCatalog {
    pub fn load_from_game_data(data: &GameData) -> Self {
        let mut by_name = HashMap::new();
        for name in data.names() {
            if !name.to_ascii_lowercase().ends_with(".lft") {
                continue;
            }
            let Ok(bytes) = data.read(name) else {
                log::warn!("cannot read embedded font {name}");
                continue;
            };
            match LivePixelFont::parse(&bytes) {
                Ok(font) => {
                    let key = normalize_face_name(&font.name);
                    log::info!(
                        "LivePixelFont {} loaded as {:?} ({} glyphs, cell {}px)",
                        name,
                        font.name,
                        font.glyph_count(),
                        font.cell_height
                    );
                    by_name.insert(key, Arc::new(font));
                }
                Err(error) => log::warn!("cannot parse LivePixelFont {name}: {error}"),
            }
        }
        Self { by_name }
    }

    pub fn get(&self, family: &str) -> Option<Arc<LivePixelFont>> {
        self.by_name.get(&normalize_face_name(family)).cloned()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    #[cfg(test)]
    pub fn insert_parsed(&mut self, font: LivePixelFont) {
        let key = normalize_face_name(&font.name);
        self.by_name.insert(key, Arc::new(font));
    }
}

impl LivePixelFont {
    pub fn parse(data: &[u8]) -> Result<Self, String> {
        if data.len() < MAGIC.len() + 12 + NAME_LEN + METRICS_LEN {
            return Err("LivePixelFont truncated header".into());
        }
        if &data[..MAGIC.len()] != MAGIC {
            return Err("not a LivePixelFont (bad magic)".into());
        }
        let version = read_u32(data, MAGIC.len())?;
        if version != 100 {
            // H20: solo visto 100; otros valores no explorados.
            log::warn!("LivePixelFont version {version} (only 100 verified; no confirmado)");
        }
        let dir_z_size = read_u32(data, MAGIC.len() + 4)? as usize;
        let bitmap_size = read_u32(data, MAGIC.len() + 8)? as usize;
        let name_raw = &data[MAGIC.len() + 12..MAGIC.len() + 12 + NAME_LEN];
        let name = decode_cp932_cstr(name_raw);
        let metrics_off = MAGIC.len() + 12 + NAME_LEN;
        let metrics = &data[metrics_off..metrics_off + METRICS_LEN];
        let cell_height = read_u32(metrics, 0)? as u16;
        let glyph_count_hdr = read_u32(metrics, 12)? as usize;
        let ascent = read_i32(metrics, 24)? as i16;
        let descent = read_i32(metrics, 28)? as i16;
        let ave_char_width = read_u32(metrics, 40)?.max(1) as u16;

        let blob_off = metrics_off + METRICS_LEN;
        if data.len() < blob_off + dir_z_size + bitmap_size {
            return Err(format!(
                "LivePixelFont truncated body (need {} bytes, have {})",
                blob_off + dir_z_size + bitmap_size,
                data.len()
            ));
        }
        let dir_z = &data[blob_off..blob_off + dir_z_size];
        let bitmaps = data[blob_off + dir_z_size..blob_off + dir_z_size + bitmap_size].to_vec();

        let directory = inflate(dir_z)?;
        if !directory.len().is_multiple_of(GLYPH_RECORD_LEN) {
            return Err(format!(
                "glyph directory length {} not multiple of {GLYPH_RECORD_LEN}",
                directory.len()
            ));
        }
        let count = directory.len() / GLYPH_RECORD_LEN;
        if count != glyph_count_hdr {
            log::warn!(
                "LivePixelFont glyph_count header {glyph_count_hdr} != directory {count} (using directory)"
            );
        }

        let mut glyphs = HashMap::with_capacity(count);
        for i in 0..count {
            let rec = &directory[i * GLYPH_RECORD_LEN..(i + 1) * GLYPH_RECORD_LEN];
            let code = read_u16(rec, 0)?;
            let width = read_u16(rec, 2)?;
            let height = read_u16(rec, 4)?;
            let bearing_x = read_i16(rec, 6)?;
            let bearing_y = read_i16(rec, 8)?;
            let advance = read_u16(rec, 10)?;
            let _unused = read_u16(rec, 12)?; // siempre 0 en lo visto (H20); no confirmado
            let offset = read_u32(rec, 14)?;
            let stride = row_stride(width as usize);
            let need = stride.saturating_mul(height as usize);
            let end = offset as usize + need;
            if end > bitmaps.len() {
                return Err(format!(
                    "glyph U+{code:04X} bitmap OOB (off {offset}, need {need}, bmp {})",
                    bitmaps.len()
                ));
            }
            glyphs.insert(
                code,
                GlyphInfo {
                    width,
                    height,
                    bearing_x,
                    bearing_y,
                    advance,
                    offset,
                },
            );
        }

        Ok(Self {
            name,
            cell_height: cell_height.max(1),
            ascent,
            descent,
            ave_char_width,
            glyphs,
            bitmaps,
        })
    }

    pub fn glyph_count(&self) -> usize {
        self.glyphs.len()
    }

    pub fn raster(&self, ch: char, px: f32) -> RasterGlyph {
        let scale = if self.cell_height > 0 && px.is_finite() && px > 0.0 {
            px / f32::from(self.cell_height)
        } else {
            1.0
        };
        if let Some(key) = cp932_glyph_key(ch)
            && let Some(info) = self.glyphs.get(&key)
        {
            return self.raster_info(*info, scale);
        }
        // Inventado en livenyan: U+0020 ausente (H20) → avance promedio de cabecera.
        if ch == ' ' {
            return RasterGlyph {
                advance: f32::from(self.ave_char_width) * scale,
                ox: 0,
                oy: 0,
                w: 0,
                h: 0,
                cov: Vec::new(),
            };
        }
        // Inventado en livenyan: ideographic space / fullwidth space → celda completa.
        if ch == '\u{3000}' || ch == '\u{00A0}' {
            return RasterGlyph {
                advance: f32::from(self.cell_height) * scale,
                ox: 0,
                oy: 0,
                w: 0,
                h: 0,
                cov: Vec::new(),
            };
        }
        RasterGlyph {
            advance: 0.0,
            ox: 0,
            oy: 0,
            w: 0,
            h: 0,
            cov: Vec::new(),
        }
    }

    fn raster_info(&self, info: GlyphInfo, scale: f32) -> RasterGlyph {
        let src_w = info.width as usize;
        let src_h = info.height as usize;
        let stride = row_stride(src_w);
        let src = &self.bitmaps[info.offset as usize..info.offset as usize + stride * src_h];
        let advance = f32::from(info.advance) * scale;
        let ox = (f32::from(info.bearing_x) * scale).round() as i32;
        // oy: top del bitmap relativo a baseline (bearing_y hacia arriba → oy negativo).
        let oy = (-f32::from(info.bearing_y) * scale).round() as i32;

        if (scale - 1.0).abs() < f32::EPSILON {
            let mut cov = Vec::with_capacity(src_w * src_h);
            for y in 0..src_h {
                cov.extend_from_slice(&src[y * stride..y * stride + src_w]);
            }
            return RasterGlyph {
                advance,
                ox,
                oy,
                w: src_w,
                h: src_h,
                cov,
            };
        }

        let dst_w = ((src_w as f32) * scale).round().max(1.0) as usize;
        let dst_h = ((src_h as f32) * scale).round().max(1.0) as usize;
        let mut cov = vec![0u8; dst_w * dst_h];
        for y in 0..dst_h {
            let sy = ((y as f32) / scale).floor() as usize;
            if sy >= src_h {
                continue;
            }
            for x in 0..dst_w {
                let sx = ((x as f32) / scale).floor() as usize;
                if sx >= src_w {
                    continue;
                }
                cov[y * dst_w + x] = src[sy * stride + sx];
            }
        }
        RasterGlyph {
            advance,
            ox,
            oy,
            w: dst_w,
            h: dst_h,
            cov,
        }
    }
}

pub fn normalize_face_name(family: &str) -> String {
    family
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

fn row_stride(width: usize) -> usize {
    width.div_ceil(4) * 4
}

fn inflate(z: &[u8]) -> Result<Vec<u8>, String> {
    let mut dec = ZlibDecoder::new(z);
    let mut out = Vec::new();
    dec.read_to_end(&mut out)
        .map_err(|e| format!("LivePixelFont zlib: {e}"))?;
    Ok(out)
}

fn decode_cp932_cstr(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let (text, _, _) = encoding_rs::SHIFT_JIS.decode(&bytes[..end]);
    text.into_owned()
}

fn cp932_glyph_key(ch: char) -> Option<u16> {
    let text = ch.to_string();
    let (cow, _, had_errors) = encoding_rs::SHIFT_JIS.encode(&text);
    if had_errors {
        return None;
    }
    match cow.as_ref() {
        [b] => Some(u16::from(*b)),
        [hi, lo] => Some(u16::from_be_bytes([*hi, *lo])),
        _ => None,
    }
}

fn read_u16(data: &[u8], off: usize) -> Result<u16, String> {
    let b = data
        .get(off..off + 2)
        .ok_or_else(|| "LivePixelFont OOB u16".to_string())?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}

fn read_i16(data: &[u8], off: usize) -> Result<i16, String> {
    Ok(read_u16(data, off)? as i16)
}

fn read_u32(data: &[u8], off: usize) -> Result<u32, String> {
    let b = data
        .get(off..off + 4)
        .ok_or_else(|| "LivePixelFont OOB u32".to_string())?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn read_i32(data: &[u8], off: usize) -> Result<i32, String> {
    Ok(read_u32(data, off)? as i32)
}

/// Construye un `.lft` mínimo sintético (solo para tests; no es asset de juego).
#[cfg(test)]
pub fn build_synthetic_lft(name_cp932: &[u8], glyphs: &[(u16, u16, &[u8])]) -> Vec<u8> {
    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use std::io::Write;

    let cell = 8u32;
    let mut directory = Vec::new();
    let mut bitmaps = Vec::new();
    for &(code, advance, cov) in glyphs {
        let w = 4u16;
        let h = 4u16;
        assert_eq!(cov.len(), 16);
        let off = bitmaps.len() as u32;
        // stride 4 for width 4
        bitmaps.extend_from_slice(cov);
        directory.extend_from_slice(&code.to_le_bytes());
        directory.extend_from_slice(&w.to_le_bytes());
        directory.extend_from_slice(&h.to_le_bytes());
        directory.extend_from_slice(&0i16.to_le_bytes()); // bx
        directory.extend_from_slice(&6i16.to_le_bytes()); // by
        directory.extend_from_slice(&advance.to_le_bytes());
        directory.extend_from_slice(&0u16.to_le_bytes());
        directory.extend_from_slice(&off.to_le_bytes());
    }
    let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
    z.write_all(&directory).unwrap();
    let dir_z = z.finish().unwrap();

    let mut name = [0u8; NAME_LEN];
    name[..name_cp932.len().min(NAME_LEN)]
        .copy_from_slice(&name_cp932[..name_cp932.len().min(NAME_LEN)]);

    let mut metrics = vec![0u8; METRICS_LEN];
    metrics[0..4].copy_from_slice(&cell.to_le_bytes());
    metrics[12..16].copy_from_slice(&(glyphs.len() as u32).to_le_bytes());
    metrics[20..24].copy_from_slice(&cell.to_le_bytes());
    metrics[24..28].copy_from_slice(&6i32.to_le_bytes()); // ascent
    metrics[28..32].copy_from_slice(&2i32.to_le_bytes()); // descent
    metrics[40..44].copy_from_slice(&4u32.to_le_bytes()); // ave

    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&100u32.to_le_bytes());
    out.extend_from_slice(&(dir_z.len() as u32).to_le_bytes());
    out.extend_from_slice(&(bitmaps.len() as u32).to_le_bytes());
    out.extend_from_slice(&name);
    out.extend_from_slice(&metrics);
    out.extend_from_slice(&dir_z);
    out.extend_from_slice(&bitmaps);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_synthetic_and_rasters_glyph() {
        let cov = [
            0u8, 255, 255, 0, 255, 0, 0, 255, 255, 0, 0, 255, 0, 255, 255, 0,
        ];
        let bytes = build_synthetic_lft(b"TestFace", &[(b'A' as u16, 5, &cov)]);
        let font = LivePixelFont::parse(&bytes).expect("parse");
        assert_eq!(font.name, "TestFace");
        assert_eq!(font.cell_height, 8);
        assert_eq!(font.glyph_count(), 1);
        let g = font.raster('A', 8.0);
        assert_eq!(g.w, 4);
        assert_eq!(g.h, 4);
        assert_eq!(g.advance, 5.0);
        assert_eq!(g.cov, cov);
        let space = font.raster(' ', 8.0);
        assert_eq!(space.advance, 4.0);
        assert!(space.cov.is_empty());
    }

    #[test]
    fn catalog_matches_normalized_family_name() {
        let cov = [255u8; 16];
        let (name_bytes, _, _) = encoding_rs::SHIFT_JIS.encode("テストフォントM");
        let bytes = build_synthetic_lft(name_bytes.as_ref(), &[(0x41, 4, &cov)]);
        let font = Arc::new(LivePixelFont::parse(&bytes).unwrap());
        let mut cat = PixelFontCatalog::default();
        cat.by_name
            .insert(normalize_face_name(&font.name), font.clone());
        assert!(cat.get("テストフォントM").is_some());
        assert!(cat.get(" テストフォントm ").is_some());
        assert!(cat.get("other").is_none());
    }
}
