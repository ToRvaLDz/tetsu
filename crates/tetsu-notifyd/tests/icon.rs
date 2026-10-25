//! `icon-static`: the `image-data` hint is validated before it is drawn.

use tetsu_notifyd::icon::from_image_data;

#[test]
fn rgb_and_rgba_rows_become_rgba() {
    // 2x1 RGB with a padded rowstride of 8.
    let data = [255, 0, 0, 0, 255, 0, 9, 9];
    let icon = from_image_data(2, 1, 8, false, 8, 3, &data).unwrap();
    assert_eq!((icon.width, icon.height), (2, 1));
    assert_eq!(&icon.rgba[..], &[255, 0, 0, 255, 0, 255, 0, 255]);

    let data = [1, 2, 3, 4];
    let icon = from_image_data(1, 1, 4, true, 8, 4, &data).unwrap();
    assert_eq!(&icon.rgba[..], &[1, 2, 3, 4]);
}

#[test]
fn inconsistent_image_data_is_ignored() {
    assert!(from_image_data(0, 1, 4, true, 8, 4, &[0; 4]).is_none());
    assert!(from_image_data(1, 1, 4, true, 16, 4, &[0; 8]).is_none());
    assert!(from_image_data(1, 1, 4, true, 8, 3, &[0; 4]).is_none());
    assert!(from_image_data(2, 2, 4, true, 8, 4, &[0; 12]).is_none());
    assert!(from_image_data(2, 1, 4, true, 8, 4, &[0; 8]).is_none());
    assert!(from_image_data(-1, 1, 4, true, 8, 4, &[0; 4]).is_none());
    assert!(from_image_data(4096, 4096, 16384, true, 8, 4, &[]).is_none());
}
