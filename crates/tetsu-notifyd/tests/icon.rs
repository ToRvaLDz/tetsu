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

mod files {
    use std::time::{Duration, Instant};

    use tetsu_notifyd::icon::load;

    fn png(width: u32, height: u32) -> Vec<u8> {
        tiny_skia::Pixmap::new(width, height)
            .unwrap()
            .encode_png()
            .unwrap()
    }

    #[test]
    fn a_png_file_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("icon.png");
        std::fs::write(&path, png(4, 3)).unwrap();
        let icon = load(path.to_str().unwrap()).unwrap();
        assert_eq!((icon.width, icon.height), (4, 3));
        let uri = format!("file://{}", path.display());
        assert!(load(&uri).is_some());
    }

    #[test]
    fn a_fifo_or_a_device_is_refused_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("fifo");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        let started = Instant::now();
        assert!(load(fifo.to_str().unwrap()).is_none());
        assert!(load("/dev/zero").is_none());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn oversized_files_and_images_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("big.png");
        let mut bytes = png(1, 1);
        bytes.resize(5 * 1024 * 1024, 0);
        std::fs::write(&big, bytes).unwrap();
        assert!(load(big.to_str().unwrap()).is_none());

        // A header claiming 5000x5000 is refused before decoding.
        let mut header = png(1, 1);
        header[16..20].copy_from_slice(&5000u32.to_be_bytes());
        header[20..24].copy_from_slice(&5000u32.to_be_bytes());
        let huge = dir.path().join("huge.png");
        std::fs::write(&huge, header).unwrap();
        assert!(load(huge.to_str().unwrap()).is_none());
    }

    #[test]
    fn relative_paths_are_not_files() {
        assert!(load("../etc/passwd").is_none());
        assert!(load("").is_none());
    }
}
