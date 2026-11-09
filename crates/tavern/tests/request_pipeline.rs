use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

use rustytavern::{
    Result,
    card::Card,
    prompt::{CoreProfile, PromptInput, PromptOptions, PromptPlan, PromptProfile},
    provider::ProviderConfig,
    runtime::Runtime,
    session::{GenerationStatus, Persona, Session},
    store::{SqliteStore, StoreBackend, now_ms},
};
use serde_json::{Value, json};
mod common;
use common::{Script, Step};

async fn setup(script: Script) -> (Runtime, Arc<SqliteStore>, String, Arc<Mutex<Vec<Value>>>) {
    let store = Arc::new(SqliteStore::memory().unwrap());
    let card = Card::import(br#"{"spec":"chara_card_v2","spec_version":"2.0","data":{"name":"Lumen","description":"A keeper.","personality":"Calm.","scenario":"A harbor.","first_mes":"Hello.","mes_example":""}}"#).unwrap();
    let persona = Persona::new("Alex".into(), "Visitor.".into()).unwrap();
    store.save_card(&card).unwrap();
    store.save_persona(&persona).unwrap();
    let session = store
        .create_session(
            &Session {
                id: String::new(),
                character_id: card.id,
                persona_id: persona.id,
                worldbook_ids: Vec::new(),
                provider: "mock".into(),
                model: "fixture".into(),
                head: None,
                version: 0,
                accept_reduced: false,
                options: PromptOptions::default(),
                composition: json!({}),
                title: None,
                hidden: Vec::new(),
            },
            None,
        )
        .unwrap();
    let dispatched = script.requests.clone();
    let runtime = Runtime::new(store.clone()).with_provider(script.arc());
    runtime
        .configure_session(&session, ProviderConfig::default())
        .await
        .unwrap();
    (runtime, store, session.id, dispatched)
}

#[tokio::test]
async fn wire_budget_removes_a_complete_turn_and_inspection_equals_dispatch() {
    let (runtime, store, session, dispatched) =
        setup(Script::new(vec![common::text("Answer.")])).await;
    for text in ["First.", "Second."] {
        runtime
            .start(&session, text, None, ProviderConfig::default())
            .await
            .unwrap()
            .wait()
            .await
            .unwrap();
    }
    let history = store.history(&session).unwrap();
    let full = runtime
        .inspect_request(&session, "Third.", ProviderConfig::default())
        .await
        .unwrap();
    let config = ProviderConfig {
        context_limit: full.plan.serialized_input_units.unwrap()
            + full.plan.output_reserve
            + full.plan.protocol_margin
            - 1,
        ..Default::default()
    };
    runtime
        .configure_session(&store.session(&session).unwrap(), config.clone())
        .await
        .unwrap();
    let inspected = runtime
        .inspect_request(&session, "Third.", config.clone())
        .await
        .unwrap();
    assert_eq!(
        inspected.plan.history_ids,
        history[2..]
            .iter()
            .map(|m| m.id.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        inspected
            .plan
            .omissions
            .iter()
            .map(|o| o.id.clone())
            .collect::<Vec<_>>(),
        history[..2]
            .iter()
            .map(|m| format!("history:{}", m.id))
            .collect::<Vec<_>>()
    );
    assert!(
        inspected
            .plan
            .omissions
            .iter()
            .all(|o| o.reason == "serialized_request_budget")
    );
    let generation = runtime
        .start(&session, "Third.", None, config)
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();
    assert_eq!(generation.status, GenerationStatus::Completed);
    assert_eq!(
        generation.request.as_ref().unwrap()["prepared"]["body"],
        inspected.prepared.body
    );
    assert_eq!(
        dispatched.lock().unwrap().last().unwrap(),
        &inspected.prepared.body
    );
    assert_eq!(store.history(&session).unwrap()[..4], history);
}

#[tokio::test]
async fn mandatory_wire_overflow_fails_before_dispatch() {
    let (runtime, store, session, dispatched) = setup(Script::default()).await;
    let full = runtime
        .inspect_request(&session, "Hello.", ProviderConfig::default())
        .await
        .unwrap();
    let config = ProviderConfig {
        context_limit: full.plan.serialized_input_units.unwrap()
            + full.plan.output_reserve
            + full.plan.protocol_margin
            - 1,
        ..Default::default()
    };
    runtime
        .configure_session(&store.session(&session).unwrap(), config.clone())
        .await
        .unwrap();
    assert_eq!(
        runtime
            .inspect_request(&session, "Hello.", config.clone())
            .await
            .unwrap_err()
            .code,
        "mandatory_context_overflow"
    );
    let result = runtime
        .start(&session, "Hello.", None, config)
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();
    assert_eq!(result.status, GenerationStatus::Failed);
    assert_eq!(result.error.unwrap().code, "mandatory_context_overflow");
    assert!(result.request.is_none() && dispatched.lock().unwrap().is_empty());
    assert_eq!(store.messages(&session).unwrap().len(), 1);
}

#[tokio::test]
async fn usage_is_durable_before_its_event_and_a_failure_records_none() {
    for (steps, expected) in [
        (vec![], GenerationStatus::Completed),
        (
            vec![Step::Fail("The provider fails.".into())],
            GenerationStatus::Failed,
        ),
    ] {
        let script = Script {
            steps,
            total_tokens: Some(11),
            ..Default::default()
        };
        let (runtime, store, session, _) = setup(script).await;
        let mut handle = runtime
            .start(&session, "Hello.", None, ProviderConfig::default())
            .await
            .unwrap();
        while let Some(event) = handle.events.recv().await {
            if event.r#type == "usage" {
                let usage = store.generation(&handle.generation_id).unwrap().usage;
                assert_eq!(usage.unwrap()["total_tokens"], 11);
            }
            if event.r#type == "completed" || event.r#type == "failed" {
                break;
            }
        }
        let result = handle.wait().await.unwrap();
        assert_eq!(result.status, expected);
        let usage = result.usage.map(|usage| usage["total_tokens"].clone());
        assert_eq!(
            usage,
            (expected == GenerationStatus::Completed).then(|| json!(11))
        );
    }
}

struct BlockingProfile {
    started: Arc<AtomicBool>,
    release: Mutex<mpsc::Receiver<()>>,
    runs: Arc<AtomicUsize>,
}
impl PromptProfile for BlockingProfile {
    fn id(&self) -> &str {
        "fixture.blocking"
    }
    fn draft(&self, input: &PromptInput<'_>) -> Result<PromptPlan> {
        self.started.store(true, Ordering::SeqCst);
        let _ = self
            .release
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(15));
        self.runs.fetch_add(1, Ordering::SeqCst);
        CoreProfile.draft(input)
    }
}
#[tokio::test]
async fn compilation_allows_cancellation_and_never_dispatches_after_cancellation() {
    let (runtime, store, session, dispatched) = setup(Script::default()).await;
    let (release, receiver) = mpsc::channel();
    let started = Arc::new(AtomicBool::new(false));
    let runs = Arc::new(AtomicUsize::new(0));
    let runtime = runtime.with_profile(Arc::new(BlockingProfile {
        started: started.clone(),
        release: Mutex::new(receiver),
        runs: runs.clone(),
    }));
    runtime
        .configure_session(&store.session(&session).unwrap(), ProviderConfig::default())
        .await
        .unwrap();
    let mut handle = runtime
        .start(&session, "Hello.", None, ProviderConfig::default())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while !started.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    handle.cancel();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), handle.wait())
            .await
            .unwrap()
            .unwrap()
            .status,
        GenerationStatus::Cancelled
    );
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while runs.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(dispatched.lock().unwrap().is_empty());
    assert!(
        store
            .generation(&handle.generation_id)
            .unwrap()
            .request
            .is_none()
    );
}

#[tokio::test]
async fn the_writer_lease_renews_during_blocking_compilation() {
    let (runtime, store, session, _) = setup(Script::default()).await;
    let (release, receiver) = mpsc::channel();
    let started = Arc::new(AtomicBool::new(false));
    let runtime = runtime.with_profile(Arc::new(BlockingProfile {
        started: started.clone(),
        release: Mutex::new(receiver),
        runs: Arc::new(AtomicUsize::new(0)),
    }));
    let probe_time = now_ms() + 31_000;
    runtime
        .configure_session(&store.session(&session).unwrap(), ProviderConfig::default())
        .await
        .unwrap();
    let mut handle = runtime
        .start(&session, "Hello.", None, ProviderConfig::default())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(9)).await;
    assert!(started.load(Ordering::SeqCst));
    assert_eq!(store.recover(&session, probe_time).unwrap(), 0);
    release.send(()).unwrap();
    assert_eq!(
        handle.wait().await.unwrap().status,
        GenerationStatus::Completed
    );
}

#[test]
fn optional_trimming_removes_turns_then_complete_examples_then_lore() {
    use rustytavern::{
        prompt::{ConservativeCounter, Section},
        session::{Message, Role},
    };
    let mut document: Value =
        serde_json::from_slice(include_bytes!("fixtures/synthetic/card-v3.json")).unwrap();
    document["data"]["mes_example"] = json!(
        "<START>\n{{user}}: Example.\n{{char}}: Reply.\n<START>\n{{user}}: Other.\n{{char}}: Answer."
    );
    document["data"]["character_book"] =
        json!({"entries":[{"id":"harbor","keys":[],"constant":true,"content":"A harbor."}]});
    let card = Card::import(&serde_json::to_vec(&document).unwrap()).unwrap();
    let persona = Persona::new("Alex".into(), "Visitor.".into()).unwrap();
    let history = [Role::User, Role::Assistant, Role::User, Role::Assistant]
        .into_iter()
        .enumerate()
        .map(|(i, role)| Message {
            id: format!("node-{i}"),
            parent_id: None,
            role,
            text: format!("Text {i}."),
            generation_id: None,
        })
        .collect::<Vec<_>>();
    let mut plan = CoreProfile
        .compile(&PromptInput {
            card: &card,
            persona: &persona,
            history: &history,
            message: "Next.",
            options: &PromptOptions::default(),
            accept_reduced: false,
            counter: &ConservativeCounter,
        })
        .unwrap();
    let mandatory = plan
        .sections
        .iter()
        .filter(|s| s.mandatory)
        .cloned()
        .collect::<Vec<Section>>();
    for expected in [
        vec!["history:node-0", "history:node-1"],
        vec!["history:node-2", "history:node-3"],
        vec!["example:1:0", "example:1:1"],
        vec!["example:0:0", "example:0:1"],
        vec!["lore:harbor"],
    ] {
        let before = plan.omissions.len();
        plan.trim_optional("fixture_budget").unwrap();
        assert_eq!(
            plan.omissions[before..]
                .iter()
                .map(|o| o.id.as_str())
                .collect::<Vec<_>>(),
            expected
        );
    }
    assert!(plan.history_ids.is_empty() && plan.lore_ids.is_empty());
    assert_eq!(plan.sections, mandatory);
    assert_eq!(
        plan.trim_optional("fixture_budget").unwrap_err().code,
        "mandatory_context_overflow"
    );
}

#[test]
fn the_history_limit_keeps_only_the_latest_messages() {
    use rustytavern::{
        prompt::ConservativeCounter,
        session::{Message, Role},
    };
    let card = Card::import(include_bytes!("fixtures/synthetic/card-v3.json")).unwrap();
    let persona = Persona::new("Alex".into(), "Visitor.".into()).unwrap();
    let history = (0..5)
        .map(|i| Message {
            id: format!("node-{i}"),
            parent_id: None,
            role: if i % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            },
            text: format!("Text {i}."),
            generation_id: None,
        })
        .collect::<Vec<_>>();
    let options = PromptOptions {
        history_limit: 2,
        ..Default::default()
    };
    let plan = CoreProfile
        .compile(&PromptInput {
            card: &card,
            persona: &persona,
            history: &history,
            message: "Next.",
            options: &options,
            accept_reduced: false,
            counter: &ConservativeCounter,
        })
        .unwrap();
    assert_eq!(plan.history_ids, ["node-3", "node-4"]);
    assert!(
        plan.omissions
            .iter()
            .filter(|o| o.reason == "history_limit")
            .map(|o| o.id.as_str())
            .eq(["node-0", "node-1", "node-2"])
    );
}
