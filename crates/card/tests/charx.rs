use rustytavern_card::{Card, MAX_INPUT_BYTES, MAX_JSON_BYTES, charx::MAX_ARCHIVE_ENTRIES};
use serde_json::json;
use std::io::{Cursor, Read, Write};
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

fn archive(entries: &[(&str, &[u8])], method: CompressionMethod) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer.set_comment("The archive retains this comment, including PK\u{5}\u{6}.");
    for (name, bytes) in entries {
        let options = SimpleFileOptions::default().compression_method(method);
        writer.start_file(*name, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn card_json() -> &'static [u8] {
    include_bytes!("fixtures/full-v3.json")
}

fn central_entries(bytes: &[u8]) -> Vec<usize> {
    bytes
        .windows(4)
        .enumerate()
        .filter_map(|(index, part)| (part == b"PK\x01\x02").then_some(index))
        .collect()
}

#[test]
fn charx_retains_card_assets_and_unknown_files_after_an_edit() {
    for method in [CompressionMethod::Stored, CompressionMethod::Deflated] {
        let bytes = archive(
            &[
                ("card.json", card_json()),
                ("assets/图像.png", b"original image"),
                ("app.json", br#"{"future":null}"#),
            ],
            method,
        );
        let card = Card::import(&bytes).unwrap();
        assert_eq!(card.format, "charx");
        assert_eq!(card.selected_envelope.as_deref(), Some("card.json"));
        assert_eq!(card.export_original(), bytes);
        assert_eq!(
            card.embedded_asset("embeded://assets/图像.png").unwrap(),
            b"original image"
        );
        assert_eq!(
            card.embedded_asset("embeded://assets/图像.PNG")
                .unwrap_err()
                .code,
            "invalid_asset_reference"
        );
        assert_eq!(
            card.embedded_asset("https://example.com/image.png")
                .unwrap_err()
                .code,
            "unsupported_asset_uri"
        );
        assert_eq!(
            card.embedded_asset("embedded://assets/图像.png")
                .unwrap_err()
                .code,
            "unsupported_asset_uri"
        );
        let mut document = card.document.clone();
        document["data"]["name"] = json!("Beacon");
        let edited = card.export_edited(&document).unwrap();
        let changed = Card::import(&edited).unwrap();
        assert_eq!(changed.name(), "Beacon");
        assert_eq!(changed.document, document);
        assert_ne!(card.id, changed.id);
        assert_eq!(changed.definition().unwrap().data.name, "Beacon");
        assert_eq!(
            changed.embedded_asset("embeded://app.json").unwrap(),
            br#"{"future":null}"#
        );
        let mut original = ZipArchive::new(Cursor::new(&bytes)).unwrap();
        let mut result = ZipArchive::new(Cursor::new(&edited)).unwrap();
        assert_eq!(original.comment(), result.comment());
        for name in ["assets/图像.png", "app.json"] {
            let mut before = Vec::new();
            let mut after = Vec::new();
            original
                .by_name(name)
                .unwrap()
                .read_to_end(&mut before)
                .unwrap();
            result
                .by_name(name)
                .unwrap()
                .read_to_end(&mut after)
                .unwrap();
            assert_eq!(before, after);
            assert_eq!(
                original.by_name(name).unwrap().compression(),
                result.by_name(name).unwrap().compression()
            );
        }
        let restored: Card = serde_json::from_slice(&serde_json::to_vec(&card).unwrap()).unwrap();
        restored.validate().unwrap();
        assert_eq!(restored.export_original(), bytes);
    }
}

#[test]
fn charx_requires_one_valid_root_card() {
    for name in ["Card.json", "nested/card.json"] {
        let bytes = archive(&[(name, card_json())], CompressionMethod::Stored);
        assert_eq!(
            Card::import(&bytes).unwrap_err().code,
            "missing_card_envelope"
        );
    }
    let bytes = archive(&[("card.json", b"{")], CompressionMethod::Stored);
    assert_eq!(Card::import(&bytes).unwrap_err().code, "invalid_card_json");
    let bytes = archive(
        &[("card.json", br#"{"spec":"future"}"#)],
        CompressionMethod::Stored,
    );
    assert_eq!(
        Card::import(&bytes).unwrap_err().code,
        "unsupported_card_format"
    );
}

#[test]
fn charx_rejects_unsafe_and_duplicate_paths() {
    for name in [
        "../file",
        "/file",
        "./file",
        "a/../file",
        "a//file",
        "C:/file",
        "a\\file",
        "a\0file",
        "a\nfile",
    ] {
        let bytes = archive(
            &[("card.json", card_json()), (name, b"data")],
            CompressionMethod::Stored,
        );
        assert_eq!(
            Card::import(&bytes).unwrap_err().code,
            "invalid_charx_path",
            "{name:?}"
        );
    }
    let mut bytes = archive(
        &[("card.json", card_json()), ("a.txt", b"a"), ("b.txt", b"b")],
        CompressionMethod::Stored,
    );
    for pos in (0..bytes.len() - 4).rev() {
        if &bytes[pos..pos + 5] == b"b.txt" {
            bytes[pos] = b'a';
        }
    }
    assert_eq!(Card::import(&bytes).unwrap_err().code, "invalid_charx_path");
}

#[test]
fn charx_checks_crc_and_rejects_encryption_and_special_files() {
    let original = archive(
        &[
            ("card.json", card_json()),
            ("asset.txt", b"unique asset content"),
        ],
        CompressionMethod::Stored,
    );
    let mut damaged = original.clone();
    let pos = damaged
        .windows(20)
        .position(|value| value == b"unique asset content")
        .unwrap();
    damaged[pos] ^= 1;
    assert_eq!(Card::import(&damaged).unwrap_err().code, "invalid_charx");
    for (offset, value, code) in [
        (8, vec![1, 0], "unsupported_charx_encoding"),
        (10, vec![99, 0], "unsupported_charx_encoding"),
        (
            38,
            (0o120777u32 << 16).to_le_bytes().to_vec(),
            "unsupported_charx_entry",
        ),
    ] {
        let mut bytes = original.clone();
        let pos = central_entries(&bytes)[1] + offset;
        bytes[pos..pos + value.len()].copy_from_slice(&value);
        assert_eq!(Card::import(&bytes).unwrap_err().code, code);
    }
    assert!(Card::import(&original[..original.len() - 12]).is_err());
}

#[test]
fn charx_limits_metadata_and_declared_and_actual_expansion() {
    let names: Vec<_> = (0..=MAX_ARCHIVE_ENTRIES)
        .map(|index| format!("{index}.bin"))
        .collect();
    let entries: Vec<_> = names.iter().map(|name| (name.as_str(), &b""[..])).collect();
    let bytes = archive(&entries, CompressionMethod::Stored);
    assert_eq!(Card::import(&bytes).unwrap_err().code, "invalid_card_size");
    let original = archive(
        &[
            ("card.json", card_json()),
            ("a", b"a"),
            ("b", b"b"),
            ("c", b"c"),
        ],
        CompressionMethod::Stored,
    );
    for (index, size) in [(0, MAX_JSON_BYTES + 1), (1, MAX_INPUT_BYTES + 1)] {
        let mut bytes = original.clone();
        let pos = central_entries(&bytes)[index] + 24;
        bytes[pos..pos + 4].copy_from_slice(&(size as u32).to_le_bytes());
        assert_eq!(Card::import(&bytes).unwrap_err().code, "invalid_card_size");
    }
    let mut bytes = original.clone();
    for index in central_entries(&bytes).into_iter().skip(1) {
        bytes[index + 24..index + 28].copy_from_slice(&(MAX_INPUT_BYTES as u32).to_le_bytes());
    }
    assert_eq!(Card::import(&bytes).unwrap_err().code, "invalid_card_size");
    let bytes = archive(
        &[
            ("card.json", card_json()),
            ("asset", &vec![0; MAX_INPUT_BYTES + 1]),
        ],
        CompressionMethod::Deflated,
    );
    assert_eq!(Card::import(&bytes).unwrap_err().code, "invalid_card_size");
    let mut bytes = original;
    let pos = central_entries(&bytes)[1] + 24;
    bytes[pos..pos + 4].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(Card::import(&bytes).unwrap_err().code, "invalid_charx");
}

#[test]
fn charx_checks_edited_json_limits_and_asset_paths() {
    let card = Card::import(&archive(
        &[("card.json", card_json())],
        CompressionMethod::Deflated,
    ))
    .unwrap();
    assert_eq!(
        card.embedded_asset("embeded://../file").unwrap_err().code,
        "invalid_charx_path"
    );
    let mut document = card.document.clone();
    document["data"]["extensions"] = json!({"large": vec!["x".repeat(1024 * 1024); 9]});
    assert_eq!(
        card.export_edited(&document).unwrap_err().code,
        "invalid_card_size"
    );
    let json = Card::import(card_json()).unwrap();
    assert_eq!(
        json.embedded_asset("embeded://card.json").unwrap_err().code,
        "unsupported_asset_container"
    );
}
