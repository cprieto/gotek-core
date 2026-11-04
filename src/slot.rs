use crate::{
    GotekError,
    disk::{GotekDisk, SlotLayout},
    fat12::Bpb,
    geometry::FloppyGeometry,
    inspect::read_sector,
    label::{encode_label, write_slot_label},
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
    /// Borrow a zero-based slot for read-only access.
    pub fn slot(&mut self, index: u16) -> Result<Slot<'_, R>, GotekError> {
        self.check_index(index)?;
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
    /// Borrow a zero-based slot for label editing.
    pub fn slot_mut(&mut self, index: u16) -> Result<SlotMut<'_, R>, GotekError> {
        self.check_index(index)?;
        Ok(SlotMut { disk: self, index })
    }
}

impl<R> Slot<'_, R> {
    fn layout(&self) -> &SlotLayout {
        &self.disk.slots[usize::from(self.index)]
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
        let offset = self.layout().offset;
        let sector = read_sector(&mut self.disk.target, offset)?;
        Ok(String::from_utf8_lossy(&sector[43..54])
            .trim_end_matches(' ')
            .to_owned())
    }
}

impl<R> SlotMut<'_, R> {
    /// Borrow this slot as a read-only view of its structural information.
    pub fn as_slot(&mut self) -> Slot<'_, R> {
        Slot {
            disk: &mut *self.disk,
            index: self.index,
        }
    }
}

impl<R: Read + Write + Seek> SlotMut<'_, R> {
    /// Change the boot-sector and root-directory volume-label bytes.
    ///
    /// The final slot's boundary copy is updated too. The root entry is checked
    /// before writing. A write failure may leave the copies inconsistent;
    /// physical-device writes are not power-failure atomic.
    pub fn set_label(&mut self, label: &str) -> Result<(), GotekError> {
        let name = encode_label(label)?;
        self.set_encoded_label(&name)
    }

    pub(crate) fn set_encoded_label(&mut self, name: &[u8; 11]) -> Result<(), GotekError> {
        let index = usize::from(self.index);
        let layout = &self.disk.slots[index];
        let is_last = index + 1 == self.disk.slots.len();
        write_slot_label(
            &mut self.disk.target,
            layout,
            self.disk.boundary_offset,
            is_last,
            name,
        )
    }
}
