//! Ids `WP_*` del Flip (efecto de pantalla).
//!
//! **fuente primaria:** `docs/livenovel/graphic.html` (tabla del comando Flip /
//! 画面効果). El orden de la tabla se toma como id entero 0..N; `0`/`1`
//! coinciden con el uso actual en livenyan (`NONE` / `FADE`). Otros valores
//! numéricos: **no confirmado** frente a un dump de constantes del runtime.
//!
//! Política livenyan (`COMPATIBILITY.md` I6/I7):
//! - `WP_NONE`: corte.
//! - `WP_FADE` y el resto de wipes (I6): las dos superficies, sin tocar `ALPHA`.
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
pub const WP_WHITE: i64 = 12;
pub const WP_BLACK: i64 = 13;
pub const WP_FLASH: i64 = 14;

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
    fn none_and_fade_match_existing_ids() {
        assert_eq!(wipe_name(0), Some("WP_NONE"));
        assert_eq!(wipe_name(1), Some("WP_FADE"));
        assert_eq!(wipe_name(12), Some("WP_WHITE"));
        assert_eq!(wipe_name(38), Some("WP_BLOCKRANDOM"));
        assert_eq!(wipe_name(39), None);
        assert_eq!(wipe_name(-1), None);
    }
}
