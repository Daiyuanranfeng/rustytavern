use crate::{MAX_INPUT_BYTES, MAX_JSON_BYTES};
use rustytavern_core::{Error, Result};
use std::{
    collections::BTreeSet,
    io::{Cursor, Read, Write},
};
use zip::{ZipArchive, ZipWriter};

pub const MAX_ARCHIVE_ENTRIES: usize = 4096;
pub const MAX_EXPANDED_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_ENTRY_BYTES: u64 = MAX_INPUT_BYTES as u64;

fn invalid() -> Error {
    Error::new("invalid_charx", "The CHARX archive is invalid.")
}

fn limit() -> Error {
    Error::new(
        "invalid_card_size",
        "The CHARX archive exceeds a resource limit.",
    )
}

fn path(name: &str) -> Result<()> {
    if name.len() > 1024
        || name.contains(['\\', ':'])
        || name.chars().any(char::is_control)
        || name
            .strip_suffix('/')
            .unwrap_or(name)
            .split('/')
            .any(|part| matches!(part, "" | "." | ".."))
    {
        return Err(Error::new(
            "invalid_charx_path",
            "The archive path is invalid.",
        ));
    }
    Ok(())
}

fn number(bytes: &[u8], start: usize, length: usize) -> Result<u64> {
    let source = bytes
        .get(start..start.checked_add(length).ok_or_else(invalid)?)
        .ok_or_else(invalid)?;
    Ok(source.iter().enumerate().fold(0, |value, (index, byte)| {
        value | (u64::from(*byte) << (8 * index))
    }))
}

// The preflight bounds metadata allocation before the ZIP reader builds its index.
fn preflight(bytes: &[u8]) -> Result<usize> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(limit());
    }
    let end = bytes
        .windows(4)
        .enumerate()
        .rev()
        .find_map(|(end, part)| {
            (part == b"PK\x05\x06"
                && end
                    .checked_add(22)
                    .and_then(|n| n.checked_add(number(bytes, end + 20, 2).ok()? as usize))
                    == Some(bytes.len()))
            .then_some(end)
        })
        .ok_or_else(invalid)?;
    let mut count = number(bytes, end + 10, 2)?;
    let mut size = number(bytes, end + 12, 4)?;
    let mut offset = number(bytes, end + 16, 4)?;
    let mut directory_end = end as u64;
    if number(bytes, end + 4, 2)? != 0
        || number(bytes, end + 6, 2)? != 0
        || number(bytes, end + 8, 2)? != count
    {
        return Err(invalid());
    }
    if count == 65535 || size == u64::from(u32::MAX) || offset == u64::from(u32::MAX) {
        let locator = end.checked_sub(20).ok_or_else(invalid)?;
        if bytes.get(locator..locator + 4) != Some(b"PK\x06\x07")
            || number(bytes, locator + 4, 4)? != 0
            || number(bytes, locator + 16, 4)? != 1
        {
            return Err(invalid());
        }
        let record = usize::try_from(number(bytes, locator + 8, 8)?).map_err(|_| invalid())?;
        if bytes.get(record..record.checked_add(4).ok_or_else(invalid)?) != Some(b"PK\x06\x06")
            || number(bytes, record + 4, 8)?
                .checked_add(12)
                .and_then(|length| (record as u64).checked_add(length))
                != Some(locator as u64)
            || number(bytes, record + 16, 4)? != 0
            || number(bytes, record + 20, 4)? != 0
            || number(bytes, record + 24, 8)? != number(bytes, record + 32, 8)?
        {
            return Err(invalid());
        }
        count = number(bytes, record + 32, 8)?;
        size = number(bytes, record + 40, 8)?;
        offset = number(bytes, record + 48, 8)?;
        directory_end = record as u64;
    }
    if count == 0 || count > MAX_ARCHIVE_ENTRIES as u64 {
        return Err(limit());
    }
    if offset.checked_add(size) != Some(directory_end) || directory_end > bytes.len() as u64 {
        return Err(invalid());
    }
    let mut cursor = offset as usize;
    let mut names = BTreeSet::new();
    for _ in 0..count {
        let header = bytes
            .get(cursor..cursor.checked_add(46).ok_or_else(invalid)?)
            .ok_or_else(invalid)?;
        if &header[..4] != b"PK\x01\x02" {
            return Err(invalid());
        }
        if number(header, 8, 2)? & 1 != 0 || !matches!(number(header, 10, 2)?, 0 | 8) {
            return Err(Error::new(
                "unsupported_charx_encoding",
                "The archive requires unsupported encryption or compression.",
            ));
        }
        let mode = (number(header, 38, 4)? >> 16) & 0o170000;
        if !matches!(mode, 0 | 0o100000 | 0o040000) {
            return Err(Error::new(
                "unsupported_charx_entry",
                "The archive contains a special file.",
            ));
        }
        let length = number(header, 28, 2)? as usize;
        let start = cursor + 46;
        let name = std::str::from_utf8(
            bytes
                .get(start..start.checked_add(length).ok_or_else(invalid)?)
                .ok_or_else(invalid)?,
        )
        .map_err(|_| invalid())?;
        path(name)?;
        if !names.insert(name) {
            return Err(Error::new(
                "invalid_charx_path",
                "The archive contains a duplicate path.",
            ));
        }
        cursor = start
            .checked_add(length)
            .and_then(|n| n.checked_add(number(header, 30, 2).ok()? as usize))
            .and_then(|n| n.checked_add(number(header, 32, 2).ok()? as usize))
            .ok_or_else(invalid)?;
        if cursor > directory_end as usize {
            return Err(invalid());
        }
    }
    if cursor != directory_end as usize {
        return Err(invalid());
    }
    Ok(count as usize)
}

fn open(bytes: &[u8]) -> Result<ZipArchive<Cursor<&[u8]>>> {
    let count = preflight(bytes)?;
    let archive = ZipArchive::with_config(
        zip::read::Config {
            archive_offset: zip::read::ArchiveOffset::Known(0),
        },
        Cursor::new(bytes),
    )
    .map_err(Error::context(
        "invalid_charx",
        "The CHARX archive is invalid.",
    ))?;
    if archive.len() != count {
        return Err(invalid());
    }
    for name in archive.file_names() {
        path(name)?;
    }
    Ok(archive)
}

pub(super) fn document(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut archive = open(bytes)?;
    let mut total = 0u64;
    let mut document = None;
    for index in 0..archive.len() {
        let file = archive.by_index(index).map_err(Error::context(
            "invalid_charx",
            "The archive entry is invalid.",
        ))?;
        let card = file.name() == "card.json";
        let maximum = if card {
            MAX_JSON_BYTES as u64
        } else {
            MAX_ENTRY_BYTES
        };
        let size = file.size();
        total = total
            .checked_add(size)
            .filter(|n| *n <= MAX_EXPANDED_BYTES)
            .ok_or_else(limit)?;
        if size > maximum || file.is_dir() && size != 0 {
            return Err(limit());
        }
    }
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(Error::context(
            "invalid_charx",
            "The archive entry is invalid.",
        ))?;
        let card = file.name() == "card.json";
        let maximum = if card {
            MAX_JSON_BYTES as u64
        } else {
            MAX_ENTRY_BYTES
        };
        let size = file.size();
        let mut reader = (&mut file).take(maximum + 1);
        let read = if card {
            let mut json = Vec::new();
            let read = reader.read_to_end(&mut json).map_err(Error::context(
                "invalid_charx",
                "The card entry cannot be read.",
            ))? as u64;
            document = Some(json);
            read
        } else {
            std::io::copy(&mut reader, &mut std::io::sink()).map_err(Error::context(
                "invalid_charx",
                "The archive entry cannot be read.",
            ))?
        };
        if read != size {
            return Err(invalid());
        }
    }
    document.ok_or_else(|| {
        Error::new(
            "missing_card_envelope",
            "The archive has no root card.json entry.",
        )
    })
}

pub(super) fn asset(bytes: &[u8], uri: &str) -> Result<Vec<u8>> {
    let name = uri.strip_prefix("embeded://").ok_or_else(|| {
        Error::new(
            "unsupported_asset_uri",
            "The asset URI requires the embeded:// scheme.",
        )
    })?;
    path(name)?;
    let mut archive = open(bytes)?;
    let mut file = archive.by_name(name).map_err(Error::context(
        "invalid_asset_reference",
        "The embedded asset does not exist.",
    ))?;
    if file.is_dir() || file.size() > MAX_ENTRY_BYTES {
        return Err(limit());
    }
    let mut result = Vec::new();
    (&mut file)
        .take(MAX_ENTRY_BYTES + 1)
        .read_to_end(&mut result)
        .map_err(Error::context("invalid_charx", "The asset cannot be read."))?;
    if result.len() as u64 != file.size() {
        return Err(invalid());
    }
    Ok(result)
}

pub(super) fn edit(bytes: &[u8], json: &[u8]) -> Result<Vec<u8>> {
    document(bytes)?;
    let mut archive = open(bytes)?;
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer.set_raw_comment(archive.comment().into());
    for index in 0..archive.len() {
        let file = archive.by_index(index).map_err(Error::context(
            "invalid_charx",
            "The archive entry is invalid.",
        ))?;
        if file.name() == "card.json" {
            writer
                .start_file("card.json", file.options())
                .map_err(Error::context(
                    "invalid_charx",
                    "The card entry cannot be written.",
                ))?;
            writer.write_all(json)?;
        } else {
            writer.raw_copy_file(file).map_err(Error::context(
                "invalid_charx",
                "The archive entry cannot be copied.",
            ))?;
        }
    }
    let bytes = writer
        .finish()
        .map_err(Error::context(
            "invalid_charx",
            "The archive cannot be completed.",
        ))?
        .into_inner();
    crate::Card::import(&bytes)?;
    Ok(bytes)
}
