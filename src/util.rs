//! Utilidades: E/S posicional multiplataforma, CP932, rutas XDG.

use std::fs::File;
use std::io;
use std::path::PathBuf;

/// Decodifica CP932 (Windows-31J / MS Shift-JIS).
pub fn cp932(b: &[u8]) -> String {
    encoding_rs::SHIFT_JIS.decode_without_bom_handling(b).0.into_owned()
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
    let mut p = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
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
