#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SliderPart {
    pub source_size: (u32, u32),
    pub cell_size: (u32, u32),
    pub frames: u32,
    horizontal: bool,
}

impl SliderPart {
    pub fn new(size: Option<(u32, u32)>, expected_frames: u32, horizontal: bool) -> Option<Self> {
        let (w, h) = size?;
        if w == 0 || h == 0 {
            return None;
        }
        let axis_size = if horizontal { w } else { h };
        let frames = if axis_size >= expected_frames && axis_size % expected_frames == 0 {
            expected_frames
        } else if expected_frames == 6 && axis_size >= 2 && axis_size % 2 == 0 {
            2
        } else {
            1
        };
        let cell_size = if horizontal {
            (w / frames, h)
        } else {
            (w, h / frames)
        };
        Some(Self {
            source_size: (w, h),
            cell_size,
            frames,
            horizontal,
        })
    }

    pub fn source_rect(self, frame: u32) -> (i32, i32, u32, u32) {
        let frame = frame.min(self.frames.saturating_sub(1));
        if self.horizontal {
            (
                (frame * self.cell_size.0) as i32,
                0,
                self.cell_size.0,
                self.cell_size.1,
            )
        } else {
            (
                0,
                (frame * self.cell_size.1) as i32,
                self.cell_size.0,
                self.cell_size.1,
            )
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SliderSize {
    pub width: u32,
    pub height: u32,
}

pub fn slider_size(
    width: i64,
    height: i64,
    horizontal: bool,
    bar: Option<SliderPart>,
    thumb: Option<SliderPart>,
    buttons: Option<SliderPart>,
) -> SliderSize {
    let parts = [bar, thumb, buttons];
    let along = |part: SliderPart| {
        if horizontal {
            part.cell_size.0
        } else {
            part.cell_size.1
        }
    };
    let across = |part: SliderPart| {
        if horizontal {
            part.cell_size.1
        } else {
            part.cell_size.0
        }
    };
    let track_length = bar
        .map(along)
        .or_else(|| parts.into_iter().flatten().map(along).max())
        .unwrap_or(0);
    let cross_size = parts.into_iter().flatten().map(across).max().unwrap_or(0);
    let (default_width, default_height) = if horizontal {
        (track_length, cross_size)
    } else {
        (cross_size, track_length)
    };
    SliderSize {
        width: if width > 0 {
            width as u32
        } else {
            default_width
        },
        height: if height > 0 {
            height as u32
        } else {
            default_height
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_standard_horizontal_slider_sheets_into_cells() {
        let bar = SliderPart::new(Some((300, 6)), 2, true).unwrap();
        let thumb = SliderPart::new(Some((39, 16)), 3, true).unwrap();
        let buttons = SliderPart::new(Some((96, 16)), 6, true).unwrap();
        assert_eq!(bar.cell_size, (150, 6));
        assert_eq!(thumb.cell_size, (13, 16));
        assert_eq!(buttons.cell_size, (16, 16));
        assert_eq!(buttons.source_rect(4), (64, 0, 16, 16));
        assert_eq!(
            slider_size(0, 0, true, Some(bar), Some(thumb), Some(buttons)),
            SliderSize {
                width: 150,
                height: 16
            }
        );
    }

    #[test]
    fn supports_vertical_sprite_sheets_and_explicit_dimensions() {
        let bar = SliderPart::new(Some((6, 300)), 2, false).unwrap();
        let thumb = SliderPart::new(Some((16, 39)), 3, false).unwrap();
        let buttons = SliderPart::new(Some((16, 96)), 6, false).unwrap();
        assert_eq!(bar.cell_size, (6, 150));
        assert_eq!(thumb.cell_size, (16, 13));
        assert_eq!(buttons.cell_size, (16, 16));
        assert_eq!(buttons.source_rect(4), (0, 64, 16, 16));
        assert_eq!(
            slider_size(120, 200, false, Some(bar), Some(thumb), Some(buttons)),
            SliderSize {
                width: 120,
                height: 200
            }
        );
    }
}
