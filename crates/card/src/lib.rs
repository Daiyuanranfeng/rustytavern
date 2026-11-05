pub mod charx;
mod field;
pub use field::OptionalField;
pub mod character;
pub mod worldbook;
pub use character::{
    CardDocument, CardVersion, CharacterAsset, CharacterCard, CharacterData, LegacyCard,
};
pub use worldbook::{BookEntry, CharacterBook, WorldBook, WorldBookDocument};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use rustytavern_core::{Error, Result, digest};

pub const MAX_INPUT_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_JSON_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_STRING_BYTES: usize = 1024 * 1024;
pub const MAX_CHUNKS: usize = 4096;
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Warning,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: String,
    pub field: String,
    pub severity: Severity,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Card {
    pub id: String,
    pub source_hash: String,
    pub format: String,
    pub selected_envelope: Option<String>,
    pub conflicting_envelopes: bool,
    pub document: Value,
    pub diagnostics: Vec<Diagnostic>,
    #[serde(with = "raw_bytes")]
    pub original: Vec<u8>,
    #[serde(default)]
    pub provenance: Option<rustytavern_core::Provenance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decoder: Option<DecoderRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecoderRecord {
    pub id: String,
    pub normalized_hash: String,
}

pub struct DecodedCard {
    pub document: Value,
    pub format: String,
    pub selected_envelope: Option<String>,
    pub conflicting_envelopes: bool,
}

pub trait CardDecoder: Send + Sync {
    fn id(&self) -> &str;
    fn decode(&self, bytes: &[u8]) -> Result<DecodedCard>;
}

pub struct CcDecoder;
impl CardDecoder for CcDecoder {
    fn id(&self) -> &str {
        "cc-v2-v3/1"
    }
    fn decode(&self, bytes: &[u8]) -> Result<DecodedCard> {
        let card = Card::import(bytes)?;
        Ok(DecodedCard {
            document: card.document,
            format: card.format,
            selected_envelope: card.selected_envelope,
            conflicting_envelopes: card.conflicting_envelopes,
        })
    }
}

impl Card {
    pub fn import(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_INPUT_BYTES {
            return Err(Error::new(
                "invalid_card_size",
                "The card exceeds the input limit.",
            ));
        }
        let (format, selected, conflict, json) = if bytes.starts_with(PNG) {
            let chunks = png_chunks(bytes)?;
            let mut v3 = None;
            let mut v2 = None;
            for (kind, data) in chunks {
                if kind != b"tEXt" {
                    continue;
                }
                let Some(split) = data.iter().position(|v| *v == 0) else {
                    continue;
                };
                let key = &data[..split];
                if key == b"ccv3" && v3.is_none() {
                    v3 = Some(&data[split + 1..]);
                }
                if key == b"chara" && v2.is_none() {
                    v2 = Some(&data[split + 1..]);
                }
            }
            let conflicting = v3.is_some() && v2.is_some();
            let (key, payload) = if let Some(data) = v3 {
                ("ccv3", data)
            } else if let Some(data) = v2 {
                ("chara", data)
            } else {
                return Err(Error::new(
                    "missing_card_envelope",
                    "The PNG has no exact chara or ccv3 keyword.",
                ));
            };
            if payload.len() > (MAX_JSON_BYTES.div_ceil(3) * 4) {
                return Err(Error::new(
                    "invalid_card_size",
                    "The decoded card exceeds the JSON limit.",
                ));
            }
            let decoded = STANDARD.decode(payload).map_err(|_| {
                Error::new(
                    "invalid_card_base64",
                    "The card metadata is not valid base64.",
                )
            })?;
            ("png", Some(key.to_owned()), conflicting, decoded)
        } else if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
            (
                "charx",
                Some("card.json".into()),
                false,
                charx::document(bytes)?,
            )
        } else {
            ("json", None, false, bytes.to_vec())
        };
        if json.len() > MAX_JSON_BYTES {
            return Err(Error::new(
                "invalid_card_size",
                "The card exceeds the JSON limit.",
            ));
        }
        let document: Value = serde_json::from_slice(&json)
            .map_err(Error::context("invalid_card_json", "Card JSON is invalid."))?;
        validate_document(&document)?;
        let source_hash = digest(bytes);
        let diagnostics = compatibility(&document);
        Ok(Self {
            id: format!("card-{}", &source_hash[..24]),
            source_hash,
            format: format.to_owned(),
            selected_envelope: selected,
            conflicting_envelopes: conflict,
            document,
            diagnostics,
            original: bytes.to_vec(),
            provenance: None,
            decoder: None,
        })
    }

    pub fn import_with(bytes: &[u8], decoder: &dyn CardDecoder) -> Result<Self> {
        if bytes.len() > MAX_INPUT_BYTES {
            return Err(Error::new(
                "invalid_card_size",
                "The card exceeds the input limit.",
            ));
        }
        let decoded = decoder.decode(bytes)?;
        validate_document(&decoded.document)?;
        if serde_json::to_vec(&decoded.document)?.len() > MAX_JSON_BYTES {
            return Err(Error::new(
                "invalid_card_size",
                "The normalized card exceeds the JSON limit.",
            ));
        }
        if decoder.id().is_empty() || decoded.format.is_empty() {
            return Err(Error::new(
                "invalid_card_decoder",
                "The decoder identity and format must not be empty.",
            ));
        }
        let source_hash = digest(bytes);
        let diagnostics = compatibility(&decoded.document);
        let normalized_hash = digest(&serde_json::to_vec(&(
            &decoded.format,
            &decoded.selected_envelope,
            decoded.conflicting_envelopes,
            &decoded.document,
        ))?);
        let legacy = decoder.id() == CcDecoder.id();
        let identity = if legacy {
            source_hash.clone()
        } else {
            digest(&serde_json::to_vec(&(
                &source_hash,
                &normalized_hash,
                decoder.id(),
            ))?)
        };
        let card = Self {
            id: format!("card-{}", &identity[..24]),
            source_hash,
            format: decoded.format,
            selected_envelope: decoded.selected_envelope,
            conflicting_envelopes: decoded.conflicting_envelopes,
            document: decoded.document,
            diagnostics,
            original: bytes.to_vec(),
            provenance: None,
            decoder: (!legacy).then(|| DecoderRecord {
                id: decoder.id().into(),
                normalized_hash,
            }),
        };
        card.validate()?;
        Ok(card)
    }

    pub fn validate(&self) -> Result<()> {
        if self
            .provenance
            .as_ref()
            .is_some_and(|p| p.content_hash != self.source_hash)
        {
            return Err(Error::new(
                "invalid_source_hash",
                "The provenance hash does not match the card bytes.",
            ));
        }
        if self.original.len() > MAX_INPUT_BYTES || self.source_hash != digest(&self.original) {
            return Err(Error::new(
                "invalid_card_identity",
                "The stored card bytes do not match their identity.",
            ));
        }
        validate_document(&self.document)?;
        if serde_json::to_vec(&self.document)?.len() > MAX_JSON_BYTES
            || self.diagnostics != compatibility(&self.document)
        {
            return Err(Error::new(
                "invalid_card_identity",
                "The stored card document or diagnostics are invalid.",
            ));
        }
        let identity = if let Some(decoder) = &self.decoder {
            let hash = digest(&serde_json::to_vec(&(
                &self.format,
                &self.selected_envelope,
                self.conflicting_envelopes,
                &self.document,
            ))?);
            if decoder.id.is_empty() || self.format.is_empty() || decoder.normalized_hash != hash {
                return Err(Error::new(
                    "invalid_card_identity",
                    "The decoder record does not match the normalized card.",
                ));
            }
            digest(&serde_json::to_vec(&(
                &self.source_hash,
                &hash,
                &decoder.id,
            ))?)
        } else {
            let original = Self::import(&self.original)?;
            if original.document != self.document
                || original.format != self.format
                || original.selected_envelope != self.selected_envelope
                || original.conflicting_envelopes != self.conflicting_envelopes
            {
                return Err(Error::new(
                    "invalid_card_identity",
                    "The stored card differs from its original envelope.",
                ));
            }
            self.source_hash.clone()
        };
        if self.id != format!("card-{}", &identity[..24]) {
            return Err(Error::new(
                "invalid_card_identity",
                "The stored card revision ID is invalid.",
            ));
        }
        Ok(())
    }

    pub fn with_provenance(mut self, provenance: &rustytavern_core::Provenance) -> Result<Self> {
        if provenance.content_hash != self.source_hash {
            return Err(Error::new(
                "invalid_source_hash",
                "The artifact hash does not match its bytes.",
            ));
        }
        self.provenance = Some(provenance.clone());
        Ok(self)
    }

    pub fn import_artifact(artifact: &rustytavern_core::Artifact) -> Result<Self> {
        Self::import(&artifact.bytes)?.with_provenance(&artifact.provenance)
    }

    pub fn definition(&self) -> Result<CharacterCard> {
        serde_json::from_value(self.document.clone()).map_err(Error::context(
            "invalid_card_field",
            "The typed character card fields are invalid.",
        ))
    }

    pub fn data(&self) -> &Value {
        &self.document["data"]
    }
    pub fn name(&self) -> &str {
        self.text("name")
    }
    pub fn text(&self, field: &str) -> &str {
        self.data()[field].as_str().unwrap_or_default()
    }

    pub fn ensure_compatible(&self, accepted: bool) -> Result<()> {
        if !accepted
            && self
                .diagnostics
                .iter()
                .any(|d| d.severity == Severity::Unsupported)
        {
            return Err(Error::new(
                "unsupported_behavior",
                "The card requires unsupported behavior. Inspect its diagnostics or accept the reduced profile explicitly.",
            ));
        }
        Ok(())
    }

    pub fn export_original(&self) -> &[u8] {
        &self.original
    }

    pub fn embedded_asset(&self, uri: &str) -> Result<Vec<u8>> {
        self.validate()?;
        if self.format != "charx" {
            return Err(Error::new(
                "unsupported_asset_container",
                "The card has no CHARX archive.",
            ));
        }
        charx::asset(&self.original, uri)
    }

    pub fn export_edited(&self, document: &Value) -> Result<Vec<u8>> {
        validate_document(document)?;
        let json = serde_json::to_vec(document)?;
        if json.len() > MAX_JSON_BYTES {
            return Err(Error::new(
                "invalid_card_size",
                "The exported card exceeds the JSON limit.",
            ));
        }
        if self.format == "charx" {
            return charx::edit(&self.original, &json);
        }
        if self.format != "png" {
            return Ok(json);
        }
        let chunks = png_chunks(&self.original)?;
        let mut out = PNG.to_vec();
        for (kind, data) in chunks {
            if kind == b"tEXt" {
                let key = data.split(|v| *v == 0).next().unwrap_or_default();
                if key.eq_ignore_ascii_case(b"ccv3") || key.eq_ignore_ascii_case(b"chara") {
                    continue;
                }
            }
            if kind == b"IEND" {
                let key = if document["spec"] == "chara_card_v3" {
                    "ccv3"
                } else {
                    "chara"
                };
                let mut metadata = key.as_bytes().to_vec();
                metadata.push(0);
                metadata.extend_from_slice(STANDARD.encode(&json).as_bytes());
                write_chunk(&mut out, b"tEXt", &metadata);
            }
            write_chunk(&mut out, kind, data);
        }
        if out.len() > MAX_INPUT_BYTES {
            return Err(Error::new(
                "invalid_card_size",
                "The exported card exceeds the input limit.",
            ));
        }
        Ok(out)
    }
}

fn validate_document(document: &Value) -> Result<()> {
    let expected = match document["spec"].as_str() {
        Some("chara_card_v2") => "2.0",
        Some("chara_card_v3") => "3.0",
        _ => {
            return Err(Error::new(
                "unsupported_card_format",
                "The importer requires a CCv2 or CCv3 envelope.",
            ));
        }
    };
    if document["spec_version"] != expected {
        return Err(Error::new(
            "unsupported_card_version",
            "The card version does not match its envelope.",
        ));
    }
    let data = &document["data"];
    if !data.is_object() {
        return Err(Error::new(
            "invalid_card_field",
            "The data field must be an object.",
        ));
    }
    for field in [
        "name",
        "description",
        "personality",
        "scenario",
        "first_mes",
        "mes_example",
    ] {
        if !data[field].is_string() {
            return Err(Error::new(
                "invalid_card_field",
                format!("The data.{field} field must be a string."),
            ));
        }
    }
    if data["name"]
        .as_str()
        .is_none_or(|name| name.trim().is_empty())
    {
        return Err(Error::new(
            "invalid_card_field",
            "The character name must not be empty.",
        ));
    }
    for field in [
        "system_prompt",
        "post_history_instructions",
        "creator_notes",
        "creator",
        "character_version",
    ] {
        if let Some(value) = data.get(field) {
            if !value.is_string() {
                return Err(Error::new(
                    "invalid_card_field",
                    format!("The data.{field} field must be a string."),
                ));
            }
        }
    }
    for field in ["alternate_greetings", "tags"] {
        if let Some(value) = data.get(field) {
            if !value
                .as_array()
                .is_some_and(|values| values.iter().all(Value::is_string))
            {
                return Err(Error::new(
                    "invalid_card_field",
                    format!("The data.{field} field must contain strings."),
                ));
            }
        }
    }
    if let Some(book) = data.get("character_book").filter(|v| !v.is_null()) {
        if !book.is_object() {
            return Err(Error::new(
                "invalid_card_field",
                "The character_book field must be an object.",
            ));
        }
        if let Some(entries) = book.get("entries") {
            let Some(entries) = entries.as_array() else {
                return Err(Error::new(
                    "invalid_card_field",
                    "The lore entries must form an array.",
                ));
            };
            if entries.len() > worldbook::MAX_BOOK_ENTRIES {
                return Err(Error::new(
                    "invalid_card_size",
                    "The lorebook contains too many entries.",
                ));
            }
            for entry in entries {
                if !entry.is_object() || !entry["content"].is_string() {
                    return Err(Error::new(
                        "invalid_card_field",
                        "Each lore entry requires text content.",
                    ));
                }
                for key in ["keys", "secondary_keys"] {
                    if let Some(value) = entry.get(key) {
                        if !value
                            .as_array()
                            .is_some_and(|a| a.iter().all(Value::is_string))
                        {
                            return Err(Error::new(
                                "invalid_card_field",
                                "Lore keys must contain strings.",
                            ));
                        }
                    }
                }
            }
        }
    }
    validate_bounds(document, 0)
}

fn validate_bounds(value: &Value, depth: usize) -> Result<()> {
    if depth > 64 {
        return Err(Error::new(
            "invalid_card_depth",
            "The card exceeds the JSON depth limit.",
        ));
    }
    match value {
        Value::String(s) if s.len() > MAX_STRING_BYTES => Err(Error::new(
            "invalid_card_size",
            "A card string exceeds the size limit.",
        )),
        Value::Array(items) => {
            for item in items {
                validate_bounds(item, depth + 1)?;
            }
            Ok(())
        }
        Value::Object(items) => {
            for (key, item) in items {
                if key.len() > MAX_STRING_BYTES {
                    return Err(Error::new(
                        "invalid_card_size",
                        "A JSON key exceeds the size limit.",
                    ));
                }
                validate_bounds(item, depth + 1)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

pub fn compatibility(document: &Value) -> Vec<Diagnostic> {
    let data = &document["data"];
    let mut diagnostics = Vec::new();
    let mut add = |code: &str, field: String, severity: Severity| {
        diagnostics.push(Diagnostic {
            code: code.to_owned(),
            field,
            severity,
        })
    };
    for field in [
        "description",
        "personality",
        "scenario",
        "first_mes",
        "mes_example",
        "system_prompt",
        "post_history_instructions",
    ] {
        scan_macros(
            data[field].as_str().unwrap_or_default(),
            &format!("data.{field}"),
            matches!(field, "system_prompt" | "post_history_instructions"),
            &mut add,
        );
    }
    if let Some(greetings) = data["alternate_greetings"].as_array() {
        for (i, greeting) in greetings.iter().enumerate() {
            scan_macros(
                greeting.as_str().unwrap_or_default(),
                &format!("data.alternate_greetings.{i}"),
                false,
                &mut add,
            );
        }
    }
    if let Some(extensions) = data["extensions"].as_object() {
        for (name, value) in extensions {
            let script =
                name.to_lowercase().contains("script") || name.to_lowercase().contains("trigger");
            if !value.is_null()
                && value != &serde_json::json!([])
                && value != &serde_json::json!({})
            {
                add(
                    if script {
                        "unsupported_script"
                    } else {
                        "unverified_extension"
                    },
                    format!("data.extensions.{name}"),
                    if script {
                        Severity::Unsupported
                    } else {
                        Severity::Warning
                    },
                );
            }
        }
    }
    let book = &data["character_book"];
    if book["recursive_scanning"].as_bool() == Some(true) {
        add(
            "unsupported_lore_rule",
            "data.character_book.recursive_scanning".into(),
            Severity::Unsupported,
        );
    }
    if let Some(entries) = book["entries"].as_array() {
        for (i, entry) in entries.iter().enumerate() {
            let base = format!("data.character_book.entries.{i}");
            scan_macros(
                entry["content"].as_str().unwrap_or_default(),
                &format!("{base}.content"),
                false,
                &mut add,
            );
            for key in [
                "use_regex",
                "recursive",
                "probability",
                "selectiveLogic",
                "selective_logic",
                "match_whole_words",
                "scan_depth",
                "depth",
                "delay",
                "cooldown",
                "sticky",
            ] {
                if let Some(value) = entry.get(key).or_else(|| entry["extensions"].get(key)) {
                    if !value.is_null() && value != &Value::Bool(false) {
                        add(
                            "unsupported_lore_rule",
                            format!("{base}.{key}"),
                            Severity::Unsupported,
                        );
                    }
                }
            }
            if entry["position"]
                .as_str()
                .is_some_and(|p| !matches!(p, "before_char" | "after_char"))
            {
                add(
                    "unsupported_lore_position",
                    format!("{base}.position"),
                    Severity::Unsupported,
                );
            }
        }
    }
    diagnostics
}

fn scan_macros(
    text: &str,
    field: &str,
    original: bool,
    add: &mut impl FnMut(&str, String, Severity),
) {
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let tail = &rest[start + 2..];
        let Some(end) = tail.find("}}") else {
            add("unsupported_macro", field.into(), Severity::Unsupported);
            break;
        };
        let key = &tail[..end];
        if !(matches!(key, "char" | "user") || original && key == "original") {
            add("unsupported_macro", field.into(), Severity::Unsupported);
        }
        rest = &tail[end + 2..];
    }
}

fn png_chunks(bytes: &[u8]) -> Result<Vec<(&[u8], &[u8])>> {
    let invalid = || Error::new("invalid_png", "The PNG chunk structure is invalid.");
    if !bytes.starts_with(PNG) {
        return Err(invalid());
    }
    let mut pos = 8usize;
    let mut chunks = Vec::new();
    let mut ended = false;
    while pos < bytes.len() {
        if chunks.len() == MAX_CHUNKS || bytes.len() - pos < 12 {
            return Err(invalid());
        }
        let length =
            u32::from_be_bytes(bytes[pos..pos + 4].try_into().map_err(|_| invalid())?) as usize;
        let end = pos
            .checked_add(length)
            .and_then(|n| n.checked_add(12))
            .filter(|end| *end <= bytes.len())
            .ok_or_else(invalid)?;
        let kind = &bytes[pos + 4..pos + 8];
        let data = &bytes[pos + 8..end - 4];
        if !kind.iter().all(u8::is_ascii_alphabetic) {
            return Err(invalid());
        }
        let actual_crc = u32::from_be_bytes(bytes[end - 4..end].try_into().map_err(|_| invalid())?);
        if crc32fast::hash(&bytes[pos + 4..end - 4]) != actual_crc {
            return Err(Error::new(
                "invalid_png_crc",
                "A PNG chunk has an invalid CRC.",
            ));
        }
        if chunks.is_empty() && (kind != b"IHDR" || length != 13) {
            return Err(invalid());
        }
        if !chunks.is_empty() && kind == b"IHDR" {
            return Err(invalid());
        }
        chunks.push((kind, data));
        pos = end;
        if kind == b"IEND" {
            if length != 0 || pos != bytes.len() {
                return Err(invalid());
            }
            ended = true;
            break;
        }
    }
    if !ended || !chunks.iter().any(|(kind, _)| *kind == b"IDAT") {
        return Err(invalid());
    }
    Ok(chunks)
}

fn write_chunk(output: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
    output.extend_from_slice(&(data.len() as u32).to_be_bytes());
    output.extend_from_slice(kind);
    output.extend_from_slice(data);
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(kind);
    hasher.update(data);
    output.extend_from_slice(&hasher.finalize().to_be_bytes());
}

mod raw_bytes {
    use super::{Engine, MAX_INPUT_BYTES, STANDARD};
    use serde::{
        Deserializer, Serializer,
        de::{Error, SeqAccess, Visitor},
    };

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        struct Bytes;
        impl<'de> Visitor<'de> for Bytes {
            type Value = Vec<u8>;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("base64 card bytes or a legacy byte array")
            }
            fn visit_str<E: Error>(self, value: &str) -> Result<Vec<u8>, E> {
                if value.len() > MAX_INPUT_BYTES.div_ceil(3) * 4 {
                    return Err(E::custom("The encoded card bytes exceed the byte limit."));
                }
                let bytes = STANDARD
                    .decode(value)
                    .map_err(|_| E::custom("The encoded card bytes are invalid."))?;
                if bytes.len() > MAX_INPUT_BYTES {
                    return Err(E::custom("The card bytes exceed the byte limit."));
                }
                Ok(bytes)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Vec<u8>, A::Error> {
                let mut bytes = Vec::new();
                while let Some(byte) = sequence.next_element::<u8>()? {
                    if bytes.len() == MAX_INPUT_BYTES {
                        return Err(A::Error::custom("The card bytes exceed the byte limit."));
                    }
                    bytes.push(byte);
                }
                Ok(bytes)
            }
        }
        deserializer.deserialize_any(Bytes)
    }
}
