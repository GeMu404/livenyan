//! Filtro de escalado del present (letterbox lógico → ventana).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScaleFilter {
    /// Bilinear en present (suave).
    #[default]
    Linear,
    /// Nearest en assets y present (píxel duro).
    Nearest,
    /// Nearest en assets y present.
    Sharp,
    /// 1:1 lógico→ventana, sin letterbox ni HiDPI. Para upscalers externos
    /// (Lossless Scaling, Magpie, Gamescope…). **interpretación** I2b.
    Native,
}

impl ScaleFilter {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "linear" | "off" => Some(Self::Linear),
            "nearest" => Some(Self::Nearest),
            "sharp" | "crisp" => Some(Self::Sharp),
            "native" | "1x" | "external" | "ls" => Some(Self::Native),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Linear => "linear",
            Self::Nearest => "nearest",
            Self::Sharp => "sharp",
            Self::Native => "native",
        }
    }

    pub fn present_nearest(self) -> bool {
        matches!(self, Self::Nearest | Self::Sharp | Self::Native)
    }

    pub fn asset_nearest(self) -> bool {
        matches!(self, Self::Nearest | Self::Sharp | Self::Native)
    }

    /// Present a resolución lógica; el escalado lo hace otra herramienta.
    pub fn external_upscale(self) -> bool {
        matches!(self, Self::Native)
    }

    pub fn from_config(value: &str) -> Self {
        if let Some(parsed) = Self::parse(value) {
            return parsed;
        }
        log::warn!("unknown scale_filter {value:?}; using linear");
        Self::Linear
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_filter_names_roundtrip() {
        for mode in [
            ScaleFilter::Linear,
            ScaleFilter::Nearest,
            ScaleFilter::Sharp,
            ScaleFilter::Native,
        ] {
            assert_eq!(ScaleFilter::parse(mode.as_str()), Some(mode));
        }
        assert_eq!(ScaleFilter::parse("off"), Some(ScaleFilter::Linear));
        assert_eq!(ScaleFilter::parse("1x"), Some(ScaleFilter::Native));
        assert_eq!(ScaleFilter::parse("ls"), Some(ScaleFilter::Native));
        assert_eq!(ScaleFilter::parse("nope"), None);
    }
}
