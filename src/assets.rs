//! Caché de recursos con pool de hilos: decodifica GAL/audio/LSB en segundo plano
//! y hace prefetch de lo que el script va a necesitar.

use crate::archive::GameData;
use crate::gal;
use crate::lsb::Lsb;
use crossbeam_channel::{unbounded, Receiver, Sender};
use std::collections::HashMap;
use std::io::Cursor;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

/// PCM entrelazado i16.
#[derive(Debug)]
pub struct Pcm {
    pub rate: u32,
    pub channels: u16,
    pub samples: Vec<i16>,
}

#[derive(Clone)]
pub enum Asset {
    Image(Arc<gal::Image>),
    Sound(Arc<Pcm>),
    Script(Arc<Lsb>),
    Bytes(Arc<Vec<u8>>),
    Failed(Arc<str>),
}

impl Asset {
    fn bytes(&self) -> usize {
        match self {
            Asset::Image(i) => i.rgba.len(),
            Asset::Sound(p) => p.samples.len() * 2,
            Asset::Script(_) => 64 * 1024,
            Asset::Bytes(b) => b.len(),
            Asset::Failed(_) => 0,
        }
    }
}

enum Slot {
    Pending,
    Ready(Asset, u64),
}

#[derive(Default)]
pub struct Stats {
    pub decoded: AtomicUsize,
    pub decode_us: AtomicUsize,
    pub prefetch_hits: AtomicUsize,
    pub sync_loads: AtomicUsize,
}

struct Shared {
    data: Arc<GameData>,
    slots: Mutex<(HashMap<String, Slot>, u64, usize)>, // (slots, reloj LRU, bytes)
    cv: Condvar,
    stats: Stats,
    cap_bytes: usize,
}

pub struct Assets {
    sh: Arc<Shared>,
    urgent: Sender<String>,
    prefetch: Sender<String>,
}

pub const IMAGE_EXTS: &[&str] = &[".gal", ".png", ".jpg", ".bmp"];
pub const SOUND_EXTS: &[&str] = &[".ogg", ".wav"];
pub const ALL_EXTS: &[&str] = &[".gal", ".png", ".jpg", ".bmp", ".ogg", ".wav", ".lsb"];

fn ext(name: &str) -> String {
    name.rsplit('.').next().unwrap_or("").to_ascii_lowercase()
}

pub fn decode(data: &GameData, name: &str) -> Asset {
    let raw = match data.read(name) {
        Ok(r) => r,
        Err(e) => return Asset::Failed(e.to_string().into()),
    };
    let fail = |e: String| Asset::Failed(format!("{name}: {e}").into());
    match ext(name).as_str() {
        "gal" | "png" | "jpg" | "jpeg" | "bmp" => match gal::decode_any(&raw) {
            Ok(i) => Asset::Image(Arc::new(i)),
            Err(e) => fail(e.to_string()),
        },
        "wav" => decode_wav(&raw).map(|p| Asset::Sound(Arc::new(p))).unwrap_or_else(fail),
        "ogg" => decode_ogg(&raw).map(|p| Asset::Sound(Arc::new(p))).unwrap_or_else(fail),
        "lsb" => match Lsb::parse(name, &raw) {
            Ok(l) => Asset::Script(Arc::new(l)),
            Err(e) => fail(e.0),
        },
        _ => Asset::Bytes(Arc::new(raw)),
    }
}

fn decode_wav(d: &[u8]) -> Result<Pcm, String> {
    let r = hound::WavReader::new(Cursor::new(d)).map_err(|e| e.to_string())?;
    let spec = r.spec();
    let samples: Vec<i16> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Int, 8) => r.into_samples::<i8>().map(|s| (s.unwrap_or(0) as i16) << 8).collect(),
        (hound::SampleFormat::Int, 16) => r.into_samples::<i16>().map(|s| s.unwrap_or(0)).collect(),
        (hound::SampleFormat::Int, b) => r.into_samples::<i32>().map(|s| (s.unwrap_or(0) >> (b - 16)) as i16).collect(),
        (hound::SampleFormat::Float, _) => {
            r.into_samples::<f32>().map(|s| (s.unwrap_or(0.0).clamp(-1.0, 1.0) * 32767.0) as i16).collect()
        }
    };
    Ok(Pcm { rate: spec.sample_rate, channels: spec.channels, samples })
}

fn decode_ogg(d: &[u8]) -> Result<Pcm, String> {
    let mut r = lewton::inside_ogg::OggStreamReader::new(Cursor::new(d)).map_err(|e| e.to_string())?;
    let rate = r.ident_hdr.audio_sample_rate;
    let channels = r.ident_hdr.audio_channels as u16;
    let mut samples = Vec::new();
    while let Some(p) = r.read_dec_packet_itl().map_err(|e| e.to_string())? {
        samples.extend_from_slice(&p);
    }
    Ok(Pcm { rate, channels, samples })
}

impl Assets {
    pub fn new(data: Arc<GameData>, threads: usize, cache_mb: usize) -> Self {
        let sh = Arc::new(Shared {
            data,
            slots: Mutex::new((HashMap::new(), 0, 0)),
            cv: Condvar::new(),
            stats: Stats::default(),
            cap_bytes: cache_mb * 1024 * 1024,
        });
        let (utx, urx) = unbounded::<String>();
        let (ptx, prx) = unbounded::<String>();
        for i in 0..threads.max(1) {
            let sh = sh.clone();
            let (urx, prx): (Receiver<String>, Receiver<String>) = (urx.clone(), prx.clone());
            std::thread::Builder::new()
                .name(format!("livenyan-dec{i}"))
                .spawn(move || worker(sh, urx, prx))
                .expect("spawn decoder");
        }
        Self { sh, urgent: utx, prefetch: ptx }
    }

    pub fn data(&self) -> &Arc<GameData> {
        &self.sh.data
    }

    pub fn stats(&self) -> &Stats {
        &self.sh.stats
    }

    fn enqueue(&self, name: &str, urgent: bool) {
        let mut g = self.sh.slots.lock().unwrap();
        if g.0.contains_key(name) {
            return;
        }
        g.0.insert(name.to_string(), Slot::Pending);
        drop(g);
        let _ = if urgent { self.urgent.send(name.to_string()) } else { self.prefetch.send(name.to_string()) };
    }

    /// Pide decodificar en segundo plano (no bloquea). `name` debe ser canónico.
    pub fn prefetch(&self, name: &str) {
        self.enqueue(name, false);
    }

    /// Igual que prefetch pero con prioridad.
    pub fn request(&self, name: &str) {
        self.enqueue(name, true);
    }

    /// Devuelve el recurso si ya está listo (no bloquea).
    pub fn try_get(&self, name: &str) -> Option<Asset> {
        let mut g = self.sh.slots.lock().unwrap();
        g.1 += 1;
        let clock = g.1;
        match g.0.get_mut(name) {
            Some(Slot::Ready(a, t)) => {
                *t = clock;
                Some(a.clone())
            }
            Some(Slot::Pending) => None,
            None => {
                drop(g);
                self.request(name);
                None
            }
        }
    }

    /// Obtiene el recurso; si nadie lo está decodificando lo decodifica en este hilo.
    pub fn get(&self, name: &str) -> Asset {
        let mut g = self.sh.slots.lock().unwrap();
        loop {
            g.1 += 1;
            let clock = g.1;
            match g.0.get_mut(name) {
                Some(Slot::Ready(a, t)) => {
                    *t = clock;
                    self.sh.stats.prefetch_hits.fetch_add(1, Ordering::Relaxed);
                    return a.clone();
                }
                Some(Slot::Pending) => {
                    g = self.sh.cv.wait(g).unwrap();
                }
                None => {
                    g.0.insert(name.to_string(), Slot::Pending);
                    drop(g);
                    self.sh.stats.sync_loads.fetch_add(1, Ordering::Relaxed);
                    let a = timed_decode(&self.sh, name);
                    store(&self.sh, name, a.clone());
                    return a;
                }
            }
        }
    }

    pub fn script(&self, name: &str) -> Option<Arc<Lsb>> {
        match self.get(name) {
            Asset::Script(s) => Some(s),
            Asset::Failed(e) => {
                log::error!("{e}");
                None
            }
            _ => None,
        }
    }
}

fn timed_decode(sh: &Shared, name: &str) -> Asset {
    let t = Instant::now();
    let a = decode(&sh.data, name);
    sh.stats.decoded.fetch_add(1, Ordering::Relaxed);
    sh.stats.decode_us.fetch_add(t.elapsed().as_micros() as usize, Ordering::Relaxed);
    if let Asset::Failed(e) = &a {
        log::warn!("decode failed: {e}");
    }
    a
}

fn store(sh: &Shared, name: &str, a: Asset) {
    let mut g = sh.slots.lock().unwrap();
    g.1 += 1;
    let clock = g.1;
    g.2 += a.bytes();
    g.0.insert(name.to_string(), Slot::Ready(a, clock));
    // expulsión LRU aproximada si superamos el límite (nunca scripts)
    if g.2 > sh.cap_bytes {
        let mut v: Vec<(u64, String, usize)> = g
            .0
            .iter()
            .filter_map(|(k, s)| match s {
                Slot::Ready(a, t) if !matches!(a, Asset::Script(_)) => Some((*t, k.clone(), a.bytes())),
                _ => None,
            })
            .collect();
        v.sort();
        for (_, k, b) in v {
            if g.2 <= sh.cap_bytes * 3 / 4 {
                break;
            }
            g.0.remove(&k);
            g.2 -= b;
        }
    }
    drop(g);
    sh.cv.notify_all();
}

fn worker(sh: Arc<Shared>, urgent: Receiver<String>, prefetch: Receiver<String>) {
    loop {
        let name = match urgent.try_recv() {
            Ok(n) => n,
            Err(_) => crossbeam_channel::select! {
                recv(urgent) -> n => match n { Ok(n) => n, Err(_) => return },
                recv(prefetch) -> n => match n { Ok(n) => n, Err(_) => return },
            },
        };
        let a = timed_decode(&sh, &name);
        store(&sh, &name, a);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pool_decodes() {
        let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/game.exe");
        let data = GameData::open(&p).unwrap();
        let a = Assets::new(data, 2, 64);
        a.prefetch("bg.gal");
        a.prefetch("se.wav");
        assert!(matches!(a.get("bg.gal"), Asset::Image(_)));
        match a.get("se.wav") {
            Asset::Sound(p) => assert_eq!((p.rate, p.channels, p.samples.len()), (22050, 1, 2205)),
            _ => panic!("wav"),
        }
        assert!(a.script("sub.lsb").is_some());
    }
}
