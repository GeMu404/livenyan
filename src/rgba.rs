//! Imagen RGBA8 lineal en CPU (raster de texto y subidas a SDL).

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
}
