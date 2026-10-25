#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InterpolationMode {
    #[default]
    Off,
    Crossfade,
    #[cfg(feature = "nvidia-optical-flow")]
    OpticalFlow,
}

impl InterpolationMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "off" => Some(Self::Off),
            "crossfade" => Some(Self::Crossfade),
            #[cfg(feature = "nvidia-optical-flow")]
            "optical-flow" => Some(Self::OpticalFlow),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Crossfade => "crossfade",
            #[cfg(feature = "nvidia-optical-flow")]
            Self::OpticalFlow => "optical-flow",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolation_mode_names_roundtrip() {
        #[cfg(feature = "nvidia-optical-flow")]
        let modes = vec![
            InterpolationMode::Off,
            InterpolationMode::Crossfade,
            InterpolationMode::OpticalFlow,
        ];
        #[cfg(not(feature = "nvidia-optical-flow"))]
        let modes = vec![InterpolationMode::Off, InterpolationMode::Crossfade];
        for mode in modes {
            assert_eq!(InterpolationMode::parse(mode.as_str()), Some(mode));
        }
        assert_eq!(InterpolationMode::parse("motion"), None);
        assert_eq!(InterpolationMode::parse("unsupported"), None);
    }
}
