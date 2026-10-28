use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{
    self, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError, TrySendError,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(1);

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

pub struct Decoder {
    frames: Receiver<Message>,
    first_frame: Option<DecodedFrame>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    temp_file: PathBuf,
}

/// Ancho×alto nativos del primer stream de vídeo (ffprobe).
/// **fuente primaria:** Movie no tiene PR_WIDTH/HEIGHT (`docs/livenovel/media.html`);
/// el tamaño base es el del archivo (`liveVideoGetSourceWidth/Height`).
pub fn probe_size(data: &[u8]) -> Result<(u32, u32), String> {
    let temp_file = create_temp_video(data)?;
    let result = probe_file(&temp_file);
    if let Err(e) = std::fs::remove_file(&temp_file)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        log::warn!("cannot remove temporary video {}: {e}", temp_file.display());
    }
    result
}

fn probe_file(path: &PathBuf) -> Result<(u32, u32), String> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0:s=x",
        ])
        .arg(path)
        .output()
        .map_err(|e| format!("cannot start ffprobe: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "ffprobe failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let text = text.trim();
    let (w, h) = text
        .split_once('x')
        .ok_or_else(|| format!("ffprobe returned no width×height: {text:?}"))?;
    let width: u32 = w
        .trim()
        .parse()
        .map_err(|_| format!("ffprobe width is not a number: {w:?}"))?;
    let height: u32 = h
        .trim()
        .parse()
        .map_err(|_| format!("ffprobe height is not a number: {h:?}"))?;
    if width == 0 || height == 0 {
        return Err(format!(
            "ffprobe reported empty video size {width}x{height}"
        ));
    }
    Ok((width, height))
}

impl Decoder {
    pub fn start(data: &[u8], width: u32, height: u32, repeat: bool) -> Result<Self, String> {
        if width == 0 || height == 0 {
            return Err("video output size must be nonzero".into());
        }
        let frame_bytes = (width as usize)
            .checked_mul(height as usize)
            .and_then(|size| size.checked_mul(4))
            .ok_or_else(|| "video output size is too large".to_string())?;
        let temp_file = create_temp_video(data)?;
        let (sender, frames) = mpsc::sync_channel(2);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread_file = temp_file.clone();
        let worker = thread::Builder::new()
            .name("livenyan-video".into())
            .spawn(move || {
                decode_frames(
                    &thread_file,
                    width,
                    height,
                    frame_bytes,
                    repeat,
                    sender,
                    thread_stop,
                );
                if let Err(e) = std::fs::remove_file(&thread_file)
                    && e.kind() != std::io::ErrorKind::NotFound
                {
                    log::warn!(
                        "cannot remove temporary video {}: {e}",
                        thread_file.display()
                    );
                }
            })
            .map_err(|e| {
                let _ = std::fs::remove_file(&temp_file);
                format!("cannot start video decoder thread: {e}")
            })?;
        let mut decoder = Self {
            frames,
            first_frame: None,
            stop,
            worker: Some(worker),
            temp_file,
        };
        match decoder.frames.recv_timeout(Duration::from_secs(5)) {
            Ok(Message::Frame(frame)) => decoder.first_frame = Some(frame),
            Ok(Message::Finished(Err(error))) => return Err(error),
            Ok(Message::Finished(Ok(()))) => {
                return Err("video ended before producing its first frame".into());
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                return Err("timed out waiting for the first decoded video frame".into());
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err("video decoder stopped before producing its first frame".into());
            }
        }
        Ok(decoder)
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
        if let Err(e) = std::fs::remove_file(&self.temp_file)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!(
                "cannot remove temporary video {}: {e}",
                self.temp_file.display()
            );
        }
    }
}

fn create_temp_video(data: &[u8]) -> Result<PathBuf, String> {
    for _ in 0..16 {
        let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("livenyan-{}-{id}.wmv", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                if let Err(e) = file.write_all(data) {
                    let _ = std::fs::remove_file(&path);
                    return Err(format!(
                        "cannot write temporary video {}: {e}",
                        path.display()
                    ));
                }
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(format!(
                    "cannot create temporary video {}: {e}",
                    path.display()
                ));
            }
        }
    }
    Err("cannot allocate a unique temporary video path".into())
}

fn decode_frames(
    path: &PathBuf,
    width: u32,
    height: u32,
    frame_bytes: usize,
    repeat: bool,
    sender: SyncSender<Message>,
    stop: Arc<AtomicBool>,
) {
    let mut command = Command::new("ffmpeg");
    command.args(["-hide_banner", "-loglevel", "info", "-nostdin"]);
    if repeat {
        command.args(["-stream_loop", "-1"]);
    }
    let filter = format!(
        "scale={width}:{height}:force_original_aspect_ratio=decrease,pad={width}:{height}:(ow-iw)/2:(oh-ih)/2,showinfo"
    );
    command
        .arg("-i")
        .arg(path)
        .args([
            "-an",
            "-sn",
            "-dn",
            "-vf",
            &filter,
            "-fps_mode",
            "passthrough",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "pipe:1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            let _ = send_message(
                &sender,
                Message::Finished(Err(format!("cannot start ffmpeg: {e}"))),
                &stop,
            );
            return;
        }
    };
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        let _ = send_message(
            &sender,
            Message::Finished(Err("ffmpeg did not provide video frames".into())),
            &stop,
        );
        return;
    };
    let (pts_sender, pts_receiver) = mpsc::channel();
    let stderr = child
        .stderr
        .take()
        .map(|pipe| thread::spawn(move || read_frame_pts(pipe, pts_sender)));

    let mut error = None;
    let mut previous_frame: Option<Arc<[u8]>> = None;
    loop {
        if stop.load(Ordering::Acquire) {
            let _ = child.kill();
            break;
        }
        let mut frame = vec![0; frame_bytes];
        match stdout.read_exact(&mut frame) {
            Ok(()) => {
                let pts_seconds = match pts_receiver.recv_timeout(Duration::from_secs(5)) {
                    Ok(pts) => pts,
                    Err(RecvTimeoutError::Timeout) => {
                        error = Some("timed out waiting for decoded video timestamp".into());
                        let _ = child.kill();
                        break;
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        error = Some("ffmpeg did not report a timestamp for a video frame".into());
                        let _ = child.kill();
                        break;
                    }
                };
                let frame: Arc<[u8]> = Arc::from(frame);
                let duplicate = previous_frame.as_deref() == Some(frame.as_ref());
                previous_frame = Some(Arc::clone(&frame));
                if !send_message(
                    &sender,
                    Message::Frame(DecodedFrame {
                        pixels: frame,
                        pts_seconds,
                        duplicate,
                    }),
                    &stop,
                ) {
                    let _ = child.kill();
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => {
                error = Some(format!("cannot read decoded video frame: {e}"));
                break;
            }
        }
    }
    let status = child.wait();
    let stderr = stderr
        .and_then(|reader| reader.join().ok())
        .and_then(Result::ok)
        .unwrap_or_default();
    if stop.load(Ordering::Acquire) {
        return;
    }
    let result = match (error, status) {
        (Some(e), _) => Err(e),
        (None, Ok(status)) if status.success() => Ok(()),
        (None, Ok(status)) => Err(format!("ffmpeg exited with {status}: {}", stderr.trim())),
        (None, Err(e)) => Err(format!("cannot wait for ffmpeg: {e}")),
    };
    let _ = send_message(&sender, Message::Finished(result), &stop);
}

fn read_frame_pts<R: Read>(pipe: R, sender: Sender<f64>) -> Result<String, String> {
    const MAX_DIAGNOSTIC_BYTES: usize = 16 * 1024;
    let mut output = String::new();
    for line in BufReader::new(pipe).lines() {
        let line = line.map_err(|e| format!("cannot read ffmpeg diagnostics: {e}"))?;
        if let Some(pts) = parse_showinfo_pts(&line) {
            sender
                .send(pts)
                .map_err(|_| "video decoder stopped while reading timestamps".to_string())?;
        }
        if line.contains("Parsed_showinfo") {
            continue;
        }
        output.push_str(&line);
        output.push('\n');
        if output.len() > MAX_DIAGNOSTIC_BYTES {
            let excess = output.len() - MAX_DIAGNOSTIC_BYTES;
            let truncate_at = output
                .char_indices()
                .find_map(|(index, _)| (index >= excess).then_some(index))
                .unwrap_or(output.len());
            output.drain(..truncate_at);
        }
    }
    Ok(output)
}

fn parse_showinfo_pts(line: &str) -> Option<f64> {
    let (_, value) = line.split_once("pts_time:")?;
    let value = value.split_whitespace().next()?;
    let pts = value.parse::<f64>().ok()?;
    pts.is_finite().then_some(pts)
}

fn send_message(sender: &SyncSender<Message>, mut message: Message, stop: &AtomicBool) -> bool {
    loop {
        if stop.load(Ordering::Acquire) {
            return false;
        }
        match sender.try_send(message) {
            Ok(()) => return true,
            Err(TrySendError::Full(returned)) => {
                message = returned;
                thread::sleep(Duration::from_millis(5));
            }
            Err(TrySendError::Disconnected(_)) => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Decoder, Message};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

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

        let mut decoder = Decoder::start(&data, 8, 8, false).unwrap();
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
    fn parses_ffmpeg_showinfo_timestamps() {
        assert_eq!(
            super::parse_showinfo_pts("[Parsed_showinfo_3] n: 1 pts: 3000 pts_time:0.033333"),
            Some(0.033333)
        );
        assert_eq!(
            super::parse_showinfo_pts("[Parsed_showinfo_3] config in time_base: 1/30"),
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
