use super::container_fixtures::{
    clean_webp, webp_chunk, webp_extended_header, webp_frame, webp_from, webp_lossless,
};
use super::{
    sniff_raster_mime, strip_raster_metadata, RasterMetadataRule, RasterMime, StrippedRaster,
};

const ICC: u8 = 0x20;
const ALPHA: u8 = 0x10;
const EXIF: u8 = 0x08;
const XMP: u8 = 0x04;
const ANIMATION: u8 = 0x02;

fn strip(bytes: &[u8]) -> StrippedRaster {
    strip_raster_metadata(bytes, RasterMime::Webp).expect("the walk follows this file")
}

fn refused(bytes: &[u8]) -> bool {
    strip_raster_metadata(bytes, RasterMime::Webp).is_err()
}

/// The first byte of the extended header's flags, when `VP8X` leads the chunks:
/// the RIFF header, then the chunk header.
const VP8X_FLAGS_AT: usize = 12 + 8;

/// An odd-size chunk whose pad byte is `pad` instead of the zero the format fixes.
fn chunk_with_pad(fourcc: &[u8; 4], payload: &[u8], pad: u8) -> Vec<u8> {
    let mut chunk = webp_chunk(fourcc, payload);
    assert_eq!(
        payload.len() % 2,
        1,
        "only an odd-size chunk has a pad byte"
    );
    *chunk.last_mut().expect("a chunk") = pad;
    chunk
}

fn riff_size_is_exact(bytes: &[u8]) -> bool {
    let declared = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    declared + 8 == bytes.len()
}

#[test]
fn exif_and_xmp_are_dropped_and_their_flags_cleared() {
    let icc = webp_chunk(b"ICCP", &[1, 2, 3, 4]);
    let input = webp_from(&[
        webp_extended_header(ICC | ALPHA | EXIF | XMP),
        icc.clone(),
        webp_lossless(1),
        webp_chunk(b"EXIF", b"Exif\0\0gps"),
        webp_chunk(b"XMP ", b"<x:xmpmeta/>"),
    ]);
    let expected = webp_from(&[webp_extended_header(ICC | ALPHA), icc, webp_lossless(1)]);

    let stripped = strip(&input);

    assert_eq!(stripped.bytes, expected);
    assert!(riff_size_is_exact(&stripped.bytes));
    assert_eq!(
        stripped.removed,
        vec![RasterMetadataRule::Exif, RasterMetadataRule::Xmp]
    );
}

#[test]
fn the_icc_profile_survives() {
    let input = webp_from(&[
        webp_extended_header(ICC),
        webp_chunk(b"ICCP", &[9; 8]),
        webp_lossless(1),
    ]);

    let stripped = strip(&input);

    assert_eq!(stripped.bytes, input);
    assert!(stripped.removed.is_empty());
}

#[test]
fn a_flag_is_cleared_only_for_what_was_dropped() {
    let input = webp_from(&[
        webp_extended_header(EXIF | XMP),
        webp_lossless(1),
        webp_chunk(b"EXIF", b"gps"),
    ]);

    let stripped = strip(&input);

    assert_eq!(
        stripped.bytes,
        webp_from(&[webp_extended_header(XMP), webp_lossless(1)])
    );
}

#[test]
fn an_odd_size_chunk_keeps_its_pad_and_a_dropped_one_takes_it_along() {
    let odd_image = webp_chunk(b"VP8L", &[0x2f, 0, 0, 0, 7]);
    let input = webp_from(&[
        odd_image.clone(),
        webp_chunk(b"EXIF", b"odd"),
        webp_chunk(b"XMP ", b"even"),
    ]);

    let stripped = strip(&input);

    assert_eq!(odd_image.len() % 2, 0, "the fixture pads an odd payload");
    assert_eq!(stripped.bytes, webp_from(&[odd_image]));
    assert!(riff_size_is_exact(&stripped.bytes));
}

#[test]
fn an_animation_comes_back_byte_for_byte() {
    let input = webp_from(&[
        webp_extended_header(ANIMATION | ALPHA),
        webp_chunk(b"ANIM", &[0, 0, 0, 0, 0, 0]),
        webp_frame(&[webp_chunk(b"ALPH", &[0, 1]), webp_lossless(1)]),
        webp_frame(&[webp_lossless(2)]),
    ]);

    let stripped = strip(&input);

    assert_eq!(stripped.bytes, input);
    assert!(stripped.removed.is_empty());
}

#[test]
fn metadata_inside_a_frame_is_dropped_its_flags_cleared_and_the_frame_resized() {
    let input = webp_from(&[
        webp_extended_header(ANIMATION | EXIF | XMP),
        webp_chunk(b"ANIM", &[0; 6]),
        webp_frame(&[
            webp_lossless(1),
            webp_chunk(b"EXIF", b"gps 1,2"),
            webp_chunk(b"XMP ", b"<x:xmpmeta/>"),
            webp_chunk(b"zzzz", b"vendor"),
        ]),
    ]);
    let expected = webp_from(&[
        webp_extended_header(ANIMATION),
        webp_chunk(b"ANIM", &[0; 6]),
        webp_frame(&[webp_lossless(1)]),
    ]);

    let stripped = strip(&input);

    assert_eq!(stripped.bytes, expected);
    assert!(riff_size_is_exact(&stripped.bytes));
    assert_eq!(
        stripped.removed,
        vec![
            RasterMetadataRule::Exif,
            RasterMetadataRule::Xmp,
            RasterMetadataRule::UnknownChunk
        ]
    );
    assert_eq!(
        stripped.bytes[VP8X_FLAGS_AT], ANIMATION,
        "the flags announce nothing that is gone"
    );
}

#[test]
fn a_nonzero_pad_byte_of_a_kept_chunk_is_written_as_zero() {
    let input = webp_from(&[
        webp_extended_header(ICC),
        chunk_with_pad(b"ICCP", &[9; 3], 0xaa),
        chunk_with_pad(b"VP8L", &[0x2f, 0, 0, 0, 7], 0xbb),
        webp_frame(&[chunk_with_pad(b"ALPH", &[1], 0xcc)]),
    ]);

    let stripped = strip(&input);

    assert_eq!(
        stripped.bytes,
        webp_from(&[
            webp_extended_header(ICC),
            webp_chunk(b"ICCP", &[9; 3]),
            webp_chunk(b"VP8L", &[0x2f, 0, 0, 0, 7]),
            webp_frame(&[webp_chunk(b"ALPH", &[1])]),
        ])
    );
}

#[test]
fn an_unknown_top_level_chunk_is_dropped() {
    let input = webp_from(&[webp_lossless(1), webp_chunk(b"zzzz", b"vendor")]);

    let stripped = strip(&input);

    assert_eq!(stripped.bytes, webp_from(&[webp_lossless(1)]));
    assert_eq!(stripped.removed, vec![RasterMetadataRule::UnknownChunk]);
}

#[test]
fn bytes_after_the_riff_container_are_dropped() {
    let mut input = clean_webp(1);
    input.extend_from_slice(b"<x:xmpmeta>hidden</x:xmpmeta>");

    let stripped = strip(&input);

    assert_eq!(stripped.bytes, clean_webp(1));
    assert_eq!(stripped.removed, vec![RasterMetadataRule::TrailingData]);
}

#[test]
fn a_chunk_that_declares_more_than_is_left_is_refused() {
    let mut input = clean_webp(1);
    // The VP8L size field: twelve bytes of header, then fourcc, then the size.
    input[16..20].copy_from_slice(&1000u32.to_le_bytes());

    assert!(refused(&input));
}

#[test]
fn a_chunk_whose_size_would_wrap_is_refused() {
    let mut input = clean_webp(1);
    input[16..20].copy_from_slice(&u32::MAX.to_le_bytes());

    assert!(refused(&input));
}

#[test]
fn an_odd_chunk_without_its_pad_byte_is_refused() {
    let mut input = webp_from(&[webp_chunk(b"VP8L", &[0x2f, 0, 0, 0, 7])]);
    input.pop();
    let size = (input.len() - 8) as u32;
    input[4..8].copy_from_slice(&size.to_le_bytes());

    assert!(refused(&input));
}

#[test]
fn a_riff_size_past_the_end_of_the_file_is_refused() {
    let mut input = clean_webp(1);
    input[4..8].copy_from_slice(&1000u32.to_le_bytes());

    assert!(refused(&input));
}

#[test]
fn every_truncation_of_a_webp_is_refused() {
    let whole = webp_from(&[
        webp_extended_header(ANIMATION),
        webp_chunk(b"ANIM", &[0; 6]),
        webp_frame(&[webp_lossless(1)]),
    ]);
    for length in 0..whole.len() {
        assert!(
            refused(&whole[..length]),
            "a WebP cut at {length} of {} bytes must be refused",
            whole.len()
        );
    }
}

#[test]
fn a_riff_size_that_cuts_a_chunk_short_is_refused() {
    let mut input = clean_webp(1);
    let size = (input.len() - 8 - 3) as u32;
    input[4..8].copy_from_slice(&size.to_le_bytes());

    assert!(refused(&input));
}

#[test]
fn a_malformed_extended_header_is_refused() {
    let short = webp_from(&[webp_chunk(b"VP8X", &[0; 4]), webp_lossless(1)]);
    let doubled = webp_from(&[
        webp_extended_header(0),
        webp_extended_header(0),
        webp_lossless(1),
    ]);

    assert!(refused(&short));
    assert!(refused(&doubled));
}

#[test]
fn a_frame_shorter_than_its_header_is_refused() {
    let input = webp_from(&[
        webp_extended_header(ANIMATION),
        webp_chunk(b"ANMF", &[0; 8]),
    ]);

    assert!(refused(&input));
}

#[test]
fn a_file_with_no_image_data_is_refused() {
    let input = webp_from(&[webp_extended_header(EXIF), webp_chunk(b"EXIF", b"gps")]);

    assert!(refused(&input));
}

#[test]
fn another_riff_form_is_neither_sniffed_nor_walked() {
    let mut input = clean_webp(1);
    input[8..12].copy_from_slice(b"AVI ");

    assert_eq!(sniff_raster_mime(&input), None);
    assert!(refused(&input));
    assert_eq!(sniff_raster_mime(&clean_webp(1)), Some(RasterMime::Webp));
    assert_eq!(sniff_raster_mime(&clean_webp(1)[..11]), None);
}
