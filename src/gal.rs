//! Decodificador GAL / GaleX (port de pylivemaker `GalImagePlugin.py`).
//!
//! Decodifica el primer frame y compone todas sus capas visibles en RGBA8.
//! Filas de arriba a abajo (igual que pylivemaker).

use crate::util::cp932;
use std::io::Read;

#[derive(Debug, Clone)]
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug)]
pub struct GalError(pub String);
impl std::fmt::Display for GalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GAL: {}", self.0)
    }
}
impl std::error::Error for GalError {}
type R<T> = Result<T, GalError>;
fn err<T>(m: impl Into<String>) -> R<T> {
    Err(GalError(m.into()))
}

struct Rd<'a> {
    d: &'a [u8],
    p: usize,
}
impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> R<&'a [u8]> {
        if self.p.checked_add(n).is_none_or(|e| e > self.d.len()) {
            return err("truncated");
        }
        let s = &self.d[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }
    fn u8(&mut self) -> R<u8> {
        Ok(self.take(1)?[0])
    }
    fn i32(&mut self) -> R<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn usize(&mut self) -> R<usize> {
        let v = self.i32()?;
        if v < 0 {
            return err("negative size");
        }
        Ok(v as usize)
    }
}

#[derive(Debug, Clone, Default)]
struct Info {
    w: u32,
    h: u32,
    bpp: u32,
    randomized: bool,
    compression: Option<u8>,
    block_w: i32,
    block_h: i32,
}

#[derive(Debug, Clone)]
struct Layer {
    left: i32,
    top: i32,
    visible: bool,
    alpha: i32,
    alpha_on: bool,
}

struct Frame {
    w: u32,
    h: u32,
    bpp: u32,
    palette: Vec<[u8; 3]>,
    layers: Vec<(Layer, Vec<u8>, Option<Vec<u8>>)>, // (info, datos crudos, alfa cruda)
}

fn inflate(d: &[u8]) -> R<Vec<u8>> {
    let mut out = Vec::with_capacity(d.len() * 4);
    flate2::read::ZlibDecoder::new(d).read_to_end(&mut out).map_err(|e| GalError(format!("zlib: {e}")))?;
    Ok(out)
}

fn stride(w: u32, bpp: u32) -> usize {
    let s = (w as usize * bpp as usize).div_ceil(8);
    if bpp >= 8 { (s + 3) & !3 } else { s }
}

/// Desempaqueta bloques (BlockWidth/BlockHeight > 0).
fn unpack_blocks(packed: &[u8], w: u32, h: u32, bpp: u32, stride: usize, info: &Info) -> R<Vec<u8>> {
    let (bw, bh) = (info.block_w, info.block_h);
    if bw <= 0 || bh <= 0 {
        return Ok(packed.to_vec());
    }
    let (bw, bh) = (bw as usize, bh as usize);
    let (w, h, bpp) = (w as usize, h as usize, bpp as usize);
    let blocks_w = w.div_ceil(bw);
    let blocks = blocks_w * h.div_ceil(bh);
    let mut r = Rd { d: packed, p: 0 };
    let mut refs = Vec::with_capacity(blocks);
    for _ in 0..blocks {
        refs.push((r.i32()?, r.i32()?));
    }
    let mut data = vec![0u8; stride * h];
    let mut i = 0;
    for y in (0..h).step_by(bh) {
        let rh = bh.min(h - y);
        for x in (0..w).step_by(bw) {
            let (fr, lr) = refs[i];
            let rw = bw.min(w - x);
            let mut dst = y * stride + (x * bpp).div_ceil(8);
            let chunk = (rw * bpp).div_ceil(8);
            match fr {
                -1 => {
                    for _ in 0..rh {
                        let c = r.take(chunk)?;
                        data[dst..dst + chunk].copy_from_slice(c);
                        dst += stride;
                    }
                }
                -2 => {
                    let lr = lr.max(0) as usize;
                    let sx = bw * (lr % blocks_w);
                    let sy = bh * (lr / blocks_w);
                    let mut src = sy * stride + (sx * bpp).div_ceil(8);
                    for _ in 0..rh {
                        data.copy_within(src..src + chunk, dst);
                        src += stride;
                        dst += stride;
                    }
                }
                _ => return err("cross-frame block references are not supported"),
            }
            i += 1;
        }
    }
    Ok(data)
}

fn layer_payload(r: &mut Rd, info: &Info, w: u32, h: u32, bpp: u32) -> R<Vec<u8>> {
    let n = r.usize()?;
    let raw = r.take(n)?;
    let st = stride(w, bpp);
    match info.compression {
        Some(2) => {
            let img = image::load_from_memory_with_format(raw, image::ImageFormat::Jpeg)
                .map_err(|e| GalError(format!("jpeg: {e}")))?
                .to_rgb8();
            // devolvemos en BGR 24bpp con stride alineado para unificar
            let mut out = vec![0u8; stride(w, 24) * h as usize];
            let s24 = stride(w, 24);
            for (x, y, p) in img.enumerate_pixels() {
                if x < w && y < h {
                    let o = y as usize * s24 + x as usize * 3;
                    out[o] = p[2];
                    out[o + 1] = p[1];
                    out[o + 2] = p[0];
                }
            }
            Ok(out)
        }
        Some(0) => unpack_blocks(&inflate(raw)?, w, h, bpp, st, info),
        _ => unpack_blocks(raw, w, h, bpp, st, info),
    }
}

fn alpha_payload(r: &mut Rd, info: &Info, w: u32, h: u32) -> R<Option<Vec<u8>>> {
    let n = r.usize()?;
    if n == 0 {
        return Ok(None);
    }
    let raw = r.take(n)?;
    let st = (w as usize + 3) & !3;
    Ok(Some(unpack_blocks(&inflate(raw)?, w, h, 8, st, info)?))
}

fn parse_gale(d: &[u8]) -> R<(Info, Frame)> {
    let mut r = Rd { d, p: 0 };
    let hdr = r.take(7)?;
    let ver: u32 = std::str::from_utf8(&hdr[4..7]).ok().and_then(|s| s.parse().ok()).ok_or(GalError("bad version".into()))?;
    let mut info = Info::default();
    let frame_count;
    if ver > 102 {
        let hs = r.usize()?;
        let h = r.take(hs)?;
        if h.len() < 0x24 {
            return err("short header");
        }
        let u = |o: usize| u32::from_le_bytes(h[o..o + 4].try_into().unwrap());
        info.w = u(4);
        info.h = u(8);
        info.bpp = u(0xC);
        frame_count = u(0x10) as i32;
        info.randomized = h[0x15] != 0;
        info.compression = Some(h[0x16]);
        info.block_w = u(0x1C) as i32;
        info.block_h = u(0x20) as i32;
    } else if ver >= 100 {
        let h = r.take(0x10)?;
        let u = |o: usize| u32::from_le_bytes(h[o..o + 4].try_into().unwrap());
        let name_len = u(0) as usize;
        r.take(name_len + 17)?;
        info.w = u(4);
        info.h = u(8);
        info.bpp = u(0xC);
        frame_count = 1;
        info.compression = None;
    } else {
        return err(format!("unsupported Gale version {ver}"));
    }
    if info.randomized {
        return err("LiveMaker Pro randomized (encrypted) images are not supported");
    }
    if frame_count < 1 {
        return err("no frames");
    }
    // primer frame
    let nl = r.usize()?;
    let _name = cp932(r.take(nl)?);
    let _mask = r.i32()?;
    r.take(9)?;
    let layer_count = r.usize()?;
    if layer_count < 1 {
        return err("invalid frame (no layers)");
    }
    let w = r.i32()?.max(0) as u32;
    let h = r.i32()?.max(0) as u32;
    let bpp = r.i32()? as u32;
    if ![4, 8, 16, 24, 32].contains(&bpp) {
        return err(format!("unsupported bpp {bpp}"));
    }
    let mut palette = Vec::new();
    if bpp <= 8 {
        let pal = r.take((1usize << bpp) * 4)?;
        palette = pal.chunks(4).map(|c| [c[2], c[1], c[0]]).collect();
    }
    let mut layers = Vec::new();
    for _ in 0..layer_count {
        let left = r.i32()?;
        let top = r.i32()?;
        let visible = r.u8()? != 0;
        let _trans = r.i32()?;
        let alpha = r.i32()?;
        let alpha_on = r.u8()? != 0;
        let nl = r.usize()?;
        r.take(nl)?;
        if ver >= 107 {
            r.u8()?;
        }
        let data = layer_payload(&mut r, &info, w, h, bpp)?;
        let a = alpha_payload(&mut r, &info, w, h)?;
        let a = if alpha_on { a } else { None };
        layers.push((Layer { left, top, visible, alpha, alpha_on }, data, a));
    }
    let bpp = if info.compression == Some(2) { 24 } else { bpp };
    Ok((info, Frame { w, h, bpp, palette, layers }))
}

/// Atributo XML mínimo (el XML de GaleX es simple y plano).
fn attr(tag: &str, key: &str) -> Option<String> {
    let mut rest = tag;
    while let Some(i) = rest.find(key) {
        let before = rest[..i].chars().last();
        let after = &rest[i + key.len()..];
        if before.is_none_or(|c| c.is_whitespace()) {
            let t = after.trim_start();
            if let Some(t) = t.strip_prefix('=') {
                let t = t.trim_start();
                let q = t.chars().next()?;
                if q == '"' || q == '\'' {
                    let end = t[1..].find(q)?;
                    return Some(t[1..1 + end].to_string());
                }
            }
        }
        rest = after;
    }
    None
}

fn tags<'a>(xml: &'a str, name: &str) -> Vec<&'a str> {
    let mut v = Vec::new();
    let pat = format!("<{name}");
    let mut s = xml;
    while let Some(i) = s.find(&pat) {
        let after = &s[i + pat.len()..];
        if after.starts_with(|c: char| c.is_whitespace() || c == '>' || c == '/') {
            let end = after.find('>').unwrap_or(after.len());
            v.push(&after[..end]);
        }
        s = after;
    }
    v
}

fn parse_galex(d: &[u8]) -> R<(Info, Frame)> {
    let mut r = Rd { d, p: 0 };
    let hdr = r.take(8)?;
    if &hdr[4..8] != b"X200" {
        return err("unsupported GaleX version");
    }
    let hs = r.usize()?;
    let xml_raw = inflate(r.take(hs)?)?;
    let xml = cp932(&xml_raw);
    // raíz = primera etiqueta que no sea <?xml ...?> ni comentario
    let root = {
        let mut s = xml.as_str();
        let mut found = "";
        while let Some(i) = s.find('<') {
            let after = &s[i + 1..];
            if !after.starts_with('?') && !after.starts_with('!') {
                found = &after[..after.find('>').unwrap_or(after.len())];
                break;
            }
            s = after;
        }
        found
    };
    let geti = |t: &str, k: &str, def: i64| attr(t, k).and_then(|v| v.parse::<i64>().ok()).unwrap_or(def);
    let mut info = Info {
        w: geti(root, "Width", 0) as u32,
        h: geti(root, "Height", 0) as u32,
        bpp: geti(root, "Bpp", 0) as u32,
        randomized: attr(root, "Randomized").is_some_and(|v| v != "0"),
        compression: Some(geti(root, "CompType", 0) as u8),
        block_w: geti(root, "BlockWidth", 0) as i32,
        block_h: geti(root, "BlockHeight", 0) as i32,
    };
    if info.randomized {
        return err("LiveMaker Pro randomized (encrypted) images are not supported");
    }
    // primer <Frame>: su <Layers> y sus <Layer>
    let find_frame = |from: usize| -> Option<usize> {
        let mut i = from;
        while let Some(j) = xml[i..].find("<Frame") {
            let at = i + j;
            if xml[at + 6..].starts_with(|c: char| c.is_whitespace() || c == '>' || c == '/') {
                return Some(at);
            }
            i = at + 6;
        }
        None
    };
    let frame_start = find_frame(0).ok_or(GalError("no Frame".into()))?;
    let frame_end = find_frame(frame_start + 6).unwrap_or(xml.len());
    let fx = &xml[frame_start..frame_end];
    let layers_tag = tags(fx, "Layers").into_iter().next().unwrap_or("");
    let w = geti(layers_tag, "Width", info.w as i64) as u32;
    let h = geti(layers_tag, "Height", info.h as i64) as u32;
    let bpp = geti(layers_tag, "bpp", info.bpp as i64) as u32;
    info.bpp = bpp;
    let mut palette = Vec::new();
    if bpp <= 8
        && let (Some(a), Some(b)) = (fx.find("<RGB>"), fx.find("</RGB>")) {
            let hex = fx[a + 5..b].trim();
            let bytes: Vec<u8> = (0..hex.len() / 2).filter_map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()).collect();
            palette = bytes.chunks(3).filter(|c| c.len() == 3).map(|c| [c[2], c[1], c[0]]).collect();
        }
    let mut layers = Vec::new();
    r.p = hs + 12;
    for lt in tags(fx, "Layer") {
        let l = Layer {
            left: geti(lt, "Left", 0) as i32,
            top: geti(lt, "Top", 0) as i32,
            visible: geti(lt, "Visible", 1) != 0,
            alpha: geti(lt, "Alpha", 255) as i32,
            alpha_on: geti(lt, "AlphaOn", 0) != 0,
        };
        let data = layer_payload(&mut r, &info, w, h, bpp)?;
        let a = if l.alpha_on { alpha_payload(&mut r, &info, w, h)? } else { None };
        layers.push((l, data, a));
    }
    if layers.is_empty() {
        return err("no layers");
    }
    let bpp = if info.compression == Some(2) { 24 } else { bpp };
    Ok((info, Frame { w, h, bpp, palette, layers }))
}

fn to_rgba(f: &Frame, data: &[u8], alpha: Option<&[u8]>) -> Vec<u8> {
    let (w, h) = (f.w as usize, f.h as usize);
    let st = stride(f.w, f.bpp);
    let ast = (w + 3) & !3;
    let mut out = vec![0u8; w * h * 4];
    let pal = |i: usize| f.palette.get(i).copied().unwrap_or([0, 0, 0]);
    for y in 0..h {
        let row = &data[(y * st).min(data.len())..];
        for x in 0..w {
            let o = (y * w + x) * 4;
            let (r, g, b, a) = match f.bpp {
                32 => row.get(x * 4..x * 4 + 4).map(|p| (p[2], p[1], p[0], p[3])).unwrap_or_default(),
                24 => row.get(x * 3..x * 3 + 3).map(|p| (p[2], p[1], p[0], 255)).unwrap_or_default(),
                16 => row
                    .get(x * 2..x * 2 + 2)
                    .map(|p| {
                        let v = u16::from_le_bytes([p[0], p[1]]);
                        let c = |s: u16| (((v >> s) & 31) as u32 * 255 / 31) as u8;
                        (c(10), c(5), c(0), 255)
                    })
                    .unwrap_or_default(),
                8 => {
                    let c = pal(*row.get(x).unwrap_or(&0) as usize);
                    (c[0], c[1], c[2], 255)
                }
                _ => {
                    let b = *row.get(x / 2).unwrap_or(&0);
                    let i = if x % 2 == 0 { b >> 4 } else { b & 15 };
                    let c = pal(i as usize);
                    (c[0], c[1], c[2], 255)
                }
            };
            let a = match alpha {
                Some(al) => *al.get(y * ast + x).unwrap_or(&255),
                None => a,
            };
            out[o..o + 4].copy_from_slice(&[r, g, b, a]);
        }
    }
    out
}

/// Decodifica un GAL/GaleX completo (primer frame, capas compuestas).
pub fn decode(d: &[u8]) -> R<Image> {
    if !d.starts_with(b"Gale") {
        return err("not a GAL file");
    }
    let (info, frame) = if d.starts_with(b"GaleX") { parse_galex(d)? } else { parse_gale(d)? };
    let (cw, ch) = (info.w.max(frame.w), info.h.max(frame.h));
    if cw == 0 || ch == 0 || (cw as u64 * ch as u64) > 16384 * 16384 {
        return err("invalid dimensions");
    }
    let visible: Vec<_> = frame.layers.iter().filter(|(l, _, _)| l.visible).collect();
    // caso rápido: una sola capa a tamaño completo
    if visible.len() <= 1 {
        let (l, data, a) = visible.first().copied().unwrap_or(&frame.layers[0]);
        let mut px = to_rgba(&frame, data, a.as_deref());
        if l.alpha < 255 {
            for p in px.chunks_mut(4) {
                p[3] = (p[3] as u32 * l.alpha.clamp(0, 255) as u32 / 255) as u8;
            }
        }
        return Ok(Image { w: frame.w, h: frame.h, rgba: px });
    }
    let mut canvas = vec![0u8; (cw * ch * 4) as usize];
    for (l, data, a) in visible {
        let px = to_rgba(&frame, data, a.as_deref());
        let la = l.alpha.clamp(0, 255) as u32;
        for y in 0..frame.h as i32 {
            let cy = y + l.top;
            if cy < 0 || cy >= ch as i32 {
                continue;
            }
            for x in 0..frame.w as i32 {
                let cx = x + l.left;
                if cx < 0 || cx >= cw as i32 {
                    continue;
                }
                let s = ((y as u32 * frame.w + x as u32) * 4) as usize;
                let dpos = ((cy as u32 * cw + cx as u32) * 4) as usize;
                let sa = px[s + 3] as u32 * la / 255;
                let da = canvas[dpos + 3] as u32;
                let oa = sa + da * (255 - sa) / 255;
                for c in 0..3 {
                    let sc = px[s + c] as u32;
                    let dc = canvas[dpos + c] as u32;
                    canvas[dpos + c] = if oa == 0 { 0 } else { ((sc * sa + dc * da * (255 - sa) / 255) / oa) as u8 };
                }
                canvas[dpos + 3] = oa as u8;
            }
        }
    }
    Ok(Image { w: cw, h: ch, rgba: canvas })
}

/// Decodifica GAL o, como alternativa, PNG/JPEG/BMP (juegos extraídos/parcheados).
pub fn decode_any(d: &[u8]) -> R<Image> {
    if d.starts_with(b"Gale") {
        return decode(d);
    }
    let img = image::load_from_memory(d).map_err(|e| GalError(format!("image: {e}")))?.to_rgba8();
    Ok(Image { w: img.width(), h: img.height(), rgba: img.into_raw() })
}

/// Lee solo la cabecera para obtener (ancho, alto). `prefix` puede estar truncado.
pub fn peek_size(d: &[u8]) -> Option<(u32, u32)> {
    if d.starts_with(b"GaleX200") && d.len() > 12 {
        let hs = i32::from_le_bytes(d[8..12].try_into().ok()?) as usize;
        let z = &d[12..d.len().min(12 + hs)];
        let mut out = Vec::new();
        let _ = flate2::read::ZlibDecoder::new(z).take(4096).read_to_end(&mut out);
        let xml = cp932(&out);
        let i = xml.find("Width")?;
        let tag = &xml[i.saturating_sub(1)..];
        let w = attr(tag, "Width")?.parse().ok()?;
        let h = attr(tag, "Height")?.parse().ok()?;
        return Some((w, h));
    }
    if d.starts_with(b"Gale") && d.len() >= 7 + 4 + 12 {
        let ver: u32 = std::str::from_utf8(&d[4..7]).ok()?.parse().ok()?;
        let u = |o: usize| d.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
        return if ver > 102 { Some((u(11 + 4)?, u(11 + 8)?)) } else { Some((u(7 + 4)?, u(7 + 8)?)) };
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gale106_rgb_alpha() {
        let d = std::fs::read(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bg.gal")).unwrap();
        let img = decode(&d).unwrap();
        assert_eq!((img.w, img.h), (4, 3));
        assert_eq!(&img.rgba[0..4], &[255, 0, 0, 255]);
        assert_eq!(&img.rgba[4..8], &[0, 255, 0, 128]);
        assert_eq!(&img.rgba[8..12], &[0, 0, 255, 0]);
        assert_eq!(&img.rgba[12..16], &[255, 255, 255, 255]);
    }
    #[test]
    fn fixtures_extra() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        // GaleX 8bpp con paleta y bloques
        let img = decode(&std::fs::read(dir.join("pal_blocks.galx")).unwrap()).unwrap();
        assert_eq!((img.w, img.h), (5, 3));
        let exp: Vec<u8> = std::fs::read(dir.join("pal_blocks.rgba")).unwrap();
        assert_eq!(img.rgba, exp);
    }
    #[test]
    fn xml_attr() {
        assert_eq!(attr(r#" Width="10" BlockWidth="4""#, "Width").as_deref(), Some("10"));
        assert_eq!(attr(r#" BlockWidth="4" Width="10""#, "Width").as_deref(), Some("10"));
    }
}
