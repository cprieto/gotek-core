use clap::Parser;
use gotek_core::{GotekImageInfo, SlotInfo, ValidationWarning, inspect};
use std::{error::Error, fs::File, path::PathBuf};

#[derive(Parser, Debug)]
#[command(version, about = "Inspect an ipcas-formatted Gotek image")]
struct Args {
    /// Path to an image file or block device.
    image: PathBuf,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();

    let mut image = File::open(&args.image)?;
    let info = inspect(&mut image)?;

    print_image_info(&info);

    Ok(())
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
