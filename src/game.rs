//! Apertura de un juego: datos, proyecto (`.lpb`), script de entrada y resolución.

use crate::archive::GameData;
use crate::lsb::{extended_to_f64, Reader};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const DEFAULT_RES: (u32, u32) = (800, 600);

#[derive(Debug, Clone, Default)]
pub struct Project {
    pub version: u32,
    pub name: String,
    pub init_lsb: String,
    pub exit_lsb: String,
    pub audio_formats: String,
    /// (nombre, valor) de los ajustes de sistema.
    pub settings: Vec<(String, String)>,
}

/// Port de `livemaker/lpb.py` (LMProject).
pub fn parse_lpb(d: &[u8]) -> Result<Project, String> {
    let mut r = Reader::new(d);
    let e = |x: crate::lsb::ParseError| x.0;
    let version = r.u32().map_err(e)?;
    if !(103..=117).contains(&version) {
        return Err(format!("unsupported lpb version {version}"));
    }
    let name = r.string().map_err(e)?;
    r.take(16).map_err(e)?;
    let init_lsb = r.string().map_err(e)?;
    let exit_lsb = if version > 0x6D { r.string().map_err(e)? } else { String::new() };
    let _dir = r.string().map_err(e)?;
    r.u32().map_err(e)?;
    r.u8().map_err(e)?;
    if version >= 0x6A {
        r.u8().map_err(e)?;
    }
    let audio_formats = if version >= 0x6D { r.string().map_err(e)? } else { String::new() };
    for min in [0x71, 0x72, 0x74] {
        if version >= min {
            r.u8().map_err(e)?;
        }
    }
    r.string().map_err(e)?;
    r.string().map_err(e)?;
    let mut settings = Vec::new();
    if let Ok(n) = r.count() {
        for _ in 0..n {
            let Ok(t) = r.u8() else { break };
            let Ok(k) = r.string() else { break };
            let v = match t {
                1 => r.i32().map(|v| v.to_string()),
                2 => r.take(10).map(|b| extended_to_f64(b).to_string()),
                3 => r.u8().map(|v| v.to_string()),
                _ => r.string(),
            };
            let Ok(v) = v else { break };
            settings.push((k, v));
        }
    }
    Ok(Project { version, name, init_lsb, exit_lsb, audio_formats, settings })
}

pub struct Game {
    pub path: PathBuf,
    pub data: Arc<GameData>,
    pub project: Option<Project>,
    pub title: String,
    pub slug: String,
    pub entry: String,
    pub logical: (u32, u32),
    pub logical_source: &'static str,
}

impl Game {
    pub fn open(path: &Path, cfg: &crate::config::Config) -> Result<Self, String> {
        let data = GameData::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let project = data
            .names()
            .iter()
            .find(|n| n.to_lowercase().ends_with(".lpb"))
            .and_then(|n| data.read(n).ok())
            .and_then(|d| parse_lpb(&d).map_err(|e| log::warn!("lpb: {e}")).ok());
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "game".into());
        let title = project.as_ref().map(|p| p.name.clone()).filter(|s| !s.is_empty()).unwrap_or(stem.clone());
        let slug = crate::util::slug(&stem);
        let entry = cfg
            .game
            .entry
            .clone()
            .or_else(|| project.as_ref().map(|p| p.init_lsb.clone()).filter(|s| !s.is_empty()).and_then(|s| resolve_lsb(&data, &s)))
            .or_else(|| resolve_lsb(&data, "ゲームメイン.lsb"))
            .or_else(|| resolve_lsb(&data, "00000001.lsb"))
            .or_else(|| data.names().iter().find(|n| n.to_lowercase().ends_with(".lsb")).cloned())
            .ok_or("no .lsb scripts found")?;
        let (logical, logical_source) = if cfg.game.logical_width > 0 && cfg.game.logical_height > 0 {
            ((cfg.game.logical_width, cfg.game.logical_height), "config")
        } else if let Some(r) = project.as_ref().and_then(guess_resolution) {
            (r, "lpb")
        } else if let Some(r) = guess_from_images(&data) {
            (r, "images")
        } else {
            (DEFAULT_RES, "default")
        };
        Ok(Self { path: path.to_path_buf(), data, project, title, slug, entry, logical, logical_source })
    }

    pub fn save_dir(&self) -> PathBuf {
        crate::util::data_dir().join(&self.slug)
    }
}

pub fn resolve_lsb(data: &GameData, name: &str) -> Option<String> {
    let n = name.trim();
    if n.is_empty() {
        return None;
    }
    data.resolve(n, &[".lsb"])
        .or_else(|| {
            let base = n.rsplit(['\\', '/']).next().unwrap_or(n);
            data.resolve(base, &[".lsb"])
        })
}

/// Heurística: busca ajustes del proyecto con nombres de ancho/alto de pantalla.
fn guess_resolution(p: &Project) -> Option<(u32, u32)> {
    let find = |keys: &[&str]| {
        p.settings.iter().find_map(|(k, v)| {
            let kl = k.to_lowercase();
            if keys.iter().any(|x| kl.contains(x)) { v.parse::<u32>().ok().filter(|&n| (160..=8192).contains(&n)) } else { None }
        })
    };
    let w = find(&["screenwidth", "画面幅", "画面横", "width"])?;
    let h = find(&["screenheight", "画面高", "画面縦", "height"])?;
    Some((w, h))
}

/// Heurística: tamaño más frecuente entre imágenes grandes (fondos), leyendo solo cabeceras.
fn guess_from_images(data: &GameData) -> Option<(u32, u32)> {
    let mut counts: std::collections::HashMap<(u32, u32), u32> = Default::default();
    for n in data.names().iter().filter(|n| n.to_lowercase().ends_with(".gal")) {
        if let Some((w, h)) = data.read_prefix(n, 1024).and_then(|p| crate::gal::peek_size(&p))
            && (640..=4096).contains(&w) && (400..=4096).contains(&h) {
                *counts.entry((w, h)).or_default() += 1;
            }
    }
    counts.into_iter().max_by_key(|&((w, h), c)| (c, w * h)).filter(|&(_, c)| c >= 2).map(|(r, _)| r)
}
