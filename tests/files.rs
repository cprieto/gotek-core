use gotek_core::{
    FormatOptions, GotekError, SlotFormatOptions, format_image, geometry::FloppyGeometry,
    layout::SLOT_STRIDE_BYTES, list_files, metadata::UpdateTime, read_file,
};
use std::io::{Cursor, Write};

fn image(geometry: FloppyGeometry) -> Cursor<Vec<u8>> {
    let time = UpdateTime {
        year: 2026,
        month: 11,
        day: 1,
        hour: 12,
        minute: 0,
        second: 0,
    };
    let options = FormatOptions::new(
        geometry,
        2,
        SlotFormatOptions {
            volume_serial: 123,
            update_time: time,
            root_label_time: time,
        },
    )
    .unwrap();
    let mut image = Cursor::new(Vec::new());
    format_image(&mut image, &options).unwrap();
    image
}

fn populate(image: &mut Cursor<Vec<u8>>, geometry: FloppyGeometry) {
    let start = SLOT_STRIDE_BYTES as usize;
    let payload = &mut image.get_mut()[start..start + geometry.payload_bytes() as usize];
    let fs = fatfs::FileSystem::new(Cursor::new(payload), fatfs::FsOptions::new()).unwrap();
    let root = fs.root_dir();
    root.create_file("hello.txt.txt")
        .unwrap()
        .write_all(b"hola!")
        .unwrap();
    let dir = root.create_dir("folder").unwrap();
    dir.create_file("binary.dat")
        .unwrap()
        .write_all(&[0, 255, 10])
        .unwrap();
}

#[test]
fn all_geometries_empty_and_populated_read_only() {
    for geometry in [
        FloppyGeometry::Kb720,
        FloppyGeometry::Mb1200,
        FloppyGeometry::Mb1440,
    ] {
        let mut image = image(geometry);
        assert!(list_files(&mut image, 0).unwrap().is_empty());
        populate(&mut image, geometry);
        let before = image.get_ref().clone();
        let entries = list_files(&mut image, 1).unwrap();
        let file = entries.iter().find(|e| e.name == "hello.txt.txt").unwrap();
        assert_eq!(file.size, 5);
        assert_eq!(file.attributes & 0x10, 0);
        assert_eq!(read_file(&mut image, 1, "HELLO.TXT.TXT").unwrap(), b"hola!");
        assert_eq!(
            read_file(&mut image, 1, "folder/binary.dat").unwrap(),
            [0, 255, 10]
        );
        assert!(read_file(&mut image, 1, "missing").is_err());
        assert!(read_file(&mut image, 1, "../hello.txt.txt").is_err());
        assert!(matches!(
            list_files(&mut image, 2),
            Err(GotekError::InvalidSlot { .. })
        ));
        assert!(matches!(
            read_file(&mut image, u16::MAX, "x"),
            Err(GotekError::InvalidSlot { .. })
        ));
        assert_eq!(image.get_ref(), &before);
    }
}

#[test]
fn corrupt_bpb_directory_and_truncated_chain_fail() {
    let geometry = FloppyGeometry::Mb1440;
    let mut image = image(geometry);
    populate(&mut image, geometry);
    let start = SLOT_STRIDE_BYTES as usize;
    image.get_mut()[start + 510] = 0;
    assert!(matches!(
        list_files(&mut image, 1),
        Err(GotekError::InvalidBpb { .. })
    ));
    image.get_mut()[start + 510] = 0x55;
    // Locate the short directory record through its observed alias, then point
    // its first cluster far outside this volume. No FAT parsing is implemented.
    let root = start + 19 * 512;
    let entry = image.get_ref()[root..root + 14 * 512]
        .as_chunks::<32>()
        .0
        .iter()
        .position(|e| e[..11] == *b"HELLOT~1TXT")
        .unwrap()
        * 32
        + root;
    image.get_mut()[entry + 26..entry + 28].copy_from_slice(&4000u16.to_le_bytes());
    assert!(read_file(&mut image, 1, "hello.txt.txt").is_err());
    image.get_mut()[entry + 26..entry + 28].copy_from_slice(&2u16.to_le_bytes());
    image.get_mut()[entry + 28..entry + 32].copy_from_slice(&1024u32.to_le_bytes());
    // The original one-cluster chain cannot satisfy the directory's length.
    assert!(read_file(&mut image, 1, "hello.txt.txt").is_err());
}

#[test]
fn private_ipcas_import_capture() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("image03.img");
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipping private image03.img capture: absent");
            return;
        }
        Err(e) => panic!("{e}"),
    };
    assert!(
        list_files(&mut file, 5)
            .unwrap()
            .iter()
            .any(|e| e.name == "hello.txt.txt" && e.size == 5)
    );
    assert_eq!(read_file(&mut file, 5, "hello.txt.txt").unwrap(), b"hola!");
}

#[test]
fn corrupted_fat_and_truncated_payload_return_errors() {
    let geometry = FloppyGeometry::Mb1440;
    let mut image = image(geometry);
    populate(&mut image, geometry);
    let start = SLOT_STRIDE_BYTES as usize;
    // Make cluster 2 continue to a reserved/bad FAT12 cluster in both copies.
    for fat in [start + 512, start + 10 * 512] {
        image.get_mut()[fat + 3] = 0xf7;
        image.get_mut()[fat + 4] = (image.get_ref()[fat + 4] & 0xf0) | 0x0f;
    }
    let root = start + 19 * 512;
    let entry = image.get_ref()[root..root + 14 * 512]
        .as_chunks::<32>()
        .0
        .iter()
        .position(|e| e[..11] == *b"HELLOT~1TXT")
        .unwrap()
        * 32
        + root;
    image.get_mut()[entry + 28..entry + 32].copy_from_slice(&1024u32.to_le_bytes());
    assert!(read_file(&mut image, 1, "hello.txt.txt").is_err());
    image.get_mut().truncate(start + 33 * 512 + 2);
    assert!(read_file(&mut image, 1, "hello.txt.txt").is_err());
}

#[cfg(feature = "cli")]
#[test]
fn cli_lists_and_emits_exact_bytes_from_regular_read_only_file() {
    use std::{fs, process::Command};
    let geometry = FloppyGeometry::Kb720;
    let mut image = image(geometry);
    populate(&mut image, geometry);
    let path = std::env::temp_dir().join(format!("gotek-files-{}.img", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    file.write_all(image.get_ref()).unwrap();
    drop(file);
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&path, permissions).unwrap();
    let ls = Command::new(env!("CARGO_BIN_EXE_gotek"))
        .arg("ls")
        .arg(&path)
        .args(["--slot", "1"])
        .output()
        .unwrap();
    assert!(ls.status.success(), "{:?}", ls);
    assert!(
        String::from_utf8(ls.stdout)
            .unwrap()
            .contains("hello.txt.txt")
    );
    let cat = Command::new(env!("CARGO_BIN_EXE_gotek"))
        .arg("cat")
        .arg(&path)
        .args(["--slot", "1", "folder/binary.dat"])
        .output()
        .unwrap();
    assert!(cat.status.success());
    assert_eq!(cat.stdout, [0, 255, 10]);
    for (slot, name) in [("2", "hello.txt.txt"), ("1", "missing")] {
        let result = Command::new(env!("CARGO_BIN_EXE_gotek"))
            .arg("cat")
            .arg(&path)
            .args(["--slot", slot, name])
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
    }
    assert_eq!(fs::read(&path).unwrap(), *image.get_ref());
    fs::remove_file(path).unwrap();
}

struct ExtractionDir(std::path::PathBuf);

impl ExtractionDir {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "gotek-extraction-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for ExtractionDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn extraction_preserves_bytes_refuses_overwrites_and_reports_errors() {
    use gotek_core::extract_file;
    use std::{fs, io::ErrorKind};
    let directory = ExtractionDir::new();
    let geometry = FloppyGeometry::Mb1440;
    let mut image = image(geometry);
    populate(&mut image, geometry);
    let before = image.get_ref().clone();
    for (name, expected) in [
        ("hello.txt.txt", b"hola!".as_slice()),
        ("folder/binary.dat", &[0, 255, 10]),
    ] {
        let output = directory.0.join(name.rsplit('/').next().unwrap());
        extract_file(&mut image, 1, name, &output).unwrap();
        assert_eq!(fs::read(&output).unwrap(), expected);
        assert!(matches!(
            extract_file(&mut image, 1, name, &output),
            Err(GotekError::DestinationIo { source, .. }) if source.kind() == ErrorKind::AlreadyExists
        ));
        assert_eq!(fs::read(&output).unwrap(), expected);
    }
    let missing = directory.0.join("not-created");
    assert!(matches!(
        extract_file(&mut image, 1, "missing", &missing),
        Err(GotekError::Io { source, .. }) if source.kind() == ErrorKind::NotFound
    ));
    assert!(!missing.exists());
    assert!(matches!(
        extract_file(&mut image, 2, "hello.txt.txt", &missing),
        Err(GotekError::InvalidSlot { .. })
    ));
    assert!(!missing.exists());
    let nested = directory.0.join("absent/output");
    let error = extract_file(&mut image, 1, "hello.txt.txt", &nested).unwrap_err();
    assert!(matches!(&error, GotekError::DestinationIo { path, source }
        if path == &nested && source.kind() == ErrorKind::NotFound));
    assert!(error.to_string().contains("absent/output"));
    assert!(!directory.0.join("absent").exists());
    assert!(matches!(
        extract_file(&mut image, 1, "hello.txt.txt", &directory.0),
        Err(GotekError::DestinationIo { .. })
    ));
    assert_eq!(image.get_ref(), &before);
}

#[cfg(feature = "cli")]
#[test]
fn cli_extract_defaults_explicit_paths_force_and_source_safety() {
    use std::{fs, process::Command};
    let directory = ExtractionDir::new();
    let geometry = FloppyGeometry::Kb720;
    let mut image = image(geometry);
    populate(&mut image, geometry);
    let source = directory.0.join("source.img");
    fs::write(&source, image.get_ref()).unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_gotek"))
            .current_dir(&directory.0)
            .args(["extract", "source.img", "--slot", "1"])
            .args(args)
            .output()
            .unwrap()
    };
    let result = run(&["hello.txt.txt"]);
    assert!(result.status.success(), "{result:?}");
    assert!(result.stdout.is_empty());
    assert!(String::from_utf8_lossy(&result.stderr).contains("Extracted"));
    assert_eq!(
        fs::read(directory.0.join("hello.txt.txt")).unwrap(),
        b"hola!"
    );
    fs::write(directory.0.join("hello.txt.txt"), b"keep").unwrap();
    assert!(!run(&["hello.txt.txt"]).status.success());
    assert_eq!(
        fs::read(directory.0.join("hello.txt.txt")).unwrap(),
        b"keep"
    );
    assert!(run(&["hello.txt.txt", "--force"]).status.success());
    assert_eq!(
        fs::read(directory.0.join("hello.txt.txt")).unwrap(),
        b"hola!"
    );
    assert!(run(&["folder/binary.dat"]).status.success());
    assert_eq!(
        fs::read(directory.0.join("binary.dat")).unwrap(),
        [0, 255, 10]
    );
    assert!(
        run(&["folder/binary.dat", "--output", "custom.bin"])
            .status
            .success()
    );
    fs::write(directory.0.join("custom.bin"), b"longer previous contents").unwrap();
    assert!(
        !run(&["folder/binary.dat", "--output", "custom.bin"])
            .status
            .success()
    );
    assert!(
        run(&["folder/binary.dat", "--output", "custom.bin", "--force"])
            .status
            .success()
    );
    assert_eq!(
        fs::read(directory.0.join("custom.bin")).unwrap(),
        [0, 255, 10]
    );
    for name in [
        "",
        ".",
        "..",
        "folder/",
        "../hello.txt.txt",
        "/hello.txt.txt",
        "folder/..",
        "C:\\escape",
    ] {
        let result = run(&[name, "--force"]);
        assert!(!result.status.success(), "accepted {name:?}");
        assert!(result.stdout.is_empty());
    }
    for args in [
        vec!["missing", "--output", "missing.txt"],
        vec!["hello.txt.txt", "--slot", "2"],
        vec!["hello.txt.txt", "--output", "absent/output"],
        vec!["hello.txt.txt", "--output", ".", "--force"],
        vec!["hello.txt.txt", "--output", "source.img", "--force"],
    ] {
        let result = run(&args);
        assert!(!result.status.success(), "{args:?}");
        assert!(result.stdout.is_empty());
    }
    let result = run(&["missing", "--output", "custom.bin", "--force"]);
    assert!(!result.status.success());
    assert_eq!(
        fs::read(directory.0.join("custom.bin")).unwrap(),
        [0, 255, 10]
    );
    assert!(fs::read_dir(&directory.0).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".gotek-extract-")
    }));
    assert!(!directory.0.join("missing.txt").exists());
    assert_eq!(fs::read(&source).unwrap(), *image.get_ref());
}

#[cfg(all(feature = "cli", unix))]
#[test]
fn extraction_symlinks_and_hard_links_do_not_modify_source() {
    use std::{fs, os::unix::fs::symlink, process::Command};
    let directory = ExtractionDir::new();
    let geometry = FloppyGeometry::Kb720;
    let mut image = image(geometry);
    populate(&mut image, geometry);
    let source = directory.0.join("source.img");
    let link = directory.0.join("link");
    fs::write(&source, image.get_ref()).unwrap();
    symlink(&source, &link).unwrap();
    assert!(gotek_core::extract_file(&mut image, 1, "hello.txt.txt", &link).is_err());
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_gotek"))
            .arg("extract")
            .arg(&source)
            .args(["--slot", "1", "hello.txt.txt", "--force", "--output"])
            .arg(&link)
            .output()
            .unwrap()
    };
    assert!(!run().status.success());
    fs::remove_file(&link).unwrap();
    fs::hard_link(&source, &link).unwrap();
    assert!(run().status.success());
    assert_eq!(fs::read(&link).unwrap(), b"hola!");
    assert_eq!(fs::read(&source).unwrap(), *image.get_ref());
}
