//! Utilidades: E/S posicional multiplataforma, CP932, rutas XDG.

use std::fs::File;
use std::io;
use std::path::PathBuf;

/// Decodifica CP932 (Windows-31J / MS Shift-JIS).
pub fn cp932(b: &[u8]) -> String {
    encoding_rs::SHIFT_JIS
        .decode_without_bom_handling(b)
        .0
        .into_owned()
}

#[cfg(unix)]
pub fn read_at(f: &File, buf: &mut [u8], off: u64) -> io::Result<()> {
    use std::os::unix::fs::FileExt;
    f.read_exact_at(buf, off)
}

#[cfg(windows)]
pub fn read_at(f: &File, buf: &mut [u8], mut off: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    let mut done = 0;
    while done < buf.len() {
        let n = f.seek_read(&mut buf[done..], off)?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        done += n;
        off += n as u64;
    }
    Ok(())
}

fn env_dir(var: &str, fallback: &[&str]) -> PathBuf {
    if let Some(v) = std::env::var_os(var).filter(|v| !v.is_empty()) {
        return PathBuf::from(v);
    }
    #[cfg(windows)]
    if let Some(a) = std::env::var_os("APPDATA") {
        return PathBuf::from(a);
    }
    let mut p = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    for f in fallback {
        p.push(f);
    }
    p
}

pub fn config_dir() -> PathBuf {
    env_dir("XDG_CONFIG_HOME", &[".config"]).join("livenyan")
}

pub fn data_dir() -> PathBuf {
    env_dir("XDG_DATA_HOME", &[".local", "share"]).join("livenyan")
}

/// Slug ASCII estable para nombres de carpeta.
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        // nombres totalmente no-ASCII: hash FNV estable
        let mut h: u64 = 0xcbf29ce484222325;
        for b in s.bytes() {
            h = (h ^ b as u64).wrapping_mul(0x100000001b3);
        }
        format!("game-{h:016x}")
    } else {
        out
    }
}

/// Avisa una sola vez por clave.
pub struct Once(std::sync::Mutex<std::collections::HashSet<String>>);
impl Once {
    pub fn new() -> Self {
        Self(std::sync::Mutex::new(Default::default()))
    }
    pub fn first(&self, key: &str) -> bool {
        self.0.lock().unwrap().insert(key.to_string())
    }
}
impl Default for Once {
    fn default() -> Self {
        Self::new()
    }
}

/// Rectángulo (x, y, w, h) en píxeles de salida donde cae el lienzo lógico con letterbox:
/// escala uniforme máxima que cabe, centrado. (Solo diagnóstico: el centrado real lo hace
/// SDL con SDL_LOGICAL_PRESENTATION_LETTERBOX.)
pub fn letterbox(out: (u32, u32), logical: (u32, u32)) -> (f64, f64, f64, f64) {
    let (ow, oh) = (out.0 as f64, out.1 as f64);
    let (lw, lh) = (logical.0.max(1) as f64, logical.1.max(1) as f64);
    let k = (ow / lw).min(oh / lh);
    let (w, h) = (lw * k, lh * k);
    ((ow - w) / 2.0, (oh - h) / 2.0, w, h)
}

#[cfg(test)]
mod letterbox_tests {
    use super::letterbox;
    #[test]
    fn wide_window_pillarboxes() {
        // 1848x1064 (ventana 1232x709 a densidad 1.5), juego 800x600
        let (x, y, w, h) = letterbox((1848, 1064), (800, 600));
        assert!((h - 1064.0).abs() < 1e-6 && (w - 1418.666).abs() < 0.01);
        assert!((x - 214.666).abs() < 0.01 && y.abs() < 1e-6);
    }
    #[test]
    fn tall_window_letterboxes() {
        let (x, y, w, h) = letterbox((1848, 2124), (1024, 768));
        assert!((w - 1848.0).abs() < 1e-6 && (h - 1386.0).abs() < 1e-6);
        assert!(x.abs() < 1e-6 && (y - 369.0).abs() < 1e-6);
    }
}
