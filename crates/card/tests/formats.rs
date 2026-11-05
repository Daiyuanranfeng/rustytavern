use rustytavern_card::{
    Card, CardDocument, CardVersion, CharacterCard, CharacterData, OptionalField, WorldBook,
    WorldBookDocument,
    worldbook::{EntryId, RecursionDelay},
};
use serde_json::{Value, json};

#[test]
fn typed_character_formats_retain_every_field() {
    for bytes in [
        include_bytes!("fixtures/full-v2.json").as_slice(),
        include_bytes!("fixtures/full-v3.json").as_slice(),
        include_bytes!("fixtures/legacy.json").as_slice(),
    ] {
        let original: Value = serde_json::from_slice(bytes).unwrap();
        let card = CardDocument::parse_json(bytes).unwrap();
        assert_eq!(serde_json::to_value(&card).unwrap(), original);
        match card {
            CardDocument::Versioned(card) => {
                assert_eq!(card.data.name, "Aster");
                assert!(card.data.extra.contains_key("future_data"));
                let book = card.data.character_book.as_ref().unwrap();
                assert_eq!(
                    book.entries[0].id.as_ref(),
                    Some(&EntryId::Text("harbor-1".into()))
                );
                assert_eq!(
                    book.entries[0].extra["future_entry"]["keep"],
                    json!([false, null, 7])
                );
                if card.version == CardVersion::V3 {
                    assert_eq!(
                        card.data.assets.as_ref().unwrap()[0].uri,
                        "embeded://assets/icon/images/main.png"
                    );
                    assert_eq!(
                        card.data.creator_notes_multilingual.as_ref().unwrap()["ja"],
                        "テスト。"
                    );
                }
            }
            CardDocument::Legacy(card) => {
                assert_eq!(card.extra["future_legacy"]["keep"], Value::Null)
            }
        }
    }
}

#[test]
fn archive_edits_use_the_typed_card_and_preserve_unknown_values() {
    let bytes = include_bytes!("fixtures/full-v3.json");
    let archive = Card::import(bytes).unwrap();
    let mut definition = archive.definition().unwrap();
    definition.data.name = "Beacon".into();
    let edited = archive
        .export_edited(&serde_json::to_value(definition).unwrap())
        .unwrap();
    let imported = Card::import(&edited).unwrap();
    assert_eq!(imported.name(), "Beacon");
    assert_eq!(
        imported.data()["future_data"],
        archive.data()["future_data"]
    );
    assert_eq!(
        imported.data()["character_book"],
        archive.data()["character_book"]
    );
    assert_eq!(archive.export_original(), bytes);
}

#[test]
fn worldbook_formats_preserve_original_bytes_and_structured_fields() {
    for bytes in [
        include_bytes!("fixtures/character-book.json").as_slice(),
        include_bytes!("fixtures/worldbook-v3.json").as_slice(),
        include_bytes!("fixtures/world-info.json").as_slice(),
    ] {
        let archive = WorldBook::import(bytes).unwrap();
        assert_eq!(archive.export_original(), bytes);
        let definition = archive.definition().unwrap();
        assert_eq!(definition.entry_count(), 1);
        assert_eq!(
            serde_json::to_value(&definition).unwrap(),
            archive.document().clone()
        );
        let edited = archive.export_edited(&definition).unwrap();
        assert_eq!(
            WorldBook::import(&edited).unwrap().document(),
            archive.document()
        );
    }
}

#[test]
fn world_info_fields_keep_missing_null_and_value_distinct() {
    let archive = WorldBook::import(include_bytes!("fixtures/world-info.json")).unwrap();
    let WorldBookDocument::SillyTavern(mut book) = archive.definition().unwrap() else {
        panic!("The fixture requires World Info.")
    };
    let entry = book.entries.get_mut("12").unwrap();
    assert!(matches!(entry.scan_depth, OptionalField::Null));
    assert_eq!(entry.uid.as_ref(), Some(&EntryId::Number(12.into())));
    assert_eq!(
        entry.delay_until_recursion.as_ref(),
        Some(&RecursionDelay::Number(0.into()))
    );
    assert_eq!(
        entry.extra.keys().map(String::as_str).collect::<Vec<_>>(),
        ["futureSetting"]
    );
    entry.scan_depth = OptionalField::Value(8.into());
    entry.delay_until_recursion = RecursionDelay::Boolean(true).into();
    entry.display_index = OptionalField::Missing;
    let bytes = archive
        .export_edited(&WorldBookDocument::SillyTavern(book))
        .unwrap();
    let result: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(result["entries"]["12"]["scanDepth"], 8);
    assert_eq!(result["entries"]["12"]["delayUntilRecursion"], true);
    assert!(result["entries"]["12"].get("displayIndex").is_none());
    assert_eq!(result["entries"]["12"]["caseSensitive"], Value::Null);
    assert_eq!(
        result["entries"]["12"]["futureSetting"],
        json!({"retain":null})
    );
}

#[test]
fn format_dispatch_rejects_wrong_kinds_and_invalid_known_field_types() {
    for value in [
        json!({"name":"Book","entries":{}}),
        json!({"spec":"future","name":"Aster","description":"Text"}),
    ] {
        assert!(CardDocument::parse_json(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    assert!(WorldBook::import(include_bytes!("fixtures/full-v3.json")).is_err());
    assert!(WorldBook::import(br#"{"entries":{"0":{"key":"wrong"}}}"#).is_err());
    let mut value: Value = serde_json::from_slice(include_bytes!("fixtures/full-v3.json")).unwrap();
    value["data"]["assets"][0]["uri"] = json!(7);
    assert!(CardDocument::parse_json(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[test]
fn both_format_readers_apply_byte_string_and_depth_limits() {
    let large = vec![b' '; rustytavern_card::MAX_JSON_BYTES + 1];
    assert_eq!(
        CardDocument::parse_json(&large).unwrap_err().code,
        "invalid_card_size"
    );
    assert_eq!(
        WorldBook::import(&large).unwrap_err().code,
        "invalid_card_size"
    );
    let value = json!({"entries":[],"future":"x".repeat(rustytavern_card::MAX_STRING_BYTES+1)});
    assert_eq!(
        WorldBook::import(&serde_json::to_vec(&value).unwrap())
            .unwrap_err()
            .code,
        "invalid_card_size"
    );
    let mut value = json!(null);
    for _ in 0..65 {
        value = json!([value]);
    }
    assert_eq!(
        WorldBook::import(&serde_json::to_vec(&json!({"entries":[],"future":value})).unwrap())
            .unwrap_err()
            .code,
        "invalid_card_depth"
    );
    let entries: serde_json::Map<String, Value> = (0
        ..=rustytavern_card::worldbook::MAX_BOOK_ENTRIES)
        .map(|index| (index.to_string(), json!({})))
        .collect();
    assert_eq!(
        WorldBook::import(&serde_json::to_vec(&json!({"entries":entries})).unwrap())
            .unwrap_err()
            .code,
        "invalid_worldbook_size"
    );
}

#[test]
fn new_cards_select_the_version_header_and_group_greetings() {
    for version in [CardVersion::V2, CardVersion::V3] {
        let card = CharacterCard::new(
            version,
            CharacterData {
                name: "Aster".into(),
                ..Default::default()
            },
        );
        let value = serde_json::to_value(card).unwrap();
        assert_eq!(
            value["spec_version"],
            if version == CardVersion::V2 {
                "2.0"
            } else {
                "3.0"
            }
        );
        assert_eq!(
            value["data"].get("group_only_greetings").is_some(),
            version == CardVersion::V3
        );
        assert!(Card::import(&serde_json::to_vec(&value).unwrap()).is_ok());
    }
}
