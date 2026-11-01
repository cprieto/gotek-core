//! Gotek-specific ASCII records in the final sector of each slot.
use crate::{
    GotekError,
    layout::{MAX_SLOTS, SECTOR_BYTES},
};

const COUNT_RECORD_OFFSET: usize = 0x50;
const COUNT_PREFIX: &[u8] = b"MAX BLOCK:";
const UPDATE_PREFIX: &[u8] = b"UPDATE TIME:";

/// A validated local wall-clock timestamp. No timezone is encoded on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdateTime {
    /// Four-digit calendar year (1–9999).
    pub year: u16,
    /// Calendar month (1–12).
    pub month: u8,
    /// Day of month, checked against leap years.
    pub day: u8,
    /// Hour (0–23).
    pub hour: u8,
    /// Minute (0–59).
    pub minute: u8,
    /// Second (0–59).
    pub second: u8,
}

/// Recognized metadata from a single slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotMetadata {
    /// Parsed update time; absent if its record is missing or malformed.
    pub update_time: Option<UpdateTime>,
    /// Exact timestamp record bytes, excluding any NUL, for diagnostics.
    pub raw_update_record: Vec<u8>,
    /// Configured slot count, present only in slot zero.
    pub declared_slot_count: Option<u16>,
}

pub(crate) fn parse_count(sector: &[u8; SECTOR_BYTES]) -> Result<u16, GotekError> {
    let record = &sector[COUNT_RECORD_OFFSET..COUNT_RECORD_OFFSET + 14];
    if &record[..10] != COUNT_PREFIX || record[13] != 0 {
        return Err(GotekError::InvalidCountRecord);
    }
    let count = decimal(&record[10..13]).ok_or(GotekError::InvalidCountRecord)?;
    if !(1..=MAX_SLOTS).contains(&count) {
        return Err(GotekError::InvalidSlotCount(count));
    }
    Ok(count)
}

pub(crate) fn parse_metadata(sector: &[u8; SECTOR_BYTES], count: Option<u16>) -> SlotMetadata {
    // The fixed record is 31 bytes plus its terminator. Do not scan into other records.
    let record = &sector[..32];
    let end = record
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(record.len());
    SlotMetadata {
        update_time: parse_update(record),
        raw_update_record: record[..end].to_vec(),
        declared_slot_count: count,
    }
}

fn decimal(bytes: &[u8]) -> Option<u16> {
    bytes.iter().try_fold(0_u16, |value, &byte| {
        if !byte.is_ascii_digit() {
            return None;
        }
        value.checked_mul(10)?.checked_add(u16::from(byte - b'0'))
    })
}

fn parse_update(record: &[u8]) -> Option<UpdateTime> {
    if &record[..12] != UPDATE_PREFIX || record[31] != 0 {
        return None;
    }
    let text = &record[12..31];
    for (index, expected) in [(4, b'-'), (7, b'-'), (10, b' '), (13, b'-'), (16, b'-')] {
        if text[index] != expected {
            return None;
        }
    }
    let year = decimal(&text[..4])?;
    let month = u8::try_from(decimal(&text[5..7])?).ok()?;
    let day = u8::try_from(decimal(&text[8..10])?).ok()?;
    let hour = u8::try_from(decimal(&text[11..13])?).ok()?;
    let minute = u8::try_from(decimal(&text[14..16])?).ok()?;
    let second = u8::try_from(decimal(&text[17..19])?).ok()?;
    let leap_year =
        year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days_in_month = match month {
        2 if leap_year => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => return None,
    };
    if year == 0 || day == 0 || day > days_in_month || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    Some(UpdateTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
    })
}
