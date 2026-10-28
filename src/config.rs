//! Configuración: `$XDG_CONFIG_HOME/livenyan/config.toml` (opcional) + overrides por CLI.

use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct Config {
    pub window: WindowCfg,
    pub game: GameCfg,
    pub text: TextCfg,
    pub keys: KeysCfg,
    pub assets: AssetsCfg,
    pub audio: AudioCfg,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WindowCfg {
    /// Tamaño inicial de la ventana; 0 = resolución lógica del juego.
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
    pub vsync: bool,
    pub interpolate_animations: bool,
    /// Escalado del present (letterbox): "linear", "nearest", "sharp" o "off" (=linear).
    pub scale_filter: String,
    /// Driver de SDL; por defecto usa Vulkan y cae al renderer automático si no está disponible.
    pub renderer: String,
    /// Activa NVIDIA Optical Flow al compilar con la feature correspondiente.
    pub optical_flow: bool,
    /// Muestra el medidor de cuadros presentados.
    pub fps_meter: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct GameCfg {
    /// Duración (ms) del placeholder de Cinema (vídeo) cuando no se conoce; clic lo salta.
    pub cinema_placeholder_ms: f64,
    /// Fuerza la resolución lógica (0 = autodetectar, si falla 800x600).
    pub logical_width: u32,
    pub logical_height: u32,
    /// Fuerza el script de entrada.
    pub entry: Option<String>,
}

impl Default for GameCfg {
    fn default() -> Self {
        Self {
            cinema_placeholder_ms: 3000.0,
            logical_width: 0,
            logical_height: 0,
            entry: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TextCfg {
    /// Ruta de fuente TTF/OTF/TTC; vacío = buscar con fontconfig.
    pub font: Option<PathBuf>,
    pub font_index: u32,
    pub size: f32,
    /// Caracteres por segundo; 0 = instantáneo.
    pub speed_cps: f32,
    pub color: [u8; 4],
    pub shadow: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct KeysCfg {
    /// Nombres de tecla SDL (SDL_GetKeyName), p. ej. "Return", "Space", "F11".
    pub advance: Vec<String>,
    pub fullscreen: Vec<String>,
    pub quit: Vec<String>,
    pub skip: Vec<String>,
    pub menu_up: Vec<String>,
    pub menu_down: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AssetsCfg {
    /// Hilos de decodificación; 0 = automático.
    pub threads: usize,
    /// Comandos por delante del PC que se escanean para prefetch.
    pub prefetch_lookahead: usize,
    /// Límite blando de caché en MiB.
    pub cache_mb: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AudioCfg {
    pub enabled: bool,
    pub volume: f32,
}

impl Default for WindowCfg {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            fullscreen: false,
            vsync: true,
            interpolate_animations: false,
            scale_filter: "sharp".into(),
            renderer: "vulkan".into(),
            optical_flow: false,
            fps_meter: false,
        }
    }
}
impl Default for TextCfg {
    fn default() -> Self {
        Self {
            font: None,
            font_index: 0,
            size: 24.0,
            speed_cps: 60.0,
            color: [255, 255, 255, 255],
            shadow: true,
        }
    }
}
fn v(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}
impl Default for KeysCfg {
    fn default() -> Self {
        Self {
            advance: v(&["Return", "Space", "Keypad Enter"]),
            fullscreen: v(&["F11"]),
            quit: v(&["Escape"]),
            skip: v(&["Left Ctrl", "Right Ctrl"]),
            menu_up: v(&["Up"]),
            menu_down: v(&["Down"]),
        }
    }
}
impl Default for AssetsCfg {
    fn default() -> Self {
        Self {
            threads: 0,
            prefetch_lookahead: 200,
            cache_mb: 768,
        }
    }
}
impl Default for AudioCfg {
    fn default() -> Self {
        Self {
            enabled: true,
            volume: 1.0,
        }
    }
}

impl Config {
    pub fn default_path() -> PathBuf {
        crate::util::config_dir().join("config.toml")
    }
    /// Carga la config; si no existe devuelve valores por defecto.
    pub fn load(path: Option<&Path>) -> Result<Self, String> {
        let p = path
            .map(Path::to_path_buf)
            .unwrap_or_else(Self::default_path);
        match std::fs::read_to_string(&p) {
            Ok(s) => toml::from_str(&s).map_err(|e| format!("{}: {e}", p.display())),
            Err(_) if path.is_none() => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", p.display())),
        }
    }

    pub fn threads(&self) -> usize {
        if self.assets.threads > 0 {
            self.assets.threads
        } else {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
                .clamp(2, 8)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::WindowCfg;

    #[test]
    fn window_defaults_to_vulkan_renderer() {
        assert_eq!(WindowCfg::default().renderer, "vulkan");
    }
}
