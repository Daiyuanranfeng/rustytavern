use rustytavern::{
    card::Card,
    prompt::{self, ConservativeCounter, PromptInput, PromptOptions},
    session::{GenerationStatus, Persona, Role, Session},
    store::{SqliteStore, StoreBackend, Submit},
};
use serde_json::{Value, json};

fn card() -> Card {
    Card::import(include_bytes!("fixtures/synthetic/card-v3.json")).unwrap()
}
fn persona() -> Persona {
    Persona::new("Alex".into(), "Alex charts the coast.".into()).unwrap()
}
fn session(store: &dyn StoreBackend) -> Session {
    let card = card();
    let persona = persona();
    store.save_card(&card).unwrap();
    store.save_persona(&persona).unwrap();
    store
        .create_session(
            &Session {
                id: String::new(),
                character_id: card.id,
                persona_id: persona.id,
                worldbook_ids: Vec::new(),
                provider: "test".into(),
                model: "mock".into(),
                head: None,
                version: 0,
                accept_reduced: false,
                options: PromptOptions::default(),
                composition: json!({}),
                title: None,
                hidden: Vec::new(),
            },
            Some("Hello, {{user}}."),
        )
        .unwrap()
}
fn submit<'a>(session: &'a Session, key: Option<&'a str>, now: i64) -> Submit<'a> {
    Submit {
        session_id: &session.id,
        expected_version: session.version,
        text: "Hello.",
        idempotency_key: key,
        retry_user: None,
        mode: Default::default(),
        snapshot: json!({"persona":"Alex"}),
        now,
        lease_ms: 100,
    }
}

#[test]
fn card_envelopes_preserve_unknown_values_and_original_bytes() {
    let raw = include_bytes!("fixtures/synthetic/dual.png");
    let card = Card::import(raw).unwrap();
    assert_eq!(card.name(), "Lumen");
    assert_eq!(card.selected_envelope.as_deref(), Some("ccv3"));
    assert!(card.conflicting_envelopes);
    assert_eq!(card.export_original(), raw);
    let mut document = card.document.clone();
    document["data"]["name"] = json!("Beacon");
    let edited = Card::import(&card.export_edited(&document).unwrap()).unwrap();
    assert_eq!(edited.name(), "Beacon");
    assert_eq!(edited.data()["extensions"], card.data()["extensions"]);
    assert!(!edited.conflicting_envelopes);
}

#[test]
fn invalid_cards_fail_before_a_revision_exists() {
    for (bytes, code) in [
        (
            &include_bytes!("fixtures/synthetic/broken-preferred.png")[..],
            "invalid_card_json",
        ),
        (
            &include_bytes!("fixtures/synthetic/bad-crc.png")[..],
            "invalid_png_crc",
        ),
        (
            &include_bytes!("fixtures/synthetic/wrong-type.json")[..],
            "invalid_card_field",
        ),
        (
            &include_bytes!("fixtures/synthetic/no-card.png")[..],
            "missing_card_envelope",
        ),
        (
            &include_bytes!("fixtures/synthetic/legacy.json")[..],
            "unsupported_card_format",
        ),
    ] {
        assert_eq!(Card::import(bytes).unwrap_err().code, code);
    }
}

#[test]
fn macros_use_one_pass_and_preserve_transcript_literals() {
    assert_eq!(
        prompt::expand("{{char}} meets {{user}}.", "Lumen", "Alex", None),
        "Lumen meets Alex."
    );
    assert_eq!(
        prompt::expand("{{char}} meets {{user}}.", "Lumen", "{{char}}", None),
        "Lumen meets {{char}}."
    );
    assert_eq!(
        prompt::instruction("Base.", "Before {{original}} After.", "Lumen", "Alex"),
        "Before Base. After."
    );
    let card = card();
    let persona = persona();
    let options = PromptOptions::default();
    let plan = prompt::compile(&PromptInput {
        card: &card,
        persona: &persona,
        history: &[],
        message: "{{char}} stays literal.",
        options: &options,
        accept_reduced: false,
        counter: &ConservativeCounter,
    })
    .unwrap();
    assert_eq!(
        plan.sections.last().unwrap().text,
        "{{char}} stays literal."
    );
    assert!(
        plan.sections
            .iter()
            .any(|s| s.id == "persona" && s.text.contains("Alex"))
    );
}

#[test]
fn custom_blocks_note_depth_and_prefill_take_their_places() {
    let card = card();
    let persona = persona();
    let message = |id: &str, role: Role| rustytavern::session::Message {
        id: id.into(),
        parent_id: None,
        role,
        text: format!("{id}."),
        generation_id: None,
    };
    let history = [message("a", Role::User), message("b", Role::Assistant)];
    let block = |name: &str, position: &str, depth: usize| {
        serde_json::from_value::<prompt::PromptBlock>(
            json!({"name":name,"text":format!("{name} {{{{char}}}}"),"position":position,"depth":depth}),
        )
        .unwrap()
    };
    let mut off = block("off", "before_history", 0);
    off.enabled = false;
    let options = PromptOptions {
        post_history_instruction: "Post.".into(),
        post_history_role: Role::User,
        note: "Note.".into(),
        note_depth: Some(1),
        prefill: "Sure,".into(),
        blocks: vec![
            block("first", "before_character", 0),
            block("after", "after_character", 0),
            block("before", "before_history", 0),
            block("end", "after_history", 0),
            block("deep", "depth", 0),
            block("top", "depth", 99),
            off,
        ],
        ..Default::default()
    };
    let plan = prompt::compile(&PromptInput {
        card: &card,
        persona: &persona,
        history: &history,
        message: "Now.",
        options: &options,
        accept_reduced: false,
        counter: &ConservativeCounter,
    })
    .unwrap();
    let ids = plan
        .sections
        .iter()
        .map(|s| s.id.as_str())
        .collect::<Vec<_>>();
    let tail = &ids[ids.iter().position(|id| *id == "block:2").unwrap()..];
    assert_eq!(
        tail,
        [
            "block:2",
            "block:5",
            "history:a",
            "history:b",
            "note",
            "current_user",
            "block:4",
            "block:3",
            "post_history",
            "prefill"
        ]
    );
    assert_eq!(ids[1], "block:0");
    assert!(
        ids.iter().position(|id| *id == "persona") < ids.iter().position(|id| *id == "block:1")
    );
    assert!(!ids.contains(&"block:6"));
    let text = |id: &str| plan.sections.iter().find(|s| s.id == id).unwrap();
    assert_eq!(text("block:0").text, "first Lumen");
    assert_eq!(text("post_history").role, Role::User);
    assert_eq!(text("prefill").role, Role::Assistant);
}

#[test]
fn default_prompt_options_keep_the_saved_configuration_hash() {
    let value = serde_json::to_value(PromptOptions::default()).unwrap();
    for key in [
        "post_history_role",
        "note_depth",
        "note_role",
        "prefill",
        "blocks",
    ] {
        assert!(value.get(key).is_none(), "{key}");
    }
}

#[test]
fn sillytavern_presets_set_instructions_blocks_and_prefill() {
    let preset = json!({
        "assistant_prefill": "Sure,",
        "prompts": [
            {"identifier":"main","content":"Main."},
            {"identifier":"nsfw","name":"Aux","content":"Aux."},
            {"identifier":"charDescription","marker":true},
            {"identifier":"custom","name":"Style","role":"user","content":"Style."},
            {"identifier":"deep","name":"Deep","content":"Deep.","injection_position":1,"injection_depth":2},
            {"identifier":"chatHistory","marker":true},
            {"identifier":"jailbreak","role":"user","content":"Post."},
            {"identifier":"unused","content":"Unused."}
        ],
        "prompt_order": [
            {"character_id":100000,"order":[]},
            {"character_id":100001,"order":[
                {"identifier":"main","enabled":true},
                {"identifier":"nsfw","enabled":false},
                {"identifier":"charDescription","enabled":true},
                {"identifier":"custom","enabled":true},
                {"identifier":"deep","enabled":true},
                {"identifier":"chatHistory","enabled":true},
                {"identifier":"jailbreak","enabled":true}
            ]}
        ]
    });
    let options = prompt::import_sillytavern(&preset, PromptOptions::default()).unwrap();
    assert_eq!(options.main_instruction, "Main.");
    assert_eq!(options.post_history_instruction, "Post.");
    assert_eq!(options.post_history_role, Role::User);
    assert_eq!(options.prefill, "Sure,");
    let blocks = serde_json::to_value(&options.blocks).unwrap();
    assert_eq!(
        blocks,
        json!([
            {"name":"Aux","text":"Aux.","position":"before_character","enabled":false},
            {"name":"Style","text":"Style.","role":"user","position":"after_character"},
            {"name":"Deep","text":"Deep.","position":"depth","depth":2}
        ])
    );
    assert!(prompt::import_sillytavern(&json!({}), PromptOptions::default()).is_err());
}

#[test]
fn lore_matches_keys_in_fixed_order_without_removing_spaces() {
    let book = json!({"entries":[
        {"id":"later","keys":["beacon"],"content":"Later.","insertion_order":2},
        {"id":"first","keys":["beacon"],"content":"First.","insertion_order":1},
        {"id":"missing","keys":["icecream"],"content":"Missing."},
        {"id":"selective","keys":["beacon"],"secondary_keys":["storm"],"selective":true,"content":"Storm."}
    ]});
    let found = prompt::matching_lore(&book, &["Beacon and ice cream.".into()], "Lumen", "Alex");
    assert_eq!(
        found.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
        ["first", "later"]
    );
    let no_scan = json!({"scan_depth":0,"entries":[{"id":"constant","constant":true,"content":"Always."},{"id":"key","keys":["beacon"],"content":"Key."}]});
    assert_eq!(
        prompt::matching_lore(&no_scan, &["beacon".into()], "Lumen", "Alex").len(),
        1
    );
}

#[test]
fn mandatory_overflow_fails_without_removing_the_user_message() {
    let card = card();
    let persona = persona();
    let options = PromptOptions {
        context_limit: 10,
        output_limit: 4,
        protocol_margin: 1,
        ..Default::default()
    };
    let error = prompt::compile(&PromptInput {
        card: &card,
        persona: &persona,
        history: &[],
        message: "Hello.",
        options: &options,
        accept_reduced: false,
        counter: &ConservativeCounter,
    })
    .unwrap_err();
    assert_eq!(error.code, "mandatory_context_overflow");
}

#[test]
fn example_dialogue_keeps_roles_and_blocks() {
    let blocks = prompt::example_blocks("<START>\n{{user}}: Hello.\n{{char}}: Welcome, {{user}}.\nAnother line.\n<START>\nLumen: Again.", "Lumen", "Alex").unwrap();
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0][0], (Role::User, "Hello.".into()));
    assert_eq!(
        blocks[0][1],
        (Role::Assistant, "Welcome, Alex.\nAnother line.".into())
    );
    assert!(prompt::example_blocks("A line without a speaker.", "Lumen", "Alex").is_err());
}

#[test]
fn durable_submission_is_idempotent_and_drafts_do_not_enter_history() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("session.sqlite3");
    let store = SqliteStore::open(&path).unwrap();
    let session = session(&store);
    let mut invalid = submit(&session, None, 100);
    invalid.snapshot = json!(7);
    assert!(store.prepare(invalid).is_err());
    assert_eq!(store.messages(&session.id).unwrap().len(), 1);
    let prep = store.prepare(submit(&session, Some("once"), 100)).unwrap();
    let again = store.prepare(submit(&session, Some("once"), 101)).unwrap();
    assert!(!again.created);
    assert_eq!(prep.generation.id, again.generation.id);
    assert_eq!(store.history(&session.id).unwrap().len(), 2);
    store
        .save_request(
            &prep.generation.id,
            prep.generation.fencing_token,
            &json!({"messages":[]}),
            101,
        )
        .unwrap();
    store
        .append_draft(
            &prep.generation.id,
            prep.generation.fencing_token,
            "Par",
            102,
        )
        .unwrap();
    store
        .settle(
            &prep.generation.id,
            prep.generation.fencing_token,
            GenerationStatus::Cancelled,
            None,
            None,
            103,
        )
        .unwrap();
    drop(store);
    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.generation(&prep.generation.id).unwrap().draft, "Par");
    assert!(
        !store
            .history(&session.id)
            .unwrap()
            .iter()
            .any(|m| m.text == "Par")
    );
    assert_eq!(
        store
            .messages(&session.id)
            .unwrap()
            .iter()
            .filter(|m| m.role == Role::User)
            .count(),
        1
    );
}

#[test]
fn writer_recovery_fences_old_writes_and_keeps_live_work() {
    let store = SqliteStore::memory().unwrap();
    let session = session(&store);
    let prep = store.prepare(submit(&session, None, 100)).unwrap();
    assert_eq!(store.recover(&session.id, 101).unwrap(), 0);
    assert_eq!(
        store.generation(&prep.generation.id).unwrap().status,
        GenerationStatus::Prepared
    );
    assert_eq!(store.recover(&session.id, 201).unwrap(), 1);
    assert_eq!(
        store.generation(&prep.generation.id).unwrap().status,
        GenerationStatus::Interrupted
    );
    assert_eq!(
        store
            .append_draft(
                &prep.generation.id,
                prep.generation.fencing_token,
                "Late.",
                202
            )
            .unwrap_err()
            .code,
        "lease_lost"
    );
    let session = store.session(&session.id).unwrap();
    let next = store.prepare(submit(&session, None, 203)).unwrap();
    assert_eq!(next.generation.fencing_token, 2);
}

#[test]
fn regeneration_preserves_alternatives_and_edits_preserve_the_old_branch() {
    let store = SqliteStore::memory().unwrap();
    let session = session(&store);
    let first = store
        .prepare(submit(&session, None, 100))
        .unwrap()
        .generation;
    store
        .save_request(&first.id, first.fencing_token, &json!({}), 101)
        .unwrap();
    store
        .append_draft(&first.id, first.fencing_token, "First.", 102)
        .unwrap();
    store
        .settle(
            &first.id,
            first.fencing_token,
            GenerationStatus::Completed,
            None,
            None,
            103,
        )
        .unwrap();
    let session = store.session(&session.id).unwrap();
    let mut request = submit(&session, None, 104);
    request.retry_user = Some(&first.user_node);
    let second = store.prepare(request).unwrap().generation;
    store
        .save_request(&second.id, second.fencing_token, &json!({}), 105)
        .unwrap();
    store
        .append_draft(&second.id, second.fencing_token, "Second.", 106)
        .unwrap();
    store
        .settle(
            &second.id,
            second.fencing_token,
            GenerationStatus::Completed,
            None,
            None,
            107,
        )
        .unwrap();
    let messages = store.messages(&session.id).unwrap();
    assert_eq!(messages.iter().filter(|m| m.role == Role::User).count(), 1);
    let alternatives = messages
        .iter()
        .filter(|m| m.parent_id.as_ref() == Some(&first.user_node))
        .collect::<Vec<_>>();
    assert_eq!(alternatives.len(), 2);
    let edited = store
        .edit(&session.id, &first.user_node, "Another question.")
        .unwrap();
    assert_ne!(edited.id, first.user_node);
    assert!(
        store
            .messages(&session.id)
            .unwrap()
            .iter()
            .any(|m| m.text == "First.")
    );
    let export: Value =
        serde_json::to_value(store.export_session(&session.id, true).unwrap()).unwrap();
    assert_eq!(export["schema_version"], 4);
}
