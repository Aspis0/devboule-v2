//! Byte-built GIF and WebP containers for the walks' tests and for the code that
//! calls the strip. Test-only: the walks never read a pixel, so the payloads
//! carry a filler that is not a decodable image.

/// `GIF89a`, a one-pixel screen with no global colour table.
pub(crate) fn gif_head() -> Vec<u8> {
    let mut out = b"GIF89a".to_vec();
    out.extend_from_slice(&[1, 0, 1, 0, 0x00, 0, 0]);
    out
}

/// One image block with no local colour table; `pixel` varies the data so two
/// frames differ.
pub(crate) fn gif_frame(pixel: u8) -> Vec<u8> {
    vec![
        0x2c, 0, 0, 0, 0, 1, 0, 1, 0, 0x00, // image descriptor
        2,    // minimum code size
        2, 0x44, pixel, // one data sub-block
        0,     // terminator
    ]
}

/// Frame timing: a ten-centisecond delay.
pub(crate) fn gif_graphic_control() -> Vec<u8> {
    vec![0x21, 0xf9, 4, 0x00, 10, 0, 0, 0]
}

/// The loop-forever application extension.
pub(crate) fn gif_netscape_loop() -> Vec<u8> {
    let mut out = vec![0x21, 0xff, 11];
    out.extend_from_slice(b"NETSCAPE2.0");
    out.extend_from_slice(&[3, 1, 0, 0, 0]);
    out
}

pub(crate) fn gif_comment(text: &[u8]) -> Vec<u8> {
    let mut out = vec![0x21, 0xfe, text.len() as u8];
    out.extend_from_slice(text);
    out.push(0);
    out
}

/// An application extension under `identifier` (eleven bytes) carrying
/// `payload` in one sub-block.
pub(crate) fn gif_application(identifier: &[u8; 11], payload: &[u8]) -> Vec<u8> {
    let mut out = vec![0x21, 0xff, 11];
    out.extend_from_slice(identifier);
    out.push(payload.len() as u8);
    out.extend_from_slice(payload);
    out.push(0);
    out
}

pub(crate) fn gif_xmp() -> Vec<u8> {
    gif_application(b"XMP DataXMP", b"<x:xmpmeta>gps 1,2</x:xmpmeta>")
}

pub(crate) fn gif_from(blocks: &[Vec<u8>]) -> Vec<u8> {
    let mut out = gif_head();
    for block in blocks {
        out.extend_from_slice(block);
    }
    out.push(0x3b);
    out
}

/// A one-frame GIF the walk accepts and removes nothing from.
pub(crate) fn clean_gif(pixel: u8) -> Vec<u8> {
    gif_from(&[gif_frame(pixel)])
}

/// A one-frame GIF whose image data holds `len` filler bytes, for the byte caps.
pub(crate) fn bulky_gif(pixel: u8, len: usize) -> Vec<u8> {
    let mut data = vec![0x2c, 0, 0, 0, 0, 1, 0, 1, 0, 0x00, 2];
    let mut remaining = len;
    while remaining > 0 {
        let step = remaining.min(255);
        data.push(step as u8);
        data.resize(data.len() + step, pixel);
        remaining -= step;
    }
    data.push(0);
    gif_from(&[data])
}

/// One RIFF chunk: type, little-endian size, payload, and the pad byte an odd
/// size takes.
pub(crate) fn webp_chunk(fourcc: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = fourcc.to_vec();
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    if payload.len() % 2 == 1 {
        out.push(0);
    }
    out
}

/// `RIFF`, a size that matches `chunks`, `WEBP`, then the chunks.
pub(crate) fn webp_from(chunks: &[Vec<u8>]) -> Vec<u8> {
    let body: Vec<u8> = chunks.concat();
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32 + 4).to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(&body);
    out
}

/// The extended header: `flags` in the first byte, a one-pixel canvas.
pub(crate) fn webp_extended_header(flags: u8) -> Vec<u8> {
    webp_chunk(b"VP8X", &[flags, 0, 0, 0, 0, 0, 0, 0, 0, 0])
}

pub(crate) fn webp_lossless(pixel: u8) -> Vec<u8> {
    webp_chunk(b"VP8L", &[0x2f, 0, 0, 0, pixel, 0])
}

/// A one-frame WebP the walk accepts and removes nothing from.
pub(crate) fn clean_webp(pixel: u8) -> Vec<u8> {
    webp_from(&[webp_lossless(pixel)])
}

/// A one-frame WebP whose bitstream holds `len` filler bytes, for the byte caps.
pub(crate) fn bulky_webp(pixel: u8, len: usize) -> Vec<u8> {
    webp_from(&[webp_chunk(b"VP8L", &vec![pixel; len])])
}

/// An animation frame: a sixteen-byte header, then `children`.
pub(crate) fn webp_frame(children: &[Vec<u8>]) -> Vec<u8> {
    let mut payload = vec![0u8; 16];
    payload.extend_from_slice(&children.concat());
    webp_chunk(b"ANMF", &payload)
}
