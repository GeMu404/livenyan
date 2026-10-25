//! Lector del archivo VF de LiveMaker (port de pylivemaker `livemaker/archive.py`).
//!
//! Soporta: archivo anexado a un `.exe` (trailer `<u32 offset>lv`), `.dat` independiente,
//! índices partidos `.ext`/`.dat` + `.001`, `.002`… y directorios extraídos.
//! Nunca escribe en los ficheros del juego.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::util::{cp932, read_at};

pub const SPLIT_PART_SIZE: u64 = 1 << 30;
const XOR_SEED: u32 = 0x75D6_EE39;
const SCRAMBLE_KEY: u32 = 0xF8EA;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compress {
    Zlib = 0,
    None = 1,
    Encrypted = 2,
    EncryptedZlib = 3,
    Unknown = 255,
}

impl From<u8> for Compress {
    fn from(v: u8) -> Self {
        match v {
            0 => Compress::Zlib,
            1 => Compress::None,
            2 => Compress::Encrypted,
            3 => Compress::EncryptedZlib,
            _ => Compress::Unknown,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub offset: u64,
    pub size: u64,
    pub compress: Compress,
    pub unk1: u32,
    pub checksum: u32,
    pub encrypt_flag: u8,
}

/// Generador de claves `TTpRandom`.
struct Obfuscator {
    key: u32,
    seed: u32,
}
impl Obfuscator {
    fn new() -> Self {
        Self {
            key: 0,
            seed: XOR_SEED,
        }
    }
    fn next(&mut self) -> u32 {
        self.key = (self.key << 2)
            .wrapping_add(self.key)
            .wrapping_add(self.seed);
        self.key
    }
}

struct Cursor<'a> {
    d: &'a [u8],
    p: usize,
}
impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        if self.p + n > self.d.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "VF directory truncated",
            ));
        }
        let s = &self.d[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }
}

fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

/// Parsea el directorio VF desde `buf` (que empieza en la firma "vf").
pub fn parse_directory(buf: &[u8]) -> io::Result<(u32, Vec<Entry>)> {
    let mut c = Cursor { d: buf, p: 0 };
    if c.take(2)? != b"vf" {
        return Err(bad("missing 'vf' signature"));
    }
    let version = c.u32()?;
    if version < 100 {
        return Err(bad(format!("unsupported VF version {version}")));
    }
    let count = c.u32()? as usize;
    if count > 10_000_000 {
        return Err(bad("absurd entry count"));
    }
    let mut ob = Obfuscator::new();
    let mut names = Vec::with_capacity(count);
    for _ in 0..count {
        let len = c.u32()? as usize;
        let raw: Vec<u8> = c.take(len)?.iter().map(|b| b ^ (ob.next() as u8)).collect();
        names.push(cp932(&raw));
    }
    // offset_low y offset_high usan cada uno su propio keystream (igual que pylivemaker).
    let mut ob_lo = Obfuscator::new();
    let mut ob_hi = Obfuscator::new();
    let mut offsets = Vec::with_capacity(count + 1);
    for _ in 0..=count {
        let lo = c.u32()? ^ ob_lo.next();
        let hi_raw = c.u32()?;
        let hi = if version >= 101 {
            let x = hi_raw ^ ob_hi.next();
            if x & 0x8000_0000 != 0 { 0xFFFF_FFFF } else { 0 }
        } else {
            hi_raw
        };
        offsets.push((((hi & 0x8000_0000) as u64) << 32) | lo as u64);
    }
    let mut comp = Vec::with_capacity(count);
    for _ in 0..count {
        comp.push(Compress::from(c.u8()?));
    }
    let mut unk1 = Vec::with_capacity(count);
    for _ in 0..count {
        unk1.push(c.u32()?);
    }
    let mut sums = Vec::with_capacity(count);
    for _ in 0..count {
        sums.push(c.u32()?);
    }
    let mut enc = Vec::with_capacity(count);
    for _ in 0..count {
        enc.push(c.u8()?);
    }
    let entries = (0..count)
        .map(|i| Entry {
            name: std::mem::take(&mut names[i]),
            offset: offsets[i],
            size: offsets[i + 1].saturating_sub(offsets[i]),
            compress: comp[i],
            unk1: unk1[i],
            checksum: sums[i],
            encrypt_flag: enc[i],
        })
        .collect();
    Ok((version, entries))
}

/// Checksum VF (CRC32 estándar sobre datos comprimidos).
pub fn checksum(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// PRNG `TScramble` de LM3 (para entradas cifradas de LiveMaker Pro).
struct Scramble {
    state: [u32; 5],
}
impl Scramble {
    fn new(seed: u32) -> Self {
        let mut seed = if seed == 0 { 0xFFFF_FFFF } else { seed };
        let mut state = [0u32; 5];
        for s in state.iter_mut() {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            *s = seed;
        }
        let mut r = Self { state };
        for _ in 0..19 {
            r.rand();
        }
        r
    }
    fn rand(&mut self) -> u32 {
        let s = &mut self.state;
        let x = (s[3] as u64)
            .wrapping_mul(0x7DD4_FFC7)
            .wrapping_add((s[2] as u64).wrapping_mul(0x5D4))
            .wrapping_add((s[1] as u64).wrapping_mul(0x6F0))
            .wrapping_add((s[0] as u64).wrapping_mul(0x13FB))
            .wrapping_add(s[4] as u64);
        s[4] = (x >> 32) as u32;
        s[3] = s[2];
        s[2] = s[1];
        s[1] = s[0];
        s[0] = x as u32;
        s[0]
    }
    fn randint(&mut self, low: usize, high: usize) -> usize {
        let f = self.rand() as f64 / 4294967296.0;
        low + (f * (high - low + 1) as f64) as usize
    }
}

fn randseq(count: usize, seed: u32) -> Vec<usize> {
    let mut sc = Scramble::new(seed);
    let mut values: Vec<usize> = (0..count).collect();
    let mut seq = vec![0; count];
    let mut i = 0;
    while !values.is_empty() {
        let n = if values.len() == 1 {
            0
        } else {
            sc.randint(0, values.len() - 2)
        };
        seq[values.remove(n)] = i;
        i += 1;
    }
    seq
}

pub fn unscramble(data: &[u8]) -> io::Result<Vec<u8>> {
    if data.len() < 8 {
        return Ok(data.to_vec());
    }
    let chunk = i32::from_le_bytes(data[0..4].try_into().unwrap());
    let seed = u32::from_le_bytes(data[4..8].try_into().unwrap());
    if chunk <= 0 {
        return Err(bad("bad scramble chunk size"));
    }
    let chunk = chunk as usize;
    let body = &data[8..];
    let total = body.len().div_ceil(chunk);
    let mut out = Vec::with_capacity(body.len());
    for i in randseq(total, seed ^ SCRAMBLE_KEY) {
        let off = i * chunk;
        out.extend_from_slice(&body[off.min(body.len())..(off + chunk).min(body.len())]);
    }
    if out.len() != body.len() {
        return Err(bad("scramble data mismatch"));
    }
    Ok(out)
}

pub fn decompress(entry: &Entry, raw: Vec<u8>) -> io::Result<Vec<u8>> {
    let data = match entry.compress {
        Compress::Encrypted | Compress::EncryptedZlib => unscramble(&raw)?,
        Compress::Unknown => {
            return Err(bad(format!("unsupported compression for {}", entry.name)));
        }
        _ => raw,
    };
    match entry.compress {
        Compress::Zlib | Compress::EncryptedZlib => {
            let mut out = Vec::with_capacity(data.len() * 3);
            flate2::read::ZlibDecoder::new(&data[..]).read_to_end(&mut out)?;
            Ok(out)
        }
        _ => Ok(data),
    }
}

/// Archivo VF abierto (solo lectura).
pub struct VfArchive {
    pub path: PathBuf,
    pub version: u32,
    pub is_exe: bool,
    pub archive_offset: u64,
    pub entries: Vec<Entry>,
    parts: Vec<File>,
    split: bool,
}

impl VfArchive {
    pub fn open(path: &Path) -> io::Result<Self> {
        let f = File::open(path)?;
        let len = f.metadata()?.len();
        let mut head = [0u8; 2];
        read_at(&f, &mut head, 0)?;
        let (is_exe, archive_offset) = if &head == b"MZ" {
            let mut tail = [0u8; 6];
            if len < 6 {
                return Err(bad("file too small"));
            }
            read_at(&f, &mut tail, len - 6)?;
            if &tail[4..6] != b"lv" {
                return Err(bad("EXE does not contain a LiveMaker archive"));
            }
            (
                true,
                u32::from_le_bytes(tail[0..4].try_into().unwrap()) as u64,
            )
        } else {
            (false, 0)
        };
        if archive_offset >= len {
            return Err(bad("archive offset out of range"));
        }
        // Lee el directorio de forma incremental: primero 64 KiB, crece si hace falta.
        let mut want = 64 * 1024u64;
        let (version, entries) = loop {
            let n = want.min(len - archive_offset) as usize;
            let mut buf = vec![0u8; n];
            read_at(&f, &mut buf, archive_offset)?;
            match parse_directory(&buf) {
                Ok(r) => break r,
                Err(e)
                    if e.kind() == io::ErrorKind::UnexpectedEof
                        && (n as u64) < len - archive_offset =>
                {
                    want *= 4;
                }
                Err(e) => return Err(e),
            }
        };
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let mut parts = Vec::new();
        let split = !is_exe && (ext == "dat" || ext == "ext");
        if split {
            let base = path.with_extension("");
            if ext == "ext" {
                let dat = base.with_extension("dat");
                parts
                    .push(File::open(&dat).map_err(|_| bad(format!("missing {}", dat.display())))?);
            } else {
                parts.push(f.try_clone()?);
            }
            for i in 1..100 {
                let p = PathBuf::from(format!("{}.{:03}", base.display(), i));
                match File::open(&p) {
                    Ok(pf) => parts.push(pf),
                    Err(_) => break,
                }
            }
        } else {
            parts.push(f);
        }
        Ok(Self {
            path: path.to_path_buf(),
            version,
            is_exe,
            archive_offset,
            entries,
            parts,
            split,
        })
    }

    pub fn read_raw(&self, e: &Entry) -> io::Result<Vec<u8>> {
        let mut out = vec![0u8; e.size as usize];
        let mut off = self.archive_offset + e.offset;
        if !self.split {
            read_at(&self.parts[0], &mut out, off)?;
            return Ok(out);
        }
        let mut done = 0usize;
        while done < out.len() {
            let idx = (off / SPLIT_PART_SIZE) as usize;
            let within = off % SPLIT_PART_SIZE;
            let n = ((SPLIT_PART_SIZE - within) as usize).min(out.len() - done);
            let part = self
                .parts
                .get(idx)
                .ok_or_else(|| bad("missing split part"))?;
            read_at(part, &mut out[done..done + n], within)?;
            done += n;
            off += n as u64;
        }
        Ok(out)
    }

    pub fn read(&self, e: &Entry) -> io::Result<Vec<u8>> {
        decompress(e, self.read_raw(e)?)
    }
}

/// Fuente de datos del juego: archivo VF o directorio extraído.
pub enum Source {
    Vf(VfArchive),
    Dir(PathBuf),
}

/// Índice unificado con búsqueda insensible a mayúsculas y separadores.
pub struct GameData {
    pub source: Source,
    /// nombre normalizado -> (nombre original, índice de entrada o ruta)
    index: HashMap<String, usize>,
    names: Vec<String>,
}

pub fn normalize(name: &str) -> String {
    name.replace('/', "\\")
        .trim_start_matches(".\\")
        .to_lowercase()
}

impl GameData {
    pub fn open(path: &Path) -> io::Result<Arc<Self>> {
        if path.is_dir() {
            // ¿Hay un .exe o .dat con archivo dentro? Si no, se trata como extraído.
            let mut candidates: Vec<PathBuf> = std::fs::read_dir(path)?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    let e = p
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    e == "exe" || e == "dat" || e == "ext"
                })
                .collect();
            candidates.sort();
            // .ext antes que .dat (índice partido), luego exe
            candidates.sort_by_key(|p| {
                match p
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|s| s.to_ascii_lowercase())
                    .as_deref()
                {
                    Some("ext") => 0,
                    Some("dat") => 1,
                    _ => 2,
                }
            });
            for c in candidates {
                if let Ok(a) = VfArchive::open(&c) {
                    log::info!("using archive {}", c.display());
                    return Ok(Arc::new(Self::from_vf(a)));
                }
            }
            return Ok(Arc::new(Self::from_dir(path)?));
        }
        Ok(Arc::new(Self::from_vf(VfArchive::open(path)?)))
    }

    pub fn from_vf(a: VfArchive) -> Self {
        let names: Vec<String> = a.entries.iter().map(|e| e.name.clone()).collect();
        let index = names
            .iter()
            .enumerate()
            .map(|(i, n)| (normalize(n), i))
            .collect();
        Self {
            source: Source::Vf(a),
            index,
            names,
        }
    }

    pub fn from_dir(root: &Path) -> io::Result<Self> {
        let mut names = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d)? {
                let p = e?.path();
                if p.is_dir() {
                    stack.push(p);
                } else if let Ok(rel) = p.strip_prefix(root) {
                    names.push(rel.to_string_lossy().replace('/', "\\"));
                }
            }
        }
        names.sort();
        let index = names
            .iter()
            .enumerate()
            .map(|(i, n)| (normalize(n), i))
            .collect();
        Ok(Self {
            source: Source::Dir(root.to_path_buf()),
            index,
            names,
        })
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Busca un nombre (exacto, normalizado). Devuelve el nombre canónico.
    pub fn find(&self, name: &str) -> Option<&str> {
        self.index
            .get(&normalize(name))
            .map(|&i| self.names[i].as_str())
    }

    /// Resuelve referencias sin extensión probando extensiones conocidas.
    pub fn resolve(&self, name: &str, exts: &[&str]) -> Option<String> {
        if name.is_empty() {
            return None;
        }
        if let Some(n) = self.find(name) {
            return Some(n.to_string());
        }
        for e in exts {
            if let Some(n) = self.find(&format!("{name}{e}")) {
                return Some(n.to_string());
            }
        }
        None
    }

    /// Resolves a resource reference first from the archive root, then relative to its owner.
    pub fn resolve_relative(&self, owner: &str, name: &str, exts: &[&str]) -> Option<String> {
        self.resolve(name, exts).or_else(|| {
            if name.starts_with(['\\', '/']) || name.as_bytes().get(1) == Some(&b':') {
                return None;
            }
            let (directory, _) = owner.rsplit_once(['\\', '/'])?;
            let mut parts: Vec<&str> = directory
                .split(['\\', '/'])
                .filter(|part| !part.is_empty() && *part != ".")
                .collect();
            for part in name.split(['\\', '/']) {
                match part {
                    "" | "." => {}
                    ".." => {
                        parts.pop()?;
                    }
                    part => parts.push(part),
                }
            }
            let relative = parts.join("\\");
            self.resolve(&relative, exts)
        })
    }

    pub fn read(&self, name: &str) -> io::Result<Vec<u8>> {
        let i = *self.index.get(&normalize(name)).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, format!("{name} not in archive"))
        })?;
        match &self.source {
            Source::Vf(a) => a.read(&a.entries[i]),
            Source::Dir(root) => {
                std::fs::read(root.join(self.names[i].replace('\\', std::path::MAIN_SEPARATOR_STR)))
            }
        }
    }

    /// Primeros `n` bytes descomprimidos si es barato (entrada sin compresión o directorio).
    pub fn read_prefix(&self, name: &str, n: usize) -> Option<Vec<u8>> {
        let i = *self.index.get(&normalize(name))?;
        match &self.source {
            Source::Vf(a) => {
                let e = &a.entries[i];
                if e.compress != Compress::None {
                    return None;
                }
                let mut buf = vec![0u8; (e.size as usize).min(n)];
                if a.split {
                    return a.read_raw(e).ok().map(|mut v| {
                        v.truncate(n);
                        v
                    });
                }
                read_at(&a.parts[0], &mut buf, a.archive_offset + e.offset).ok()?;
                Some(buf)
            }
            Source::Dir(root) => {
                let f = File::open(
                    root.join(self.names[i].replace('\\', std::path::MAIN_SEPARATOR_STR)),
                )
                .ok()?;
                let mut buf = Vec::with_capacity(n);
                f.take(n as u64).read_to_end(&mut buf).ok()?;
                Some(buf)
            }
        }
    }

    pub fn entry(&self, name: &str) -> Option<&Entry> {
        match &self.source {
            Source::Vf(a) => self.index.get(&normalize(name)).map(|&i| &a.entries[i]),
            Source::Dir(_) => None,
        }
    }

    pub fn describe(&self) -> String {
        match &self.source {
            Source::Vf(a) => format!(
                "VF v{} ({}{}), offset {}",
                a.version,
                if a.is_exe { "exe" } else { "dat" },
                if a.split { ", split" } else { "" },
                a.archive_offset
            ),
            Source::Dir(p) => format!("directory {}", p.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fx(n: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(n)
    }

    #[test]
    fn exe_and_dat_match() {
        let exe = VfArchive::open(&fx("game.exe")).unwrap();
        let dat = VfArchive::open(&fx("game.dat")).unwrap();
        assert!(exe.is_exe);
        assert!(!dat.is_exe);
        assert_eq!(exe.entries.len(), 5);
        let names: Vec<_> = exe.entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"ゲームメイン.lsb"));
        for e in &exe.entries {
            let d = dat.entries.iter().find(|x| x.name == e.name).unwrap();
            let raw = exe.read_raw(e).unwrap();
            assert_eq!(checksum(&raw), e.checksum, "checksum {}", e.name);
            assert_eq!(exe.read(e).unwrap(), dat.read(d).unwrap());
        }
        let on_disk = std::fs::read(fx("bg.gal")).unwrap();
        let gd = GameData::from_vf(exe);
        assert_eq!(gd.read("BG.GAL").unwrap(), on_disk);
        assert_eq!(gd.resolve("bg", &[".gal"]).as_deref(), Some("bg.gal"));
    }

    #[test]
    fn resolves_archive_root_then_owner_relative() {
        let names = vec![
            "ui\\buttons\\start.gal".to_string(),
            "start.gal".to_string(),
        ];
        let index = names
            .iter()
            .enumerate()
            .map(|(i, n)| (normalize(n), i))
            .collect();
        let data = GameData {
            source: Source::Dir(PathBuf::new()),
            index,
            names,
        };

        assert_eq!(
            data.resolve_relative("ui\\buttons\\menu.lpm", "start.gal", &[".gal"])
                .as_deref(),
            Some("start.gal")
        );
        assert_eq!(
            data.resolve_relative("ui\\buttons\\menu.lpm", "missing.gal", &[".gal"]),
            None
        );

        let names = vec!["ui\\buttons\\start.gal".to_string()];
        let index = names
            .iter()
            .enumerate()
            .map(|(i, n)| (normalize(n), i))
            .collect();
        let data = GameData {
            source: Source::Dir(PathBuf::new()),
            index,
            names,
        };
        assert_eq!(
            data.resolve_relative("ui\\buttons\\menu.lpm", "start.gal", &[".gal"])
                .as_deref(),
            Some("ui\\buttons\\start.gal")
        );
    }

    #[test]
    fn resolves_parent_relative_paths_without_escaping_archive_root() {
        let names = vec![
            "ui\\buttons\\start.gal".to_string(),
            "ui\\start.gal".to_string(),
        ];
        let index = names
            .iter()
            .enumerate()
            .map(|(i, n)| (normalize(n), i))
            .collect();
        let data = GameData {
            source: Source::Dir(PathBuf::new()),
            index,
            names,
        };

        assert_eq!(
            data.resolve_relative("ui\\buttons\\menu.lpm", "..\\start.gal", &[".gal"])
                .as_deref(),
            Some("ui\\start.gal")
        );
        assert_eq!(
            data.resolve_relative("ui\\buttons\\menu.lpm", ".\\start.gal", &[".gal"])
                .as_deref(),
            Some("ui\\buttons\\start.gal")
        );
        assert_eq!(
            data.resolve_relative("ui\\menu.lpm", "..\\..\\outside.gal", &[".gal"]),
            None
        );
    }

    #[test]
    fn scramble_roundtrip_identity_small() {
        assert_eq!(unscramble(b"abc").unwrap(), b"abc");
        // 1 chunk -> identity
        let mut d = vec![];
        d.extend_from_slice(&16i32.to_le_bytes());
        d.extend_from_slice(&1234u32.to_le_bytes());
        d.extend_from_slice(b"hello world");
        assert_eq!(unscramble(&d).unwrap(), b"hello world");
    }
}

#[cfg(test)]
mod scramble_tests {
    #[test]
    fn matches_pylivemaker() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let enc = std::fs::read(dir.join("scrambled.bin")).unwrap();
        let plain = std::fs::read(dir.join("scrambled.plain")).unwrap();
        assert_eq!(super::unscramble(&enc).unwrap(), plain);
    }
}
