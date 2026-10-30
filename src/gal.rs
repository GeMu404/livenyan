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
    /// TransColor de la capa (TColor 0x00BBGGRR; -1 = ninguno).
    trans: i32,
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
            log::debug!(
                "GAL: truncated zlib stream ({e}); using {} bytes",
                out.len()
            );
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
/// Fuente de bloques de otros frames (referencias entre frames: animaciones que reutilizan
/// bloques sin cambios). `alpha` elige el canal alfa en vez del color.
#[derive(Clone, Copy)]
struct Prev<'a> {
    frames: &'a [Frame],
    alpha: bool,
    /// Capas ya leídas del frame en curso (índice de frame = frames.len()).
    cur: &'a [LayerData],
}

impl Prev<'_> {
    fn block_src(&self, fr: i32, lr: i32) -> Option<&[u8]> {
        let fr = usize::try_from(fr).ok()?;
        let layers: &[LayerData] = if fr == self.frames.len() {
            self.cur
        } else {
            &self.frames.get(fr)?.layers
        };
        let (_, d, a) = layers.get(usize::try_from(lr).ok()?)?;
        if self.alpha {
            a.as_deref()
        } else {
            Some(d.as_slice())
        }
    }
}

fn unpack_blocks(
    packed: &[u8],
    w: u32,
    h: u32,
    bpp: u32,
    stride: usize,
    info: &Info,
    prev: Prev,
) -> R<Vec<u8>> {
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
                _ => {
                    // bloque igual al del frame `fr`, capa `lr`, misma posición
                    let Some(src) = prev.block_src(fr, lr) else {
                        return err(format!(
                            "block references frame {fr} layer {lr}, not available"
                        ));
                    };
                    for _ in 0..rh {
                        if dst + chunk > src.len() {
                            return err("cross-frame block out of range");
                        }
                        data[dst..dst + chunk].copy_from_slice(&src[dst..dst + chunk]);
                        dst += stride;
                    }
                }
            }
            i += 1;
        }
    }
    Ok(data)
}

type LayerData = (Layer, Vec<u8>, Option<Vec<u8>>);

fn layer_payload(
    r: &mut Rd,
    info: &Info,
    w: u32,
    h: u32,
    bpp: u32,
    prev: &[Frame],
    cur: &[LayerData],
) -> R<Vec<u8>> {
    let prev = Prev {
        frames: prev,
        alpha: false,
        cur,
    };
    let n = r.usize()?;
    if n == 0 {
        // capa vacía (GaleX: capas sin contenido en este frame)
        return Ok(Vec::new());
    }
    if info.compression == Some(0) {
        let packed = zlib_block(r, n)?;
        return unpack_blocks(&packed, w, h, bpp, stride(w, bpp), info, prev);
    }
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
        _ => unpack_blocks(raw, w, h, bpp, st, info, prev),
    }
}

fn alpha_payload(
    r: &mut Rd,
    info: &Info,
    w: u32,
    h: u32,
    prev: &[Frame],
    cur: &[LayerData],
) -> R<Option<Vec<u8>>> {
    let n = r.usize()?;
    if n == 0 {
        return Ok(None);
    }
    // GARbro: el alfa va en zlib con CompType 0 y 2 (JPEG); con otros tipos, bloques crudos
    let packed = if matches!(info.compression, Some(0) | Some(2)) {
        zlib_block(r, n)?
    } else {
        r.take(n)?.to_vec()
    };
    let st = (w as usize + 3) & !3;
    Ok(Some(unpack_blocks(
        &packed,
        w,
        h,
        8,
        st,
        info,
        Prev {
            frames: prev,
            alpha: true,
            cur,
        },
    )?))
}

/// Bloque zlib de tamaño declarado `n`. Si ese tamaño no cuadra con el flujo (flujo
/// incompleto: el campo de tamaño no cubre el flujo entero), se descomprime en modo
/// streaming desde el inicio hasta el fin real del flujo y el lector avanza hasta ahí.
fn zlib_block(r: &mut Rd, n: usize) -> R<Vec<u8>> {
    let start = r.p;
    if let Ok((out, used)) = inflate_stream(r.d.get(start..start + n).unwrap_or(&r.d[start..]))
        && used <= n
    {
        r.p = start + n;
        return Ok(out);
    }
    match inflate_stream(&r.d[start..]) {
        Ok((out, used)) => {
            log::debug!("GAL: zlib block declared {n} B, stream really {used} B; resyncing");
            r.p = start + used.max(n.min(r.d.len() - start));
            if used > n {
                r.p = start + used;
            }
            Ok(out)
        }
        Err(e) => {
            // último recurso: tolerante (salida parcial)
            let raw = r.take(n)?;
            inflate(raw).map_err(|e2| GalError(format!("{e2} (streaming: {e})")))
        }
    }
}

/// Descomprime un flujo zlib hasta su final real; devuelve (datos, bytes consumidos).
fn inflate_stream(d: &[u8]) -> R<(Vec<u8>, usize)> {
    let mut z = flate2::Decompress::new(true);
    let mut out = Vec::with_capacity(d.len() * 4 + 1024);
    loop {
        if out.capacity() - out.len() < 4096 {
            out.reserve(out.capacity().max(65536));
        }
        let before_in = z.total_in();
        let before_out = z.total_out();
        let st = z
            .decompress_vec(
                &d[z.total_in() as usize..],
                &mut out,
                flate2::FlushDecompress::None,
            )
            .map_err(|e| GalError(format!("zlib: {e}")))?;
        if st == flate2::Status::StreamEnd {
            return Ok((out, z.total_in() as usize));
        }
        if z.total_in() == before_in && z.total_out() == before_out {
            return err("zlib: incomplete deflate stream");
        }
    }
}

/// Gale binario: hasta `max` frames (los frames van seguidos en el archivo).
fn parse_gale_all(d: &[u8], max: usize) -> R<(Info, Vec<Frame>)> {
    let mut r = Rd { d, p: 0 };
    let hdr = r.take(7)?;
    let ver: u32 = std::str::from_utf8(&hdr[4..7])
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or(GalError("bad version".into()))?;
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
            frames.push(read_gale_frame(&mut r, &info, ver, &[])?);
            continue;
        }
        // Los frames siguientes no siempre empiezan justo tras el anterior (hay datos
        // intermedios no documentados). Se resincroniza buscando una cabecera de frame
        // plausible (mismo tamaño que el frame 0) en los bytes siguientes.
        let (w0, h0) = (frames[0].w, frames[0].h);
        let mut found = None;
        let mut last_err = String::from("no candidate header");
        for off in 0..65536usize {
            let p = r.p + off;
            if p + 24 > d.len() {
                break;
            }
            if !frame_header_ok(d, p, w0, h0) {
                continue;
            }
            let mut t = r;
            t.p = p;
            match read_gale_frame(&mut t, &info, ver, &frames) {
                Ok(f) => {
                    found = Some((f, t.p, off));
                    break;
                }
                Err(e) => last_err = format!("candidate at +{off}: {e}"),
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
            None => {
                return err(format!(
                    "frame {i}: no valid frame header found ({last_err})"
                ));
            }
        }
    }
    info.frames = frame_count as u32;
    info.version = format!("Gale{ver}");
    Ok((info, frames))
}

/// Comprueba barato si en `p` empieza una cabecera de frame Gale con tamaño w0 x h0.
fn frame_header_ok(d: &[u8], p: usize, w0: u32, h0: u32) -> bool {
    let rd = |o: usize| {
        d.get(o..o + 4)
            .map(|b| i32::from_le_bytes(b.try_into().unwrap()))
    };
    let Some(nl) = rd(p) else { return false };
    if !(0..=256).contains(&nl) {
        return false;
    }
    let q = p + 4 + nl as usize + 4 + 9;
    let (Some(lc), Some(w), Some(h), Some(bpp)) = (rd(q), rd(q + 4), rd(q + 8), rd(q + 12)) else {
        return false;
    };
    let sane =
        |v: i32, v0: u32| v as u32 == v0 || (1..=(v0.max(1) as i32 * 4).max(64)).contains(&v);
    (1..=64).contains(&lc) && sane(w, w0) && sane(h, h0) && [1, 4, 8, 16, 24, 32].contains(&bpp)
}

/// Lee un frame Gale (cabecera + capas) en la posición actual.
fn read_gale_frame(r: &mut Rd, info: &Info, ver: u32, prev: &[Frame]) -> R<Frame> {
    let nl = r.usize()?;
    let _name = cp932(r.take(nl)?);
    let hdr_start = r.p;
    r.take(13)?;
    let mut raw_hdr: Vec<u8> = r.d[hdr_start..hdr_start + 13].to_vec();
    // Bytes 0..4: máscara de transparencia en orden B,G,R (igual que el framebuffer
    // 24bpp). Layer/GaleX TransColor usa TColor 0x00BBGGRR (R en el byte bajo).
    // Normalizar aquí para que [`apply_key`] compare RGB de forma uniforme.
    // **experimento:** btn.gal del novel system (matte RGB 236,94,96; cabecera `60 5e ec`).
    if raw_hdr.len() >= 3 {
        raw_hdr.swap(0, 2);
    }
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
        let trans = r.i32()?;
        let alpha = r.i32()?;
        let alpha_on = r.u8()? != 0;
        let nl = r.usize()?;
        r.take(nl)?;
        if ver >= 107 {
            r.u8()?;
        }
        let data = layer_payload(r, info, w, h, bpp, prev, &layers)?;
        let a = alpha_payload(r, info, w, h, prev, &layers)?;
        let a = if alpha_on { a } else { None };
        layers.push((
            Layer {
                left,
                top,
                visible,
                alpha,
                alpha_on,
                trans,
            },
            data,
            a,
        ));
    }
    let bpp = if info.compression == Some(2) { 24 } else { bpp };
    Ok(Frame {
        w,
        h,
        bpp,
        palette,
        layers,
        delay: None,
        raw_hdr,
    })
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
    let geti = |t: &str, k: &str, def: i64| {
        attr(t, k)
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(def)
    };
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
        let fdelay = ["Delay", "Wait", "Interval", "Time"]
            .iter()
            .find_map(|k| attr(ftag, k))
            .and_then(|v| v.parse::<u32>().ok());
        let layers_tag = tags(fx, "Layers").into_iter().next().unwrap_or("");
        let w = geti(layers_tag, "Width", info.w as i64) as u32;
        let h = geti(layers_tag, "Height", info.h as i64) as u32;
        let bpp = geti(layers_tag, "bpp", base_bpp as i64) as u32;
        info.bpp = bpp;
        let mut palette = frames
            .last()
            .map(|f: &Frame| f.palette.clone())
            .unwrap_or_default();
        let pal_src: &str = if fx.contains("<RGB>") { fx } else { &xml };
        if bpp <= 8
            && let (Some(a), Some(b)) = (pal_src.find("<RGB>"), pal_src.find("</RGB>"))
        {
            let fx = pal_src;
            let hex = fx[a + 5..b].trim();
            let bytes: Vec<u8> = (0..hex.len() / 2)
                .filter_map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok())
                .collect();
            palette = bytes
                .chunks(3)
                .filter(|c| c.len() == 3)
                .map(|c| [c[2], c[1], c[0]])
                .collect();
        }
        let mut layers = Vec::new();
        for lt in tags(fx, "Layer") {
            let l = Layer {
                left: geti(lt, "Left", 0) as i32,
                top: geti(lt, "Top", 0) as i32,
                visible: geti(lt, "Visible", 1) != 0,
                alpha: geti(lt, "Alpha", 255) as i32,
                alpha_on: geti(lt, "AlphaOn", 0) != 0,
                trans: geti(lt, "TransColor", -1) as i32,
            };
            let data = layer_payload(&mut r, &info, w, h, bpp, &frames, &layers)?;
            // GaleX siempre escribe el tamaño del payload alfa (0 si AlphaOn=0), igual
            // que Gale binario; omitirlo desalinea capas/frames siguientes.
            let a = alpha_payload(&mut r, &info, w, h, &frames, &layers)?;
            let a = if l.alpha_on { a } else { None };
            layers.push((l, data, a));
        }
        if layers.is_empty() {
            return err("no layers");
        }
        let bpp = if info.compression == Some(2) { 24 } else { bpp };
        // BGColor (GaleX) no se usa como color clave; TransColor del frame sí (como la máscara
        // de la cabecera binaria; -1 = ninguno)
        let ftrans = attr(ftag, "TransColor")
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(-1) as i32;
        frames.push(Frame {
            w,
            h,
            bpp,
            palette,
            layers,
            delay: fdelay,
            raw_hdr: ftrans.to_le_bytes().to_vec(),
        });
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
            for p in img.rgba.as_chunks::<4>().0 {
                if p[3] != 0 {
                    transparent = false;
                    if p[0] | p[1] | p[2] != 0 {
                        opaque_black = false;
                    }
                }
                if !opaque_black && !transparent {
                    return if img.frames.is_empty() {
                        "ok".into()
                    } else {
                        format!("ok ({} frames)", img.frames.len() + 1)
                    };
                }
            }
            if !img.frames.is_empty() {
                return format!(
                    "ok ({} frames, frame 0 {})",
                    img.frames.len() + 1,
                    if transparent { "transparent" } else { "black" }
                );
            }
            (if transparent {
                "all-transparent"
            } else {
                "all-black"
            })
            .into()
        }
    }
}

/// Estructura binaria de un Gale (solo offsets, tamaños y campos de cabecera; nada de
/// píxeles): para diagnosticar formatos raros sin compartir la imagen.
pub fn dump(d: &[u8]) -> String {
    let mut o = String::new();
    if d.starts_with(b"GaleX") {
        let rd = |p: usize| {
            d.get(p..p + 4)
                .map(|b| i32::from_le_bytes(b.try_into().unwrap()))
        };
        let hs = rd(8).unwrap_or(0).max(0) as usize;
        o += &format!("dump: GaleX file {} B, xml header @12 size {hs}\n", d.len());
        let Some(zx) = d.get(12..12 + hs) else {
            return o + "  truncated header\n";
        };
        let xml = match inflate(zx) {
            Ok(x) => cp932(&x),
            Err(e) => return o + &format!("  xml: {e}\n"),
        };
        let strip_names = |t: &str| {
            // atributos sin Name (los nombres de capa podrían ser texto del juego)
            t.split_whitespace()
                .filter(|kv| !kv.starts_with("Name"))
                .collect::<Vec<_>>()
                .join(" ")
        };
        for t in tags(&xml, "Frames") {
            o += &format!("  <Frames {}>\n", strip_names(t));
        }
        let comp = tags(&xml, "Frames")
            .first()
            .and_then(|t| attr(t, "CompType"))
            .and_then(|v| v.parse::<i32>().ok())
            .unwrap_or(0);
        let mut p = 12 + hs;
        for (fi, fx) in xml
            .split("<Frame")
            .skip(1)
            .filter(|x| x.starts_with(|c: char| c.is_whitespace() || c == '>'))
            .enumerate()
        {
            o += &format!(
                "  frame {fi}: <Frame {}>\n",
                strip_names(&fx[..fx.find('>').unwrap_or(0)])
            );
            for lt in tags(fx, "Layers") {
                o += &format!("    <Layers {}>\n", strip_names(lt));
            }
            for (li, lt) in tags(fx, "Layer").into_iter().enumerate() {
                o += &format!("    layer {li}: <Layer {}>\n", strip_names(lt));
                for what in ["data", "alpha"] {
                    let Some(n) = rd(p) else {
                        return o + &format!("  EOF @{p}\n");
                    };
                    let body = (p + 4).min(d.len());
                    let mut extra = String::new();
                    if n > 0 && (comp == 0 || (what == "alpha" && comp == 2)) {
                        extra = match inflate_stream(&d[body..]) {
                            Ok((out, used)) => format!(
                                " | zlib stream {used} B -> {} B{}",
                                out.len(),
                                if used as i64 != n as i64 {
                                    " (MISMATCH)"
                                } else {
                                    ""
                                }
                            ),
                            Err(e) => format!(
                                " | zlib: {e} (first bytes {:02x?})",
                                &d[body..(body + 2).min(d.len())]
                            ),
                        };
                    }
                    o += &format!("      {what} size @{p} = {n}{extra}\n");
                    if n < 0 {
                        return o;
                    }
                    p = body + n as usize;
                }
            }
        }
        return o + &format!("  walk stopped @{p} of {}\n", d.len());
    }
    let rd = |p: usize| {
        d.get(p..p + 4)
            .map(|b| i32::from_le_bytes(b.try_into().unwrap()))
    };
    let ver: u32 = d
        .get(4..7)
        .and_then(|v| std::str::from_utf8(v).ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    o += &format!("dump: file {} B, version {ver}\n", d.len());
    if ver <= 102 {
        return o + "  (old Gale, not dumped)\n";
    }
    let hs = rd(7).unwrap_or(0).max(0) as usize;
    let h = |off: usize| rd(11 + off).unwrap_or(-1);
    o += &format!(
        "  header @7 size {hs}: w {} h {} bpp {} frames {} randomized {} compression {} blocks {}x{}\n",
        h(4),
        h(8),
        h(0xC),
        h(0x10),
        d.get(11 + 0x15).copied().unwrap_or(0),
        d.get(11 + 0x16).copied().unwrap_or(0),
        h(0x1C),
        h(0x20)
    );
    let comp = d.get(11 + 0x16).copied();
    let frames = h(0x10).max(0) as usize;
    let mut p = 11 + hs;
    let mut f = 0;
    'outer: while f < frames.min(64) && p < d.len() {
        let start = p;
        let Some(nl) = rd(p) else { break };
        if !(0..=256).contains(&nl) {
            o += &format!("  frame {f} @{p}: implausible name length {nl}; stopping walk\n");
            break;
        }
        p += 4 + nl as usize;
        let hdr: Vec<u8> = d.get(p..p + 13).map(|x| x.to_vec()).unwrap_or_default();
        p += 13;
        let (lc, w, hh, bpp) = (
            rd(p).unwrap_or(-1),
            rd(p + 4).unwrap_or(-1),
            rd(p + 8).unwrap_or(-1),
            rd(p + 12).unwrap_or(-1),
        );
        o += &format!(
            "  frame {f} @{start}: name_len {nl} hdr {hdr:02x?} layers {lc} size {w}x{hh} bpp {bpp}\n"
        );
        p += 16;
        if (1..=8).contains(&bpp) {
            p += (1usize << bpp) * 4;
        }
        if !(1..=64).contains(&lc) {
            break;
        }
        for li in 0..lc {
            let ls = p;
            let (left, top) = (rd(p).unwrap_or(0), rd(p + 4).unwrap_or(0));
            let vis = d.get(p + 8).copied().unwrap_or(0);
            let alpha_on = d.get(p + 17).copied().unwrap_or(0);
            p += 18;
            let lnl = rd(p).unwrap_or(0).max(0) as usize;
            p += 4 + lnl + usize::from(ver >= 107);
            for what in ["data", "alpha"] {
                let Some(n) = rd(p) else { break 'outer };
                let body = p + 4;
                let mut extra = String::new();
                if n > 0 && (comp == Some(0) || what == "alpha") {
                    match inflate_stream(&d[body.min(d.len())..]) {
                        Ok((out, used)) => {
                            extra = format!(
                                " | zlib stream {used} B -> {} B{}",
                                out.len(),
                                if used as i64 != n as i64 {
                                    " (MISMATCH)"
                                } else {
                                    ""
                                }
                            )
                        }
                        Err(e) => extra = format!(" | zlib: {e}"),
                    }
                }
                o += &format!(
                    "    layer {li} @{ls}: pos ({left},{top}) visible {vis} alpha_on {alpha_on} name_len {lnl} | {what} size @{p} = {n}{extra}\n"
                );
                if n < 0 {
                    break 'outer;
                }
                p = body + n as usize;
            }
        }
        o += &format!("  frame {f} ends @{p}\n");
        // ¿empieza otro frame justo aquí? si no, candidatos cercanos
        if f + 1 < frames {
            let mut cands = vec![];
            for off in 0..65536usize {
                let q = p + off;
                if q + 24 > d.len() || cands.len() >= 5 {
                    break;
                }
                let Some(nl) = rd(q) else { break };
                if !(0..=256).contains(&nl) {
                    continue;
                }
                let r0 = q + 4 + nl as usize + 13;
                let (Some(lc), Some(w2), Some(h2), Some(b2)) =
                    (rd(r0), rd(r0 + 4), rd(r0 + 8), rd(r0 + 12))
                else {
                    continue;
                };
                if (1..=64).contains(&lc)
                    && (1..=16384).contains(&w2)
                    && (1..=16384).contains(&h2)
                    && [1, 4, 8, 16, 24, 32].contains(&b2)
                {
                    cands.push(format!(
                        "+{off} (name_len {nl} layers {lc} {w2}x{h2} bpp {b2})"
                    ));
                }
            }
            o += &format!(
                "  next-frame header candidates after @{p}: {}\n",
                if cands.is_empty() {
                    "none".to_string()
                } else {
                    cands.join(", ")
                }
            );
            if let Some(first) = cands.first()
                && let Some(off) = first
                    .strip_prefix('+')
                    .and_then(|x| x.split(' ').next())
                    .and_then(|x| x.parse::<usize>().ok())
            {
                p += off;
            } else {
                break;
            }
        }
        f += 1;
    }
    o += &format!("  walk stopped @{p} of {}\n", d.len());
    o
}

pub fn stats(d: &[u8]) -> R<String> {
    if !d.starts_with(b"Gale") {
        return err("not a GAL file");
    }
    let all = |m| {
        if d.starts_with(b"GaleX") {
            parse_galex_all(d, m)
        } else {
            parse_gale_all(d, m)
        }
    };
    let (mut note, (info, frames)) = match all(64) {
        Ok(v) => (String::new(), v),
        Err(e) => (
            format!("note: only frame 0 parsed; later frames failed: {e}\n"),
            all(1)?,
        ),
    };
    let comp = match info.compression {
        Some(0) => "0 (zlib)".to_string(),
        Some(2) => "2 (jpeg)".to_string(),
        Some(c) => format!("{c} (raw)"),
        None => "none (old Gale)".to_string(),
    };
    let mut out = format!(
        "version {} size {}x{} bpp {} frames {} (parsed {}) compression {} blocks {}x{}\n",
        info.version,
        info.w,
        info.h,
        info.bpp,
        info.frames,
        frames.len(),
        comp,
        info.block_w,
        info.block_h
    );
    out += &std::mem::take(&mut note);
    for (fi, f) in frames.iter().enumerate() {
        out += &format!(
            "frame {fi}: {}x{} bpp {} palette {} layers {} delay {:?} hdr {:02x?}\n",
            f.w,
            f.h,
            f.bpp,
            f.palette.len(),
            f.layers.len(),
            f.delay,
            f.raw_hdr
        );
        for (li, (l, data, a)) in f.layers.iter().enumerate() {
            let mut px = to_rgba(f, data, a.as_deref());
            let key = color_key(f, l, a.is_some());
            apply_key_frame(f, data, &mut px, key);
            let n = (px.len() / 4).max(1) as u64;
            let (mut r, mut g, mut b, mut al, mut nz) = (0u64, 0u64, 0u64, 0u64, 0u64);
            for p in px.as_chunks::<4>().0 {
                r += p[0] as u64;
                g += p[1] as u64;
                b += p[2] as u64;
                al += p[3] as u64;
                if p[0] | p[1] | p[2] != 0 {
                    nz += 1;
                }
            }
            out += &format!(
                "  layer {li}: pos ({},{}) visible {} alpha {} alpha_on {} mask {} colorkey {} | data {} B | avg rgb ({},{},{}) avg alpha {} | non-black px {:.1}%\n",
                l.left,
                l.top,
                l.visible,
                l.alpha,
                l.alpha_on,
                a.is_some(),
                key.map(|k| format!("{k:06x}")).unwrap_or("none".into()),
                data.len(),
                r / n,
                g / n,
                b / n,
                al / n,
                nz as f64 * 100.0 / n as f64
            );
        }
    }
    Ok(out)
}

/// Color clave (transparente) de una capa sin canal alfa: TransColor de la capa o, si no
/// hay, la máscara de la cabecera del frame (bytes 0..4; BGColor en GaleX). -1 = ninguno.
fn color_key(f: &Frame, l: &Layer, has_alpha: bool) -> Option<u32> {
    if has_alpha {
        return None;
    }
    // -1 / 0xFFFFFFFF / 0x00FFFFFF = ninguno. Con bpp <= 8 el valor es un índice de paleta.
    let valid =
        |v: u32| v != 0xFFFF_FFFF && v != 0x00FF_FFFF && v >> 24 == 0 && (f.bpp > 8 || v < 256);
    if valid(l.trans as u32) {
        return Some(l.trans as u32);
    }
    let m = f
        .raw_hdr
        .get(0..4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))?;
    valid(m).then_some(m)
}

/// Aplica el color clave: con paleta compara índices; si no, TColor 0x00BBGGRR.
fn apply_key_frame(f: &Frame, data: &[u8], px: &mut [u8], key: Option<u32>) {
    let Some(k) = key else { return };
    if f.bpp <= 8 {
        let st = stride(f.w, f.bpp);
        for y in 0..f.h as usize {
            for x in 0..f.w as usize {
                let idx = if f.bpp == 8 {
                    data.get(y * st + x).copied()
                } else {
                    data.get(y * st + x / 2)
                        .map(|b| if x % 2 == 0 { b >> 4 } else { b & 15 })
                };
                if idx == Some(k as u8) {
                    px[(y * f.w as usize + x) * 4 + 3] = 0;
                }
            }
        }
        return;
    }
    apply_key(px, Some(k));
}

fn apply_key(px: &mut [u8], key: Option<u32>) {
    let Some(k) = key else { return };
    let (r, g, b) = (
        (k & 0xFF) as u8,
        ((k >> 8) & 0xFF) as u8,
        ((k >> 16) & 0xFF) as u8,
    );
    for p in px.chunks_mut(4) {
        if p[0] == r && p[1] == g && p[2] == b {
            p[3] = 0;
        }
    }
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
                32 => row
                    .get(x * 4..x * 4 + 4)
                    .map(|p| (p[2], p[1], p[0], p[3]))
                    .unwrap_or_default(),
                24 => row
                    .get(x * 3..x * 3 + 3)
                    .map(|p| (p[2], p[1], p[0], 255))
                    .unwrap_or_default(),
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
fn compose(info: &Info, frame: &Frame, previous: Option<&[u8]>) -> R<(u32, u32, Vec<u8>)> {
    let (cw, ch) = (info.w.max(frame.w), info.h.max(frame.h));
    if cw == 0 || ch == 0 || (cw as u64 * ch as u64) > 16384 * 16384 {
        return err("invalid dimensions");
    }
    if frame.layers.iter().all(|(_, data, _)| data.is_empty())
        && let Some(previous) = previous
        && previous.len() == (frame.w as usize * frame.h as usize * 4)
    {
        return Ok((frame.w, frame.h, previous.to_vec()));
    }
    let visible: Vec<_> = frame
        .layers
        .iter()
        .filter(|(l, d, _)| l.visible && !d.is_empty())
        .collect();
    // caso rápido: una sola capa a tamaño completo
    if visible.is_empty() {
        return Ok((
            frame.w,
            frame.h,
            vec![0u8; (frame.w * frame.h * 4) as usize],
        ));
    }
    if visible.len() == 1 && visible[0].0.left == 0 && visible[0].0.top == 0 {
        let (l, data, a) = visible.first().copied().unwrap_or(&frame.layers[0]);
        let mut px = to_rgba(frame, data, a.as_deref());
        apply_key_frame(frame, data, &mut px, color_key(frame, l, a.is_some()));
        if l.alpha < 255 {
            for p in px.chunks_mut(4) {
                p[3] = (p[3] as u32 * l.alpha.clamp(0, 255) as u32 / 255) as u8;
            }
        }
        return Ok((frame.w, frame.h, px));
    }
    let mut canvas = vec![0u8; (cw * ch * 4) as usize];
    for (l, data, a) in visible {
        let mut px = to_rgba(frame, data, a.as_deref());
        apply_key_frame(frame, data, &mut px, color_key(frame, l, a.is_some()));
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
                    canvas[dpos + c] = (sc * sa + dc * da * (255 - sa) / 255)
                        .checked_div(oa)
                        .unwrap_or(0) as u8;
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
    let all = |m| {
        if x {
            parse_galex_all(d, m)
        } else {
            parse_gale_all(d, m)
        }
    };
    let (info, frames) = match all(1024) {
        Ok(v) => v,
        Err(e) => {
            log::debug!("GAL: later frames unreadable ({e}); using frame 0 only");
            all(1)?
        }
    };
    let (w, h, rgba) = compose(&info, &frames[0], None)?;
    // Gale binario: los bytes 4..8 de la cabecera de frame (tras la máscara) son la duración
    // del frame en ms (**experimento**, H23).
    let delay_of = |f: &Frame| {
        f.delay.or_else(|| {
            f.raw_hdr
                .get(4..8)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                .filter(|d| (1..=600_000).contains(d))
        })
    };
    let mut extra = vec![];
    let mut delays = vec![];
    if frames.len() > 1 {
        delays.push(delay_of(&frames[0]).unwrap_or(DEFAULT_FRAME_MS));
        let mut previous = rgba.clone();
        for f in &frames[1..] {
            match compose(&info, f, Some(&previous)) {
                Ok((fw, fh, px)) if fw == w && fh == h => {
                    previous = px.clone();
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
    Ok(Image {
        w,
        h,
        rgba,
        frames: extra,
        delays,
    })
}

/// Codifica RGBA8 → GaleX200 24bpp zlib (CompType 0), legible por [`decode`].
/// Formato de escritura mínimo de livenyan (**no** es el encoder del LiveMaker original).
pub fn encode_galex_rgba(w: u32, h: u32, rgba: &[u8]) -> R<Vec<u8>> {
    use std::io::Write;
    let need = (w as usize).saturating_mul(h as usize).saturating_mul(4);
    if w == 0 || h == 0 || rgba.len() < need {
        return err("invalid rgba for GaleX encode");
    }
    let st = stride(w, 24);
    let mut bgr = vec![0u8; st.saturating_mul(h as usize)];
    for y in 0..h as usize {
        for x in 0..w as usize {
            let i = (y * w as usize + x) * 4;
            let o = y * st + x * 3;
            bgr[o] = rgba[i + 2];
            bgr[o + 1] = rgba[i + 1];
            bgr[o + 2] = rgba[i];
        }
    }
    let zlib = |data: &[u8]| -> R<Vec<u8>> {
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(data)
            .map_err(|e| GalError(format!("zlib: {e}")))?;
        enc.finish().map_err(|e| GalError(format!("zlib: {e}")))
    };
    let xml = format!(
        concat!(
            r#"<?xml version="1.0"?>"#,
            r#"<Frames Version="200" Width="{w}" Height="{h}" Bpp="24" Count="1" "#,
            r#"Randomized="0" CompType="0" BlockWidth="0" BlockHeight="0">"#,
            r#"<Frame TransColor="-1" Delay="0">"#,
            r#"<Layers Count="1" Width="{w}" Height="{h}" Bpp="24">"#,
            r#"<Layer Left="0" Top="0" Visible="1" TransColor="-1" Alpha="255" AlphaOn="0"/>"#,
            r#"</Layers></Frame></Frames>"#
        ),
        w = w,
        h = h
    );
    let zx = zlib(xml.as_bytes())?;
    let zpixels = zlib(&bgr)?;
    let mut out = b"GaleX200".to_vec();
    out.extend_from_slice(&(zx.len() as u32).to_le_bytes());
    out.extend(&zx);
    out.extend_from_slice(&(zpixels.len() as u32).to_le_bytes());
    out.extend(&zpixels);
    out.extend_from_slice(&0u32.to_le_bytes()); // sin canal alfa
    Ok(out)
}

/// Decodifica GAL o, como alternativa, PNG/JPEG/BMP (juegos extraídos/parcheados).
pub fn decode_any(d: &[u8]) -> R<Image> {
    if d.starts_with(b"Gale") {
        return decode(d);
    }
    let img = image::load_from_memory(d)
        .map_err(|e| GalError(format!("image: {e}")))?
        .to_rgba8();
    Ok(Image {
        w: img.width(),
        h: img.height(),
        rgba: img.into_raw(),
        frames: vec![],
        delays: vec![],
    })
}

/// Lee solo la cabecera para obtener (ancho, alto). `prefix` puede estar truncado.
pub fn peek_size(d: &[u8]) -> Option<(u32, u32)> {
    if d.starts_with(b"GaleX200") && d.len() > 12 {
        let hs = i32::from_le_bytes(d[8..12].try_into().ok()?) as usize;
        let z = &d[12..d.len().min(12 + hs)];
        let mut out = Vec::new();
        let _ = flate2::read::ZlibDecoder::new(z)
            .take(4096)
            .read_to_end(&mut out);
        let xml = cp932(&out);
        let i = xml.find("Width")?;
        let tag = &xml[i.saturating_sub(1)..];
        let w = attr(tag, "Width")?.parse().ok()?;
        let h = attr(tag, "Height")?.parse().ok()?;
        return Some((w, h));
    }
    if d.starts_with(b"Gale") && d.len() >= 7 + 4 + 12 {
        let ver: u32 = std::str::from_utf8(&d[4..7]).ok()?.parse().ok()?;
        let u = |o: usize| {
            d.get(o..o + 4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        };
        return if ver > 102 {
            Some((u(11 + 4)?, u(11 + 8)?))
        } else {
            Some((u(7 + 4)?, u(7 + 8)?))
        };
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_animation_frame_keeps_previous_pixels() {
        let info = Info {
            w: 1,
            h: 1,
            ..Info::default()
        };
        let layer = Layer {
            left: 0,
            top: 0,
            visible: true,
            alpha: 255,
            alpha_on: false,
            trans: -1,
        };
        let empty = Frame {
            w: 1,
            h: 1,
            bpp: 24,
            palette: vec![],
            layers: vec![(layer, vec![], None)],
            delay: None,
            raw_hdr: vec![],
        };
        let previous = [255, 0, 0, 255];

        assert_eq!(compose(&info, &empty, Some(&previous)).unwrap().2, previous);
        assert_eq!(compose(&info, &empty, None).unwrap().2, [0, 0, 0, 0]);
    }

    #[test]
    fn galex_omitted_frame_retains_previous_image() {
        use std::io::Write;
        let z = |data: &[u8]| {
            let mut encoder =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(data).unwrap();
            encoder.finish().unwrap()
        };
        let xml = r#"<Frames Version="200" Width="1" Height="1" Bpp="8" Count="2" Randomized="0" CompType="0" BlockWidth="0" BlockHeight="0"><Frame Delay="67" TransColor="-1"><Layers Width="1" Height="1" Bpp="8"><RGB>000000FF0000</RGB><Layer Left="0" Top="0" Visible="1" TransColor="-1" Alpha="255" AlphaOn="0"/></Layers></Frame><Frame Delay="67" TransColor="-1"><Layers Width="1" Height="1" Bpp="8"><RGB>000000FF0000</RGB><Layer Left="0" Top="0" Visible="1" TransColor="-1" Alpha="255" AlphaOn="0"/></Layers></Frame></Frames>"#;
        let zx = z(xml.as_bytes());
        let mut data = b"GaleX200".to_vec();
        data.extend((zx.len() as u32).to_le_bytes());
        data.extend(zx);
        let pixels = z(&[1, 0, 0, 0]);
        data.extend((pixels.len() as u32).to_le_bytes());
        data.extend(pixels);
        data.extend(0u32.to_le_bytes()); // alpha frame 0
        data.extend(0u32.to_le_bytes()); // data frame 1 omitted
        data.extend(0u32.to_le_bytes()); // alpha frame 1

        let image = decode(&data).unwrap();
        assert_eq!(image.frames.len(), 1);
        assert_eq!(image.frames[0], image.rgba);
        assert_eq!(image.delays, [67, 67]);
    }

    #[test]
    fn galex_rgba_roundtrip() {
        let (w, h) = (3u32, 2u32);
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                rgba.extend_from_slice(&[x as u8 * 40, y as u8 * 80, 200, 255]);
            }
        }
        let gal = encode_galex_rgba(w, h, &rgba).unwrap();
        let img = decode(&gal).unwrap();
        assert_eq!((img.w, img.h), (w, h));
        assert_eq!(img.rgba, rgba);
    }

    #[test]
    fn gale106_rgb_alpha() {
        let d = std::fs::read(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bg.gal"),
        )
        .unwrap();
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
            f.extend([0xffu8, 0xff, 0xff, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
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
    /// Frames con bloques que referencian otro frame + tamaño zlib declarado demasiado corto.
    #[test]
    fn gale_cross_frame_blocks_and_short_zlib_size() {
        use std::io::Write;
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let base = std::fs::read(dir.join("render2/pic.gal")).unwrap();
        let hs = u32::from_le_bytes(base[7..11].try_into().unwrap()) as usize;
        let mut hdr = base[11..11 + hs].to_vec();
        hdr[0x10..0x14].copy_from_slice(&2u32.to_le_bytes());
        hdr[0x1C..0x20].copy_from_slice(&8u32.to_le_bytes());
        hdr[0x20..0x24].copy_from_slice(&8u32.to_le_bytes());
        // 16x8, 24bpp, bloques 8x8 -> 2 bloques; fila de bloque = 24 B, stride 48
        let frame = |refs: [(i32, i32); 2], fills: &[[u8; 3]], name: &str, short: bool| {
            let mut packed = vec![];
            for (a, b) in refs {
                packed.extend(a.to_le_bytes());
                packed.extend(b.to_le_bytes());
            }
            for c in fills {
                packed.extend(c.iter().copied().cycle().take(24 * 8));
            }
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            z.write_all(&packed).unwrap();
            let z = z.finish().unwrap();
            let mut f = vec![];
            f.extend((name.len() as u32).to_le_bytes());
            f.extend(name.as_bytes());
            f.extend([0xffu8, 0xff, 0xff, 0xff, 0x43, 0, 0, 0, 2, 0, 0, 0, 0]);
            for v in [1u32, 16, 8, 24] {
                f.extend(v.to_le_bytes());
            }
            f.extend(0i32.to_le_bytes());
            f.extend(0i32.to_le_bytes());
            f.push(1);
            f.extend((-1i32).to_le_bytes());
            f.extend(255i32.to_le_bytes());
            f.push(0);
            f.extend(0u32.to_le_bytes());
            f.extend((if short { 5 } else { z.len() } as u32).to_le_bytes());
            f.extend(&z);
            f.extend(0u32.to_le_bytes());
            f
        };
        let mut d = b"Gale106".to_vec();
        d.extend((hs as u32).to_le_bytes());
        d.extend(&hdr);
        d.extend(frame(
            [(-1, 0), (-1, 0)],
            &[[0, 0, 255], [0, 255, 0]],
            "F0",
            false,
        ));
        d.extend(frame([(0, 0), (-1, 0)], &[[255, 0, 0]], "F1", true));
        let img = decode(&d).unwrap();
        assert_eq!(img.frames.len(), 1);
        // bloque izquierdo copiado del frame 0 (rojo), derecho nuevo (azul)
        assert_eq!(&img.frames[0][0..4], &[255, 0, 0, 255]);
        assert_eq!(&img.frames[0][8 * 4..8 * 4 + 4], &[0, 0, 255, 255]);
        assert!(dump(&d).contains("(MISMATCH)"));
    }

    /// GaleX 8bpp: capa vacía (0 B) + TransColor del frame como índice de paleta.
    #[test]
    fn galex_empty_layer_and_index_key() {
        use std::io::Write;
        let z = |b: &[u8]| {
            let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            e.write_all(b).unwrap();
            e.finish().unwrap()
        };
        let xml = r#"<?xml version="1.0"?><Frames Version="200" Width="4" Height="2" Bpp="8" Count="1" SyncPal="1" Randomized="0" CompType="0" BGColor="16777215" BlockWidth="0" BlockHeight="0"><Frame TransColor="1" Delay="400" Count="1" Count="0"><Layers Count="2" Width="4" Height="2" Bpp="8"><RGB>0000FF00FF00</RGB><Layer Left="0" Top="0" Visible="1" TransColor="-1" Alpha="255" AlphaOn="0"/><Layer Left="0" Top="0" Visible="1" TransColor="-1" Alpha="255" AlphaOn="0"/></Layers></Frame></Frames>"#;
        let zx = z(xml.as_bytes());
        let mut d = b"GaleX200".to_vec();
        d.extend((zx.len() as u32).to_le_bytes());
        d.extend(&zx);
        d.extend(0u32.to_le_bytes()); // capa 0 vacía
        d.extend(0u32.to_le_bytes()); // alpha capa 0
        let px = z(&[0, 1, 0, 1, 1, 0, 1, 0]);
        d.extend((px.len() as u32).to_le_bytes());
        d.extend(&px);
        d.extend(0u32.to_le_bytes()); // alpha capa 1
        let img = decode(&d).unwrap();
        let alphas: Vec<u8> = img.rgba.chunks(4).map(|p| p[3]).collect();
        assert_eq!(alphas, vec![255, 0, 255, 0, 0, 255, 0, 255]);
    }

    /// GaleX con AlphaOn=0 sigue trayendo un u32 de tamaño alfa (0); sin leerlo
    /// se pierden frames con bloques entre frames (p.ej. icono 滞在中 del mapa).
    #[test]
    fn galex_alpha_size_present_when_alpha_on_is_zero() {
        use std::io::Write;
        let z = |b: &[u8]| {
            let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            e.write_all(b).unwrap();
            e.finish().unwrap()
        };
        // 2x2, 8bpp, BlockWidth=2 → 1 bloque; frame1 copia el bloque del frame0 capa0
        let xml = r#"<?xml version="1.0"?><Frames Version="200" Width="2" Height="2" Bpp="8" Count="2" SyncPal="1" Randomized="0" CompType="0" BlockWidth="2" BlockHeight="2"><Frame TransColor="-1" Delay="33"><Layers Count="1" Width="2" Height="2" Bpp="8"><RGB>000000FFFFFF</RGB><Layer Left="0" Top="0" Visible="1" TransColor="-1" Alpha="255" AlphaOn="0"/></Layers></Frame><Frame TransColor="-1" Delay="33"><Layers Count="1" Width="2" Height="2" Bpp="8"><Layer Left="0" Top="0" Visible="1" TransColor="-1" Alpha="255" AlphaOn="0"/></Layers></Frame></Frames>"#;
        let zx = z(xml.as_bytes());
        let mut d = b"GaleX200".to_vec();
        d.extend((zx.len() as u32).to_le_bytes());
        d.extend(&zx);
        let mut packed = Vec::new();
        packed.extend((-1i32).to_le_bytes());
        packed.extend(0i32.to_le_bytes());
        packed.extend([1u8, 1, 1, 1]);
        let pz = z(&packed);
        d.extend((pz.len() as u32).to_le_bytes());
        d.extend(&pz);
        d.extend(0u32.to_le_bytes());
        let mut packed = Vec::new();
        packed.extend(0i32.to_le_bytes());
        packed.extend(0i32.to_le_bytes());
        let pz = z(&packed);
        d.extend((pz.len() as u32).to_le_bytes());
        d.extend(&pz);
        d.extend(0u32.to_le_bytes());

        let img = decode(&d).unwrap();
        assert_eq!(
            img.frames.len(),
            1,
            "second frame must parse via cross-frame blocks"
        );
        assert_eq!(img.delays, [33, 33]);
        assert_eq!(&img.rgba[..4], &[255, 255, 255, 255]);
        assert_eq!(&img.frames[0][..4], &[255, 255, 255, 255]);
        assert!(stats(&d).unwrap().contains("frames 2 (parsed 2)"));
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
        assert_eq!(
            attr(r#" Width="10" BlockWidth="4""#, "Width").as_deref(),
            Some("10")
        );
        assert_eq!(
            attr(r#" BlockWidth="4" Width="10""#, "Width").as_deref(),
            Some("10")
        );
    }

    #[test]
    #[ignore = "necesita LIVENYAN_TEST_GAME_DIR (carpeta data de un juego LiveNovel)"]
    fn novel_btn_gal_colorkey_is_transparent() {
        let dir = std::env::var("LIVENYAN_TEST_GAME_DIR").expect("LIVENYAN_TEST_GAME_DIR");
        let data = std::fs::read(std::path::Path::new(&dir).join("ノベルシステム/画像/btn.gal"))
            .expect("ノベルシステム/画像/btn.gal en LIVENYAN_TEST_GAME_DIR");
        let img = decode_any(&data).expect("btn.gal");
        let mut transparent = 0usize;
        let mut leftover_key = 0usize;
        for p in img.rgba.chunks(4) {
            if p[3] == 0 {
                transparent += 1;
            } else if p[0] == 236 && p[1] == 94 && p[2] == 96 {
                leftover_key += 1;
            }
        }
        assert!(
            transparent > 0,
            "btn.gal Gale106 mask should key out the red matte"
        );
        assert_eq!(leftover_key, 0, "keyed matte must not stay opaque");
    }
}
