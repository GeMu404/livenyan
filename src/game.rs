//! Apertura de un juego: datos, proyecto (`.lpb`), script de entrada y resolución.

use crate::archive::GameData;
use crate::lsb::{Reader, extended_to_f64};
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
    /// Los 16 bytes sin identificar tras el nombre (unk1/unk2 en pylivemaker), como 4 x i32.
    pub unk: [i32; 4],
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
    let ub = r.take(16).map_err(e)?;
    let mut unk = [0i32; 4];
    for (i, u) in unk.iter_mut().enumerate() {
        *u = i32::from_le_bytes(ub[i * 4..i * 4 + 4].try_into().unwrap());
    }
    let init_lsb = r.string().map_err(e)?;
    let exit_lsb = if version > 0x6D {
        r.string().map_err(e)?
    } else {
        String::new()
    };
    let _dir = r.string().map_err(e)?;
    r.u32().map_err(e)?;
    r.u8().map_err(e)?;
    if version >= 0x6A {
        r.u8().map_err(e)?;
    }
    let audio_formats = if version >= 0x6D {
        r.string().map_err(e)?
    } else {
        String::new()
    };
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
    Ok(Project {
        version,
        name,
        init_lsb,
        exit_lsb,
        audio_formats,
        settings,
        unk,
    })
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
            .and_then(|n| {
                let started = std::time::Instant::now();
                let bytes = data.read(n).ok()?;
                log::info!(
                    "load project {n}: {} bytes in {:.1} ms",
                    bytes.len(),
                    started.elapsed().as_secs_f64() * 1000.0
                );
                Some(bytes)
            })
            .and_then(|d| parse_lpb(&d).map_err(|e| log::warn!("lpb: {e}")).ok());
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "game".into());
        let title = project
            .as_ref()
            .map(|p| p.name.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or(stem.clone());
        let slug = crate::util::slug(&stem);
        let entry = cfg
            .game
            .entry
            .clone()
            .or_else(|| {
                project
                    .as_ref()
                    .map(|p| p.init_lsb.clone())
                    .filter(|s| !s.is_empty())
                    .and_then(|s| resolve_lsb(&data, &s))
            })
            .or_else(|| resolve_lsb(&data, "ゲームメイン.lsb"))
            .or_else(|| resolve_lsb(&data, "00000001.lsb"))
            .or_else(|| {
                data.names()
                    .iter()
                    .find(|n| n.to_lowercase().ends_with(".lsb"))
                    .cloned()
            })
            .ok_or("no .lsb scripts found")?;
        let (logical, logical_source) = if cfg.game.logical_width > 0 && cfg.game.logical_height > 0
        {
            ((cfg.game.logical_width, cfg.game.logical_height), "config")
        } else if let Some(r) = project.as_ref().and_then(lpb_resolution) {
            (r, "lpb-header")
        } else if let Some(r) = project.as_ref().and_then(guess_resolution) {
            (r, "lpb-settings")
        } else if let Some(r) = guess_from_images(&data) {
            (r, "images")
        } else {
            log::warn!(
                "could not determine the game's resolution; using fallback {}x{} (override with --res)",
                DEFAULT_RES.0,
                DEFAULT_RES.1
            );
            (DEFAULT_RES, "fallback")
        };
        log::info!(
            "logical resolution {}x{} from {}",
            logical.0,
            logical.1,
            logical_source
        );
        Ok(Self {
            path: path.to_path_buf(),
            data,
            project,
            title,
            slug,
            entry,
            logical,
            logical_source,
        })
    }

    pub fn save_dir(&self) -> PathBuf {
        crate::util::data_dir().join(&self.slug)
    }

    pub fn translations_dir(&self) -> PathBuf {
        let root = if self.path.is_dir() {
            self.path.as_path()
        } else {
            self.path.parent().unwrap_or_else(|| Path::new("."))
        };
        root.join("translations")
    }
}

pub fn resolve_lsb(data: &GameData, name: &str) -> Option<String> {
    let n = name.trim();
    if n.is_empty() {
        return None;
    }
    // .lsc = fuente de texto del mismo script; en el juego compilado existe como .lsb
    let lower = n.to_lowercase();
    let stem_lsb;
    let n = if lower.ends_with(".lsc") {
        stem_lsb = format!("{}.lsb", &n[..n.len() - 4]);
        stem_lsb.as_str()
    } else {
        n
    };
    data.resolve(n, &[".lsb"]).or_else(|| {
        let base = n.rsplit(['\\', '/']).next().unwrap_or(n);
        data.resolve(base, &[".lsb"])
    })
}

/// Heurística: busca ajustes del proyecto con nombres de ancho/alto de pantalla.
fn guess_resolution(p: &Project) -> Option<(u32, u32)> {
    let find = |keys: &[&str]| {
        p.settings.iter().find_map(|(k, v)| {
            let kl = k.to_lowercase();
            if keys.iter().any(|x| kl.contains(x)) {
                v.parse::<u32>().ok().filter(|&n| (160..=8192).contains(&n))
            } else {
                None
            }
        })
    };
    let w = find(&["screenwidth", "画面幅", "画面横", "width"])?;
    let h = find(&["screenheight", "画面高", "画面縦", "height"])?;
    Some((w, h))
}

/// Heurística: tamaño más frecuente entre imágenes grandes (fondos), leyendo solo cabeceras.
fn guess_from_images(data: &GameData) -> Option<(u32, u32)> {
    let mut counts: std::collections::HashMap<(u32, u32), u32> = Default::default();
    for n in data
        .names()
        .iter()
        .filter(|n| n.to_lowercase().ends_with(".gal"))
    {
        if let Some((w, h)) = data
            .read_prefix(n, 1024)
            .and_then(|p| crate::gal::peek_size(&p))
            && (640..=4096).contains(&w)
            && (400..=4096).contains(&h)
        {
            *counts.entry((w, h)).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|&((w, h), c)| (c, w * h))
        .filter(|&(_, c)| c >= 2)
        .map(|(r, _)| r)
}

/// Tamaño de pantalla desde la cabecera del .lpb (los 16 bytes "unk" de pylivemaker:
/// dos pares ancho/alto). Si difieren se registra y se usa el primero válido.
pub fn lpb_resolution(p: &Project) -> Option<(u32, u32)> {
    let ok = |w: i32, h: i32| (160..=8192).contains(&w) && (120..=8192).contains(&h);
    let a = (p.unk[0], p.unk[1]);
    let b = (p.unk[2], p.unk[3]);
    if ok(a.0, a.1) && ok(b.0, b.1) && a != b {
        log::info!(
            "lpb header has two different sizes {}x{} and {}x{}; using the first",
            a.0,
            a.1,
            b.0,
            b.1
        );
    }
    [a, b]
        .into_iter()
        .find(|&(w, h)| ok(w, h))
        .map(|(w, h)| (w as u32, h as u32))
}

/// Formulario Delphi (TPF0) (solo diagnóstico en --info: suelen ser ventanas del depurador) hallado en los recursos del .exe con ClientWidth/ClientHeight.
#[derive(Debug, Clone)]
pub struct ExeForm {
    pub class: String,
    pub w: u32,
    pub h: u32,
    /// Formulario elegido como ventana principal del juego.
    pub main: bool,
}

/// Busca formularios Delphi en el intérprete (solo la parte anterior al archivo).
/// El tamaño de pantalla de un juego LiveMaker vive en el formulario principal del .exe.
pub fn exe_forms(path: &Path) -> Vec<ExeForm> {
    let exe = if path.is_file() {
        Some(path.to_path_buf())
    } else {
        std::fs::read_dir(path).ok().and_then(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .find(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe")))
        })
    };
    let Some(exe) = exe else { return vec![] };
    let Ok(mut b) = std::fs::read(&exe) else {
        return vec![];
    };
    if !b.starts_with(b"MZ") {
        return vec![];
    }
    // cortar en el archivo anexado (trailer "<u32 offset>lv")
    if b.len() > 6 && &b[b.len() - 2..] == b"lv" {
        let off = u32::from_le_bytes(b[b.len() - 6..b.len() - 2].try_into().unwrap()) as usize;
        if off < b.len() {
            b.truncate(off);
        }
    }
    parse_forms(&b)
}

fn dfm_int(b: &[u8], i: usize) -> Option<i64> {
    match *b.get(i)? {
        2 => Some(*b.get(i + 1)? as i8 as i64),
        3 => Some(i16::from_le_bytes(b.get(i + 1..i + 3)?.try_into().ok()?) as i64),
        4 => Some(i32::from_le_bytes(b.get(i + 1..i + 5)?.try_into().ok()?) as i64),
        _ => None,
    }
}

pub fn parse_forms(b: &[u8]) -> Vec<ExeForm> {
    let find = |hay: &[u8], pat: &[u8]| hay.windows(pat.len()).position(|w| w == pat);
    let mut out = vec![];
    let mut i = 0;
    while let Some(p) = find(&b[i..], b"TPF0") {
        let start = i + p;
        i = start + 4;
        let Some(&cl) = b.get(i) else { break };
        let class = String::from_utf8_lossy(b.get(i + 1..i + 1 + cl as usize).unwrap_or_default())
            .to_string();
        // propiedades del formulario raíz: dentro de una ventana razonable
        let end = (start + 4096).min(b.len());
        let next = find(&b[i..end], b"TPF0").map(|q| i + q).unwrap_or(end);
        let blk = &b[i..next];
        let get = |name: &[u8]| {
            let mut pat = vec![name.len() as u8];
            pat.extend_from_slice(name);
            find(blk, &pat).and_then(|q| dfm_int(blk, q + pat.len()))
        };
        if let (Some(w), Some(h)) = (get(b"ClientWidth"), get(b"ClientHeight")) {
            if (160..=8192).contains(&w) && (120..=8192).contains(&h) {
                out.push(ExeForm {
                    class,
                    w: w as u32,
                    h: h as u32,
                    main: false,
                });
            }
        }
    }
    // principal: clase con "Main"/"Live"/"Novel"; si no, la primera de mayor área
    let pick = out
        .iter()
        .position(|f| {
            ["main", "live", "novel", "game"]
                .iter()
                .any(|k| f.class.to_lowercase().contains(k))
        })
        .or_else(|| {
            out.iter()
                .enumerate()
                .max_by_key(|(_, f)| f.w * f.h)
                .map(|(i, _)| i)
        });
    if let Some(k) = pick {
        out[k].main = true;
    }
    out
}

#[cfg(test)]
mod form_tests {
    #[test]
    fn lpb_header_resolution() {
        let mut p = super::Project::default();
        p.unk = [1024, 768, 1024, 768];
        assert_eq!(super::lpb_resolution(&p), Some((1024, 768)));
        p.unk = [0, 0, 640, 480];
        assert_eq!(super::lpb_resolution(&p), Some((640, 480)));
        p.unk = [0, 0, 0, 0];
        assert_eq!(super::lpb_resolution(&p), None);
    }

    #[test]
    fn finds_main_form() {
        let mut b = b"junkTPF0\x08TAboutDlg\x05About\x0bClientWidth\x03\x40\x01\x0cClientHeight\x03\xc8\x00".to_vec();
        b.extend_from_slice(b"..TPF0\x09TMainForm\x08MainForm\x0bClientWidth\x03\x80\x02\x0cClientHeight\x03\xe0\x01\x00");
        let f = super::parse_forms(&b);
        assert_eq!(f.len(), 2);
        let m = f.iter().find(|f| f.main).unwrap();
        assert_eq!((m.class.as_str(), m.w, m.h), ("TMainForm", 640, 480));
    }
}
