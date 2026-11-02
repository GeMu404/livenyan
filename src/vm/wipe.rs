//! Ids `WP_*` del Flip (efecto de pantalla).
//!
//! **fuente primaria:** `docs/livenovel/graphic.html` (tabla del comando Flip /
//! 画面効果). El orden de la tabla se toma como id entero 0..N; `0`/`1`
//! coinciden con el uso actual en livenyan (`NONE` / `FADE`). Otros valores
//! numéricos: **no confirmado** frente a un dump de constantes del runtime.
//!
//! Política livenyan (`COMPATIBILITY.md` I6/I7):
//! - `WP_NONE`: corte.
//! - `WP_FADE` y los wipes restantes (I6): las dos superficies, sin tocar `ALPHA`.
//! - `WP_BLINDH` / `WP_BLINDV`: franjas con ancho y retraso entre franjas del manual.
//! - `WP_WHITE` / `WP_BLACK` / `WP_FLASH`: overlay de color
//!   (**interpretación**: manual describe 表→色→裏 / 白→裏).

use std::collections::HashSet;

/// Nombres `WP_*` en el orden de `graphic.html` (Flip / 画面効果).
pub const WP_NAMES: &[&str] = &[
    "WP_NONE",        // 0  ワイプなし
    "WP_FADE",        // 1  裏が(移動しながら)フェードイン
    "WP_BLINDH",      // 2
    "WP_BLINDV",      // 3
    "WP_CURTAINH",    // 4
    "WP_CURTAINV",    // 5
    "WP_SCROLLH",     // 6
    "WP_SCROLLV",     // 7
    "WP_GRID",        // 8
    "WP_GRIDH",       // 9
    "WP_GRIDV",       // 10
    "WP_DITHER",      // 11
    "WP_WHITE",       // 12  表→白→裏
    "WP_BLACK",       // 13  表→黒→裏
    "WP_FLASH",       // 14  白→裏
    "WP_MOSAIC",      // 15
    "WP_SUDAREH",     // 16
    "WP_SUDAREV",     // 17
    "WP_SPOT",        // 18
    "WP_MASKFADE",    // 19
    "WP_MASKWHITE",   // 20
    "WP_MASKBLACK",   // 21
    "WP_SMALL",       // 22
    "WP_BIG",         // 23
    "WP_RIPPLE",      // 24
    "WP_BLURWHITE",   // 25
    "WP_BLURBLACK",   // 26
    "WP_TWISTH",      // 27
    "WP_TWISTV",      // 28
    "WP_CRACK",       // 29
    "WP_ZOOMIN",      // 30
    "WP_CLOCKHAND",   // 31
    "WP_RUBBERH",     // 32
    "WP_RUBBERV",     // 33
    "WP_FAN",         // 34
    "WP_WFAN",        // 35
    "WP_CIRCLE",      // 36
    "WP_BLOCKCOIL",   // 37
    "WP_BLOCKRANDOM", // 38
];

/// Nombre documentado para `wipe`, o `None` si el id no está en la tabla.
pub fn wipe_name(wipe: i64) -> Option<&'static str> {
    let i = usize::try_from(wipe).ok()?;
    WP_NAMES.get(i).copied()
}

/// Etiqueta para logs: `12 WP_WHITE` o solo el número si es desconocido.
pub fn wipe_label(wipe: i64) -> String {
    match wipe_name(wipe) {
        Some(n) => format!("{wipe} {n}"),
        None => wipe.to_string(),
    }
}

/// Ids con overlay de color dedicado (graphic.html).
pub const WP_BLINDH: i64 = 2;
pub const WP_BLINDV: i64 = 3;
pub const WP_WHITE: i64 = 12;
pub const WP_BLACK: i64 = 13;
pub const WP_FLASH: i64 = 14;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WipeKind {
    Fade,
    Blind {
        horizontal: bool,
        span: i32,
        appear_ms: f64,
        reverse: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Strip {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

pub fn blind_strips(
    horizontal: bool,
    span: i32,
    appear_ms: f64,
    reverse: bool,
    elapsed_ms: f64,
    dur_ms: f64,
    screen_w: i32,
    screen_h: i32,
) -> Vec<Strip> {
    let span = span.max(1);
    let dur = dur_ms.max(1.0);
    let elapsed = elapsed_ms.clamp(0.0, dur);
    let extent = if horizontal { screen_h } else { screen_w };
    if extent <= 0 || screen_w <= 0 || screen_h <= 0 {
        return Vec::new();
    }

    let count = (extent + span - 1) / span;
    let delay = appear_ms.max(0.0);
    let last_start = delay * f64::from(count - 1);
    let (opening_ms, stagger_ms) = if last_start + 1.0 < dur {
        (dur - last_start, delay)
    } else {
        (dur, 0.0)
    };
    let mut strips = Vec::new();
    for index in 0..count {
        let time_index = if reverse { count - 1 - index } else { index };
        let start = f64::from(time_index) * stagger_ms;
        let progress = ((elapsed - start) / opening_ms.max(1.0)).clamp(0.0, 1.0);
        let origin = index * span;
        let thickness = span.min(extent - origin);
        let revealed = ((progress * f64::from(span)).round() as i32).clamp(0, thickness);
        if revealed == 0 {
            continue;
        }

        if horizontal {
            let (y, h) = if reverse {
                (origin + thickness - revealed, revealed)
            } else {
                (origin, revealed)
            };
            strips.push(Strip {
                x: 0,
                y,
                w: screen_w,
                h,
            });
        } else {
            let (x, w) = if reverse {
                (origin + thickness - revealed, revealed)
            } else {
                (origin, revealed)
            };
            strips.push(Strip {
                x,
                y: 0,
                w,
                h: screen_h,
            });
        }
    }
    strips
}

pub fn blind_complement_strips(
    horizontal: bool,
    span: i32,
    revealed: &[Strip],
    screen_w: i32,
    screen_h: i32,
) -> Vec<Strip> {
    let span = span.max(1);
    let extent = if horizontal { screen_h } else { screen_w };
    if extent <= 0 || screen_w <= 0 || screen_h <= 0 {
        return Vec::new();
    }

    let count = (extent + span - 1) / span;
    (0..count)
        .filter_map(|index| {
            let origin = index * span;
            let thickness = span.min(extent - origin);
            let shown = revealed.iter().find(|strip| {
                if horizontal {
                    strip.x == 0
                        && strip.w == screen_w
                        && strip.y >= origin
                        && strip.y < origin + thickness
                } else {
                    strip.y == 0
                        && strip.h == screen_h
                        && strip.x >= origin
                        && strip.x < origin + thickness
                }
            });
            let (start, end) = match (horizontal, shown) {
                (_, None) => (origin, origin + thickness),
                (true, Some(strip)) if strip.y == origin => (origin + strip.h, origin + thickness),
                (true, Some(strip)) => (origin, strip.y),
                (false, Some(strip)) if strip.x == origin => (origin + strip.w, origin + thickness),
                (false, Some(strip)) => (origin, strip.x),
            };
            (end > start).then_some(if horizontal {
                Strip {
                    x: 0,
                    y: start,
                    w: screen_w,
                    h: end - start,
                }
            } else {
                Strip {
                    x: start,
                    y: 0,
                    w: end - start,
                    h: screen_h,
                }
            })
        })
        .collect()
}

/// Fundido de Flip sobre las dos superficies. No toca `ALPHA` de los objetos.
///
/// `graphic.html`: `WP_FADE` es la de atrás que entra. La de adelante sale.
/// El `Time` es el del comando.
#[derive(Debug, Clone)]
pub struct WipeFade {
    pub started_at: f64,
    pub dur_ms: f64,
    /// Solo en la superficie vieja.
    pub out: HashSet<String>,
    /// Solo en la superficie nueva.
    pub inn: HashSet<String>,
    pub kind: WipeKind,
}

impl WipeFade {
    pub fn progress(&self, now: f64) -> f64 {
        ((now - self.started_at) / self.dur_ms.max(1.0)).clamp(0.0, 1.0)
    }
}

/// Overlay de wipe WHITE/BLACK/FLASH.
#[derive(Debug, Clone)]
pub struct WipeFlash {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub started_at: f64,
    pub dur_ms: f64,
    /// `true` = pico a mitad (表→色→裏); `false` = decae desde opaco (白→裏).
    pub peak_mid: bool,
}

impl WipeFlash {
    pub fn white_black(now: f64, dur_ms: f64, white: bool) -> Self {
        let (r, g, b) = if white { (255, 255, 255) } else { (0, 0, 0) };
        Self {
            r,
            g,
            b,
            started_at: now,
            dur_ms: dur_ms.max(1.0),
            peak_mid: true,
        }
    }

    pub fn flash(now: f64, dur_ms: f64) -> Self {
        Self {
            r: 255,
            g: 255,
            b: 255,
            started_at: now,
            dur_ms: dur_ms.max(1.0),
            peak_mid: false,
        }
    }

    /// RGBA del overlay en `now`, o `None` si ya terminó.
    pub fn rgba(&self, now: f64) -> Option<(u8, u8, u8, u8)> {
        let p = ((now - self.started_at) / self.dur_ms).clamp(0.0, 1.0);
        if p >= 1.0 {
            return None;
        }
        let a = if self.peak_mid {
            // Triángulo 0→1→0
            let t = if p < 0.5 { p * 2.0 } else { (1.0 - p) * 2.0 };
            (t * 255.0).round() as u8
        } else {
            ((1.0 - p) * 255.0).round() as u8
        };
        Some((self.r, self.g, self.b, a))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blind_outgoing_strips_are_the_exact_complement_at_every_progress() {
        let (screen_w, screen_h, span, duration) = (800, 600, 20, 200.0);
        for horizontal in [false, true] {
            for reverse in [false, true] {
                for elapsed in [0.0, 1.0, 10.0, 50.0, 100.0, 199.0, 200.0] {
                    let revealed = blind_strips(
                        horizontal, span, 1.0, reverse, elapsed, duration, screen_w, screen_h,
                    );
                    let outgoing =
                        blind_complement_strips(horizontal, span, &revealed, screen_w, screen_h);
                    let revealed_area: i64 = revealed
                        .iter()
                        .map(|strip| i64::from(strip.w) * i64::from(strip.h))
                        .sum();
                    let outgoing_area: i64 = outgoing
                        .iter()
                        .map(|strip| i64::from(strip.w) * i64::from(strip.h))
                        .sum();
                    assert_eq!(
                        revealed_area + outgoing_area,
                        i64::from(screen_w) * i64::from(screen_h),
                        "horizontal={horizontal} reverse={reverse} elapsed={elapsed}"
                    );
                }
            }
        }
    }

    #[test]
    fn none_and_fade_match_existing_ids() {
        assert_eq!(wipe_name(0), Some("WP_NONE"));
        assert_eq!(wipe_name(1), Some("WP_FADE"));
        assert_eq!(wipe_name(12), Some("WP_WHITE"));
        assert_eq!(wipe_name(38), Some("WP_BLOCKRANDOM"));
        assert_eq!(wipe_name(39), None);
        assert_eq!(wipe_name(-1), None);
    }
}
