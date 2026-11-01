//! Run with `cargo run --example inspect_image -- path/to/image.img`.
use std::{error::Error, fs::File, io};

fn main() -> Result<(), Box<dyn Error>> {
    let path = std::env::args_os().nth(1).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: inspect_image <image.img>",
        )
    })?;
    let mut image = File::open(path)?;
    let info = gotek_core::inspect(&mut image)?;
    println!("Geometry: {}", info.geometry);
    println!("Slots: {}", info.floppy_count);
    for slot in &info.slots {
        println!(
            "Slot {}: {} ({})",
            slot.index, slot.volume_label, slot.geometry
        );
        println!(
            "  Update time (local, timezone unknown): {:?}",
            slot.metadata.update_time
        );
        if let Some(count) = slot.metadata.declared_slot_count {
            println!("  MAX BLOCK: {count}");
        }
        if slot.metadata.update_time.is_none() {
            println!("  Raw update record: {:?}", slot.metadata.raw_update_record);
        }
        for warning in &slot.warnings {
            println!("  Warning: {:?}", warning.kind);
        }
    }
    for warning in &info.warnings {
        println!("Boundary warning: {:?}", warning.kind);
    }
    Ok(())
}
