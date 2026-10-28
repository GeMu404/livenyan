//! Reader and sampler for LiveCinema 108-112 image and text tracks.

use encoding_rs::SHIFT_JIS;

const MAX_BYTES: usize = 1024 * 1024;
const MAX_CLIPS: usize = 4096;
const MAX_FRAMES: u32 = 216_000;

#[derive(Debug, Clone)]
pub struct Movie {
    pub width: u32,
    pub height: u32,
    pub duration_frames: u32,
    pub clips: Vec<Track>,
}

#[derive(Debug, Clone)]
pub struct Track {
    pub previous: Option<usize>,
    pub start: u32,
    pub duration: u32,
    pub order: i32,
    /// Punto de anclaje en píxeles de la fuente (centro de escala/rotación).
    /// Posición del track = dónde cae este pivot en el lienzo del cinema.
    pub pivot: [i32; 2],
    pub position_start: [i32; 2],
    pub position_end: [i32; 2],
    pub position_curve: [u8; 2],
    pub scale_x: [f64; 2],
    pub scale_y: [f64; 2],
    pub scale_curve: [u8; 2],
    pub opacity_start: i32,
    pub opacity_end: i32,
    pub content: TrackContent,
}

#[derive(Debug, Clone)]
pub enum TrackContent {
    Image(String),
    Text {
        value: String,
        font_height: u32,
        font_style: i32,
        color: i32,
    },
    Opaque,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sprite {
    pub media: String,
    pub x: f32,
    pub y: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    pub alpha: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextSprite {
    pub value: String,
    pub x: f32,
    pub y: f32,
    pub alpha: u8,
    pub font_height: u32,
    pub font_style: i32,
    pub color: i32,
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], String> {
        let end = self.offset.checked_add(len).ok_or("LCM offset overflow")?;
        let bytes = self.bytes.get(self.offset..end).ok_or_else(|| {
            format!(
                "truncated LCM at byte {} (need {len}, file has {})",
                self.offset,
                self.bytes.len()
            )
        })?;
        self.offset = end;
        Ok(bytes)
    }

    fn int(&mut self) -> Result<i32, String> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn real(&mut self) -> Result<f64, String> {
        let value = f64::from_le_bytes(self.take(8)?.try_into().unwrap());
        if !value.is_finite() || value.abs() > 65_536.0 {
            return Err("invalid LCM transform value".into());
        }
        Ok(value)
    }

    fn point(&mut self) -> Result<[i32; 2], String> {
        Ok([self.int()?, self.int()?])
    }

    fn flag(&mut self) -> Result<bool, String> {
        match self.take(1)?[0] {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err("invalid LCM flag".into()),
        }
    }

    fn curve(&mut self) -> Result<u8, String> {
        let offset = self.offset;
        let value = self.take(1)?[0];
        if value > 2 {
            return Err(format!(
                "unsupported LCM interpolation curve {value} at byte {offset}"
            ));
        }
        Ok(value)
    }

    fn media(&mut self) -> Result<String, String> {
        let value = self.string()?;
        if value.contains(':') || value.starts_with(['/', '\\']) {
            return Err("invalid LCM media path".into());
        }
        Ok(value.replace('/', "\\"))
    }

    fn string(&mut self) -> Result<String, String> {
        let offset = self.offset;
        let len = self.int()?;
        if !(0..=4096).contains(&len) {
            return Err(format!(
                "invalid LCM string length {len} at offset {offset}"
            ));
        }
        let (value, errors) = SHIFT_JIS.decode_without_bom_handling(self.take(len as usize)?);
        if errors || value.contains('\0') {
            return Err("invalid LCM text encoding".into());
        }
        Ok(value.into_owned())
    }
}

fn common_header(bytes: &[u8], at: usize, index: usize, version: u32, tracks: &[Track]) -> bool {
    let Some(header) = bytes.get(at..at + 28) else {
        return false;
    };
    let kind = header[0];
    let is_text = kind == 1;
    if kind != 0 && !is_text {
        return false;
    }
    let int_at = |offset: usize| i32::from_le_bytes(header[offset..offset + 4].try_into().unwrap());
    let previous = int_at(1);
    let duration = int_at(5);
    let start = int_at(13);
    let period = int_at(17);
    if previous < -1
        || previous >= index as i32
        || !(1..=MAX_FRAMES as i32).contains(&duration)
        || !(0..=MAX_FRAMES as i32).contains(&start)
        || !(0..=MAX_FRAMES as i32).contains(&period)
        || header[25..28].iter().any(|flag| *flag != 0)
    {
        return false;
    }
    if previous >= 0 {
        let predecessor = &tracks[previous as usize];
        if !matches!(
            (&predecessor.content, if is_text { 1 } else { 0 }),
            (TrackContent::Image(_), 0) | (TrackContent::Text { .. }, 1)
        ) {
            return false;
        }
    }
    let header_size = if version < 112 { 139 } else { 163 };
    let Some(payload) = at.checked_add(header_size) else {
        return false;
    };
    let Some(length_bytes) = bytes.get(payload..payload + 4) else {
        return false;
    };
    let length = i32::from_le_bytes(length_bytes.try_into().unwrap());
    if !(0..=4096).contains(&length) {
        return false;
    }
    let content_start = payload + 4;
    let Some(content_end) = content_start.checked_add(length as usize) else {
        return false;
    };
    let Some(content_bytes) = bytes.get(content_start..content_end) else {
        return false;
    };
    let (content, errors) = SHIFT_JIS.decode_without_bom_handling(content_bytes);
    if errors || content.contains('\0') {
        return false;
    }
    match kind {
        0 if content.contains(':') || content.starts_with(['/', '\\']) => return false,
        1 => {
            let Some(style) = bytes.get(content_end..content_end + 19) else {
                return false;
            };
            let font_height = i32::from_le_bytes(style[5..9].try_into().unwrap());
            if !(1..=512).contains(&font_height) {
                return false;
            }
        }
        _ => {}
    }
    true
}

pub fn parse(bytes: &[u8]) -> Result<Movie, String> {
    if bytes.len() > MAX_BYTES {
        return Err("LCM exceeds 1 MiB limit".into());
    }
    let mut r = Reader { bytes, offset: 0 };
    let signature = r.take(10)?;
    let version_bytes = r.take(3)?;
    let version = std::str::from_utf8(version_bytes)
        .ok()
        .and_then(|text| text.parse::<u32>().ok())
        .ok_or("invalid LCM version")?;
    if signature != b"LiveCinema" || !(108..=112).contains(&version) {
        return Err(format!(
            "unsupported LCM version {version} (supported: 108-112)"
        ));
    }
    let width = r.int()?;
    let height = r.int()?;
    if !(1..=8192).contains(&width) || !(1..=8192).contains(&height) {
        return Err("invalid LCM dimensions".into());
    }
    let background = r.int()?;
    if !(-1..=0x00ff_ffff).contains(&background) {
        return Err("invalid LCM background color".into());
    }
    let anchored = r.flag()?;
    let anchor = r.point()?;
    let _parameter = r.int()?;
    // Ancla de cinema: aún no aplicada al layout del lienzo; no bloquear el parseo.
    let _ = (anchored, anchor);

    let count = r.int()?;
    if !(0..=MAX_CLIPS as i32).contains(&count) {
        return Err("invalid LCM clip count".into());
    }
    let mut clips: Vec<Track> = Vec::with_capacity(count as usize);
    let mut duration_frames = 0;
    for index in 0..count as usize {
        let kind = r.take(1)?[0];
        let is_text = kind == 1;
        let is_opaque_legacy = version < 112 && kind == 2;
        if kind != 0 && !is_text && !is_opaque_legacy {
            return Err(format!("unsupported LCM clip type {kind} at index {index}"));
        }
        let previous_raw = r.int()?;
        if previous_raw < -1 || previous_raw >= index as i32 {
            return Err("invalid LCM predecessor reference".into());
        }
        let previous = (previous_raw >= 0).then_some(previous_raw as usize);
        let duration = r.int()?;
        let order = r.int()?;
        let authored_start = r.int()?;
        let period = r.int()?;
        let _clip_parameter = r.int()?;
        // Flags de clip: leídos y omitidos (no bloquean; semántica no confirmada).
        let _flags = [r.flag()?, r.flag()?, r.flag()?];
        if !(1..=MAX_FRAMES as i32).contains(&duration)
            || !(0..=MAX_FRAMES as i32).contains(&period)
        {
            return Err("invalid LCM clip timing".into());
        }
        let start = if let Some(prev) = previous {
            clips[prev]
                .start
                .checked_add(clips[prev].duration)
                .ok_or("LCM timeline overflow")?
        } else if authored_start >= 0 {
            authored_start as u32
        } else {
            return Err("negative LCM clip start".into());
        };
        let end = start
            .checked_add(duration as u32)
            .ok_or("LCM timeline overflow")?;
        if end > MAX_FRAMES {
            return Err("LCM timeline exceeds frame limit".into());
        }

        if previous.is_some_and(|prev| {
            !matches!(
                (&clips[prev].content, if is_text { 1 } else { 0 }),
                (TrackContent::Image(_), 0) | (TrackContent::Text { .. }, 1)
            )
        }) {
            return Err("invalid LCM predecessor clip type".into());
        }

        if is_opaque_legacy {
            duration_frames = duration_frames.max(end);
            clips.push(Track {
                previous,
                start,
                duration: duration as u32,
                order,
                pivot: [0, 0],
                position_start: [0, 0],
                position_end: [0, 0],
                position_curve: [0, 0],
                scale_x: [1.0, 1.0],
                scale_y: [1.0, 1.0],
                scale_curve: [0, 0],
                opacity_start: 255,
                opacity_end: 255,
                content: TrackContent::Opaque,
            });
            if index + 1 < count as usize {
                let next = (r.offset..bytes.len())
                    .find(|at| common_header(bytes, *at, index + 1, version, &clips))
                    .ok_or_else(|| {
                        format!("could not locate next LCM clip after opaque legacy clip {index}")
                    })?;
                r.offset = next;
            } else {
                r.offset = bytes.len();
            }
            continue;
        }

        let pivot = r.point()?;
        let position_start = r.point()?;
        let position_end = r.point()?;
        let position_curve = [r.curve()?, r.curve()?];
        let angle = if version < 112 {
            [r.real()?, 0.0]
        } else {
            [r.real()?, r.real()?]
        };
        let scale_x = [r.real()?, r.real()?];
        let scale_x_curve = r.curve()?;
        let opacity_start = r.int()?;
        let opacity_end = r.int()?;
        let _angle_curve = r.curve()?;
        let clip_start = [r.point()?, r.point()?];
        let clip_end = [r.point()?, r.point()?];
        let _clip_curve = [r.curve()?, r.curve()?];
        let scale_y = [r.real()?, r.real()?];
        let scale_y_curve = r.curve()?;
        let extra = if version < 112 {
            [0; 4]
        } else {
            [r.int()?, r.int()?, r.int()?, r.int()?]
        };
        let content = if !is_text {
            TrackContent::Image(r.media()?)
        } else {
            let value = r.string()?;
            let style = r.take(19)?;
            let font_height = i32::from_le_bytes(style[5..9].try_into().unwrap());
            let font_style = i32::from_le_bytes(style[9..13].try_into().unwrap());
            let color = i32::from_le_bytes(style[14..18].try_into().unwrap());
            if !(1..=512).contains(&font_height) {
                return Err(format!("invalid LCM text font height at clip {index}"));
            }
            TrackContent::Text {
                value,
                font_height: font_height as u32,
                font_style,
                color,
            }
        };

        // Escala negativa no tiene sentido de dibujo actual. Ángulo/clip/extra: se
        // aceptan en el parseo; rotación y clip de fuente aún no se aplican al render
        // (pivot + scale sí). **inventado en livenyan** omitir ángulo/clip vs fallar todo el LCM.
        if scale_x
            .iter()
            .chain(scale_y.iter())
            .any(|scale| *scale < 0.0)
        {
            return Err(format!(
                "unsupported negative scale in LCM image clip {index}: scale=({scale_x:?}, {scale_y:?})"
            ));
        }
        if !(0..=255).contains(&opacity_start) || !(0..=255).contains(&opacity_end) {
            return Err(format!(
                "invalid LCM opacity in clip {index}: ({opacity_start}, {opacity_end})"
            ));
        }
        let _ = (angle, clip_start, clip_end, extra);
        duration_frames = duration_frames.max(end);
        clips.push(Track {
            previous,
            start,
            duration: duration as u32,
            order,
            pivot,
            position_start,
            position_end,
            position_curve,
            scale_x,
            scale_y,
            scale_curve: [scale_x_curve, scale_y_curve],
            opacity_start,
            opacity_end,
            content,
        });
        if is_text && index + 1 < count as usize {
            let from = r.offset;
            let next = (from..bytes.len())
                .find(|at| common_header(bytes, *at, index + 1, version, &clips))
                .ok_or_else(|| {
                    format!("could not locate next LCM text clip after index {index}")
                })?;
            r.offset = next;
        } else if is_text {
            r.offset = bytes.len();
        }
    }
    if r.offset != bytes.len() {
        return Err("unexpected trailing data in LCM".into());
    }
    Ok(Movie {
        width: width as u32,
        height: height as u32,
        duration_frames,
        clips,
    })
}

impl Movie {
    pub fn sprites_at(&self, elapsed_ms: f64) -> Vec<Sprite> {
        let mut sprites: Vec<_> = self
            .active_tracks(elapsed_ms)
            .into_iter()
            .filter_map(|(index, t, x, y, alpha, scale_x, scale_y)| {
                let TrackContent::Image(media) = &self.clips[index].content else {
                    return None;
                };
                Some((
                    t.order,
                    index,
                    Sprite {
                        media: media.clone(),
                        x: x as f32,
                        y: y as f32,
                        scale_x: scale_x as f32,
                        scale_y: scale_y as f32,
                        alpha,
                    },
                ))
            })
            .collect();
        sprites.sort_by_key(|(order, index, _)| (*order, *index));
        sprites.into_iter().map(|(_, _, sprite)| sprite).collect()
    }

    pub fn texts_at(&self, elapsed_ms: f64) -> Vec<TextSprite> {
        let mut texts: Vec<_> = self
            .active_tracks(elapsed_ms)
            .into_iter()
            .filter_map(|(index, _, x, y, alpha, _, _)| {
                let TrackContent::Text {
                    value,
                    font_height,
                    font_style,
                    color,
                } = &self.clips[index].content
                else {
                    return None;
                };
                if value.is_empty() {
                    return None;
                }
                Some((
                    self.clips[index].order,
                    index,
                    TextSprite {
                        value: value.clone(),
                        x: x as f32,
                        y: y as f32,
                        alpha,
                        font_height: *font_height,
                        font_style: *font_style,
                        color: *color,
                    },
                ))
            })
            .collect();
        texts.sort_by_key(|(order, index, _)| (*order, *index));
        texts.into_iter().map(|(_, _, text)| text).collect()
    }

    fn active_tracks(&self, elapsed_ms: f64) -> Vec<(usize, &Track, i32, i32, u8, f64, f64)> {
        let frame = ((elapsed_ms.max(0.0) * 60.0 / 1000.0).round() as u32)
            .min(self.duration_frames.saturating_sub(1));
        let mut superseded = vec![false; self.clips.len()];
        for clip in &self.clips {
            if frame >= clip.start {
                if let Some(previous) = clip.previous {
                    superseded[previous] = true;
                }
            }
        }
        self.clips
            .iter()
            .enumerate()
            .filter_map(|(index, clip)| {
                if frame < clip.start
                    || frame >= clip.start.saturating_add(clip.duration)
                    || superseded[index]
                {
                    return None;
                }
                let t = frame
                    .saturating_sub(clip.start)
                    .min(clip.duration.saturating_sub(1));
                let from = clip
                    .previous
                    .map_or(clip.position_start, |prev| self.clips[prev].position_end);
                let x = interpolate(
                    from[0],
                    clip.position_end[0],
                    clip.position_curve[0],
                    t,
                    clip.duration,
                );
                let y = interpolate(
                    from[1],
                    clip.position_end[1],
                    clip.position_curve[1],
                    t,
                    clip.duration,
                );
                let alpha_from = clip
                    .previous
                    .map_or(clip.opacity_start, |prev| self.clips[prev].opacity_end);
                let alpha = interpolate(
                    opaque(alpha_from),
                    opaque(clip.opacity_end),
                    0,
                    t,
                    clip.duration,
                )
                .clamp(0, 255) as u8;
                let scale_x_from = clip
                    .previous
                    .map_or(clip.scale_x[0], |prev| self.clips[prev].scale_x[1]);
                let scale_y_from = clip
                    .previous
                    .map_or(clip.scale_y[0], |prev| self.clips[prev].scale_y[1]);
                let scale_x = interpolate_real(
                    scale_x_from,
                    clip.scale_x[1],
                    clip.scale_curve[0],
                    t,
                    clip.duration,
                );
                let scale_y = interpolate_real(
                    scale_y_from,
                    clip.scale_y[1],
                    clip.scale_curve[1],
                    t,
                    clip.duration,
                );
                // Posición del track = dónde cae el pivot; el draw usa esquina superior-izquierda.
                let x = (x as f64 - clip.pivot[0] as f64 * scale_x).round() as i32;
                let y = (y as f64 - clip.pivot[1] as f64 * scale_y).round() as i32;
                Some((index, clip, x, y, alpha, scale_x, scale_y))
            })
            .collect()
    }
}

fn opaque(alpha: i32) -> i32 {
    if alpha == 0 { 255 } else { alpha }
}

fn interpolate(from: i32, to: i32, curve: u8, t: u32, duration: u32) -> i32 {
    if t == 0 {
        return from;
    }
    if duration <= 1 || t >= duration - 1 {
        return to;
    }
    let n = duration as f64;
    let t = t as f64;
    let progress = match curve {
        0 => t / (n - 1.0),
        1 => 1.0 - ((n - t - 1.0) * std::f64::consts::FRAC_PI_2 / n).sin(),
        _ => (t * std::f64::consts::FRAC_PI_2 / n).sin(),
    };
    (from as f64 + (to - from) as f64 * progress).round() as i32
}

fn interpolate_real(from: f64, to: f64, curve: u8, t: u32, duration: u32) -> f64 {
    if t == 0 {
        return from;
    }
    if duration <= 1 || t >= duration - 1 {
        return to;
    }
    let n = duration as f64;
    let t = t as f64;
    let progress = match curve {
        0 => t / (n - 1.0),
        1 => 1.0 - ((n - t - 1.0) * std::f64::consts::FRAC_PI_2 / n).sin(),
        _ => (t * std::f64::consts::FRAC_PI_2 / n).sin(),
    };
    from + (to - from) * progress
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push_i32(out: &mut Vec<u8>, value: i32) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    fn push_point(out: &mut Vec<u8>, point: [i32; 2]) {
        push_i32(out, point[0]);
        push_i32(out, point[1]);
    }

    fn push_real(out: &mut Vec<u8>, value: f64) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    fn simple_movie() -> Vec<u8> {
        let mut out = b"LiveCinema112".to_vec();
        push_point(&mut out, [320, 240]);
        push_i32(&mut out, -1);
        out.push(0);
        push_point(&mut out, [0, 0]);
        push_i32(&mut out, 0);
        push_i32(&mut out, 1);
        out.push(0);
        push_i32(&mut out, -1);
        push_i32(&mut out, 2);
        push_i32(&mut out, 0);
        push_i32(&mut out, 0);
        push_i32(&mut out, 1);
        push_i32(&mut out, 0);
        out.extend_from_slice(&[0, 0, 0]);
        push_point(&mut out, [0, 0]);
        push_point(&mut out, [10, 20]);
        push_point(&mut out, [30, 40]);
        out.extend_from_slice(&[0, 0]);
        push_real(&mut out, 0.0);
        push_real(&mut out, 0.0);
        push_real(&mut out, 1.0);
        push_real(&mut out, 1.0);
        out.push(0);
        push_i32(&mut out, 1);
        push_i32(&mut out, 0);
        out.push(0);
        for _ in 0..4 {
            push_point(&mut out, [0, 0]);
        }
        out.extend_from_slice(&[0, 0]);
        push_real(&mut out, 1.0);
        push_real(&mut out, 1.0);
        out.push(0);
        for _ in 0..4 {
            push_i32(&mut out, 0);
        }
        let media = "image.gal".as_bytes();
        push_i32(&mut out, media.len() as i32);
        out.extend_from_slice(media);
        out
    }

    fn text_movie() -> Vec<u8> {
        text_movie_version(112)
    }

    fn text_movie_version(version: u32) -> Vec<u8> {
        let signature = format!("LiveCinema{version}");
        let mut out = signature.into_bytes();
        push_point(&mut out, [80, 24]);
        push_i32(&mut out, -1);
        out.push(0);
        push_point(&mut out, [0, 0]);
        push_i32(&mut out, 0);
        push_i32(&mut out, 2);
        for (text, start) in [("scene A", 0), ("scene B", 2)] {
            out.push(1);
            for value in [-1, 2, 1, start, 0, 0] {
                push_i32(&mut out, value);
            }
            out.extend_from_slice(&[0; 3]);
            push_point(&mut out, [0, 0]);
            push_point(&mut out, [16, 0]);
            push_point(&mut out, [16, 0]);
            out.extend_from_slice(&[0; 2]);
            push_real(&mut out, 0.0);
            if version >= 112 {
                push_real(&mut out, 0.0);
            }
            push_real(&mut out, 1.0);
            push_real(&mut out, 1.0);
            out.push(0);
            push_i32(&mut out, 255);
            push_i32(&mut out, 255);
            out.push(0);
            for _ in 0..4 {
                push_point(&mut out, [0, 0]);
            }
            out.extend_from_slice(&[0; 2]);
            push_real(&mut out, 1.0);
            push_real(&mut out, 1.0);
            out.push(0);
            if version >= 112 {
                for _ in 0..4 {
                    push_i32(&mut out, 0);
                }
            }
            push_i32(&mut out, text.len() as i32);
            out.extend_from_slice(text.as_bytes());
            out.extend_from_slice(&[0; 5]);
            push_i32(&mut out, 24);
            push_i32(&mut out, 0);
            out.push(0);
            push_i32(&mut out, 0x00FF_FFFF);
            out.push(0);
        }
        out
    }

    fn legacy_opaque_movie() -> Vec<u8> {
        let mut out = b"LiveCinema108".to_vec();
        push_point(&mut out, [80, 24]);
        push_i32(&mut out, -1);
        out.push(0);
        push_point(&mut out, [0, 0]);
        push_i32(&mut out, 0);
        push_i32(&mut out, 1);
        out.push(2);
        for value in [-1, 1, 0, 0, 0, 0] {
            push_i32(&mut out, value);
        }
        out.extend_from_slice(&[0; 3]);
        out.extend_from_slice(&[0; 128]);
        out
    }

    #[test]
    fn parses_and_samples_simple_image_movie() {
        let movie = parse(&simple_movie()).unwrap();
        assert_eq!(
            (movie.width, movie.height, movie.duration_frames),
            (320, 240, 2)
        );
        assert_eq!(movie.sprites_at(0.0).len(), 1);
        let final_sprite = &movie.sprites_at(1000.0 / 60.0)[0];
        assert_eq!(
            (final_sprite.x, final_sprite.y, final_sprite.alpha),
            (30.0, 40.0, 255)
        );
        assert_eq!((final_sprite.scale_x, final_sprite.scale_y), (1.0, 1.0));
        assert_eq!(final_sprite.media, "image.gal");
    }

    #[test]
    fn samples_animated_scale_tracks() {
        let mut movie = parse(&simple_movie()).unwrap();
        movie.clips[0].scale_y = [0.0, 1.0];
        assert_eq!(movie.sprites_at(0.0)[0].scale_y, 0.0);
        assert_eq!(movie.sprites_at(1000.0 / 60.0)[0].scale_y, 1.0);
    }

    #[test]
    fn pivot_offsets_sprite_top_left_by_scaled_anchor() {
        let mut movie = parse(&simple_movie()).unwrap();
        movie.clips[0].pivot = [10, 5];
        movie.clips[0].scale_x = [2.0, 2.0];
        movie.clips[0].scale_y = [2.0, 2.0];
        // position_end at t=end is (30,40); top-left = pos - pivot*scale
        let sprite = &movie.sprites_at(1000.0 / 60.0)[0];
        assert_eq!((sprite.x, sprite.y), (10.0, 30.0));
        assert_eq!((sprite.scale_x, sprite.scale_y), (2.0, 2.0));
    }

    #[test]
    fn accepts_non_zero_pivot_without_rejecting_movie() {
        let mut bytes = simple_movie();
        // En simple_movie el pivot es el primer point tras los 3 flags del clip (ceros).
        // Sustituir [0,0] del pivot por [298,43]: tras flags hay push_point([0,0]) = 8 bytes.
        let marker = b"image.gal";
        let media_at = bytes
            .windows(marker.len())
            .position(|w| w == marker)
            .expect("media name");
        // pivot está 8 (point) + … atrás desde position_start; más simple: regenerar.
        let _ = (media_at, &mut bytes);
        let movie = parse(&{
            let mut out = simple_movie();
            // Bytes del pivot: inmediatamente después de 3 flags (0,0,0) del primer clip.
            // Cabecera LiveCinema112 (13) + w/h (8) + bg(4) + flag(1) + anchor(8) + param(4) + count(4)
            // = 42; clip: kind(1)+prev(4)+dur(4)+order(4)+start(4)+period(4)+param(4)+flags(3)=28 → offset 70.
            let pivot_off = 13 + 8 + 4 + 1 + 8 + 4 + 4 + 1 + 4 + 4 + 4 + 4 + 4 + 4 + 3;
            assert_eq!(&out[pivot_off..pivot_off + 8], &[0u8; 8]);
            out[pivot_off..pivot_off + 4].copy_from_slice(&298i32.to_le_bytes());
            out[pivot_off + 4..pivot_off + 8].copy_from_slice(&43i32.to_le_bytes());
            out
        })
        .unwrap();
        assert_eq!(movie.clips[0].pivot, [298, 43]);
    }

    #[test]
    fn parses_and_samples_text_tracks() {
        let movie = parse(&text_movie()).unwrap();
        assert!(movie.sprites_at(0.0).is_empty());
        assert_eq!(movie.texts_at(0.0)[0].value, "scene A");
        assert_eq!(movie.texts_at(1000.0 / 60.0)[0].value, "scene A");
        assert_eq!(movie.texts_at(2.0 * 1000.0 / 60.0)[0].value, "scene B");
    }

    #[test]
    fn parses_legacy_108_text_tracks() {
        let movie = parse(&text_movie_version(108)).unwrap();
        assert_eq!(movie.clips.len(), 2);
        assert_eq!(movie.texts_at(0.0)[0].value, "scene A");
        assert_eq!(movie.texts_at(2.0 * 1000.0 / 60.0)[0].value, "scene B");
    }

    #[test]
    fn keeps_opaque_legacy_tracks_without_breaking_movie_parsing() {
        let movie = parse(&legacy_opaque_movie()).unwrap();
        assert_eq!(movie.clips.len(), 1);
        assert!(matches!(movie.clips[0].content, TrackContent::Opaque));
    }

    #[test]
    fn rejects_unsupported_lcm_versions() {
        assert!(
            parse(b"LiveCinema107")
                .unwrap_err()
                .contains("unsupported LCM version")
        );
        assert!(
            parse(b"LiveCinema113")
                .unwrap_err()
                .contains("unsupported LCM version")
        );
    }
}
