use clap::{Parser, Subcommand, ValueEnum};
use gotek_core::{
    FormatOptions, GotekImageInfo, SlotFormatOptions, SlotInfo, ValidationWarning, create_image,
    format_existing_image, geometry::FloppyGeometry, inspect, metadata::UpdateTime,
};
use std::{
    error::Error,
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
};

#[derive(Parser, Debug)]
#[command(version, about = "Inspect and initialize original Gotek floppy images")]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Inspect an existing Gotek image or device (read-only).
    Inspect {
        /// Image file or block device to inspect.
        path: PathBuf,
    },
    /// Initialize a new image, or overwrite a regular image with --force.
    Init {
        /// Output image path (must not exist unless --force is used).
        path: PathBuf,
        /// Number of floppy slots (1–100).
        #[arg(long, value_parser = clap::value_parser!(u16).range(1..=100))]
        floppies: u16,
        /// Floppy geometry: 720k, 1.2m, or 1.44m.
        #[arg(long, value_enum)]
        size: FloppySize,
        /// Reinitialize an existing regular image, preserving its total size.
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
            // Fixed, deterministic test inputs. The formatter intentionally does not
            // invent timestamps or serial numbers; we can expose these later.
            let timestamp = UpdateTime {
                year: 2026,
                month: 11,
                day: 1,
                hour: 12,
                minute: 0,
                second: 0,
            };
            let slot = SlotFormatOptions {
                volume_serial: 0x1234_5678,
                update_time: timestamp,
                root_label_time: timestamp,
            };
            let options = FormatOptions::new(size.into(), floppies, slot)?;
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

// Check the path before opening it so special files (notably FIFOs) are not
// opened for writing. The core checks the opened handle too, guarding against
// the path being replaced with a device between this check and open.
fn initialize_image(
    path: &Path,
    options: &FormatOptions,
    force: bool,
) -> Result<u64, Box<dyn Error>> {
    let required_bytes = options.image_size_bytes()?;
    if force {
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                if !metadata.is_file() {
                    return Err(io::Error::new(io::ErrorKind::InvalidInput, "--force requires a regular image file; devices, directories, and symlinks are refused").into());
                }
                // Never truncate or append: the formatter only replaces the prefix
                // needed by the requested slots and their boundary sector.
                let mut image = OpenOptions::new().write(true).open(path)?;
                format_existing_image(&mut image, options)?;
                return Ok(image.metadata()?.len());
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    create_image(path, options)?;
    Ok(required_bytes)
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
