#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

impl RgbaImage {
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self, String> {
        let expected_len = width
            .checked_mul(height)
            .and_then(|size| size.checked_mul(4))
            .and_then(|size| usize::try_from(size).ok())
            .ok_or_else(|| "RGBA image dimensions are too large".to_string())?;
        if width == 0 || height == 0 {
            return Err("RGBA image dimensions must be nonzero".into());
        }
        if pixels.len() != expected_len {
            return Err(format!(
                "RGBA image has {} bytes, expected {expected_len} for {width}x{height}",
                pixels.len()
            ));
        }
        Ok(Self {
            w: width,
            h: height,
            rgba: pixels,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextureId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureFilter {
    Nearest,
    Linear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendMode {
    Opaque,
    Alpha,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    fn is_valid(self) -> bool {
        [self.x, self.y, self.width, self.height]
            .into_iter()
            .all(f32::is_finite)
            && self.width > 0.0
            && self.height > 0.0
            && (self.x + self.width).is_finite()
            && (self.y + self.height).is_finite()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color(pub [u8; 4]);

#[derive(Debug, Clone, PartialEq)]
pub enum DrawCommand {
    Clear(Color),
    Copy {
        texture: TextureId,
        source: Option<Rect>,
        destination: Rect,
        alpha: u8,
        filter: TextureFilter,
        blend: BlendMode,
    },
    FillRect {
        rect: Rect,
        color: Color,
        blend: BlendMode,
    },
    StrokeRect {
        rect: Rect,
        color: Color,
        blend: BlendMode,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct DrawList {
    width: u32,
    height: u32,
    commands: Vec<DrawCommand>,
}

impl DrawList {
    pub fn new(width: u32, height: u32) -> Result<Self, String> {
        if width == 0 || height == 0 {
            return Err("draw-list dimensions must be nonzero".into());
        }
        Ok(Self {
            width,
            height,
            commands: Vec::new(),
        })
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn commands(&self) -> &[DrawCommand] {
        &self.commands
    }

    pub fn push(&mut self, command: DrawCommand) -> Result<(), String> {
        match &command {
            DrawCommand::Copy {
                source,
                destination,
                ..
            } => {
                if !destination.is_valid() || source.is_some_and(|rect| !rect.is_valid()) {
                    return Err("draw-list copy contains invalid rectangle geometry".into());
                }
            }
            DrawCommand::FillRect { rect, .. } | DrawCommand::StrokeRect { rect, .. } => {
                if !rect.is_valid() {
                    return Err("draw-list rectangle contains invalid geometry".into());
                }
            }
            DrawCommand::Clear(_) => {}
        }
        self.commands.push(command);
        Ok(())
    }
}

pub trait RendererBackend {
    fn upload_texture(&mut self, image: &RgbaImage) -> Result<TextureId, String>;

    fn update_texture(&mut self, texture: TextureId, image: &RgbaImage) -> Result<(), String>;

    fn destroy_texture(&mut self, texture: TextureId) -> Result<(), String>;

    fn submit(&mut self, draw_list: &DrawList) -> Result<(), String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_image_validates_dimensions_and_pixel_storage() {
        assert!(RgbaImage::new(2, 3, vec![0; 24]).is_ok());
        assert!(RgbaImage::new(0, 1, vec![]).is_err());
        assert!(RgbaImage::new(2, 3, vec![0; 23]).is_err());
        assert!(RgbaImage::new(u32::MAX, u32::MAX, vec![]).is_err());
    }

    #[test]
    fn draw_list_preserves_order_and_rejects_invalid_geometry() {
        let mut list = DrawList::new(800, 600).unwrap();
        let clear = DrawCommand::Clear(Color([0, 0, 0, 255]));
        let sprite = DrawCommand::Copy {
            texture: TextureId(7),
            source: None,
            destination: Rect {
                x: 10.0,
                y: 20.0,
                width: 32.0,
                height: 24.0,
            },
            alpha: 200,
            filter: TextureFilter::Linear,
            blend: BlendMode::Alpha,
        };
        list.push(clear.clone()).unwrap();
        list.push(sprite.clone()).unwrap();
        assert_eq!(list.dimensions(), (800, 600));
        assert_eq!(list.commands(), &[clear, sprite]);

        let invalid = DrawCommand::FillRect {
            rect: Rect {
                x: f32::NAN,
                y: 0.0,
                width: 10.0,
                height: 10.0,
            },
            color: Color([255; 4]),
            blend: BlendMode::Alpha,
        };
        assert!(list.push(invalid).is_err());
        let overflowing = DrawCommand::StrokeRect {
            rect: Rect {
                x: f32::MAX,
                y: 0.0,
                width: f32::MAX,
                height: 1.0,
            },
            color: Color([0; 4]),
            blend: BlendMode::Opaque,
        };
        assert!(list.push(overflowing).is_err());
        assert_eq!(list.commands().len(), 2);
    }

    #[test]
    fn draw_list_requires_nonzero_dimensions_and_positive_rectangles() {
        assert!(DrawList::new(0, 1).is_err());
        let mut list = DrawList::new(1, 1).unwrap();
        let invalid = DrawCommand::StrokeRect {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 1.0,
            },
            color: Color([0; 4]),
            blend: BlendMode::Opaque,
        };
        assert!(list.push(invalid).is_err());
    }
}
