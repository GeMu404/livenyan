//! Decodificación de Movie: backends intercambiables.
//!
//! - **GAL anim** (piso portable): GaleX multi-frame + delays; sin `Command`.
//! - **ffmpeg CLI** (wenardo del host): WMV/etc. vía proceso externo.
//!
//! La VM solo ve [`Decoder`] / [`Message`]. **interpretación livenyan** (I1, portabilidad).

mod ffmpeg_cli;
mod gal_anim;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::thread::JoinHandle;

#[derive(Debug)]
pub enum Message {
    Frame(DecodedFrame),
    Finished(Result<(), String>),
}

#[derive(Debug)]
pub struct DecodedFrame {
    pub pixels: Arc<[u8]>,
    pub pts_seconds: f64,
    pub duplicate: bool,
}

/// Tamaño de salida del decoder.
pub enum OutputSize {
    /// Escalar/pad a este tamaño (tests / override explícito).
    Exact(u32, u32),
    /// Probe + frames a tamaño nativo (sin scale).
    Source,
    /// Tamaño nativo ya conocido (sin probe ni scale).
    KnownNative(u32, u32),
}

pub struct Decoder {
    frames: Receiver<Message>,
    first_frame: Option<DecodedFrame>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    temp_file: Option<PathBuf>,
}

impl Decoder {
    pub(super) fn from_worker(
        frames: Receiver<Message>,
        first_frame: Option<DecodedFrame>,
        stop: Arc<AtomicBool>,
        worker: JoinHandle<()>,
        temp_file: Option<PathBuf>,
    ) -> Self {
        Self {
            frames,
            first_frame,
            stop,
            worker: Some(worker),
            temp_file,
        }
    }

    /// Arranca el backend adecuado: GaleX si el magic encaja; si no, ffmpeg CLI.
    pub fn start(data: &[u8], size: OutputSize, repeat: bool) -> Result<(Self, u32, u32), String> {
        if gal_anim::is_gal(data) {
            gal_anim::start(data, size, repeat)
        } else {
            ffmpeg_cli::start(data, size, repeat)
        }
    }

    pub fn try_recv(&mut self) -> Result<Message, TryRecvError> {
        match self.first_frame.take() {
            Some(frame) => Ok(Message::Frame(frame)),
            None => self.frames.try_recv(),
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            log::warn!("video decoder thread panicked");
        }
        if let Some(temp_file) = self.temp_file.take()
            && let Err(e) = std::fs::remove_file(&temp_file)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!("cannot remove temporary video {}: {e}", temp_file.display());
        }
    }
}

/// Ancho×alto nativos. Preferir [`Decoder::start`] con [`OutputSize::Source`] en el hot path.
pub fn probe_size(data: &[u8]) -> Result<(u32, u32), String> {
    if gal_anim::is_gal(data) {
        crate::gal::peek_size(data).ok_or_else(|| "cannot probe GaleX size".into())
    } else {
        ffmpeg_cli::probe_size(data)
    }
}

#[cfg(test)]
mod tests {
    use super::{Decoder, Message, OutputSize};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    #[test]
    fn gal_anim_backend_plays_without_ffmpeg() {
        let red = [255u8, 0, 0, 255].repeat(4 * 4);
        let blue = [0u8, 0, 255, 255].repeat(4 * 4);
        let gal = crate::gal::encode_galex_anim(4, 4, &[red, blue], &[100, 200])
            .expect("encode gal anim");
        let (mut decoder, w, h) = Decoder::start(&gal, OutputSize::Source, false).unwrap();
        assert_eq!((w, h), (4, 4));
        let Message::Frame(first) = decoder.try_recv().unwrap() else {
            panic!("expected first frame");
        };
        assert_eq!(first.pts_seconds, 0.0);
        assert!(!first.duplicate);
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match decoder.try_recv() {
                Ok(Message::Frame(next)) => {
                    assert!((next.pts_seconds - 0.1).abs() < 1e-6);
                    assert!(!next.duplicate);
                    break;
                }
                Ok(Message::Finished(r)) => panic!("finished early: {r:?}"),
                Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(e) => panic!("{e}"),
            }
        }
    }

    #[test]
    fn decoder_emits_scaled_rgba_frames() {
        if Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_err()
        {
            eprintln!("ffmpeg is unavailable; skipping decoder integration test");
            return;
        }
        let path =
            std::env::temp_dir().join(format!("livenyan-video-test-{}.mkv", std::process::id()));
        let result = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=red:s=16x16:r=2:d=1",
                "-c:v",
                "ffv1",
                "-y",
            ])
            .arg(&path)
            .output()
            .expect("generate test video");
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let data = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();

        let (mut decoder, w, h) = Decoder::start(&data, OutputSize::Exact(8, 8), false).unwrap();
        assert_eq!((w, h), (8, 8));
        let Message::Frame(frame) = decoder.try_recv().unwrap() else {
            panic!("decoder did not provide its prefetched first frame");
        };
        assert_eq!(frame.pixels.len(), 8 * 8 * 4);
        assert!(frame.pts_seconds.is_finite());
        assert!(!frame.duplicate);

        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match decoder.try_recv() {
                Ok(Message::Frame(next)) => {
                    assert!(next.pts_seconds > frame.pts_seconds);
                    assert!((next.pts_seconds - frame.pts_seconds - 0.5).abs() < 0.02);
                    assert!(next.duplicate);
                    break;
                }
                Ok(Message::Finished(result)) => {
                    panic!("video ended before the repeated output frame: {result:?}");
                }
                Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("timed out waiting for the next video frame: {error}"),
            }
        }
    }

    #[test]
    fn decoder_source_size_skips_rescale() {
        if Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_err()
            || Command::new("ffprobe")
                .arg("-version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_err()
        {
            eprintln!("ffmpeg/ffprobe unavailable; skipping source-size test");
            return;
        }
        let path =
            std::env::temp_dir().join(format!("livenyan-video-src-{}.mkv", std::process::id()));
        let result = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=green:s=12x10:r=1:d=0.5",
                "-c:v",
                "ffv1",
                "-y",
            ])
            .arg(&path)
            .output()
            .expect("generate source-size test video");
        assert!(result.status.success());
        let data = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let (mut decoder, w, h) = Decoder::start(&data, OutputSize::Source, false).unwrap();
        assert_eq!((w, h), (12, 10));
        let Message::Frame(frame) = decoder.try_recv().unwrap() else {
            panic!("expected first frame");
        };
        assert_eq!(frame.pixels.len(), 12 * 10 * 4);
    }

    #[test]
    fn parses_ffmpeg_showinfo_timestamps() {
        assert_eq!(
            super::ffmpeg_cli::parse_showinfo_pts(
                "[Parsed_showinfo_3] n: 1 pts: 3000 pts_time:0.033333"
            ),
            Some(0.033333)
        );
        assert_eq!(
            super::ffmpeg_cli::parse_showinfo_pts("[Parsed_showinfo_3] config in time_base: 1/30"),
            None
        );
    }

    #[test]
    fn probe_size_reads_native_dimensions() {
        if Command::new("ffprobe")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_err()
        {
            eprintln!("ffprobe is unavailable; skipping probe_size test");
            return;
        }
        let path =
            std::env::temp_dir().join(format!("livenyan-probe-test-{}.mkv", std::process::id()));
        let result = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=blue:s=24x18:r=1:d=0.5",
                "-c:v",
                "ffv1",
                "-y",
            ])
            .arg(&path)
            .output()
            .expect("generate probe test video");
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let data = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(super::probe_size(&data).unwrap(), (24, 18));
    }
}
