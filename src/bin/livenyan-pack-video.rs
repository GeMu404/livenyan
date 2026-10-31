//! Precocina un vídeo a GaleX anim (piso portable Movie).
//!
//! Uso: `livenyan-pack-video entrada.wmv salida.gal`
//! Requiere `ffmpeg` y `ffprobe` en PATH (solo en el host de empaquetado).

use livenyan::gal;
use std::env;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;

fn main() {
    let mut args = env::args().skip(1);
    let Some(input) = args.next() else {
        eprintln!("uso: livenyan-pack-video <entrada> <salida.gal>");
        std::process::exit(2);
    };
    let Some(output) = args.next() else {
        eprintln!("uso: livenyan-pack-video <entrada> <salida.gal>");
        std::process::exit(2);
    };
    match pack(&PathBuf::from(input), &PathBuf::from(output)) {
        Ok(stats) => println!("{stats}"),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

fn pack(input: &PathBuf, output: &PathBuf) -> Result<String, String> {
    let (width, height) = probe(input)?;
    let frame_bytes = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| "video too large".to_string())?;

    let mut child = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "info", "-nostdin", "-i"])
        .arg(input)
        .args([
            "-an",
            "-sn",
            "-dn",
            "-vf",
            "format=rgba,showinfo",
            "-fps_mode",
            "passthrough",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "pipe:1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start ffmpeg: {e}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "ffmpeg has no stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "ffmpeg has no stderr".to_string())?;
    let (pts_tx, pts_rx) = mpsc::channel::<f64>();
    let pts_thread = std::thread::spawn(move || read_pts(stderr, pts_tx));

    let mut unique: Vec<Vec<u8>> = Vec::new();
    let mut delays: Vec<u32> = Vec::new();
    let mut prev: Option<Vec<u8>> = None;
    let mut prev_pts: Option<f64> = None;
    let mut total_in = 0u32;
    let mut reader = stdout;

    loop {
        let mut buf = vec![0u8; frame_bytes];
        match reader.read_exact(&mut buf) {
            Ok(()) => {
                let pts = pts_rx
                    .recv()
                    .map_err(|_| "ffmpeg closed pts channel".to_string())?;
                total_in += 1;
                if let (Some(prev_buf), Some(p0)) = (prev.as_ref(), prev_pts) {
                    if prev_buf == &buf {
                        // Hold: alargar el último delay.
                        if let Some(d) = delays.last_mut() {
                            let dt = ((pts - p0) * 1000.0).round().max(1.0) as u32;
                            *d = d.saturating_add(dt);
                        }
                        prev_pts = Some(pts);
                        continue;
                    }
                    let dt = ((pts - p0) * 1000.0).round().max(1.0) as u32;
                    if let Some(d) = delays.last_mut() {
                        *d = dt;
                    }
                }
                unique.push(buf.clone());
                delays.push(1);
                prev = Some(buf);
                prev_pts = Some(pts);
            }
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(format!("read frame: {e}")),
        }
    }

    let status = child.wait().map_err(|e| format!("wait ffmpeg: {e}"))?;
    let _ = pts_thread.join();
    if !status.success() {
        return Err(format!("ffmpeg exited with {status}"));
    }
    if unique.is_empty() {
        return Err("no frames decoded".into());
    }
    // Último delay: al menos 1 ms (hold ya acumulado si hubo dups al final).
    if let Some(d) = delays.last_mut()
        && *d == 0
    {
        *d = 1;
    }

    let gal = gal::encode_galex_anim(width, height, &unique, &delays).map_err(|e| e.to_string())?;
    std::fs::write(output, &gal).map_err(|e| format!("write {}: {e}", output.display()))?;
    let ratio = if total_in == 0 {
        0.0
    } else {
        100.0 * (1.0 - unique.len() as f64 / f64::from(total_in))
    };
    Ok(format!(
        "{}→{}: {width}x{height}, {total_in} in → {} unique ({ratio:.1}% dedup), {} bytes",
        input.display(),
        output.display(),
        unique.len(),
        gal.len()
    ))
}

fn probe(path: &PathBuf) -> Result<(u32, u32), String> {
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
        .map_err(|e| format!("ffprobe: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "ffprobe failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let (w, h) = text
        .trim()
        .split_once('x')
        .ok_or_else(|| format!("bad ffprobe size: {text:?}"))?;
    Ok((
        w.trim().parse().map_err(|_| format!("bad width {w:?}"))?,
        h.trim().parse().map_err(|_| format!("bad height {h:?}"))?,
    ))
}

fn read_pts<R: Read>(pipe: R, tx: mpsc::Sender<f64>) {
    for line in BufReader::new(pipe).lines().map_while(Result::ok) {
        if let Some((_, rest)) = line.split_once("pts_time:") {
            let value = rest.split_whitespace().next().unwrap_or("");
            if let Ok(pts) = value.parse::<f64>()
                && pts.is_finite()
                && tx.send(pts).is_err()
            {
                return;
            }
        }
    }
}
