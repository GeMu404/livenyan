//! Dimensiones GAL vs resolución lógica (cabecera solamente; sin píxeles).
use std::io::Read;

fn main() -> Result<(), String> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: gal_dims <game.exe|dir>")?;
    let t0 = std::time::Instant::now();
    let cfg = livenyan::config::Config::load(None).unwrap_or_default();
    let game = livenyan::game::Game::open(std::path::Path::new(&path), &cfg)?;
    let logical = game.logical;
    println!(
        "logical={}x{} (source={})",
        logical.0, logical.1, game.logical_source
    );

    let mut by_size: std::collections::BTreeMap<(u32, u32), (u32, u32)> =
        std::collections::BTreeMap::new();
    let mut samples_mismatch: Vec<(u32, u32, u32, String)> = Vec::new();
    let mut samples_match: Vec<(u32, String)> = Vec::new();
    let mut total = 0u32;
    let mut animated = 0u32;
    let mut fail = 0u32;
    let mut eq_logical = 0u32;
    let mut larger = 0u32;
    let mut smaller = 0u32;

    for name in game.data.names() {
        if !name.to_ascii_lowercase().ends_with(".gal") {
            continue;
        }
        total += 1;
        let data = match game.data.read(name) {
            Ok(d) => d,
            Err(_) => {
                fail += 1;
                continue;
            }
        };
        let Some((w, h)) = livenyan::gal::peek_size(&data) else {
            fail += 1;
            continue;
        };
        let frames = peek_frames(&data).unwrap_or(1).max(1);
        let e = by_size.entry((w, h)).or_insert((0, 0));
        e.0 += 1;
        if (w, h) == logical {
            eq_logical += 1;
        } else if w > logical.0 || h > logical.1 {
            larger += 1;
        } else {
            smaller += 1;
        }
        if frames > 1 {
            animated += 1;
            e.1 += 1;
            if (w, h) == logical {
                if samples_match.len() < 15 {
                    samples_match.push((frames, leaf(name)));
                }
            } else if samples_mismatch.len() < 40 {
                samples_mismatch.push((w, h, frames, leaf(name)));
            }
        }
    }

    println!("gal_files={total} peek_fail={fail} animated_frames_gt1={animated}");
    println!("vs_logical: EQ={eq_logical} LARGER={larger} smaller={smaller}");
    println!("--- histogram WxH (files, animated) [top 40 by count] ---");
    let mut rows: Vec<_> = by_size
        .iter()
        .map(|(&(w, h), &(n, a))| (n, a, w, h))
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.0));
    for (n, a, w, h) in rows.into_iter().take(40) {
        let tag = if (w, h) == logical {
            "EQ_logical"
        } else if w > logical.0 || h > logical.1 {
            "LARGER"
        } else {
            "smaller"
        };
        println!("{w:>4}x{h:<4}  files={n:<4} animated={a:<4}  [{tag}]");
    }

    println!("--- animated EQ logical (sample) ---");
    for (frames, name) in &samples_match {
        println!("  {}x{} frames={frames}  {name}", logical.0, logical.1);
    }
    println!("--- animated NOT logical (sample) ---");
    for (w, h, frames, name) in &samples_mismatch {
        println!("  {w}x{h} frames={frames}  {name}");
    }
    println!("elapsed_ms={:.1}", t0.elapsed().as_secs_f64() * 1000.0);
    Ok(())
}

fn leaf(name: &str) -> String {
    name.rsplit(['\\', '/']).next().unwrap_or(name).to_string()
}

fn peek_frames(d: &[u8]) -> Option<u32> {
    if d.starts_with(b"GaleX200") && d.len() > 12 {
        let hs = i32::from_le_bytes(d.get(8..12)?.try_into().ok()?) as usize;
        let z = &d[12..d.len().min(12 + hs)];
        let mut out = Vec::new();
        let _ = flate2::read::ZlibDecoder::new(z)
            .take(8192)
            .read_to_end(&mut out);
        let xml = livenyan::util::cp932(&out);
        return attr_u32(&xml, "Count").or_else(|| attr_u32(&xml, "Frames"));
    }
    if d.starts_with(b"Gale") && d.len() >= 7 + 4 + 0x14 {
        let ver: u32 = std::str::from_utf8(d.get(4..7)?).ok()?.parse().ok()?;
        if ver > 102 {
            let f = i32::from_le_bytes(d.get(11 + 0x10..11 + 0x14)?.try_into().ok()?);
            return Some(f.max(1) as u32);
        }
    }
    None
}

fn attr_u32(s: &str, key: &str) -> Option<u32> {
    let p = format!("{key}=\"");
    let i = s.find(&p)?;
    let rest = &s[i + p.len()..];
    let end = rest.find('"')?;
    rest[..end].parse().ok()
}
