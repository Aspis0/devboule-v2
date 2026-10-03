use super::container_fixtures::{
    clean_gif, gif_application, gif_comment, gif_frame, gif_from, gif_graphic_control, gif_head,
    gif_netscape_loop, gif_xmp,
};
use super::{
    sniff_raster_mime, strip_raster_metadata, RasterMetadataRule, RasterMime, StrippedRaster,
};

fn strip(bytes: &[u8]) -> StrippedRaster {
    strip_raster_metadata(bytes, RasterMime::Gif).expect("the walk follows this file")
}

#[test]
fn a_comment_and_an_xmp_extension_are_dropped_and_the_rest_is_kept() {
    let input = gif_from(&[
        gif_graphic_control(),
        gif_netscape_loop(),
        gif_comment(b"shot on a phone"),
        gif_xmp(),
        gif_frame(1),
    ]);
    let expected = gif_from(&[gif_graphic_control(), gif_netscape_loop(), gif_frame(1)]);

    let stripped = strip(&input);

    assert_eq!(stripped.bytes, expected);
    assert_eq!(
        stripped.removed,
        vec![RasterMetadataRule::Comment, RasterMetadataRule::Xmp]
    );
}

#[test]
fn a_two_frame_animation_comes_back_byte_for_byte() {
    let input = gif_from(&[
        gif_netscape_loop(),
        gif_graphic_control(),
        gif_frame(1),
        gif_graphic_control(),
        gif_frame(2),
    ]);

    let stripped = strip(&input);

    assert_eq!(stripped.bytes, input);
    assert!(stripped.removed.is_empty());
}

#[test]
fn the_animexts_looping_extension_is_kept_too() {
    let mut animexts = vec![0x21, 0xff, 11];
    animexts.extend_from_slice(b"ANIMEXTS1.0");
    animexts.extend_from_slice(&[3, 1, 0, 0, 0]);
    let input = gif_from(&[animexts, gif_frame(1)]);

    assert_eq!(strip(&input).bytes, input);
}

#[test]
fn a_plain_text_extension_is_kept() {
    // Its text is drawn on the frame, so it is content and not metadata.
    let mut plain_text = vec![0x21, 0x01, 12];
    plain_text.extend_from_slice(&[0; 12]);
    plain_text.extend_from_slice(&[2, b'h', b'i', 0]);
    let input = gif_from(&[plain_text, gif_frame(1)]);

    assert_eq!(strip(&input).bytes, input);
}

#[test]
fn a_looping_extension_that_carries_anything_else_is_dropped() {
    // The name is trusted only for the shape that sets a loop count; a payload
    // under it would ride through in a kept wrapper.
    let mut smuggled = vec![0x21, 0xff, 11];
    smuggled.extend_from_slice(b"NETSCAPE2.0");
    smuggled.extend_from_slice(&[5, b'h', b'e', b'l', b'l', b'o', 0]);
    let input = gif_from(&[smuggled, gif_frame(1)]);

    let stripped = strip(&input);

    assert_eq!(stripped.bytes, clean_gif(1));
    assert_eq!(stripped.removed, vec![RasterMetadataRule::UnknownChunk]);
}

#[test]
fn another_application_and_an_unknown_extension_are_dropped() {
    let vendor = gif_application(b"VENDORAPP10", b"serial 99");
    let unknown = vec![0x21, 0x80, 2, 9, 9, 0];
    let input = gif_from(&[vendor, unknown, gif_frame(1)]);

    let stripped = strip(&input);

    assert_eq!(stripped.bytes, clean_gif(1));
    assert_eq!(stripped.removed, vec![RasterMetadataRule::UnknownChunk]);
}

#[test]
fn bytes_after_the_trailer_are_dropped() {
    let mut input = clean_gif(1);
    input.extend_from_slice(b"<x:xmpmeta>hidden</x:xmpmeta>");

    let stripped = strip(&input);

    assert_eq!(stripped.bytes, clean_gif(1));
    assert_eq!(stripped.removed, vec![RasterMetadataRule::TrailingData]);
}

#[test]
fn colour_tables_are_walked_and_kept() {
    // A global table of four colours and a local table of two, each sized by
    // the packed flags and not guessed.
    let mut head = b"GIF89a".to_vec();
    head.extend_from_slice(&[1, 0, 1, 0, 0x81, 0, 0]);
    head.extend_from_slice(&[0; 12]);
    let mut frame = gif_frame(1);
    frame[9] = 0x80;
    let mut with_local = frame[..10].to_vec();
    with_local.extend_from_slice(&[0; 6]);
    with_local.extend_from_slice(&frame[10..]);
    let mut input = head;
    input.extend_from_slice(&with_local);
    input.push(0x3b);

    let stripped = strip(&input);

    assert_eq!(stripped.bytes, input);
}

#[test]
fn every_truncation_of_a_gif_is_refused() {
    let whole = gif_from(&[gif_graphic_control(), gif_netscape_loop(), gif_frame(1)]);
    for length in 0..whole.len() {
        assert!(
            strip_raster_metadata(&whole[..length], RasterMime::Gif).is_err(),
            "a GIF cut at {length} of {} bytes must be refused",
            whole.len()
        );
    }
}

#[test]
fn a_sub_block_that_runs_past_the_end_is_refused() {
    let mut input = gif_head();
    input.extend_from_slice(&[0x2c, 0, 0, 0, 0, 1, 0, 1, 0, 0x00, 2]);
    input.extend_from_slice(&[0xff, 1, 2, 3]);

    assert!(strip_raster_metadata(&input, RasterMime::Gif).is_err());
}

#[test]
fn a_sub_block_chain_that_never_terminates_is_refused() {
    // Every length is honest and the chain still has no end, so the walk must
    // stop at the input's end instead of waiting for a zero.
    let mut input = gif_head();
    input.extend_from_slice(&[0x21, 0xfe]);
    for _ in 0..64 {
        input.extend_from_slice(&[2, b'a', b'b']);
    }

    assert!(strip_raster_metadata(&input, RasterMime::Gif).is_err());
}

#[test]
fn a_file_without_a_trailer_is_refused() {
    let mut input = clean_gif(1);
    input.pop();

    assert!(strip_raster_metadata(&input, RasterMime::Gif).is_err());
}

#[test]
fn a_byte_that_introduces_no_block_is_refused() {
    let mut input = gif_head();
    input.extend_from_slice(&[0x07, 0x3b]);

    assert!(strip_raster_metadata(&input, RasterMime::Gif).is_err());
}

#[test]
fn a_malformed_graphic_control_extension_is_refused() {
    let input = gif_from(&[vec![0x21, 0xf9, 5, 0, 0, 0, 0, 7, 0], gif_frame(1)]);

    assert!(strip_raster_metadata(&input, RasterMime::Gif).is_err());
}

#[test]
fn a_bad_signature_is_refused() {
    let mut input = clean_gif(1);
    input[3] = b'9';
    input[4] = b'0';

    assert!(strip_raster_metadata(&input, RasterMime::Gif).is_err());
    assert_eq!(sniff_raster_mime(&input), None);
}

#[test]
fn both_gif_versions_are_sniffed() {
    assert_eq!(sniff_raster_mime(&clean_gif(1)), Some(RasterMime::Gif));
    let mut old = clean_gif(1);
    old[4] = b'7';
    assert_eq!(sniff_raster_mime(&old), Some(RasterMime::Gif));
}

#[test]
fn a_long_run_of_dropped_extensions_is_walked_once() {
    // The walk is bounded by the input: a hundred thousand comments, each
    // three bytes, are read once and none reaches the output.
    let mut input = gif_head();
    for _ in 0..100_000 {
        input.extend_from_slice(&[0x21, 0xfe, 0]);
    }
    input.push(0x3b);

    let stripped = strip(&input);

    assert_eq!(stripped.bytes, gif_from(&[]));
    assert_eq!(stripped.removed, vec![RasterMetadataRule::Comment]);
}
