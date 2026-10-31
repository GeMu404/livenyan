//! Backend básico portable: Movie como GaleX multi-frame + delays (ms).
//! Sin `std::process`. El playhead de la VM sigue usando PTS (I1).

use super::{DecodedFrame, Decoder, Message, OutputSize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::thread;
use std::time::Duration;

pub(super) fn is_gal(data: &[u8]) -> bool {
    data.starts_with(b"Gale")
}

pub(super) fn start(
    data: &[u8],
    size: OutputSize,
    repeat: bool,
) -> Result<(Decoder, u32, u32), String> {
    let image = crate::gal::decode(data).map_err(|e| e.to_string())?;
    let (native_w, native_h) = (image.w, image.h);
    if native_w == 0 || native_h == 0 {
        return Err("GaleX video size must be nonzero".into());
    }
    let (out_w, out_h, scale) = match size {
        OutputSize::Exact(w, h) => {
            if w == 0 || h == 0 {
                return Err("video output size must be nonzero".into());
            }
            (w, h, true)
        }
        OutputSize::Source | OutputSize::KnownNative(_, _) => (native_w, native_h, false),
    };

    let mut rgba_frames: Vec<Arc<[u8]>> = Vec::with_capacity(1 + image.frames.len());
    rgba_frames.push(Arc::from(maybe_scale(
        &image.rgba,
        native_w,
        native_h,
        out_w,
        out_h,
        scale,
    )));
    for frame in &image.frames {
        rgba_frames.push(Arc::from(maybe_scale(
            frame, native_w, native_h, out_w, out_h, scale,
        )));
    }

    let delays = normalize_delays(&image.delays, rgba_frames.len());
    let sequence = build_sequence(&rgba_frames, &delays);
    let cycle_ms: u64 = delays.iter().map(|d| u64::from(*d)).sum();

    let (sender, frames) = mpsc::sync_channel(2);
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let worker = thread::Builder::new()
        .name("livenyan-video-gal".into())
        .spawn(move || {
            emit_sequence(&sequence, cycle_ms, repeat, sender, thread_stop);
        })
        .map_err(|e| format!("cannot start GaleX video thread: {e}"))?;

    let first_frame = match frames.recv_timeout(Duration::from_secs(5)) {
        Ok(Message::Frame(frame)) => Some(frame),
        Ok(Message::Finished(Err(error))) => return Err(error),
        Ok(Message::Finished(Ok(()))) => {
            return Err("GaleX video ended before producing its first frame".into());
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            return Err("timed out waiting for the first GaleX video frame".into());
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            return Err("GaleX video decoder stopped before producing its first frame".into());
        }
    };
    Ok((
        Decoder::from_worker(frames, first_frame, stop, worker, None),
        out_w,
        out_h,
    ))
}

fn normalize_delays(delays: &[u32], frame_count: usize) -> Vec<u32> {
    if frame_count == 0 {
        return Vec::new();
    }
    if delays.len() == frame_count {
        return delays
            .iter()
            .map(|d| if *d == 0 { 1 } else { *d })
            .collect();
    }
    // Sin delays útiles: estático = 1 ms; varios ≈ ~60 fps (como Cinema LCM).
    if frame_count == 1 {
        vec![1]
    } else {
        vec![16; frame_count]
    }
}

struct TimedFrame {
    pixels: Arc<[u8]>,
    pts_ms: u64,
    duplicate: bool,
}

fn build_sequence(frames: &[Arc<[u8]>], delays: &[u32]) -> Vec<TimedFrame> {
    let mut out = Vec::with_capacity(frames.len());
    let mut pts_ms: u64 = 0;
    let mut prev: Option<&[u8]> = None;
    for (pixels, delay) in frames.iter().zip(delays.iter()) {
        let duplicate = prev.is_some_and(|p| p == pixels.as_ref());
        out.push(TimedFrame {
            pixels: Arc::clone(pixels),
            pts_ms,
            duplicate,
        });
        pts_ms = pts_ms.saturating_add(u64::from(*delay));
        prev = Some(pixels.as_ref());
    }
    out
}

fn emit_sequence(
    sequence: &[TimedFrame],
    cycle_ms: u64,
    repeat: bool,
    sender: SyncSender<Message>,
    stop: Arc<AtomicBool>,
) {
    if sequence.is_empty() {
        let _ = send_message(
            &sender,
            Message::Finished(Err("GaleX video has no frames".into())),
            &stop,
        );
        return;
    }
    let cycle_ms = cycle_ms.max(1);
    let mut pass = 0u64;
    loop {
        for frame in sequence {
            if stop.load(Ordering::Acquire) {
                return;
            }
            let pts_ms = frame.pts_ms.saturating_add(pass.saturating_mul(cycle_ms));
            if !send_message(
                &sender,
                Message::Frame(DecodedFrame {
                    pixels: Arc::clone(&frame.pixels),
                    pts_seconds: pts_ms as f64 / 1000.0,
                    duplicate: frame.duplicate,
                }),
                &stop,
            ) {
                return;
            }
        }
        if !repeat {
            break;
        }
        pass = pass.saturating_add(1);
    }
    let _ = send_message(&sender, Message::Finished(Ok(())), &stop);
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

fn maybe_scale(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32, scale: bool) -> Vec<u8> {
    if !scale || (sw == dw && sh == dh) {
        return src.to_vec();
    }
    resample_nearest(src, sw, sh, dw, dh)
}

fn resample_nearest(rgba: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let mut out = vec![0u8; (dw as usize) * (dh as usize) * 4];
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return out;
    }
    for y in 0..dh as usize {
        let sy = y * sh as usize / dh as usize;
        for x in 0..dw as usize {
            let sx = x * sw as usize / dw as usize;
            let si = (sy * sw as usize + sx) * 4;
            let di = (y * dw as usize + x) * 4;
            if si + 4 <= rgba.len() {
                out[di..di + 4].copy_from_slice(&rgba[si..si + 4]);
            }
        }
    }
    out
}
