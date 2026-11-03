use gotek_core::{
    FormatOptions, SlotFormatOptions, format_image,
    geometry::FloppyGeometry,
    inspect,
    layout::{METADATA_OFFSET_BYTES, SLOT_STRIDE_BYTES},
    load_image,
    metadata::UpdateTime,
    read_file,
};
use std::{
    fs,
    io::{Cursor, Write},
};

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
fn bank(geometry: FloppyGeometry) -> Vec<u8> {
    let slot = SlotFormatOptions {
        volume_serial: 42,
        update_time: time(),
        root_label_time: time(),
    };
    let mut writer = Cursor::new(Vec::new());
    format_image(&mut writer, &FormatOptions::new(geometry, 2, slot).unwrap()).unwrap();
    writer.into_inner()
}
fn dir() -> std::path::PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "gotek-load-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    fs::create_dir(&path).unwrap();
    path
}
#[test]
fn load_each_geometry_preserves_everything_outside_payload_and_timestamp() {
    for geometry in [
        FloppyGeometry::Kb720,
        FloppyGeometry::Mb1200,
        FloppyGeometry::Mb1440,
    ] {
        let folder = dir();
        let image = folder.join("bank.img");
        let source = folder.join("floppy.img");
        let before = bank(geometry);
        let start = SLOT_STRIDE_BYTES as usize;
        let len = geometry.payload_bytes() as usize;
        let mut floppy = before[start..start + len].to_vec();
        floppy[62] ^= 0x5a;
        {
            let fs = fatfs::FileSystem::new(Cursor::new(&mut floppy[..]), fatfs::FsOptions::new())
                .unwrap();
            fs.root_dir()
                .create_file("HELLO.TXT")
                .unwrap()
                .write_all(b"loaded")
                .unwrap();
            fs.unmount().unwrap();
        }
        fs::write(&image, &before).unwrap();
        fs::write(&source, &floppy).unwrap();
        let updated = UpdateTime {
            second: 12,
            ..time()
        };
        load_image(&image, 1, &source, updated).unwrap();
        let after = fs::read(&image).unwrap();
        assert_eq!(&after[start..start + len], floppy);
        assert_eq!(&after[..start], &before[..start]);
        assert_eq!(
            &after[start + len..start + METADATA_OFFSET_BYTES as usize],
            &before[start + len..start + METADATA_OFFSET_BYTES as usize]
        );
        let meta = start + METADATA_OFFSET_BYTES as usize;
        assert_eq!(&after[meta + 31..], &before[meta + 31..]);
        assert_eq!(&after[meta..meta + 31], b"UPDATE TIME:2026-11-03 09-10-12");
        assert_eq!(
            inspect(&mut fs::File::open(&image).unwrap())
                .unwrap()
                .floppy_count,
            2
        );
        assert_eq!(
            read_file(&mut fs::File::open(&image).unwrap(), 1, "HELLO.TXT").unwrap(),
            b"loaded"
        );
        fs::remove_dir_all(folder).unwrap();
    }
}
#[test]
fn load_rejects_bad_inputs_without_writing() {
    let folder = dir();
    let image = folder.join("bank.img");
    let source = folder.join("floppy.img");
    let before = bank(FloppyGeometry::Kb720);
    fs::write(&image, &before).unwrap();
    let valid = before[..FloppyGeometry::Kb720.payload_bytes() as usize].to_vec();
    for bad in [
        valid[..valid.len() - 1].to_vec(),
        {
            let mut v = valid.clone();
            v.push(0);
            v
        },
        {
            let mut v = valid.clone();
            v[510] = 0;
            v
        },
        bank(FloppyGeometry::Mb1440)[..FloppyGeometry::Mb1440.payload_bytes() as usize].to_vec(),
    ] {
        fs::write(&source, bad).unwrap();
        assert!(load_image(&image, 1, &source, time()).is_err());
        assert_eq!(fs::read(&image).unwrap(), before);
    }
    fs::write(&source, &valid).unwrap();
    assert!(load_image(&image, 2, &source, time()).is_err());
    let invalid = UpdateTime {
        month: 13,
        ..time()
    };
    assert!(load_image(&image, 1, &source, invalid).is_err());
    assert!(load_image(&image, 1, &image, time()).is_err());
    let alias = folder.join("alias.img");
    fs::hard_link(&image, &alias).unwrap();
    assert!(load_image(&image, 1, &alias, time()).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let link = folder.join("link.img");
        symlink(&image, &link).unwrap();
        assert!(load_image(&link, 1, &source, time()).is_err());
    }
    assert!(load_image(&folder, 1, &source, time()).is_err());
    assert_eq!(fs::read(&image).unwrap(), before);
    fs::remove_dir_all(folder).unwrap();
}

#[cfg(feature = "cli")]
#[test]
fn cli_load_image_accepts_matching_image_and_rejects_mismatch() {
    use std::process::Command;
    let folder = dir();
    let image = folder.join("disk.img");
    let source = folder.join("boot.img");
    let original = bank(FloppyGeometry::Kb720);
    fs::write(&image, &original).unwrap();
    fs::write(
        &source,
        &original[..FloppyGeometry::Kb720.payload_bytes() as usize],
    )
    .unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_gotek"))
            .arg("load-image")
            .arg(&image)
            .args(["--slot", "1"])
            .arg(&source)
            .output()
            .unwrap()
    };
    assert!(run().status.success());
    fs::write(&image, &original).unwrap();
    let other = bank(FloppyGeometry::Mb1440);
    fs::write(
        &source,
        &other[..FloppyGeometry::Mb1440.payload_bytes() as usize],
    )
    .unwrap();
    let failed = run();
    assert!(!failed.status.success());
    assert!(
        String::from_utf8_lossy(&failed.stderr).contains("geometry mismatch"),
        "{:?}",
        failed
    );
    assert_eq!(fs::read(&image).unwrap(), original);
    fs::remove_dir_all(folder).unwrap();
}

#[test]
fn save_image_exports_exact_payload_and_preserves_source() {
    use gotek_core::save_image;
    for geometry in [
        FloppyGeometry::Kb720,
        FloppyGeometry::Mb1200,
        FloppyGeometry::Mb1440,
    ] {
        let folder = dir();
        let image = folder.join("disk.img");
        let output = folder.join("saved.img");
        let mut original = bank(geometry);
        let start = SLOT_STRIDE_BYTES as usize;
        original[start + 62] ^= 0x37;
        fs::write(&image, &original).unwrap();
        save_image(&image, 1, &output).unwrap();
        let saved = fs::read(&output).unwrap();
        assert_eq!(saved.len() as u64, geometry.payload_bytes());
        assert_eq!(saved, original[start..start + saved.len()]);
        assert_eq!(fs::read(&image).unwrap(), original);
        assert!(save_image(&image, 1, &output).is_err());
        assert_eq!(fs::read(&output).unwrap(), saved);
        fs::remove_dir_all(folder).unwrap();
    }
}

#[test]
fn save_image_rejects_invalid_sources_and_aliases() {
    use gotek_core::save_image;
    let folder = dir();
    let image = folder.join("disk.img");
    let output = folder.join("saved.img");
    let original = bank(FloppyGeometry::Kb720);
    fs::write(&image, &original).unwrap();
    assert!(save_image(&image, 2, &output).is_err());
    assert!(!output.exists());
    assert!(save_image(&image, 1, &image).is_err());
    let alias = folder.join("alias.img");
    fs::hard_link(&image, &alias).unwrap();
    assert!(save_image(&image, 1, &alias).is_err());
    let mut invalid = original.clone();
    invalid[SLOT_STRIDE_BYTES as usize + 510] = 0;
    fs::write(&image, &invalid).unwrap();
    assert!(save_image(&image, 1, &output).is_err());
    invalid[SLOT_STRIDE_BYTES as usize + 510] = 0x55;
    invalid[SLOT_STRIDE_BYTES as usize + 19] = 0;
    invalid[SLOT_STRIDE_BYTES as usize + 20] = 0;
    fs::write(&image, &invalid).unwrap();
    assert!(save_image(&image, 1, &output).is_err());
    fs::write(&image, &original[..original.len() - 1]).unwrap();
    assert!(save_image(&image, 1, &output).is_err());
    assert!(!output.exists());
    fs::remove_dir_all(folder).unwrap();
}

#[test]
fn load_then_save_image_round_trips() {
    use gotek_core::save_image;
    let folder = dir();
    let disk = folder.join("disk.img");
    let source = folder.join("boot.img");
    let saved = folder.join("saved.img");
    let original = bank(FloppyGeometry::Mb1440);
    let mut floppy = original[..FloppyGeometry::Mb1440.payload_bytes() as usize].to_vec();
    floppy[62] ^= 0x42;
    fs::write(&disk, original).unwrap();
    fs::write(&source, &floppy).unwrap();
    load_image(&disk, 1, &source, time()).unwrap();
    save_image(&disk, 1, &saved).unwrap();
    assert_eq!(fs::read(saved).unwrap(), floppy);
    fs::remove_dir_all(folder).unwrap();
}

#[cfg(feature = "cli")]
#[test]
fn cli_save_image_exports_and_refuses_existing_output() {
    use std::process::Command;
    let folder = dir();
    let disk = folder.join("disk.img");
    let output = folder.join("boot.img");
    let original = bank(FloppyGeometry::Kb720);
    fs::write(&disk, &original).unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_gotek"))
            .arg("save-image")
            .arg(&disk)
            .args(["--slot", "1"])
            .arg(&output)
            .output()
            .unwrap()
    };
    assert!(run().status.success());
    assert!(!run().status.success());
    assert_eq!(
        fs::read(&output).unwrap(),
        original[SLOT_STRIDE_BYTES as usize
            ..SLOT_STRIDE_BYTES as usize + FloppyGeometry::Kb720.payload_bytes() as usize]
    );
    fs::remove_dir_all(folder).unwrap();
}
