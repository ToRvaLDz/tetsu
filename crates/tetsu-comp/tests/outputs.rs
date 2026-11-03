//! The pure output helpers: EDID, mode lists, logical size, packing.

use tetsu_bus::output::{Mode, Transform};
use tetsu_comp::outputs::edid::{self, EdidInfo};
use tetsu_comp::outputs::geometry::{PlaceIn, logical_size, pack, place};
use tetsu_comp::outputs::modes::from_list;

/// A 128-byte EDID with the PNP id `GSM`, the product name "LG HDR 4K" and
/// a serial string, checksummed.
fn lg_edid() -> Vec<u8> {
    let mut e = vec![0u8; 128];
    e[..8].copy_from_slice(&[0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0]);
    // G=7, S=19, M=13.
    let id: u16 = (7 << 10) | (19 << 5) | 13;
    e[8..10].copy_from_slice(&id.to_be_bytes());
    e[10..12].copy_from_slice(&0x5a1bu16.to_le_bytes());
    e[12..16].copy_from_slice(&0x0004a1b2u32.to_le_bytes());
    descriptor(&mut e, 0, 0xfc, "LG HDR 4K");
    seal(e)
}

fn descriptor(e: &mut [u8], slot: usize, tag: u8, text: &str) {
    let at = 54 + slot * 18;
    e[at..at + 5].copy_from_slice(&[0, 0, 0, tag, 0]);
    let mut field = [0x20u8; 13];
    let bytes = text.as_bytes();
    let n = bytes.len().min(13);
    field[..n].copy_from_slice(&bytes[..n]);
    if n < 13 {
        field[n] = 0x0a;
    }
    e[at + 5..at + 18].copy_from_slice(&field);
}

fn seal(mut e: Vec<u8>) -> Vec<u8> {
    e[127] = 0;
    let sum = e.iter().fold(0u8, |a, b| a.wrapping_add(*b));
    e[127] = 0u8.wrapping_sub(sum);
    e
}

#[test]
fn a_real_edid_gives_make_model_and_the_serial_number() {
    let info = edid::parse(&lg_edid());
    assert_eq!(
        info,
        EdidInfo {
            make: Some("GSM".into()),
            model: Some("LG HDR 4K".into()),
            serial: Some("0x0004a1b2".into()),
        }
    );
}

#[test]
fn a_serial_descriptor_wins_over_the_serial_number() {
    let mut e = lg_edid();
    descriptor(&mut e, 1, 0xff, "ABC123");
    let info = edid::parse(&seal(e));
    assert_eq!(info.serial.as_deref(), Some("ABC123"));
}

#[test]
fn a_zero_serial_number_and_no_descriptor_is_no_serial() {
    let mut e = lg_edid();
    e[12..16].copy_from_slice(&[0; 4]);
    assert_eq!(edid::parse(&seal(e)).serial, None);
}

#[test]
fn a_bad_header_checksum_or_length_gives_nothing() {
    let none = EdidInfo::default();
    let mut header = lg_edid();
    header[0] = 1;
    assert_eq!(edid::parse(&seal(header)), none);
    let mut sum = lg_edid();
    sum[127] = sum[127].wrapping_add(1);
    assert_eq!(edid::parse(&sum), none);
    assert_eq!(edid::parse(&lg_edid()[..127]), none);
    assert_eq!(edid::parse(&[]), none);
}

#[test]
fn control_characters_are_stripped_and_fields_cut() {
    let mut e = lg_edid();
    // No terminator: 13 characters with a control byte and a non-ASCII one.
    let at = 54 + 5;
    e[at..at + 13].copy_from_slice(b"A\x1b\x07B\xffCDEFGHIJ");
    let info = edid::parse(&seal(e));
    assert_eq!(info.model.as_deref(), Some("ABCDEFGHIJ"));

    // Longer than the limit after the extension blocks: cut to 32.
    assert_eq!(edid::clean(&"x".repeat(100)).unwrap().len(), 32);
    assert_eq!(edid::clean("  \t\n"), None);
}

#[test]
fn an_edid_longer_than_one_block_is_read_from_the_first() {
    let mut e = lg_edid();
    e.extend_from_slice(&[0u8; 128]);
    assert_eq!(edid::parse(&e).make.as_deref(), Some("GSM"));
}

fn mode(w: i64, h: i64, r: i64, p: bool) -> Mode {
    Mode {
        width: w,
        height: h,
        refresh_mhz: r,
        preferred: p,
    }
}

#[test]
fn duplicate_modes_are_dropped_and_the_first_wins() {
    let (modes, current) = from_list(
        &[
            (3840, 2160, 60000, true),
            (1920, 1080, 60000, false),
            (3840, 2160, 60000, false),
            (1920, 1080, 50000, false),
            (1920, 1080, 60000, true),
        ],
        (1920, 1080, 50000),
    );
    assert_eq!(
        modes,
        [
            mode(3840, 2160, 60000, true),
            mode(1920, 1080, 60000, false),
            mode(1920, 1080, 50000, false),
        ]
    );
    assert_eq!(current, Some(2));
}

#[test]
fn the_current_mode_is_none_when_not_listed_or_the_list_is_empty() {
    let (modes, current) = from_list(&[(1280, 720, 60000, false)], (1920, 1080, 60000));
    assert_eq!(modes.len(), 1);
    assert_eq!(current, None);
    assert_eq!(from_list(&[], (1920, 1080, 60000)), (vec![], None));
}

#[test]
fn rotated_and_flipped_rotated_outputs_swap_their_sides() {
    let m = mode(1920, 1080, 60000, true);
    let swapped = [
        Transform::Rotate90,
        Transform::Rotate270,
        Transform::Flipped90,
        Transform::Flipped270,
    ];
    for t in swapped {
        assert_eq!(logical_size(&m, 1.0, t), (1080, 1920), "{t:?}");
    }
    let same = [
        Transform::Normal,
        Transform::Rotate180,
        Transform::Flipped,
        Transform::Flipped180,
        Transform::Unknown,
    ];
    for t in same {
        assert_eq!(logical_size(&m, 1.0, t), (1920, 1080), "{t:?}");
    }
}

#[test]
fn the_logical_size_divides_by_the_scale_and_rounds() {
    let m = mode(3840, 2160, 60000, true);
    assert_eq!(logical_size(&m, 2.0, Transform::Normal), (1920, 1080));
    let odd = mode(1366, 768, 60000, true);
    assert_eq!(logical_size(&odd, 1.5, Transform::Normal), (911, 512));
}

#[test]
fn a_bad_scale_counts_as_one() {
    let m = mode(1920, 1080, 60000, true);
    for scale in [0.0, -2.0, f64::NAN, f64::INFINITY] {
        assert_eq!(logical_size(&m, scale, Transform::Normal), (1920, 1080));
    }
}

#[test]
fn outputs_are_packed_left_to_right_and_disabled_ones_skipped() {
    let packed = pack(&[
        ("out-1", 1920, true),
        ("out-2", 1280, false),
        ("out-3", 2560, true),
    ]);
    assert_eq!(
        packed,
        [("out-1".to_owned(), 0, 0), ("out-3".to_owned(), 1920, 0)]
    );
    assert_eq!(pack(&[]), []);
}

fn input(id: &str, w: i64, h: i64, enabled: bool, pin: Option<(i64, i64)>) -> PlaceIn<'_> {
    PlaceIn {
        id,
        w,
        h,
        enabled,
        pin,
    }
}

fn at(id: &str, x: i64, y: i64) -> (String, i64, i64) {
    (id.to_owned(), x, y)
}

#[test]
fn with_no_pins_place_is_pack() {
    let (placed, fell) = place(&[
        input("out-1", 1920, 1080, true, None),
        input("out-2", 1280, 720, false, None),
        input("out-3", 2560, 1440, true, None),
    ]);
    assert_eq!(
        placed,
        pack(&[
            ("out-1", 1920, true),
            ("out-2", 1280, false),
            ("out-3", 2560, true)
        ])
    );
    assert!(fell.is_empty());
}

#[test]
fn pins_are_honoured_negative_ones_included() {
    let (placed, fell) = place(&[
        input("out-1", 1920, 1080, true, Some((-1920, 100))),
        input("out-2", 1920, 1080, true, Some((0, -50))),
    ]);
    assert_eq!(placed, [at("out-1", -1920, 100), at("out-2", 0, -50)]);
    assert!(fell.is_empty());
}

#[test]
fn automatic_outputs_start_right_of_the_rightmost_pin() {
    let (placed, _) = place(&[
        input("out-1", 1000, 500, true, None),
        input("out-2", 2000, 500, true, Some((300, 40))),
        input("out-3", 800, 500, true, None),
        input("out-4", 1920, 1080, true, Some((0, 600))),
    ]);
    assert_eq!(
        placed,
        [
            at("out-1", 2300, 0),
            at("out-2", 300, 40),
            at("out-3", 3300, 0),
            at("out-4", 0, 600),
        ]
    );
}

#[test]
fn a_pin_over_an_earlier_pin_falls_back_and_is_reported() {
    let (placed, fell) = place(&[
        input("out-1", 1920, 1080, true, Some((0, 0))),
        input("out-2", 1920, 1080, true, Some((1000, 500))),
    ]);
    assert_eq!(placed, [at("out-1", 0, 0), at("out-2", 1920, 0)]);
    assert_eq!(fell, ["out-2"]);
}

#[test]
fn outputs_that_only_touch_at_an_edge_do_not_overlap() {
    let (placed, fell) = place(&[
        input("out-1", 1920, 1080, true, Some((0, 0))),
        input("out-2", 1920, 1080, true, Some((1920, 0))),
        input("out-3", 1920, 1080, true, Some((0, 1080))),
    ]);
    assert_eq!(
        placed,
        [
            at("out-1", 0, 0),
            at("out-2", 1920, 0),
            at("out-3", 0, 1080)
        ]
    );
    assert!(fell.is_empty());
}

#[test]
fn a_disabled_output_is_left_out_and_its_pin_does_not_block() {
    let (placed, fell) = place(&[
        input("out-1", 1920, 1080, false, Some((0, 0))),
        input("out-2", 1920, 1080, true, Some((0, 0))),
    ]);
    assert_eq!(placed, [at("out-2", 0, 0)]);
    assert!(fell.is_empty());
}
