use gotek_core::{
    FormatOptions, SlotFormatOptions, format_image, geometry::FloppyGeometry,
    import_file_with_time, layout::SLOT_STRIDE_BYTES, metadata::UpdateTime, slot_usage,
};
use std::io::Cursor;

fn time() -> UpdateTime {
    UpdateTime {
        year: 2026,
        month: 11,
        day: 3,
        hour: 9,
        minute: 10,
        second: 11,
    }
}
fn image(geometry: FloppyGeometry) -> Vec<u8> {
    let slot = SlotFormatOptions {
        volume_serial: 1,
        update_time: time(),
        root_label_time: time(),
    };
    let mut out = Cursor::new(Vec::new());
    format_image(
        &mut out,
        &FormatOptions::with_slot_options(geometry, 2, slot).unwrap(),
    )
    .unwrap();
    out.into_inner()
}
fn set_entry(bytes: &mut [u8], fat_start: usize, cluster: usize, value: u16) {
    let at = fat_start + cluster + cluster / 2;
    if cluster.is_multiple_of(2) {
        bytes[at] = value as u8;
        bytes[at + 1] = (bytes[at + 1] & 0xf0) | ((value >> 8) as u8 & 0x0f);
    } else {
        bytes[at] = (bytes[at] & 0x0f) | ((value as u8) << 4);
        bytes[at + 1] = (value >> 4) as u8;
    }
}
#[test]
fn fresh_geometries_are_free_and_read_only() {
    for (geometry, total) in [
        (FloppyGeometry::Kb720, 713),
        (FloppyGeometry::Mb1200, 1185),
        (FloppyGeometry::Mb1440, 2847),
    ] {
        let source = image(geometry);
        let mut reader = Cursor::new(source.clone());
        let usage = slot_usage(&mut reader, 1).unwrap();
        assert_eq!(
            (
                usage.total_clusters,
                usage.free_clusters,
                usage.used_clusters,
                usage.bad_clusters,
                usage.reserved_clusters
            ),
            (total, total, 0, 0, 0)
        );
        assert_eq!(reader.into_inner(), source);
    }
}
#[test]
fn packed_entries_classify_even_odd_and_special_markers() {
    let mut bytes = image(FloppyGeometry::Kb720);
    let first = SLOT_STRIDE_BYTES as usize + 512;
    let second = first + 3 * 512;
    for fat in [first, second] {
        for (cluster, value) in [
            (2, 0x003),
            (3, 0xfff),
            (4, 0xff0),
            (5, 0xff7),
            (6, 0x001),
            (7, 0xff8),
        ] {
            set_entry(&mut bytes, fat, cluster, value);
        }
    }
    let mut reader = Cursor::new(bytes.clone());
    let u = slot_usage(&mut reader, 1).unwrap();
    assert_eq!(
        (
            u.total_clusters,
            u.used_clusters,
            u.bad_clusters,
            u.reserved_clusters,
            u.free_clusters
        ),
        (713, 3, 1, 2, 707)
    );
    assert_eq!(
        u.total_clusters,
        u.used_clusters + u.free_clusters + u.bad_clusters + u.reserved_clusters
    );
    assert_eq!(reader.into_inner(), bytes);
    bytes[second + 3] ^= 1;
    assert!(slot_usage(&mut Cursor::new(bytes), 1).is_err());
}
#[test]
fn invalid_slot_bpb_and_truncated_fat_fail() {
    let mut bytes = image(FloppyGeometry::Kb720);
    assert!(slot_usage(&mut Cursor::new(bytes.clone()), 2).is_err());
    bytes[SLOT_STRIDE_BYTES as usize + 510] = 0;
    assert!(slot_usage(&mut Cursor::new(bytes.clone()), 1).is_err());
    bytes[SLOT_STRIDE_BYTES as usize + 510] = 0x55;
    bytes.truncate(SLOT_STRIDE_BYTES as usize + 520);
    assert!(slot_usage(&mut Cursor::new(bytes), 1).is_err());
}
#[test]
fn importing_small_file_uses_one_cluster() {
    let folder = std::env::temp_dir().join(format!("gotek-usage-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let disk = folder.join("disk.img");
    let source = folder.join("tiny.txt");
    std::fs::write(&disk, image(FloppyGeometry::Kb720)).unwrap();
    std::fs::write(&source, b"x").unwrap();
    import_file_with_time(&disk, 1, &source, None, false, time()).unwrap();
    let u = slot_usage(&mut std::fs::File::open(&disk).unwrap(), 1).unwrap();
    assert_eq!((u.used_clusters, u.free_clusters), (1, 712));
    std::fs::remove_dir_all(folder).unwrap();
}

#[test]
fn optional_ipcas_import_capture_has_one_allocated_cluster() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("image03.img");
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => panic!("{error}"),
    };
    let usage = slot_usage(&mut file, 5).unwrap();
    assert_eq!(usage.used_clusters, 1);
}
