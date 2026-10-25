#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InterpolationMode {
    #[default]
    Off,
    Crossfade,
}

impl InterpolationMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "off" => Some(Self::Off),
            "crossfade" => Some(Self::Crossfade),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Crossfade => "crossfade",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolation_mode_names_roundtrip() {
        for mode in [InterpolationMode::Off, InterpolationMode::Crossfade] {
            assert_eq!(InterpolationMode::parse(mode.as_str()), Some(mode));
        }
        assert_eq!(InterpolationMode::parse("motion"), None);
        assert_eq!(InterpolationMode::parse("unsupported"), None);
    }
}
