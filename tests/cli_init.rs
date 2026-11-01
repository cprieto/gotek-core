#![cfg(feature = "cli")]

use gotek_core::{
    geometry::FloppyGeometry,
    inspect,
    layout::{SECTOR_BYTES, SLOT_STRIDE_BYTES},
};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

struct TestDirectory(PathBuf);
impl TestDirectory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "gotek-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn image(&self) -> PathBuf {
        self.0.join("image.img")
    }
}
impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn init(path: &std::path::Path, count: &str, size: &str, force: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_gotek"));
    command
        .arg("init")
        .arg(path)
        .args(["--floppies", count, "--size", size]);
    if force {
        command.arg("--force");
    }
    command.output().unwrap()
}

#[test]
fn force_formats_each_geometry_preserving_length_and_trailing_bytes() {
    for (size, geometry, count) in [
        ("720k", FloppyGeometry::Kb720, 1),
        ("1.2m", FloppyGeometry::Mb1200, 1),
        ("1.44m", FloppyGeometry::Mb1440, 50),
    ] {
        let directory = TestDirectory::new();
        let path = directory.image();
        let required = count * SLOT_STRIDE_BYTES + SECTOR_BYTES as u64;
        let tail = b"preserve bytes after the formatted region";
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.set_len(required).unwrap();
        file.seek(SeekFrom::End(0)).unwrap();
        file.write_all(tail).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        file.write_all(b"old filesystem").unwrap();
        let output = init(&path, &count.to_string(), size, true);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(file.metadata().unwrap().len(), required + tail.len() as u64);
        file.seek(SeekFrom::Start(required)).unwrap();
        let mut preserved = Vec::new();
        file.read_to_end(&mut preserved).unwrap();
        assert_eq!(preserved, tail);
        let info = inspect(&mut file).unwrap();
        assert_eq!(u64::from(info.floppy_count), count);
        assert_eq!(info.geometry, geometry);
        assert!(info.warnings.is_empty());
        assert!(info.slots.iter().all(|slot| slot.warnings.is_empty()));
    }
}

#[test]
fn force_accepts_exact_capacity_and_missing_paths_still_create_new_images() {
    let directory = TestDirectory::new();
    let path = directory.image();
    let required = SLOT_STRIDE_BYTES + SECTOR_BYTES as u64;
    File::create(&path).unwrap().set_len(required).unwrap();
    assert!(init(&path, "1", "720k", true).status.success());
    assert_eq!(fs::metadata(&path).unwrap().len(), required);
    let new_path = directory.0.join("new.img");
    assert!(init(&new_path, "1", "720k", true).status.success());
    assert_eq!(fs::metadata(&new_path).unwrap().len(), required);
    let normal_path = directory.0.join("normal.img");
    assert!(init(&normal_path, "1", "720k", false).status.success());
}

#[test]
fn without_force_existing_files_are_unchanged_even_when_large_enough() {
    let directory = TestDirectory::new();
    let path = directory.image();
    let original = vec![0x5a; SLOT_STRIDE_BYTES as usize + SECTOR_BYTES];
    fs::write(&path, &original).unwrap();
    assert!(!init(&path, "1", "720k", false).status.success());
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn force_rejects_insufficient_capacity_and_invalid_configuration_before_writing() {
    let directory = TestDirectory::new();
    let path = directory.image();
    for length in [0, 512, SLOT_STRIDE_BYTES as usize + SECTOR_BYTES - 1] {
        let original = vec![0x5a; length];
        fs::write(&path, &original).unwrap();
        let output = init(&path, "1", "1.44m", true);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("ImageTooSmall"));
        assert_eq!(fs::read(&path).unwrap(), original);
    }
    let original = vec![0x5a; SLOT_STRIDE_BYTES as usize + SECTOR_BYTES];
    fs::write(&path, &original).unwrap();
    for (count, size) in [
        ("50", "1.44m"),
        ("0", "720k"),
        ("101", "720k"),
        ("1", "unsupported"),
    ] {
        assert!(!init(&path, count, size, true).status.success());
        assert_eq!(fs::read(&path).unwrap(), original);
    }
}

#[test]
fn force_rejects_directories() {
    let directory = TestDirectory::new();
    assert!(!init(&directory.0, "1", "720k", true).status.success());
    assert!(directory.0.is_dir());
}

#[cfg(unix)]
#[test]
fn force_rejects_symlinks_and_special_files_without_touching_targets() {
    use std::os::unix::fs::symlink;
    let directory = TestDirectory::new();
    let target = directory.0.join("target.img");
    fs::write(&target, b"must survive").unwrap();
    let link = directory.image();
    symlink(&target, &link).unwrap();
    assert!(!init(&link, "1", "720k", true).status.success());
    assert_eq!(fs::read(&target).unwrap(), b"must survive");
    let dangling = directory.0.join("dangling.img");
    symlink(directory.0.join("absent.img"), &dangling).unwrap();
    assert!(!init(&dangling, "1", "720k", true).status.success());
    assert!(!directory.0.join("absent.img").exists());
    let output = init(std::path::Path::new("/dev/null"), "1", "720k", true);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("regular image file"));
}
