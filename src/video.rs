use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
pub enum Message {
    Frame(Vec<u8>),
    Finished(Result<(), String>),
}

pub struct Decoder {
    frames: Receiver<Message>,
    first_frame: Option<Vec<u8>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    temp_file: PathBuf,
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
                if let Err(e) = std::fs::remove_file(&thread_file) {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        log::warn!(
                            "cannot remove temporary video {}: {e}",
                            thread_file.display()
                        );
                    }
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
        if let Err(e) = std::fs::remove_file(&self.temp_file) {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!(
                    "cannot remove temporary video {}: {e}",
                    self.temp_file.display()
                );
            }
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
    command.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-re"]);
    if repeat {
        command.args(["-stream_loop", "-1"]);
    }
    let filter = format!(
        "fps=30,scale={width}:{height}:force_original_aspect_ratio=decrease,pad={width}:{height}:(ow-iw)/2:(oh-ih)/2"
    );
    command
        .arg("-i")
        .arg(path)
        .args([
            "-an", "-sn", "-dn", "-vf", &filter, "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1",
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
    let stderr = child.stderr.take().map(|mut pipe| {
        thread::spawn(move || {
            let mut output = String::new();
            let _ = pipe.read_to_string(&mut output);
            output
        })
    });

    let mut error = None;
    loop {
        if stop.load(Ordering::Acquire) {
            let _ = child.kill();
            break;
        }
        let mut frame = vec![0; frame_bytes];
        match stdout.read_exact(&mut frame) {
            Ok(()) => {
                if !send_message(&sender, Message::Frame(frame), &stop) {
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
        assert_eq!(frame.len(), 8 * 8 * 4);
    }
}
