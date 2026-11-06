use crate::{
    GotekError,
    disk::{GotekDisk, SlotLayout},
    fat12::Bpb,
    files::{self, FileInfo},
    geometry::FloppyGeometry,
    import,
    inspect::read_sector,
    label::{encode_label, write_slot_label},
    layout::METADATA_OFFSET_BYTES,
    metadata::{self, SlotMetadata, UpdateTime, encode_update_time},
    slot_image,
    usage::{self, SlotUsage},
};
use std::io::{Read, Seek, Write};

/// A read-only view of a slot, borrowing its [`GotekDisk`] exclusively.
///
/// The handle exposes validated structural information and reads the current
/// boot-sector label on request. It does not cache label text.
pub struct Slot<'a, R> {
    disk: &'a mut GotekDisk<R>,
    index: u16,
}

/// A mutable handle to one slot, borrowing its [`GotekDisk`] exclusively.
pub struct SlotMut<'a, R> {
    disk: &'a mut GotekDisk<R>,
    index: u16,
}

impl<R: Read + Seek> GotekDisk<R> {
    /// Validate and borrow one zero-based slot for read-only access.
    /// Other slots and the extra boundary sector are not read.
    pub fn slot(&mut self, index: u16) -> Result<Slot<'_, R>, GotekError> {
        self.validate_slot(index)?;
        Ok(Slot { disk: self, index })
    }

    fn check_index(&self, index: u16) -> Result<(), GotekError> {
        if usize::from(index) >= self.slots.len() {
            return Err(GotekError::InvalidSlot {
                index,
                count: self.slots.len() as u16,
            });
        }
        Ok(())
    }
}

impl<R: Read + Write + Seek> GotekDisk<R> {
    /// Validate and borrow one zero-based slot for mutation.
    /// Requires a complete, structurally valid extra boundary sector.
    pub fn slot_mut(&mut self, index: u16) -> Result<SlotMut<'_, R>, GotekError> {
        self.ensure_healthy()?;
        self.check_index(index)?;
        self.validate_boundary()?;
        self.validate_slot(index)?;
        Ok(SlotMut { disk: self, index })
    }
}

impl<R> Slot<'_, R> {
    fn layout(&self) -> &SlotLayout {
        self.disk.slots[usize::from(self.index)]
            .as_ref()
            .expect("slot validated before handle creation")
    }

    /// Zero-based slot index.
    pub fn index(&self) -> u16 {
        self.index
    }

    /// Absolute byte offset of the slot's FAT12 boot sector.
    pub fn offset(&self) -> u64 {
        self.layout().offset
    }

    /// Validated floppy geometry.
    pub fn geometry(&self) -> FloppyGeometry {
        self.layout().geometry
    }

    /// Validated FAT12 BIOS parameter block.
    pub fn bpb(&self) -> &Bpb {
        &self.layout().bpb
    }
}

impl<R: Read + Seek> Slot<'_, R> {
    /// Read the current boot-sector volume label, without caching it.
    pub fn label(&mut self) -> Result<String, GotekError> {
        self.disk.ensure_healthy()?;
        let offset = self.layout().offset;
        let sector = read_sector(&mut self.disk.target, offset)?;
        Ok(String::from_utf8_lossy(&sector[43..54])
            .trim_end_matches(' ')
            .to_owned())
    }

    /// Read this slot's current Gotek metadata sector.
    pub fn metadata(&mut self) -> Result<SlotMetadata, GotekError> {
        self.disk.ensure_healthy()?;
        let offset = self.layout().offset + METADATA_OFFSET_BYTES;
        let sector = read_sector(&mut self.disk.target, offset)?;
        let count = (self.index == 0).then_some(self.disk.slots.len() as u16);
        Ok(metadata::parse_metadata(&sector, count))
    }

    /// List files and directories in the FAT12 root directory.
    pub fn files(&mut self) -> Result<Vec<FileInfo>, GotekError> {
        self.disk.ensure_healthy()?;
        let layout = self.disk.slots[usize::from(self.index)]
            .as_ref()
            .expect("slot validated before handle creation");
        files::list_at(
            &mut self.disk.target,
            layout.offset,
            layout.geometry.payload_bytes(),
        )
    }

    /// Read a slash-separated relative FAT12 file path as exact bytes.
    pub fn read_file(&mut self, path: &str) -> Result<Vec<u8>, GotekError> {
        self.disk.ensure_healthy()?;
        let layout = self.disk.slots[usize::from(self.index)]
            .as_ref()
            .expect("slot validated before handle creation");
        files::read_at(
            &mut self.disk.target,
            layout.offset,
            layout.geometry.payload_bytes(),
            path,
        )
    }

    /// Count FAT12 allocation states, requiring matching FAT copies.
    pub fn usage(&mut self) -> Result<SlotUsage, GotekError> {
        self.disk.ensure_healthy()?;
        let layout = self.disk.slots[usize::from(self.index)]
            .as_ref()
            .expect("slot validated before handle creation");
        usage::usage_at(&mut self.disk.target, layout.offset, &layout.bpb)
    }

    /// Write exactly this slot's FAT12 payload to an independent writer.
    ///
    /// The caller controls destination creation and overwrite behavior. The
    /// source disk is only read; a failed write may leave partial output.
    pub fn save_image<W: Write + ?Sized>(&mut self, writer: &mut W) -> Result<(), GotekError> {
        self.disk.ensure_healthy()?;
        let layout = self.disk.slots[usize::from(self.index)]
            .as_ref()
            .expect("slot validated before handle creation");
        slot_image::save_at(
            &mut self.disk.target,
            layout.offset,
            layout.geometry.payload_bytes(),
            writer,
        )
    }
}

impl<R: Read + Seek> SlotMut<'_, R> {
    /// Borrow this slot as a read-only view of its structural information.
    /// Returns `DiskNeedsReopen` after a failed write may have changed disk bytes.
    pub fn as_slot(&mut self) -> Result<Slot<'_, R>, GotekError> {
        self.disk.ensure_healthy()?;
        Ok(Slot {
            disk: &mut *self.disk,
            index: self.index,
        })
    }
}

impl<R: Read + Write + Seek> SlotMut<'_, R> {
    /// Import one FAT12 file from bytes, updating this slot's timestamp.
    ///
    /// `name` is a single FAT filename. Existing files require `overwrite`.
    /// Changes are staged before target writes; physical writes are not
    /// power-failure atomic.
    pub fn import_file(
        &mut self,
        name: &str,
        bytes: &[u8],
        overwrite: bool,
        update_time: UpdateTime,
    ) -> Result<(), GotekError> {
        self.disk.ensure_healthy()?;
        let record = import::validate_import(name, update_time)?;
        let request = import::ImportRequest {
            name,
            bytes,
            overwrite,
            record,
        };
        let layout = self.disk.slots[usize::from(self.index)]
            .as_ref()
            .expect("slot validated before handle creation");
        let mut write_started = false;
        let result = import::import_at(
            &mut self.disk.target,
            layout.offset,
            &layout.bpb,
            &request,
            &mut write_started,
        );
        if result.is_err() && write_started {
            self.disk.poisoned = true;
        }
        result
    }

    /// Replace this slot with a complete FAT12 image of identical geometry.
    ///
    /// The source must be an independent `Read + Seek` stream with exactly the
    /// declared payload length. Its boot sector and FAT12 contents are copied
    /// byte-for-byte. For the final slot, the extra boundary copy is synchronized
    /// with the new boot sector (with its first three bytes zero). This is
    /// destructive and not power-failure atomic. Back up the target first;
    /// after an I/O failure, reopen the disk before more operations.
    pub fn load_image<S: Read + Seek + ?Sized>(
        &mut self,
        source: &mut S,
        update_time: UpdateTime,
    ) -> Result<(), GotekError> {
        self.disk.ensure_healthy()?;
        let record = encode_update_time(update_time)?;
        self.load_image_with_record(source, &record)
    }

    pub(crate) fn load_image_with_record<S: Read + Seek + ?Sized>(
        &mut self,
        source: &mut S,
        record: &[u8; 31],
    ) -> Result<(), GotekError> {
        self.disk.ensure_healthy()?;
        let index = usize::from(self.index);
        let layout = self.disk.slots[index]
            .as_ref()
            .expect("slot validated before handle creation");
        let boundary_offset =
            (index + 1 == self.disk.slots.len()).then_some(self.disk.boundary_offset);
        let destination = slot_image::LoadTarget {
            capacity: self.disk.capacity,
            start: layout.offset,
            bpb: &layout.bpb,
            boundary_offset,
        };
        let mut write_started = false;
        let result = slot_image::load_at(
            &mut self.disk.target,
            destination,
            source,
            record,
            &mut write_started,
        );
        match result {
            Ok(bpb) => {
                // Identical geometry can still have a different valid BPB.
                self.disk.slots[index]
                    .as_mut()
                    .expect("slot validated before handle creation")
                    .bpb = bpb;
                Ok(())
            }
            Err(error) => {
                if write_started {
                    self.disk.poisoned = true;
                }
                Err(error)
            }
        }
    }

    /// Change the boot-sector and root-directory volume-label bytes.
    ///
    /// The final slot's boundary copy is updated too. The root entry is checked
    /// before writing. A write failure may leave the copies inconsistent;
    /// physical-device writes are not power-failure atomic.
    pub fn set_label(&mut self, label: &str) -> Result<(), GotekError> {
        self.disk.ensure_healthy()?;
        let name = encode_label(label)?;
        self.set_encoded_label(&name)
    }

    pub(crate) fn set_encoded_label(&mut self, name: &[u8; 11]) -> Result<(), GotekError> {
        self.disk.ensure_healthy()?;
        let index = usize::from(self.index);
        let layout = self.disk.slots[index]
            .as_ref()
            .expect("slot validated before handle creation");
        let is_last = index + 1 == self.disk.slots.len();
        let mut write_started = false;
        let result = write_slot_label(
            &mut self.disk.target,
            layout,
            self.disk.boundary_offset,
            is_last,
            name,
            &mut write_started,
        );
        if result.is_err() && write_started {
            self.disk.poisoned = true;
        }
        result
    }
}
