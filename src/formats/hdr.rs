//! Radiance HDR (.hdr) image format loader
//!
//! Supports loading Radiance HDR images with RLE compression and RGBe → linear RGB32F conversion.

use crate::core::error::{RenderError, RenderResult};
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

/// HDR image data
#[derive(Debug, Clone)]
pub struct HdrImage {
    /// Width in pixels
    pub width: u32,
    /// Height in pixels
    pub height: u32,
    /// Linear RGB32F data (3 components per pixel, row-major)
    pub data: Vec<f32>,
}

impl HdrImage {
    /// Get pixel count
    pub fn pixel_count(&self) -> usize {
        (self.width * self.height) as usize
    }

    /// Get data size in bytes
    pub fn data_size_bytes(&self) -> usize {
        self.data.len() * 4 // 4 bytes per f32
    }

    /// Convert to RGBA format by adding alpha channel
    pub fn to_rgba(&self) -> Vec<f32> {
        let mut rgba_data = Vec::with_capacity(self.pixel_count() * 4);

        for i in 0..self.pixel_count() {
            let base = i * 3;
            rgba_data.push(self.data[base]); // R
            rgba_data.push(self.data[base + 1]); // G
            rgba_data.push(self.data[base + 2]); // B
            rgba_data.push(1.0); // A
        }

        rgba_data
    }
}

/// Load a Radiance HDR file
pub fn load_hdr<P: AsRef<Path>>(path: P) -> RenderResult<HdrImage> {
    let file = File::open(path.as_ref())
        .map_err(|e| RenderError::io(format!("Failed to open HDR file: {}", e)))?;

    let mut reader = BufReader::new(file);

    // Parse header (dimensions + scanline orientation).
    let (width, height, flip_x, flip_y) = parse_header(&mut reader)?;

    // Read scanlines in file order.
    let mut rgbe_data = read_scanlines(&mut reader, width, height)?;

    // Radiance orientation: the resolution line's sign on each axis says whether
    // the first scanline/pixel in the file starts at the TOP (-Y) / LEFT (+X) or
    // the BOTTOM (+Y) / RIGHT (-X) of the image. The conventional marker `-Y +X`
    // is already top-down (it is the "common orientation" for reference
    // decoders such as the `image` crate, and both shipped HDRI assets — a
    // sunrise sky with a bright zenith in the first scanline and a dark ground
    // in the last — only read correctly that way), so only `+Y` and `-X` need a
    // flip to land on row 0 = top. Flipping `-Y` instead inverted every
    // conventional environment, lighting scenes from the ground up.
    if flip_y {
        let w = width as usize;
        let h = height as usize;
        for row in 0..h / 2 {
            let top = row * w;
            let bottom = (h - 1 - row) * w;
            for i in 0..w {
                rgbe_data.swap(top + i, bottom + i);
            }
        }
    }
    if flip_x {
        let w = width as usize;
        let h = height as usize;
        for row in 0..h {
            let base = row * w;
            for i in 0..w / 2 {
                rgbe_data.swap(base + i, base + (w - 1 - i));
            }
        }
    }

    // Convert RGBe to linear RGB
    let rgb_data = convert_rgbe_to_rgb(&rgbe_data);

    Ok(HdrImage {
        width,
        height,
        data: rgb_data,
    })
}

/// Parse HDR file header
fn parse_header<R: BufRead>(reader: &mut R) -> RenderResult<(u32, u32, bool, bool)> {
    let mut line = String::new();

    // Read magic line
    reader
        .read_line(&mut line)
        .map_err(|e| RenderError::io(format!("Failed to read HDR header: {}", e)))?;

    if !line.starts_with("#?RADIANCE") && !line.starts_with("#?RGBE") {
        return Err(RenderError::upload(
            "Invalid HDR file: missing magic header".to_string(),
        ));
    }

    // Read header lines until empty line
    let mut format_found = false;
    line.clear();

    while reader
        .read_line(&mut line)
        .map_err(|e| RenderError::io(format!("Failed to read HDR header: {}", e)))?
        > 0
    {
        let line_trimmed = line.trim();

        if line_trimmed.is_empty() {
            break; // End of header
        }

        if line_trimmed.starts_with("FORMAT=") {
            if line_trimmed != "FORMAT=32-bit_rle_rgbe" && line_trimmed != "FORMAT=32-bit_rle_xyze"
            {
                return Err(RenderError::upload(format!(
                    "Unsupported HDR format: {}",
                    line_trimmed
                )));
            }
            format_found = true;
        }

        line.clear();
    }

    if !format_found {
        return Err(RenderError::upload(
            "HDR file missing FORMAT specification".to_string(),
        ));
    }

    // Read resolution line
    line.clear();
    reader
        .read_line(&mut line)
        .map_err(|e| RenderError::io(format!("Failed to read HDR resolution: {}", e)))?;

    let resolution_line = line.trim();

    // Parse the resolution line as (axis, count) pairs, e.g. "-Y 1024 +X 2048".
    // Axis order is not guaranteed by the format, so read each pair rather than
    // fixed offsets. The sign records the scan direction relative to the
    // row-0-at-top, left-to-right raster order this loader normalises to:
    // `-Y` runs top->bottom (the conventional default) and `+X` left->right,
    // so only `+Y` (bottom->top) and `-X` (right->left) need a flip.
    let parts: Vec<&str> = resolution_line.split_whitespace().collect();
    if parts.len() != 4 {
        return Err(RenderError::upload(format!(
            "Invalid HDR resolution line: {}",
            resolution_line
        )));
    }

    let mut width: Option<u32> = None;
    let mut height: Option<u32> = None;
    let mut flip_x = false;
    let mut flip_y = false;
    let mut index = 0;
    while index + 1 < parts.len() {
        let axis = parts[index].to_ascii_uppercase();
        let count = parts[index + 1]
            .parse::<u32>()
            .map_err(|_| RenderError::upload(format!("Invalid HDR resolution value: {}", parts[index + 1])))?;
        match axis.as_str() {
            "+X" => width = Some(count),
            "-X" => {
                width = Some(count);
                flip_x = true;
            }
            "-Y" => height = Some(count),
            "+Y" => {
                height = Some(count);
                flip_y = true;
            }
            other => {
                return Err(RenderError::upload(format!(
                    "Invalid HDR resolution axis: {}",
                    other
                )));
            }
        }
        index += 2;
    }

    let width = width
        .ok_or_else(|| RenderError::upload("HDR resolution line missing X axis".to_string()))?;
    let height = height
        .ok_or_else(|| RenderError::upload("HDR resolution line missing Y axis".to_string()))?;

    if width == 0 || height == 0 {
        return Err(RenderError::upload(
            "HDR image dimensions cannot be zero".to_string(),
        ));
    }

    Ok((width, height, flip_x, flip_y))
}

/// Read HDR scanlines with RLE decompression
fn read_scanlines<R: Read>(reader: &mut R, width: u32, height: u32) -> RenderResult<Vec<[u8; 4]>> {
    let mut rgbe_data = Vec::with_capacity((width * height) as usize);

    for y in 0..height {
        let scanline = read_scanline(reader, width, y)?;
        rgbe_data.extend(scanline);
    }

    Ok(rgbe_data)
}

/// Read a single HDR scanline
fn read_scanline<R: Read>(reader: &mut R, width: u32, y: u32) -> RenderResult<Vec<[u8; 4]>> {
    let mut header = [0u8; 4];
    reader.read_exact(&mut header).map_err(|e| {
        RenderError::io(format!(
            "Failed to read scanline header at row {}: {}",
            y, e
        ))
    })?;

    // Check if this is a new-style RLE scanline
    if header[0] == 2
        && header[1] == 2
        && header[2] == (((width >> 8) & 0xFF) as u8)
        && header[3] == ((width & 0xFF) as u8)
    {
        // New-style RLE
        read_rle_scanline(reader, width)
    } else {
        // Old-style or uncompressed
        let mut scanline = vec![[0u8; 4]; width as usize];
        scanline[0] = header; // First pixel is in the header we just read

        // Read remaining pixels
        for pixel in scanline.iter_mut().skip(1) {
            reader.read_exact(pixel).map_err(|e| {
                RenderError::io(format!("Failed to read pixel data at row {}: {}", y, e))
            })?;
        }

        Ok(scanline)
    }
}

/// Read RLE-compressed scanline
fn read_rle_scanline<R: Read>(reader: &mut R, width: u32) -> RenderResult<Vec<[u8; 4]>> {
    let mut scanline = vec![[0u8; 4]; width as usize];

    // Read each component (RGBE) separately
    for component in 0..4 {
        let mut pos = 0usize;

        while pos < width as usize {
            let mut run_info = [0u8; 1];
            reader
                .read_exact(&mut run_info)
                .map_err(|e| RenderError::io(format!("Failed to read RLE run info: {}", e)))?;

            let run_length = run_info[0];

            if run_length > 128 {
                // RLE run: repeat next value
                let repeat_count = (run_length - 128) as usize;
                if pos + repeat_count > width as usize {
                    return Err(RenderError::upload(
                        "HDR RLE run exceeds scanline width".to_string(),
                    ));
                }

                let mut value = [0u8; 1];
                reader.read_exact(&mut value).map_err(|e| {
                    RenderError::io(format!("Failed to read RLE repeat value: {}", e))
                })?;

                for i in 0..repeat_count {
                    scanline[pos + i][component] = value[0];
                }

                pos += repeat_count;
            } else {
                // Literal run: copy next values
                let copy_count = run_length as usize;
                if pos + copy_count > width as usize {
                    return Err(RenderError::upload(
                        "HDR literal run exceeds scanline width".to_string(),
                    ));
                }

                for i in 0..copy_count {
                    let mut value = [0u8; 1];
                    reader.read_exact(&mut value).map_err(|e| {
                        RenderError::io(format!("Failed to read literal value: {}", e))
                    })?;
                    scanline[pos + i][component] = value[0];
                }

                pos += copy_count;
            }
        }
    }

    Ok(scanline)
}

/// Convert RGBe data to linear RGB32F
fn convert_rgbe_to_rgb(rgbe_data: &[[u8; 4]]) -> Vec<f32> {
    let mut rgb_data = Vec::with_capacity(rgbe_data.len() * 3);

    for &[r, g, b, e] in rgbe_data {
        let (rf, gf, bf) = rgbe_to_rgb(r, g, b, e);
        rgb_data.push(rf);
        rgb_data.push(gf);
        rgb_data.push(bf);
    }

    rgb_data
}

/// Convert a single RGBe pixel to RGB
#[inline]
fn rgbe_to_rgb(r: u8, g: u8, b: u8, e: u8) -> (f32, f32, f32) {
    if e == 0 {
        // All components are zero
        (0.0, 0.0, 0.0)
    } else {
        // Convert using shared exponent
        let exp = 2.0f32.powi((e as i32) - 128 - 8);
        ((r as f32) * exp, (g as f32) * exp, (b as f32) * exp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rgbe_to_rgb_zero() {
        let (r, g, b) = rgbe_to_rgb(0, 0, 0, 0);
        assert_eq!(r, 0.0);
        assert_eq!(g, 0.0);
        assert_eq!(b, 0.0);
    }

    #[test]
    fn test_rgbe_to_rgb_nonzero() {
        // Test a known conversion
        let (r, g, b) = rgbe_to_rgb(128, 128, 128, 128);

        // With exponent 128, the multiplier should be 2^(128-128-8) = 2^(-8) = 1/256
        let expected = 128.0 / 256.0;

        assert!((r - expected).abs() < 1e-6);
        assert!((g - expected).abs() < 1e-6);
        assert!((b - expected).abs() < 1e-6);
    }

    #[test]
    fn test_rgbe_to_rgb_bright() {
        // Test high dynamic range value
        let (r, g, b) = rgbe_to_rgb(255, 128, 64, 140);

        // With exponent 140, multiplier is 2^(140-128-8) = 2^4 = 16
        let exp_r = 255.0 * 16.0;
        let exp_g = 128.0 * 16.0;
        let exp_b = 64.0 * 16.0;

        assert!((r - exp_r).abs() < 1e-4);
        assert!((g - exp_g).abs() < 1e-4);
        assert!((b - exp_b).abs() < 1e-4);
    }

    #[test]
    fn test_hdr_image_to_rgba() {
        let hdr = HdrImage {
            width: 2,
            height: 1,
            data: vec![1.0, 0.5, 0.25, 0.75, 1.0, 0.5], // 2 RGB pixels
        };

        let rgba = hdr.to_rgba();
        let expected = vec![
            1.0, 0.5, 0.25, 1.0, // First pixel + alpha
            0.75, 1.0, 0.5, 1.0, // Second pixel + alpha
        ];

        assert_eq!(rgba, expected);
    }

    #[test]
    fn load_hdr_uncompressed_round_trip_dims_and_values() {
        use std::io::Write;
        use std::time::{SystemTime, UNIX_EPOCH};

        let mut path = std::env::temp_dir();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before unix epoch")
            .as_nanos();
        path.push(format!(
            "forge3d_hdr_radiance_{}_{}.hdr",
            std::process::id(),
            nonce
        ));
        {
            let mut f = File::create(&path).expect("create radiance fixture");
            f.write_all(b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 2 +X 4\n")
                .unwrap();
            let px = [128u8, 64, 32, 129];
            for _ in 0..8 {
                f.write_all(&px).unwrap();
            }
        }
        let img = load_hdr(&path).expect("decode radiance fixture");
        std::fs::remove_file(&path).ok();

        assert_eq!((img.width, img.height), (4, 2));
        assert_eq!(img.data.len(), 4 * 2 * 3);
        let exp = 2.0f32.powi(129 - 128 - 8);
        for i in 0..8 {
            assert!((img.data[i * 3] - 128.0 * exp).abs() < 1e-6);
            assert!((img.data[i * 3 + 1] - 64.0 * exp).abs() < 1e-6);
            assert!((img.data[i * 3 + 2] - 32.0 * exp).abs() < 1e-6);
        }
    }

    #[test]
    fn hdr_minus_y_keeps_file_order_top_down() {
        use std::io::Write;

        let mut path = std::env::temp_dir();
        path.push(format!("forge3d_hdr_minus_y_{}.hdr", std::process::id()));
        {
            let mut f = File::create(&path).expect("create radiance fixture");
            // -Y +X is the COMMON orientation for the format: the first
            // scanline in the file is the TOP row (see
            // `hdr_minus_y_matches_image_crate_reference_orientation`). Two
            // distinct scanlines prove the loader keeps that order.
            f.write_all(b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 2 +X 4\n").unwrap();
            let first = [200u8, 200, 200, 128];
            let second = [10u8, 10, 10, 128];
            for _ in 0..4 {
                f.write_all(&first).unwrap();
            }
            for _ in 0..4 {
                f.write_all(&second).unwrap();
            }
        }
        let img = load_hdr(&path).expect("decode radiance fixture");
        std::fs::remove_file(&path).ok();

        let exp = 2.0f32.powi(128 - 128 - 8);
        // Row 0 (top) must be the FIRST file scanline (200).
        assert!(
            (img.data[0] - 200.0 * exp).abs() < 1e-6,
            "top row should be the file's first (top) scanline"
        );
        let second_row = (4 * 3) as usize; // width 4 -> second decoded row starts here
        assert!(
            (img.data[second_row] - 10.0 * exp).abs() < 1e-6,
            "bottom row should be the file's second (bottom) scanline"
        );
    }

    /// `+Y` stores the scanlines bottom-up (the opposite of the conventional
    /// `-Y`), so the first file scanline must land in the LAST decoded row.
    #[test]
    fn hdr_plus_y_scanlines_are_flipped_to_top_down() {
        use std::io::Write;

        let mut path = std::env::temp_dir();
        path.push(format!("forge3d_hdr_plus_y_{}.hdr", std::process::id()));
        {
            let mut f = File::create(&path).expect("create radiance fixture");
            f.write_all(b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n+Y 2 +X 4\n").unwrap();
            let first = [10u8, 10, 10, 128];
            let second = [200u8, 200, 200, 128];
            for _ in 0..4 {
                f.write_all(&first).unwrap();
            }
            for _ in 0..4 {
                f.write_all(&second).unwrap();
            }
        }
        let img = load_hdr(&path).expect("decode radiance fixture");
        std::fs::remove_file(&path).ok();

        let exp = 2.0f32.powi(128 - 128 - 8);
        // Row 0 (top) must be the SECOND file scanline (200).
        assert!(
            (img.data[0] - 200.0 * exp).abs() < 1e-6,
            "top row should be the file's last scanline for +Y"
        );
        assert!(
            (img.data[4 * 3] - 10.0 * exp).abs() < 1e-6,
            "bottom row should be the file's first scanline for +Y"
        );
    }

    /// Cross-check our orientation against the `image` crate's Radiance
    /// decoder, the reference implementation for `-Y +X`: its
    /// `parse_dimensions_line` documents that combination as the COMMON
    /// orientation (left-right, top-down) and reads scanlines in file order
    /// without flipping. If our loader and the reference disagree on which
    /// file row is "up", exactly one of us is vertically inverted — and an
    /// inverted environment lights the scene from the ground up. (The shipped
    /// `qwantani_sunrise_puresky_2k.hdr` and `assets/hdri/sky.hdr` are both
    /// `-Y` with a bright zenith in the first scanline, which agrees with the
    /// reference rather than with the flip.)
    #[test]
    fn hdr_minus_y_matches_image_crate_reference_orientation() {
        use std::io::Write;

        let mut path = std::env::temp_dir();
        path.push(format!("forge3d_hdr_ref_{}.hdr", std::process::id()));
        {
            let mut f = File::create(&path).expect("create radiance fixture");
            f.write_all(b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 2 +X 4\n").unwrap();
            for pixel in [[10u8, 10, 10, 128], [200, 200, 200, 128]] {
                for _ in 0..4 {
                    f.write_all(&pixel).unwrap();
                }
            }
        }

        let img = load_hdr(&path).expect("decode radiance fixture");
        let reference = image::open(&path).expect("image crate decode fixture").to_rgb32f();
        std::fs::remove_file(&path).ok();

        assert_eq!((img.width, img.height), reference.dimensions());
        let stride = img.width as usize * 3;
        for y in 0..img.height as usize {
            let px = reference.get_pixel(0, y as u32).0;
            let ours = &img.data[y * stride..y * stride + 3];
            for channel in 0..3 {
                assert!(
                    (ours[channel] - px[channel]).abs() < 1e-6,
                    "row {y} channel {channel}: ours={} reference={}",
                    ours[channel],
                    px[channel]
                );
            }
        }
    }
}
