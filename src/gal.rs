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
    /// Frames adicionales (1..) de una animación GAL, mismo tamaño que `rgba`.
    pub frames: Vec<Vec<u8>>,
    /// Duración de cada frame (ms), incluido el 0; vacío = imagen estática.
    pub delays: Vec<u32>,
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

#[derive(Clone, Copy)]
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
    frames: u32,
    version: String,
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
    /// Duración del frame (ms) si el formato la indica (atributo XML de GaleX).
    delay: Option<u32>,
    /// Gale binario: máscara (4 bytes) + 9 bytes sin documentar de la cabecera del frame
    /// (candidatos a duración del frame); se muestran en --gal-stats.
    raw_hdr: Vec<u8>,
}

fn inflate(d: &[u8]) -> R<Vec<u8>> {
    let mut out = Vec::with_capacity(d.len() * 4);
    match flate2::read::ZlibDecoder::new(d).read_to_end(&mut out) {
        Ok(_) => Ok(out),
        // Algunos GAL traen el flujo zlib sin cerrar (sin el bloque final/Adler32): se usa
        // lo descomprimido; el resto del búfer queda a cero.
        Err(e) if !out.is_empty() => {
            log::debug!("GAL: truncated zlib stream ({e}); using {} bytes", out.len());
            Ok(out)
        }
        Err(e) => {
            // ¿deflate sin cabecera zlib?
            let mut o2 = Vec::new();
            match flate2::read::DeflateDecoder::new(d).read_to_end(&mut o2) {
                Ok(_) if !o2.is_empty() => Ok(o2),
                _ if !o2.is_empty() => Ok(o2),
                _ => Err(GalError(format!("zlib: {e}"))),
            }
        }
    }
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


/// Gale binario: hasta `max` frames (los frames van seguidos en el archivo).
fn parse_gale_all(d: &[u8], max: usize) -> R<(Info, Vec<Frame>)> {
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
    let mut frames: Vec<Frame> = Vec::new();
    for i in 0..(frame_count as usize).min(max.max(1)) {
        if i == 0 {
            frames.push(read_gale_frame(&mut r, &info, ver)?);
            continue;
        }
        // Los frames siguientes no siempre empiezan justo tras el anterior (hay datos
        // intermedios no documentados). Se resincroniza buscando una cabecera de frame
        // plausible (mismo tamaño que el frame 0) en los bytes siguientes.
        let (w0, h0) = (frames[0].w, frames[0].h);
        let mut found = None;
        for off in 0..65536usize {
            let p = r.p + off;
            if p + 40 > d.len() {
                break;
            }
            if !frame_header_ok(d, p, w0, h0) {
                continue;
            }
            let mut t = r;
            t.p = p;
            if let Ok(f) = read_gale_frame(&mut t, &info, ver) {
                found = Some((f, t.p, off));
                break;
            }
        }
        match found {
            Some((f, np, off)) => {
                if off > 0 {
                    log::debug!("GAL frame {i}: resynced +{off} bytes");
                }
                r.p = np;
                frames.push(f);
            }
            None => return err(format!("frame {i}: no valid frame header found")),
        }
    }
    info.frames = frame_count as u32;
    info.version = format!("Gale{ver}");
    Ok((info, frames))
}


/// Comprueba barato si en `p` empieza una cabecera de frame Gale con tamaño w0 x h0.
fn frame_header_ok(d: &[u8], p: usize, w0: u32, h0: u32) -> bool {
    let rd = |o: usize| d.get(o..o + 4).map(|b| i32::from_le_bytes(b.try_into().unwrap()));
    let Some(nl) = rd(p) else { return false };
    if !(0..=256).contains(&nl) {
        return false;
    }
    let q = p + 4 + nl as usize + 4 + 9;
    let (Some(lc), Some(w), Some(h), Some(bpp)) = (rd(q), rd(q + 4), rd(q + 8), rd(q + 12)) else { return false };
    (1..=64).contains(&lc) && w as u32 == w0 && h as u32 == h0 && [4, 8, 16, 24, 32].contains(&bpp)
}

/// Lee un frame Gale (cabecera + capas) en la posición actual.
fn read_gale_frame(r: &mut Rd, info: &Info, ver: u32) -> R<Frame> {
    let nl = r.usize()?;
    let _name = cp932(r.take(nl)?);
    let hdr_start = r.p;
    r.take(13)?;
    let raw_hdr: Vec<u8> = r.d[hdr_start..hdr_start + 13].to_vec();
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
        let data = layer_payload(r, info, w, h, bpp)?;
        let a = alpha_payload(r, info, w, h)?;
        let a = if alpha_on { a } else { None };
        layers.push((Layer { left, top, visible, alpha, alpha_on }, data, a));
    }
    let bpp = if info.compression == Some(2) { 24 } else { bpp };
    Ok(Frame { w, h, bpp, palette, layers, delay: None, raw_hdr })
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


fn parse_galex_all(d: &[u8], max: usize) -> R<(Info, Vec<Frame>)> {
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
        frames: geti(root, "Count", 1) as u32,
        version: "GaleX200".into(),
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
    let mut frames = Vec::new();
    r.p = hs + 12;
    let base_bpp = info.bpp;
    let mut next = find_frame(0);
    if next.is_none() {
        return err("no Frame");
    }
    while let Some(frame_start) = next {
    if frames.len() >= max.max(1) {
        break;
    }
    let frame_end = find_frame(frame_start + 6).unwrap_or(xml.len());
    next = find_frame(frame_start + 6);
    let fx = &xml[frame_start..frame_end];
    let ftag = &fx[..fx.find('>').unwrap_or(fx.len())];
    let fdelay = ["Delay", "Wait", "Interval", "Time"].iter().find_map(|k| attr(ftag, k)).and_then(|v| v.parse::<u32>().ok());
    let layers_tag = tags(fx, "Layers").into_iter().next().unwrap_or("");
    let w = geti(layers_tag, "Width", info.w as i64) as u32;
    let h = geti(layers_tag, "Height", info.h as i64) as u32;
    let bpp = geti(layers_tag, "bpp", base_bpp as i64) as u32;
    info.bpp = bpp;
    let mut palette = Vec::new();
    if bpp <= 8
        && let (Some(a), Some(b)) = (fx.find("<RGB>"), fx.find("</RGB>")) {
            let hex = fx[a + 5..b].trim();
            let bytes: Vec<u8> = (0..hex.len() / 2).filter_map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()).collect();
            palette = bytes.chunks(3).filter(|c| c.len() == 3).map(|c| [c[2], c[1], c[0]]).collect();
        }
    let mut layers = Vec::new();
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
    frames.push(Frame { w, h, bpp, palette, layers, delay: fdelay, raw_hdr: vec![] });
    }
    Ok((info, frames))
}

/// Estadísticas de un GAL sin exponer su contenido: versión, frames, capas, bpp,
/// compresión, tamaño y medias de color/alfa por capa (para diagnosticar en el PC del
/// usuario sin ver la imagen).
pub fn quick_class(d: &[u8]) -> String {
    match decode(d) {
        Err(e) => format!("error: {e}"),
        Ok(img) => {
            let mut opaque_black = true;
            let mut transparent = true;
            for p in img.rgba.chunks_exact(4) {
                if p[3] != 0 {
                    transparent = false;
                    if p[0] | p[1] | p[2] != 0 {
                        opaque_black = false;
                    }
                }
                if !opaque_black && !transparent {
                    return if img.frames.is_empty() { "ok".into() } else { format!("ok ({} frames)", img.frames.len() + 1) };
                }
            }
            if !img.frames.is_empty() {
                return format!("ok ({} frames, frame 0 {})", img.frames.len() + 1, if transparent { "transparent" } else { "black" });
            }
            (if transparent { "all-transparent" } else { "all-black" }).into()
        }
    }
}

pub fn stats(d: &[u8]) -> R<String> {
    if !d.starts_with(b"Gale") {
        return err("not a GAL file");
    }
    let all = |m| if d.starts_with(b"GaleX") { parse_galex_all(d, m) } else { parse_gale_all(d, m) };
    let (mut note, (info, frames)) = match all(64) {
        Ok(v) => (String::new(), v),
        Err(e) => (format!("note: only frame 0 parsed; later frames failed: {e}\n"), all(1)?),
    };
    let comp = match info.compression {
        Some(0) => "0 (zlib)".to_string(),
        Some(2) => "2 (jpeg)".to_string(),
        Some(c) => format!("{c} (raw)"),
        None => "none (old Gale)".to_string(),
    };
    let mut out = format!(
        "version {} size {}x{} bpp {} frames {} (parsed {}) compression {} blocks {}x{}\n",
        info.version, info.w, info.h, info.bpp, info.frames, frames.len(), comp, info.block_w, info.block_h
    );
    out += &std::mem::take(&mut note);
    for (fi, f) in frames.iter().enumerate() {
        out += &format!(
            "frame {fi}: {}x{} bpp {} palette {} layers {} delay {:?} hdr {:02x?}\n",
            f.w, f.h, f.bpp, f.palette.len(), f.layers.len(), f.delay, f.raw_hdr
        );
        for (li, (l, data, a)) in f.layers.iter().enumerate() {
            let px = to_rgba(f, data, a.as_deref());
            let n = (px.len() / 4).max(1) as u64;
            let (mut r, mut g, mut b, mut al, mut nz) = (0u64, 0u64, 0u64, 0u64, 0u64);
            for p in px.chunks_exact(4) {
                r += p[0] as u64;
                g += p[1] as u64;
                b += p[2] as u64;
                al += p[3] as u64;
                if p[0] | p[1] | p[2] != 0 {
                    nz += 1;
                }
            }
            out += &format!(
                "  layer {li}: pos ({},{}) visible {} alpha {} alpha_on {} mask {} | data {} B | avg rgb ({},{},{}) avg alpha {} | non-black px {:.1}%\n",
                l.left, l.top, l.visible, l.alpha, l.alpha_on, a.is_some(), data.len(), r / n, g / n, b / n, al / n, nz as f64 * 100.0 / n as f64
            );
        }
    }
    Ok(out)
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

/// Compone las capas visibles de un frame en RGBA.
fn compose(info: &Info, frame: &Frame) -> R<(u32, u32, Vec<u8>)> {
    let (cw, ch) = (info.w.max(frame.w), info.h.max(frame.h));
    if cw == 0 || ch == 0 || (cw as u64 * ch as u64) > 16384 * 16384 {
        return err("invalid dimensions");
    }
    let visible: Vec<_> = frame.layers.iter().filter(|(l, _, _)| l.visible).collect();
    // caso rápido: una sola capa a tamaño completo
    if visible.len() <= 1 {
        let (l, data, a) = visible.first().copied().unwrap_or(&frame.layers[0]);
        let mut px = to_rgba(frame, data, a.as_deref());
        if l.alpha < 255 {
            for p in px.chunks_mut(4) {
                p[3] = (p[3] as u32 * l.alpha.clamp(0, 255) as u32 / 255) as u8;
            }
        }
        return Ok((frame.w, frame.h, px));
    }
    let mut canvas = vec![0u8; (cw * ch * 4) as usize];
    for (l, data, a) in visible {
        let px = to_rgba(frame, data, a.as_deref());
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
    Ok((cw, ch, canvas))
}

/// Duración por defecto de un frame de animación GAL cuando el archivo no la indica (ms).
pub const DEFAULT_FRAME_MS: u32 = 100;

/// Decodifica un GAL/GaleX: todos los frames (animación) con sus capas compuestas.
pub fn decode(d: &[u8]) -> R<Image> {
    if !d.starts_with(b"Gale") {
        return err("not a GAL file");
    }
    let x = d.starts_with(b"GaleX");
    let all = |m| if x { parse_galex_all(d, m) } else { parse_gale_all(d, m) };
    let (info, frames) = match all(1024) {
        Ok(v) => v,
        Err(e) => {
            log::debug!("GAL: later frames unreadable ({e}); using frame 0 only");
            all(1)?
        }
    };
    let (w, h, rgba) = compose(&info, &frames[0])?;
    // Gale binario: los bytes 4..8 de la cabecera de frame (tras la máscara) son la duración
    // del frame en ms (visto en un juego real: 67 ms y una pausa de 2333 ms).
    let delay_of = |f: &Frame| {
        f.delay.or_else(|| {
            f.raw_hdr.get(4..8).map(|b| u32::from_le_bytes(b.try_into().unwrap())).filter(|d| (1..=600_000).contains(d))
        })
    };
    let mut extra = vec![];
    let mut delays = vec![];
    if frames.len() > 1 {
        delays.push(delay_of(&frames[0]).unwrap_or(DEFAULT_FRAME_MS));
        for f in &frames[1..] {
            match compose(&info, f) {
                Ok((fw, fh, px)) if fw == w && fh == h => {
                    extra.push(px);
                    delays.push(delay_of(f).unwrap_or(DEFAULT_FRAME_MS));
                }
                _ => break,
            }
        }
        if extra.is_empty() {
            delays.clear();
        }
    }
    Ok(Image { w, h, rgba, frames: extra, delays })
}

/// Decodifica GAL o, como alternativa, PNG/JPEG/BMP (juegos extraídos/parcheados).
pub fn decode_any(d: &[u8]) -> R<Image> {
    if d.starts_with(b"Gale") {
        return decode(d);
    }
    let img = image::load_from_memory(d).map_err(|e| GalError(format!("image: {e}")))?.to_rgba8();
    Ok(Image { w: img.width(), h: img.height(), rgba: img.into_raw(), frames: vec![], delays: vec![] })
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
    /// Gale106 de 3 frames con bytes intermedios no documentados entre frames 1 y 2
    /// (como en juegos reales): se deben leer los 3 frames.
    #[test]
    fn gale_multiframe_with_gap() {
        use std::io::Write;
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let base = std::fs::read(dir.join("render2/pic.gal")).unwrap();
        let hs = u32::from_le_bytes(base[7..11].try_into().unwrap()) as usize;
        let mut hdr = base[11..11 + hs].to_vec();
        hdr[0x10..0x14].copy_from_slice(&3u32.to_le_bytes());
        let frame = |bgr: [u8; 3], name: &str| {
            let mut f = vec![];
            f.extend((name.len() as u32).to_le_bytes());
            f.extend(name.as_bytes());
            f.extend([0u8; 13]);
            f.extend(1u32.to_le_bytes());
            f.extend(16u32.to_le_bytes());
            f.extend(8u32.to_le_bytes());
            f.extend(24u32.to_le_bytes());
            f.extend(0i32.to_le_bytes());
            f.extend(0i32.to_le_bytes());
            f.push(1);
            f.extend((-1i32).to_le_bytes());
            f.extend(255i32.to_le_bytes());
            f.push(0);
            f.extend(0u32.to_le_bytes());
            let raw: Vec<u8> = bgr.iter().copied().cycle().take(16 * 3 * 8).collect();
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            z.write_all(&raw).unwrap();
            let z = z.finish().unwrap();
            f.extend((z.len() as u32).to_le_bytes());
            f.extend(&z);
            f.extend(0u32.to_le_bytes());
            f
        };
        let mut d = b"Gale106".to_vec();
        d.extend((hs as u32).to_le_bytes());
        d.extend(&hdr);
        d.extend(frame([0, 0, 0], "Frame0"));
        d.extend(frame([0, 0, 255], "Frame1"));
        d.extend(b"\x05\x00\x00\x00junk!\x00\x00");
        d.extend(frame([0, 255, 0], "Frame2"));
        let img = decode(&d).unwrap();
        assert_eq!(&img.rgba[0..4], &[0, 0, 0, 255]);
        assert_eq!(img.frames.len(), 2);
        assert_eq!(&img.frames[0][0..4], &[255, 0, 0, 255]);
        assert_eq!(&img.frames[1][0..4], &[0, 255, 0, 255]);
        assert_eq!(img.delays, vec![DEFAULT_FRAME_MS; 3]);
        assert!(stats(&d).unwrap().contains("frames 3 (parsed 3)"));
    }
    #[test]
    fn truncated_zlib_tolerated() {
        use std::io::Write;
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(&[7u8; 1000]).unwrap();
        let z = z.finish().unwrap();
        let out = inflate(&z[..z.len() - 6]).unwrap();
        assert!(!out.is_empty() && out.iter().all(|&b| b == 7));
    }
    #[test]
    fn xml_attr() {
        assert_eq!(attr(r#" Width="10" BlockWidth="4""#, "Width").as_deref(), Some("10"));
        assert_eq!(attr(r#" BlockWidth="4" Width="10""#, "Width").as_deref(), Some("10"));
    }
}
