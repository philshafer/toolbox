//! `/img/` filtering: sniff the real format, drop tiny images, and
//! detect animation by looking at the file (frame counts, APNG acTL,
//! WebP animation flag), never by extension.  Animated images are
//! dropped or, with `--animated=first-frame`, re-encoded as a PNG of
//! frame one.

use std::io::Cursor;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Animated {
    Drop,
    FirstFrame,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Png,
    Gif,
    Jpeg,
    Webp,
    Avif,
}

impl Kind {
    pub fn mime(self) -> &'static str {
        match self {
            Kind::Png => "image/png",
            Kind::Gif => "image/gif",
            Kind::Jpeg => "image/jpeg",
            Kind::Webp => "image/webp",
            Kind::Avif => "image/avif",
        }
    }
}

pub enum Verdict {
    Keep { body: Vec<u8>, mime: &'static str },
    Drop(&'static str),
}

/// 1x1 transparent GIF, served in place of a dropped image so the
/// page shows nothing rather than a broken-image box.
pub const BLANK_GIF: &[u8] = &[
    0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00,
    0xff, 0xff, 0xff, 0x21, 0xf9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x2c, 0x00, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3b,
];

pub fn filter(body: Vec<u8>, animated: Animated) -> Verdict {
    let Some(kind) = sniff(&body) else {
        return Verdict::Drop("not an image");
    };
    if let Some((w, h)) = dimensions(kind, &body)
        && ((w < 64 && h < 64) || w <= 4 || h <= 4) {
            return Verdict::Drop("tiny");
        }
    if !is_animated(kind, &body) {
        return Verdict::Keep { body, mime: kind.mime() };
    }
    match animated {
        Animated::Drop => Verdict::Drop("animated"),
        Animated::FirstFrame => match first_frame_png(&body) {
            Some(png) => Verdict::Keep { body: png, mime: "image/png" },
            None => Verdict::Drop("animated, could not decode"),
        },
    }
}

pub fn sniff(b: &[u8]) -> Option<Kind> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Kind::Png)
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some(Kind::Gif)
    } else if b.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(Kind::Jpeg)
    } else if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some(Kind::Webp)
    } else if b.len() >= 12 && &b[4..8] == b"ftyp" && (&b[8..12] == b"avif" || &b[8..12] == b"avis") {
        Some(Kind::Avif)
    } else {
        None
    }
}

fn be16(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from(*b.get(i)?) << 8 | u32::from(*b.get(i + 1)?))
}

fn le16(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from(*b.get(i)?) | u32::from(*b.get(i + 1)?) << 8)
}

fn le24(b: &[u8], i: usize) -> Option<u32> {
    Some(le16(b, i)? | u32::from(*b.get(i + 2)?) << 16)
}

pub fn dimensions(kind: Kind, b: &[u8]) -> Option<(u32, u32)> {
    match kind {
        Kind::Png => Some((be16(b, 16)? << 16 | be16(b, 18)?, be16(b, 20)? << 16 | be16(b, 22)?)),
        Kind::Gif => Some((le16(b, 6)?, le16(b, 8)?)),
        Kind::Jpeg => jpeg_dims(b),
        Kind::Webp => match b.get(12..16)? {
            b"VP8X" => Some((1 + le24(b, 24)?, 1 + le24(b, 27)?)),
            b"VP8 " => Some((le16(b, 26)? & 0x3fff, le16(b, 28)? & 0x3fff)),
            b"VP8L" => {
                let (b1, b2, b3, b4) = (*b.get(21)? as u32, *b.get(22)? as u32, *b.get(23)? as u32, *b.get(24)? as u32);
                Some((1 + (b1 | (b2 & 0x3f) << 8), 1 + (b2 >> 6 | b3 << 2 | (b4 & 0x0f) << 10)))
            }
            _ => None,
        },
        Kind::Avif => None,
    }
}

fn jpeg_dims(b: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2;
    while i + 9 < b.len() {
        if b[i] != 0xff {
            return None;
        }
        let m = b[i + 1];
        if m == 0xff {
            i += 1; // fill byte
            continue;
        }
        let sof = (0xc0..=0xcf).contains(&m) && m != 0xc4 && m != 0xc8 && m != 0xcc;
        if sof {
            return Some((be16(b, i + 7)?, be16(b, i + 5)?));
        }
        i += 2 + be16(b, i + 2)? as usize;
    }
    None
}

pub fn is_animated(kind: Kind, b: &[u8]) -> bool {
    match kind {
        Kind::Gif => gif_frames(b) > 1,
        Kind::Png => apng(b),
        Kind::Webp => b.get(12..16) == Some(b"VP8X") && b.get(20).is_some_and(|f| f & 0x02 != 0),
        Kind::Avif => b.get(8..12) == Some(b"avis"),
        Kind::Jpeg => false,
    }
}

/// Count image descriptors, stopping at two.
fn gif_frames(b: &[u8]) -> usize {
    fn skip_blocks(b: &[u8], mut i: usize) -> Option<usize> {
        loop {
            let n = *b.get(i)? as usize;
            i += 1;
            if n == 0 {
                return Some(i);
            }
            i += n;
        }
    }
    fn table(flags: u8) -> usize {
        if flags & 0x80 != 0 { 3 << ((flags & 7) + 1) } else { 0 }
    }
    let Some(&flags) = b.get(10) else { return 0 };
    let mut i = 13 + table(flags);
    let mut frames = 0;
    while let Some(&c) = b.get(i) {
        match c {
            0x2c => {
                frames += 1;
                if frames > 1 {
                    break;
                }
                let Some(&lf) = b.get(i + 9) else { break };
                i += 10 + table(lf) + 1; // descriptor, local table, LZW size
                match skip_blocks(b, i) {
                    Some(n) => i = n,
                    None => break,
                }
            }
            0x21 => match skip_blocks(b, i + 2) {
                Some(n) => i = n,
                None => break,
            },
            _ => break, // 0x3b trailer, or garbage
        }
    }
    frames
}

/// APNG: an acTL chunk before the first IDAT, with more than one frame.
fn apng(b: &[u8]) -> bool {
    let mut i = 8;
    while i + 8 <= b.len() {
        let len = (be16(b, i).unwrap_or(0) << 16 | be16(b, i + 2).unwrap_or(0)) as usize;
        let ty = &b[i + 4..i + 8];
        if ty == b"acTL" {
            let n = be16(b, i + 8).unwrap_or(0) << 16 | be16(b, i + 10).unwrap_or(0);
            return n > 1;
        }
        if ty == b"IDAT" {
            return false;
        }
        i += 12 + len;
    }
    false
}

fn first_frame_png(b: &[u8]) -> Option<Vec<u8>> {
    let img = ::image::load_from_memory(b).ok()?;
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, ::image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal GIF with `frames` 1x1 frames and the given size.
    fn gif(w: u16, h: u16, frames: usize) -> Vec<u8> {
        let mut v = b"GIF89a".to_vec();
        v.extend_from_slice(&w.to_le_bytes());
        v.extend_from_slice(&h.to_le_bytes());
        v.extend_from_slice(&[0x80, 0, 0, 0, 0, 0, 0xff, 0xff, 0xff]); // 2-color table
        v.extend_from_slice(&[0x21, 0xff, 0x0b]);
        v.extend_from_slice(b"NETSCAPE2.0");
        v.extend_from_slice(&[0x03, 0x01, 0x00, 0x00, 0x00]);
        for _ in 0..frames {
            v.extend_from_slice(&[0x21, 0xf9, 0x04, 0x00, 0x0a, 0x00, 0x00, 0x00]);
            v.extend_from_slice(&[0x2c, 0, 0, 0, 0, 1, 0, 1, 0, 0]);
            v.extend_from_slice(&[0x02, 0x02, 0x44, 0x01, 0x00]);
        }
        v.push(0x3b);
        v
    }

    #[test]
    fn gif_animation() {
        assert_eq!(gif_frames(&gif(200, 100, 1)), 1);
        assert_eq!(gif_frames(&gif(200, 100, 3)), 2);
        assert!(!is_animated(Kind::Gif, BLANK_GIF));
        assert_eq!(dimensions(Kind::Gif, &gif(200, 100, 1)), Some((200, 100)));
    }

    #[test]
    fn verdicts() {
        assert!(matches!(filter(gif(200, 100, 1), Animated::Drop), Verdict::Keep { mime: "image/gif", .. }));
        assert!(matches!(filter(gif(200, 100, 2), Animated::Drop), Verdict::Drop("animated")));
        assert!(matches!(filter(gif(200, 100, 2), Animated::FirstFrame), Verdict::Keep { mime: "image/png", .. }));
        assert!(matches!(filter(BLANK_GIF.to_vec(), Animated::Drop), Verdict::Drop("tiny")));
        assert!(matches!(filter(b"<html>".to_vec(), Animated::Drop), Verdict::Drop("not an image")));
    }

    #[test]
    fn png_apng() {
        let mut p = b"\x89PNG\r\n\x1a\n".to_vec();
        p.extend_from_slice(&[0, 0, 0, 13]);
        p.extend_from_slice(b"IHDR");
        p.extend_from_slice(&[0, 0, 1, 0, 0, 0, 0, 200, 8, 6, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(dimensions(Kind::Png, &p), Some((256, 200)));
        assert!(!apng(&p));
        let mut a = p.clone();
        a.extend_from_slice(&[0, 0, 0, 8]);
        a.extend_from_slice(b"acTL");
        a.extend_from_slice(&[0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert!(apng(&a));
    }

    #[test]
    fn jpeg() {
        // SOI, APP0 (len 4), SOF0 with h=300 w=400
        let j = [0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0x00, 0x00, 0xff, 0xc0, 0x00, 0x11, 0x08,
                 0x01, 0x2c, 0x01, 0x90, 0x03, 0, 0, 0, 0];
        assert_eq!(dimensions(Kind::Jpeg, &j), Some((400, 300)));
    }
}
