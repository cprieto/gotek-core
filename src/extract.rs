use std::{
    fs::OpenOptions,
    io::{self, Read, Seek, Write},
    path::Path,
};

use crate::{GotekError, files::read_file};

/// Extract a slot file to a new host filesystem path without modifying the source.
/// `filename` is a `/`-separated FAT path; `destination` accepts any host path.
///
/// Reuses [`crate::read_file`], buffering at most one floppy payload in memory. The
/// destination is created atomically and existing paths (including symlinks) are
/// refused. Parent directories must already exist. Source errors occur before
/// destination creation; destination I/O errors may leave a partial file.
/// Reader position is unspecified. Successful writes are flushed, not synced.
///
/// ```no_run
/// let mut image = std::fs::File::open("image.img")?;
/// gotek_core::extract_file(&mut image, 5, "HELLO.TXT", std::path::Path::new("output/HELLO.TXT"))?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn extract_file<R: Read + Seek + ?Sized>(
    reader: &mut R,
    slot: u16,
    filename: &str,
    destination: impl AsRef<Path>,
) -> Result<(), GotekError> {
    let bytes = read_file(reader, slot, filename)?;
    let path = destination.as_ref();
    let save = || -> io::Result<()> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        file.write_all(&bytes)?;
        file.flush()
    };
    save().map_err(|source| GotekError::DestinationIo {
        path: path.to_path_buf(),
        source,
    })
}
