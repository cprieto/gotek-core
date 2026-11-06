use clap::{Parser, Subcommand, ValueEnum};
use gotek_core::{
    FormatOptions, GotekImageInfo, SlotInfo, ValidationWarning, create_image,
    format_existing_image, geometry::FloppyGeometry, inspect, list_files, read_file,
};
use std::{
    error::Error,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

#[cfg(target_os = "linux")]
mod linux_device;

#[derive(Parser, Debug)]
#[command(version, about = "Inspect and initialize original Gotek floppy images")]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Save a slot as a complete standalone FAT12 floppy image.
    SaveImage {
        image: PathBuf,
        #[arg(long)]
        slot: u16,
        destination: PathBuf,
    },
    /// Replace a slot with a complete FAT12 floppy image of identical geometry.
    LoadImage {
        image: PathBuf,
        #[arg(long)]
        slot: u16,
        source: PathBuf,
    },
    /// Import one local file into a floppy slot.
    Import {
        image: PathBuf,
        #[arg(long)]
        slot: u16,
        source: PathBuf,
        /// Name inside the slot (defaults to source basename).
        #[arg(long)]
        name: Option<String>,
        /// Replace an existing file in the slot.
        #[arg(long)]
        force: bool,
    },
    /// Save a slot file to disk without modifying the image.
    Extract {
        image: PathBuf,
        #[arg(long)]
        slot: u16,
        filename: String,
        /// Destination path (defaults to the FAT filename's basename).
        #[arg(long)]
        output: Option<PathBuf>,
        /// Replace an existing destination file.
        #[arg(long)]
        force: bool,
    },
    /// List a slot's root directory (read-only).
    Ls {
        image: PathBuf,
        /// Zero-based slot number.
        #[arg(long)]
        slot: u16,
    },
    /// Write a slot file's exact bytes to stdout (read-only).
    Cat {
        image: PathBuf,
        #[arg(long)]
        slot: u16,
        filename: String,
    },
    /// Inspect an existing Gotek image or device (read-only).
    Inspect {
        /// Image file or block device to inspect.
        path: PathBuf,
    },
    /// Initialize a new image, or reformat an existing image or USB device with --force.
    Init {
        /// Output image path (must not exist unless --force is used).
        path: PathBuf,
        /// Number of floppy slots (1–100).
        #[arg(long, value_parser = clap::value_parser!(u16).range(1..=100))]
        floppies: u16,
        /// Floppy geometry: 720k, 1.2m, or 1.44m.
        #[arg(long, value_enum)]
        size: FloppySize,
        /// Reinitialize an existing regular image or whole USB block device.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum FloppySize {
    #[value(name = "720k")]
    K720,
    #[value(name = "1.2m")]
    M1200,
    #[value(name = "1.44m")]
    M1440,
}

impl From<FloppySize> for FloppyGeometry {
    fn from(value: FloppySize) -> Self {
        match value {
            FloppySize::K720 => Self::Kb720,
            FloppySize::M1200 => Self::Mb1200,
            FloppySize::M1440 => Self::Mb1440,
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    match Args::parse().command {
        Command::SaveImage {
            image,
            slot,
            destination,
        } => {
            gotek_core::save_image(image, slot, destination)
                .map_err(|error| io::Error::other(error.to_string()))?;
        }
        Command::LoadImage {
            image,
            slot,
            source,
        } => {
            gotek_core::load_image(image, slot, source)
                .map_err(|error| io::Error::other(error.to_string()))?;
        }
        Command::Import {
            image,
            slot,
            source,
            name,
            force,
        } => {
            gotek_core::import_file(image, slot, source, name.as_deref(), force)?;
        }
        Command::Extract {
            image,
            slot,
            filename,
            output,
            force,
        } => {
            let destination = match output {
                Some(path) => path,
                None => default_destination(&filename)?,
            };
            let result = extract_to_path(&image, slot, &filename, &destination, force);
            result.map_err(|error| {
                io::Error::other(format!("extract to {}: {error}", destination.display()))
            })?;
            eprintln!("Extracted {filename} to {}", destination.display());
        }
        Command::Ls { image, slot } => {
            let mut source = File::open(image)?;
            for entry in list_files(&mut source, slot)? {
                println!("{:02x}\t{}\t{}", entry.attributes, entry.size, entry.name);
            }
        }
        Command::Cat {
            image,
            slot,
            filename,
        } => {
            let mut source = File::open(image)?;
            let bytes = read_file(&mut source, slot, &filename)?;
            io::stdout().lock().write_all(&bytes)?;
        }
        Command::Inspect { path } => {
            let mut image = File::open(&path)?;
            let info = inspect(&mut image)?;
            print_image_info(&info);
        }
        Command::Init {
            path,
            floppies,
            size,
            force,
        } => {
            let options = FormatOptions::new(size.into(), floppies)?;
            let bytes = initialize_image(&path, &options, force)?;
            let action = if force { "Initialized" } else { "Created" };
            println!(
                "{action} {}: {} floppy slots, {} each, {} bytes",
                path.display(),
                floppies,
                FloppyGeometry::from(size),
                bytes
            );
        }
    }
    Ok(())
}

fn default_destination(filename: &str) -> io::Result<PathBuf> {
    let name = filename.rsplit('/').next().unwrap_or_default();
    // Reject separators and platform prefixes as well as dot components on every OS.
    if name.is_empty() || name == "." || name == ".." || name.contains(['\\', ':']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unsafe default destination name; specify --output",
        ));
    }
    Ok(PathBuf::from(name))
}

fn extract_to_path(
    image: &Path,
    slot: u16,
    filename: &str,
    destination: &Path,
    force: bool,
) -> Result<(), Box<dyn Error>> {
    let mut source = File::open(image)?;
    if !force || !destination.try_exists()? {
        gotek_core::extract_file(&mut source, slot, filename, destination)?;
        return Ok(());
    }
    if !fs::symlink_metadata(destination)?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "--force requires a regular destination file",
        )
        .into());
    }
    if fs::canonicalize(image)? == fs::canonicalize(destination)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "destination must not be the source image",
        )
        .into());
    }
    // Replace the directory entry instead of truncating its inode: even a hard
    // link to the source image cannot cause source writes. Stage in the same
    // directory so rename stays on the same filesystem.
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    for attempt in 0..1000 {
        let temporary = parent.join(format!(
            ".gotek-extract-{}-{attempt}.tmp",
            std::process::id()
        ));
        match gotek_core::extract_file(&mut source, slot, filename, &temporary) {
            Err(gotek_core::GotekError::DestinationIo { source, .. })
                if source.kind() == io::ErrorKind::AlreadyExists =>
            {
                continue;
            }
            Err(error) => {
                if matches!(error, gotek_core::GotekError::DestinationIo { .. }) {
                    let _ = fs::remove_file(&temporary);
                }
                return Err(error.into());
            }
            Ok(()) => {
                let result = fs::rename(&temporary, destination);
                if result.is_err() {
                    let _ = fs::remove_file(&temporary);
                }
                result?;
                return Ok(());
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "cannot allocate extraction temporary file",
    )
    .into())
}

// Check the path before opening it so special files (notably FIFOs) are not
// opened for writing. The core checks the opened handle too, guarding against
// the path being replaced with a device between this check and open.
fn initialize_image(
    path: &Path,
    options: &FormatOptions,
    force: bool,
) -> Result<u64, Box<dyn Error>> {
    let required_bytes = options.image_size_bytes()?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => {
            if force {
                // Never truncate or append: the formatter only replaces the prefix
                // needed by the requested slots and their boundary sector.
                let mut image = OpenOptions::new().write(true).open(path)?;
                format_existing_image(&mut image, options)?;
                return Ok(image.metadata()?.len());
            }
            create_image(path, options)?;
        }
        Ok(metadata) => {
            #[cfg(target_os = "linux")]
            {
                use std::os::unix::fs::FileTypeExt;
                if metadata.file_type().is_block_device() {
                    return linux_device::format_device(path, &metadata, options, force);
                }
            }
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "destination must be a regular image file or a whole USB block device; symlinks and other file types are refused").into());
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if is_device_namespace(path)? {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "refusing to create a missing path under /dev; select an existing USB block device").into());
            }
            create_image(path, options)?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(required_bytes)
}

fn is_device_namespace(path: &Path) -> io::Result<bool> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok(fs::canonicalize(parent)?.starts_with("/dev"))
}

fn print_image_info(info: &GotekImageInfo) {
    println!("Gotek image inspection");
    println!("======================");
    println!("Geometry:        {}", info.geometry);
    println!("Slots:           {}", info.floppy_count);
    println!(
        "Boundary offset: {:#x} ({} bytes)",
        info.boundary_offset, info.boundary_offset
    );
    println!();
    println!("Slots");
    println!("-----");
    for slot in &info.slots {
        print_slot(slot);
    }
    println!();
    println!("Validation");
    println!("----------");
    let warning_count: usize = info
        .slots
        .iter()
        .map(|slot| slot.warnings.len())
        .sum::<usize>()
        + info.warnings.len();
    if warning_count == 0 {
        println!("No warnings found.");
    } else {
        println!("{warning_count} warning(s) found:");
        for slot in &info.slots {
            for warning in &slot.warnings {
                print_warning(warning);
            }
        }
        for warning in &info.warnings {
            print_warning(warning);
        }
    }
}

fn print_slot(slot: &SlotInfo) {
    let timestamp = match slot.metadata.update_time {
        Some(time) => format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            time.year, time.month, time.day, time.hour, time.minute, time.second
        ),
        None => "Unknown".to_string(),
    };
    println!(
        "Slot {:>3}: {:<11} | {:<7} | offset {:#010x} | {}",
        slot.index, slot.volume_label, slot.geometry, slot.offset, timestamp
    );
}

fn print_warning(warning: &ValidationWarning) {
    println!("  {:?}: {:?}", warning.location, warning.kind);
}

#[cfg(test)]
mod init_tests {
    use super::is_device_namespace;
    use std::path::Path;

    #[test]
    fn missing_device_paths_are_not_image_creation_targets() {
        assert!(is_device_namespace(Path::new("/dev/nonexistent-gotek-device")).unwrap());
        assert!(!is_device_namespace(Path::new("new-image.img")).unwrap());
    }
}
