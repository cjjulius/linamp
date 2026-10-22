//! Decode a classic skin's `.cur` files (Windows cursors). A `.cur` is an ICO
//! container (type 2) holding a DIB (bottom-up BITMAPINFOHEADER bitmap) plus the
//! cursor hotspot. The classic Winamp cursors are 32×32 4-bit paletted DIBs with
//! a 1-bit AND transparency mask — a form `image`'s ICO decoder does not handle —
//! so we decode the DIB ourselves (1/4/8-bit paletted and 24/32-bit direct), and
//! fall back to `image` only for the rare PNG-compressed entry. The result is
//! premultiplied ARGB ready for `XcursorImage`.

/// A decoded cursor: premultiplied ARGB pixels (row-major) plus the hotspot.
#[derive(Clone)]
pub struct CursorImage {
    pub width: u32,
    pub height: u32,
    pub xhot: u16,
    pub yhot: u16,
    /// Premultiplied ARGB, 0xAARRGGBB, one per pixel.
    pub argb: Vec<u32>,
}

/// The hotspot (xhot, yhot) from a `.cur` directory entry, or `None` if the
/// header is too short. First ICONDIRENTRY is at offset 6; for cursors its bytes
/// 4..8 are the hotspot as u16 little-endian — i.e. file offsets 10..14.
fn hotspot(bytes: &[u8]) -> Option<(u16, u16)> {
    if bytes.len() < 14 {
        return None;
    }
    Some((
        u16::from_le_bytes([bytes[10], bytes[11]]),
        u16::from_le_bytes([bytes[12], bytes[13]]),
    ))
}

// Little-endian field readers that return `None` on a short slice.
fn u16le(b: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(off)?, *b.get(off + 1)?]))
}
fn u32le(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *b.get(off)?,
        *b.get(off + 1)?,
        *b.get(off + 2)?,
        *b.get(off + 3)?,
    ]))
}
fn i32le(b: &[u8], off: usize) -> Option<i32> {
    u32le(b, off).map(|v| v as i32)
}

/// Pack a straight RGBA pixel into premultiplied `0xAARRGGBB`.
fn pack(r: u8, g: u8, b: u8, a: u8) -> u32 {
    let pm = |c: u8| ((c as u16 * a as u16) / 255) as u32;
    ((a as u32) << 24) | (pm(r) << 16) | (pm(g) << 8) | pm(b)
}

/// Parse a `.cur` file. Returns `None` if the bytes aren't a usable cursor.
pub fn parse(bytes: &[u8]) -> Option<CursorImage> {
    let (xhot, yhot) = hotspot(bytes)?;
    // ICONDIR: reserved(0), type(2=cursor), count. Use the first entry.
    let count = u16le(bytes, 4)?;
    if count == 0 {
        return None;
    }
    // First ICONDIRENTRY at offset 6: w,h,colors,reserved, xhot, yhot,
    // bytesInRes(u32), imageOffset(u32).
    let bytes_in_res = u32le(bytes, 14)? as usize;
    let image_offset = u32le(bytes, 18)? as usize;
    if image_offset >= bytes.len() {
        return None;
    }
    let end = image_offset
        .saturating_add(bytes_in_res)
        .min(bytes.len());
    let data = bytes.get(image_offset..end)?;

    // A PNG-compressed entry (uncommon for cursors) begins with the PNG magic.
    let (width, height, argb) = if data.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        let img = image::load_from_memory_with_format(data, image::ImageFormat::Png)
            .ok()?
            .to_rgba8();
        let (w, h) = img.dimensions();
        let argb = img
            .pixels()
            .map(|p| {
                let [r, g, b, a] = p.0;
                pack(r, g, b, a)
            })
            .collect();
        (w, h, argb)
    } else {
        decode_dib(data)?
    };

    if width == 0 || height == 0 {
        return None;
    }
    // Clamp the hotspot into the image so a bogus header can't point outside it.
    Some(CursorImage {
        width,
        height,
        xhot: xhot.min(width.saturating_sub(1) as u16),
        yhot: yhot.min(height.saturating_sub(1) as u16),
        argb,
    })
}

/// Decode a bottom-up `BITMAPINFOHEADER` DIB from an ICO/CUR entry into
/// top-down premultiplied ARGB. Handles 1/4/8-bit paletted and 24/32-bit direct
/// colour, with the 1-bit AND mask (stored after the colour bitmap) supplying
/// transparency. Returns `(width, height, argb)`.
fn decode_dib(d: &[u8]) -> Option<(u32, u32, Vec<u32>)> {
    let hdr_size = u32le(d, 0)? as usize;
    // Only the classic uncompressed BITMAPINFOHEADER (40) is supported.
    if hdr_size < 40 {
        return None;
    }
    let bi_width = i32le(d, 4)?;
    let bi_height = i32le(d, 8)?;
    let bit_count = u16le(d, 14)?;
    let compression = u32le(d, 16)?;
    if compression != 0 || bi_width <= 0 || bi_height == 0 {
        return None;
    }
    let width = bi_width as u32;
    // The DIB stores the colour bitmap and the AND mask stacked, so the header
    // height is twice the cursor height.
    let full_h = bi_height.unsigned_abs();
    let height = (full_h / 2).max(1);
    // Sanity cap so a bogus header can't ask for a gigantic allocation.
    if width > 1024 || height > 1024 {
        return None;
    }
    let clr_used = u32le(d, 32).unwrap_or(0) as usize;

    // Palette (BGRA quads) for paletted depths, right after the header.
    let palette: Vec<(u8, u8, u8)> = if bit_count <= 8 {
        let n = if clr_used != 0 { clr_used } else { 1usize << bit_count };
        let base = hdr_size;
        (0..n)
            .map(|i| {
                let o = base + i * 4;
                (
                    *d.get(o + 2).unwrap_or(&0), // R
                    *d.get(o + 1).unwrap_or(&0), // G
                    *d.get(o).unwrap_or(&0),     // B
                )
            })
            .collect()
    } else {
        Vec::new()
    };

    let w = width as usize;
    let h = height as usize;
    // Rows are padded to a 4-byte boundary.
    let xor_row = ((w * bit_count as usize + 31) / 32) * 4;
    let and_row = ((w + 31) / 32) * 4;
    let xor_off = hdr_size + palette.len() * 4;
    let and_off = xor_off + xor_row * h;

    // For 32-bit sources, honour the embedded alpha only if any is non-zero
    // (many store all-zero alpha and rely on the AND mask instead).
    let has_alpha = bit_count == 32 && {
        let mut any = false;
        for row in 0..h {
            let base = xor_off + row * xor_row;
            for x in 0..w {
                if d.get(base + x * 4 + 3).copied().unwrap_or(0) != 0 {
                    any = true;
                    break;
                }
            }
            if any {
                break;
            }
        }
        any
    };

    let mut argb = vec![0u32; w * h];
    for y in 0..h {
        // Bottom-up: output row y comes from file row (h-1-y).
        let fr = h - 1 - y;
        let xrow = xor_off + fr * xor_row;
        let arow = and_off + fr * and_row;
        for x in 0..w {
            // AND mask bit: 1 = transparent.
            let masked = {
                let byte = *d.get(arow + x / 8).unwrap_or(&0xFF);
                (byte >> (7 - (x % 8))) & 1 == 1
            };
            let (r, g, b, mut a) = match bit_count {
                1 | 4 | 8 => {
                    let idx = read_index(d, xrow, x, bit_count);
                    let (r, g, b) = palette.get(idx).copied().unwrap_or((0, 0, 0));
                    (r, g, b, 255u8)
                }
                24 => {
                    let o = xrow + x * 3;
                    (
                        *d.get(o + 2).unwrap_or(&0),
                        *d.get(o + 1).unwrap_or(&0),
                        *d.get(o).unwrap_or(&0),
                        255,
                    )
                }
                32 => {
                    let o = xrow + x * 4;
                    let a = if has_alpha {
                        *d.get(o + 3).unwrap_or(&0)
                    } else {
                        255
                    };
                    (
                        *d.get(o + 2).unwrap_or(&0),
                        *d.get(o + 1).unwrap_or(&0),
                        *d.get(o).unwrap_or(&0),
                        a,
                    )
                }
                _ => return None,
            };
            // The AND mask cuts a pixel out unless the 32-bit alpha already
            // described it as visible.
            if masked && !has_alpha {
                a = 0;
            }
            argb[y * w + x] = pack(r, g, b, a);
        }
    }
    Some((width, height, argb))
}

/// Read the palette index of pixel `x` from a packed 1/4/8-bit row at `row_off`.
fn read_index(d: &[u8], row_off: usize, x: usize, bit_count: u16) -> usize {
    match bit_count {
        1 => {
            let byte = *d.get(row_off + x / 8).unwrap_or(&0);
            ((byte >> (7 - (x % 8))) & 1) as usize
        }
        4 => {
            let byte = *d.get(row_off + x / 2).unwrap_or(&0);
            if x % 2 == 0 {
                (byte >> 4) as usize
            } else {
                (byte & 0x0F) as usize
            }
        }
        8 => *d.get(row_off + x).unwrap_or(&0) as usize,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_hotspot_little_endian() {
        // ICONDIR(6) + entry start(6..): width,height,colors,reserved, xhot, yhot…
        let b = [
            0, 0, 2, 0, 1, 0, // ICONDIR: reserved, type=2, count=1
            32, 32, 0, 0, // entry: w,h,colors,reserved
            5, 0, 7, 0, // xhot=5, yhot=7 (LE)
        ];
        assert_eq!(hotspot(&b), Some((5, 7)));
    }

    #[test]
    fn rejects_short_input() {
        assert!(hotspot(&[0, 0, 2, 0]).is_none());
        assert!(parse(&[0, 0, 2, 0]).is_none());
    }

    #[test]
    fn decodes_1x1_32bpp_cursor() {
        // A hand-built 1×1 32-bit cursor: one opaque pixel R=0x12 G=0x34 B=0x56.
        let mut b: Vec<u8> = Vec::new();
        b.extend_from_slice(&[0, 0, 2, 0, 1, 0]); // ICONDIR: type=2, count=1
        // ICONDIRENTRY: w=1,h=1,colors=0,reserved=0
        b.extend_from_slice(&[1, 1, 0, 0]);
        b.extend_from_slice(&3u16.to_le_bytes()); // xhot=3 (clamped to 0 for a 1px image)
        b.extend_from_slice(&0u16.to_le_bytes()); // yhot=0
        let dib_off = 6 + 16;
        // bytesInRes filled after we know the DIB length; imageOffset known now.
        let res_len_pos = b.len();
        b.extend_from_slice(&0u32.to_le_bytes()); // bytesInRes (patched below)
        b.extend_from_slice(&(dib_off as u32).to_le_bytes()); // imageOffset
        // BITMAPINFOHEADER: size=40,w=1,h=2(xor+and),planes=1,bpp=32,comp=0,…
        let dib_start = b.len();
        b.extend_from_slice(&40u32.to_le_bytes());
        b.extend_from_slice(&1i32.to_le_bytes());
        b.extend_from_slice(&2i32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&32u16.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes()); // compression
        b.extend_from_slice(&0u32.to_le_bytes()); // sizeImage
        b.extend_from_slice(&0i32.to_le_bytes()); // xppm
        b.extend_from_slice(&0i32.to_le_bytes()); // yppm
        b.extend_from_slice(&0u32.to_le_bytes()); // clrUsed
        b.extend_from_slice(&0u32.to_le_bytes()); // clrImportant
        // XOR row (1 px, padded to 4 bytes): B,G,R,A
        b.extend_from_slice(&[0x56, 0x34, 0x12, 0xFF]);
        // AND row (1 px, padded to 4 bytes): bit 0 = opaque
        b.extend_from_slice(&[0x00, 0, 0, 0]);
        let dib_len = (b.len() - dib_start) as u32;
        b[res_len_pos..res_len_pos + 4].copy_from_slice(&dib_len.to_le_bytes());

        let cur = parse(&b).expect("decodes");
        assert_eq!((cur.width, cur.height), (1, 1));
        assert_eq!(cur.argb, vec![0xFF_12_34_56]);
        assert_eq!(cur.xhot, 0); // clamped into the 1px image
    }
}
