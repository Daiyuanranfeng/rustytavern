use rustytavern_card::{
    BookEntry, Card, CardVersion, CharacterBook, CharacterCard, CharacterData, OptionalField,
    WorldBook,
};
use rustytavern_core::Result;
use serde_json::json;

fn main() -> Result<()> {
    let book = CharacterBook {
        entries: vec![BookEntry {
            keys: vec!["harbor".into()],
            content: "The harbor has a stone tower.".into(),
            enabled: true.into(),
            insertion_order: OptionalField::Value(10.into()),
            use_regex: false.into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let definition = CharacterCard::new(
        CardVersion::V3,
        CharacterData {
            name: "Aster".into(),
            character_book: book.clone().into(),
            ..Default::default()
        },
    );
    let card = Card::import(&serde_json::to_vec(&definition)?)?;
    let worldbook = WorldBook::import(&serde_json::to_vec(&book)?)?;
    println!(
        "{}",
        json!({"character":card.definition()?.data.name,"worldbook_entries":worldbook.definition()?.entry_count()})
    );
    Ok(())
}
