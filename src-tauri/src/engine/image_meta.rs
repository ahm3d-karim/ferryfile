//! Lossless metadata removal for JPEG, PNG and WebP: the container is
//! rewritten without its EXIF/XMP/IPTC/comment blocks while the compressed
//! pixel data is copied byte for byte. Colour profiles are kept, and a JPEG's
//! EXIF orientation survives as a one-tag EXIF block so the photo stays upright.

/// JPEG without APP1 (EXIF, XMP), APP13 (IPTC), comments and other app
/// segments. Keeps JFIF (APP0), ICC profiles (APP2) and Adobe (APP14).
pub fn strip_jpeg(data: &[u8]) -> Result<Vec<u8>, String> {
    const BAD: &str = "This doesn't look like a valid JPEG file";
    if !data.starts_with(&[0xFF, 0xD8]) {
        return Err(BAD.into());
    }
    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(&[0xFF, 0xD8]);
    let mut pos = 2;
    while pos < data.len() {
        if data[pos] != 0xFF {
            return Err(BAD.into());
        }
        while pos < data.len() && data[pos] == 0xFF {
            pos += 1; // fill bytes
        }
        let Some(&marker) = data.get(pos) else { break };
        pos += 1;
        if marker == 0xD9 {
            break; // EOI; anything after it (e.g. MPF extras) is dropped
        }
        if marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            out.extend_from_slice(&[0xFF, marker]);
            continue;
        }
        let len = data
            .get(pos..pos + 2)
            .map(|b| u16::from_be_bytes([b[0], b[1]]) as usize)
            .filter(|&l| l >= 2 && pos + l <= data.len())
            .ok_or(BAD)?;
        let payload = &data[pos + 2..pos + len];
        let keep = match marker {
            0xE0 | 0xEE => true,
            0xE2 => payload.starts_with(b"ICC_PROFILE\0"),
            0xE1 => {
                if let Some(o) = exif_orientation(payload).filter(|o| (2..=8).contains(o)) {
                    out.extend_from_slice(&orientation_app1(o));
                }
                false
            }
            0xE3..=0xEF | 0xFE => false,
            _ => true,
        };
        if keep {
            out.extend_from_slice(&[0xFF, marker]);
            out.extend_from_slice(&data[pos..pos + len]);
        }
        pos += len;
        if marker == 0xDA {
            // Entropy-coded scan data runs until the next real marker.
            let start = pos;
            while pos + 1 < data.len()
                && !(data[pos] == 0xFF && data[pos + 1] != 0 && !(0xD0..=0xD7).contains(&data[pos + 1]))
            {
                pos += 1;
            }
            if pos + 1 >= data.len() {
                pos = data.len();
            }
            out.extend_from_slice(&data[start..pos]);
        }
    }
    out.extend_from_slice(&[0xFF, 0xD9]);
    Ok(out)
}

/// The orientation tag (0x0112) from an APP1 EXIF payload.
fn exif_orientation(payload: &[u8]) -> Option<u16> {
    let tiff = payload.strip_prefix(b"Exif\0\0")?;
    let le = match tiff.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |i: usize| tiff.get(i..i + 2).map(|b| if le { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) });
    let u32_at = |i: usize| {
        tiff.get(i..i + 4).map(|b| {
            let b = [b[0], b[1], b[2], b[3]];
            if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) }
        })
    };
    let ifd = u32_at(4)? as usize;
    let count = u16_at(ifd)? as usize;
    (0..count)
        .map(|i| ifd + 2 + i * 12)
        .find(|&e| u16_at(e) == Some(0x0112))
        .and_then(|e| u16_at(e + 8))
}

/// An APP1 segment holding nothing but the orientation tag.
fn orientation_app1(orientation: u16) -> Vec<u8> {
    let mut seg = vec![0xFF, 0xE1, 0, 0];
    seg.extend_from_slice(b"Exif\0\0MM\0\x2A\0\0\0\x08"); // big-endian TIFF, IFD0 at 8
    seg.extend_from_slice(&[0, 1, 0x01, 0x12, 0, 3, 0, 0, 0, 1]); // 1 entry: SHORT x1
    seg.extend_from_slice(&orientation.to_be_bytes());
    seg.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // padding, no next IFD
    let len = (seg.len() - 2) as u16;
    seg[2..4].copy_from_slice(&len.to_be_bytes());
    seg
}

/// PNG without eXIf, text and timestamp chunks.
pub fn strip_png(data: &[u8]) -> Result<Vec<u8>, String> {
    const SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
    const BAD: &str = "This doesn't look like a valid PNG file";
    if !data.starts_with(SIG) {
        return Err(BAD.into());
    }
    let mut out = SIG.to_vec();
    let mut pos = SIG.len();
    while pos + 8 <= data.len() {
        let len = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        let end = pos + 12 + len;
        if end > data.len() {
            return Err(BAD.into());
        }
        let kind = &data[pos + 4..pos + 8];
        if !matches!(kind, b"eXIf" | b"tEXt" | b"zTXt" | b"iTXt" | b"tIME") {
            out.extend_from_slice(&data[pos..end]);
        }
        pos = end;
        if kind == b"IEND" {
            break;
        }
    }
    Ok(out)
}

/// WebP without its EXIF and XMP chunks (and their VP8X flags).
pub fn strip_webp(data: &[u8]) -> Result<Vec<u8>, String> {
    const BAD: &str = "This doesn't look like a valid WebP file";
    if data.len() < 12 || &data[..4] != b"RIFF" || &data[8..12] != b"WEBP" {
        return Err(BAD.into());
    }
    let mut out = data[..12].to_vec();
    let mut pos = 12;
    while pos + 8 <= data.len() {
        let kind = &data[pos..pos + 4];
        let size = u32::from_le_bytes(data[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let end = (pos + 8 + size + (size & 1)).min(data.len());
        if pos + 8 + size > data.len() {
            return Err(BAD.into());
        }
        match kind {
            b"EXIF" | b"XMP " => {}
            b"VP8X" if size >= 1 => {
                let start = out.len();
                out.extend_from_slice(&data[pos..end]);
                out[start + 8] &= !(0x08 | 0x04); // EXIF and XMP present flags
            }
            _ => out.extend_from_slice(&data[pos..end]),
        }
        pos = end;
    }
    let riff_size = (out.len() - 8) as u32;
    out[4..8].copy_from_slice(&riff_size.to_le_bytes());
    Ok(out)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// APP1 EXIF with orientation 6 and a GPS IFD pointer (tag 0x8825).
    pub fn exif_app1() -> Vec<u8> {
        let mut tiff = b"II\x2A\0\x08\0\0\0".to_vec();
        tiff.extend_from_slice(&[2, 0]);
        tiff.extend_from_slice(&[0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0]);
        tiff.extend_from_slice(&[0x25, 0x88, 4, 0, 1, 0, 0, 0, 38, 0, 0, 0]);
        tiff.extend_from_slice(&[0, 0, 0, 0]);
        tiff.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // empty GPS IFD at 38
        let mut seg = vec![0xFF, 0xE1, 0, 0];
        seg.extend_from_slice(b"Exif\0\0");
        seg.extend_from_slice(&tiff);
        let len = (seg.len() - 2) as u16;
        seg[2..4].copy_from_slice(&len.to_be_bytes());
        seg
    }

    #[test]
    fn reads_and_rewrites_orientation() {
        let app1 = exif_app1();
        assert_eq!(exif_orientation(&app1[4..]), Some(6));
        let minimal = orientation_app1(6);
        assert_eq!(exif_orientation(&minimal[4..]), Some(6));
        assert!(minimal.len() < app1.len());
    }

    #[test]
    fn png_drops_text_chunks() {
        let mut buf = Vec::new();
        image::RgbImage::from_pixel(4, 4, image::Rgb([9, 9, 9]))
            .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        // Insert a tEXt chunk after IHDR (8 sig + 25 IHDR).
        let mut chunk = 11u32.to_be_bytes().to_vec();
        chunk.extend_from_slice(b"tEXtGPS\0secret!");
        chunk.extend_from_slice(&[0, 0, 0, 0]);
        let mut tagged = buf[..33].to_vec();
        tagged.extend_from_slice(&chunk);
        tagged.extend_from_slice(&buf[33..]);

        let clean = strip_png(&tagged).unwrap();
        assert_eq!(clean, buf);
    }

    #[test]
    fn webp_drops_exif_chunk() {
        let mut data = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0\x08\0\0\0\0\0\0\0\0\0".to_vec();
        data.extend_from_slice(b"EXIF\x03\0\0\0abc\0");
        let n = (data.len() - 8) as u32;
        data[4..8].copy_from_slice(&n.to_le_bytes());

        let clean = strip_webp(&data).unwrap();
        assert_eq!(clean.len(), 12 + 18);
        assert_eq!(clean[20], 0);
        assert_eq!(u32::from_le_bytes(clean[4..8].try_into().unwrap()) as usize, clean.len() - 8);
    }
}
