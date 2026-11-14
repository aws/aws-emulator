// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateDataset`: the PATCH of one dataset's metadata, the
//! statuses that block it, and the identifier that names nothing.
//!
//! Every test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised. The store is the one shared
//! engine, so what an update writes is what the next `GetDataset` reads.
//!
//! The clock-owed transition out of `CREATING` is waited for by POLLING the dataset
//! until it reports the status the test needs, never by assuming how long the host takes.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol as agentcore;

/// The region the tests sign for, which is what the handlers read the ARN Region from.
const REGION: &str = "us-west-2";

/// How long a poll waits for a status the clock owes before giving up. Generous: it
/// bounds a transition the emulator makes in seconds, so a loaded host still passes.
const POLL_DEADLINE: Duration = Duration::from_secs(120);

/// How long a poll waits between looks. Short enough that the loop is cheap, and never
/// asserted on: the test waits for the CONDITION.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// `DatasetSchemaType` values, from `smithy_query shape=DatasetSchemaType`.
const SCHEMA_TYPES: [&str; 3] = [
    "AGENTCORE_EVALUATION_PREDEFINED_V1",
    "AGENTCORE_EVALUATION_SIMULATED_V1",
    "THIRD_PARTY_EVALUATION_V1",
];

/// `description`'s modelled ceiling on `UpdateDatasetRequest`: `length max 200`
/// (`smithy_query operation=UpdateDataset`).
const MAX_DESCRIPTION_LEN: usize = 200;

/// A `clientToken` inside the modelled `ClientToken` constraints (len 33..256,
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$`, `smithy_query operation=UpdateDataset`).
fn client_token(seed: &str) -> String {
    let token = format!("{seed}{}", "0".repeat(40));
    token[..40].to_string()
}

/// This emulator: the control plane alone, which an inline dataset source needs nothing
/// beside.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        // `default()` registers no authenticator, so the suite exercises routing and the
        // handlers rather than this process's SigV4 keys.
        let mut router = MultiServiceRouter::default();
        router.register_service(agentcore::EmulatedService::new(Arc::clone(&engine)));
        Self { router }
    }

    /// One `restJson1` request to the control plane, signed for [`REGION`].
    async fn call(&self, method: &str, uri: &str, body: Option<&Value>) -> Answer {
        let payload = body.map_or_else(Bytes::new, |value| {
            Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
        });
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261029/{REGION}/bedrock-agentcore/\
             aws4_request, SignedHeaders=host, Signature=abc"
        );
        let mut request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", authorization)
            .header("host", "bedrock-agentcore-control.amazonaws.com");
        if body.is_some() {
            request = request.header("content-type", "application/json");
        }
        let request = request.body(payload).expect("a well-formed request");
        Answer::of(self.router.dispatch(request).await)
    }

    /// `CreateDataset`: `POST /datasets`.
    async fn create_dataset(&self, body: &Value) -> Answer {
        self.call("POST", "/datasets", Some(body)).await
    }

    /// `GetDataset`: `GET /datasets/{datasetId}`.
    async fn get_dataset(&self, dataset_id: &str) -> Answer {
        self.call("GET", &format!("/datasets/{dataset_id}"), None)
            .await
    }

    /// `UpdateDataset`: `PUT /datasets/{datasetId}`
    /// (`smithy_query operation=UpdateDataset`).
    async fn update_dataset(&self, dataset_id: &str, body: &Value) -> Answer {
        self.call("PUT", &format!("/datasets/{dataset_id}"), Some(body))
            .await
    }

    /// `CreateDatasetVersion`: `POST /datasets/{datasetId}/versions`.
    async fn create_dataset_version(&self, dataset_id: &str, body: &Value) -> Answer {
        self.call(
            "POST",
            &format!("/datasets/{dataset_id}/versions"),
            Some(body),
        )
        .await
    }

    /// Create a dataset carrying `description`, and answer its `datasetId`.
    async fn seed_dataset(
        &self,
        case: &str,
        name: &str,
        schema_type: &str,
        description: Option<&str>,
    ) -> String {
        let mut body = json!({
            "datasetName": name,
            "schemaType": schema_type,
            "source": inline_source(2),
        });
        if let Some(description) = description {
            body["description"] = json!(description);
        }
        let created = self.create_dataset(&body).await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: expected HTTP 202 from CreateDataset, got {} and body {}",
            created.status, created.body
        );
        created.text("datasetId")
    }

    /// Look at the dataset until it reports `status`, which the clock owes it, and answer
    /// that read. Waits for the CONDITION: it polls until the status holds or
    /// [`POLL_DEADLINE`] passes.
    async fn poll_until_status(&self, case: &str, dataset_id: &str, status: &str) -> Answer {
        let started = Instant::now();
        loop {
            let read = self.get_dataset(dataset_id).await;
            if read.text("status") == status {
                return read;
            }
            assert!(
                started.elapsed() < POLL_DEADLINE,
                "case {case:?}: dataset {dataset_id} never reached {status}, last read was \
                 status {:?} and body {}",
                read.text("status"),
                read.body
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// One answer from the emulator: its status and its JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    fn of(response: http::Response<Bytes>) -> Self {
        let status = response.status().as_u16();
        let body = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        Self { status, body }
    }

    /// The error code `restJson1` puts in `__type` / `x-amzn-errortype`.
    fn error_code(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or("<no __type>")
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    fn text(&self, member: &str) -> String {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// Whether the body carries `member` at all, which is how an omitted optional member
    /// is told apart from one carrying a value.
    fn has(&self, member: &str) -> bool {
        self.body.get(member).is_some_and(|value| !value.is_null())
    }

    fn seconds(&self, member: &str) -> f64 {
        self.body
            .get(member)
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("{member} must be an epoch-seconds number: {}", self.body))
    }
}

/// A `source` carrying `count` inline examples, each carrying the field every AgentCore
/// evaluation schema requires of one (`crate::state::required_example_fields`).
fn inline_source(count: usize) -> Value {
    let examples: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "scenario_id": format!("scenario-{index}"),
                "input": format!("turn {index}"),
            })
        })
        .collect();
    json!({"inlineExamples": {"examples": examples}})
}

/// One update of a dataset's metadata: what the dataset was created with, what the
/// update carries, and the description the dataset must report afterwards.
struct PatchRow {
    case: &'static str,
    name: &'static str,
    schema_type: &'static str,
    /// The description the create carried, if any.
    created_with: Option<&'static str>,
    /// The description the update carries. `None` is a request that OMITS the member.
    update_with: Option<String>,
    /// What the dataset reports afterwards. `None` is a dataset still carrying none.
    expected: Option<String>,
}

/// `UpdateDataset` writes the members the request carries and leaves the ones it omits,
/// and every later read comes from that record.
///
/// The rows differ along what the rule generalises over: whether the dataset already
/// carried the member, whether the request carries it, and the value it carries — a
/// replacement, an empty one, a one-character one and the modelled maximum length — over
/// several schema types, none of which an update may change.
#[tokio::test]
async fn update_dataset_writes_the_members_the_request_carries_and_leaves_the_rest() {
    let emulator = Emulator::new();

    let rows: [PatchRow; 6] = [
        PatchRow {
            case: "a description set on a dataset that carried none",
            name: "undescribed_set",
            schema_type: SCHEMA_TYPES[0],
            created_with: None,
            update_with: Some("the shared evaluation scenarios".to_string()),
            expected: Some("the shared evaluation scenarios".to_string()),
        },
        PatchRow {
            case: "a description replaced by another",
            name: "described_set",
            schema_type: SCHEMA_TYPES[1],
            created_with: Some("the first wording"),
            update_with: Some("the second wording".to_string()),
            expected: Some("the second wording".to_string()),
        },
        PatchRow {
            case: "an update carrying no member at all",
            name: "untouched_set",
            schema_type: SCHEMA_TYPES[2],
            created_with: Some("the wording the create carried"),
            update_with: None,
            expected: Some("the wording the create carried".to_string()),
        },
        PatchRow {
            case: "an update carrying no member for a dataset that carried none",
            name: "still_undescribed_set",
            schema_type: SCHEMA_TYPES[0],
            created_with: None,
            update_with: None,
            expected: None,
        },
        PatchRow {
            case: "a one-character description",
            name: "terse_set",
            schema_type: SCHEMA_TYPES[1],
            created_with: Some("the first wording"),
            update_with: Some("d".to_string()),
            expected: Some("d".to_string()),
        },
        PatchRow {
            case: "a description at the modelled maximum length",
            name: "verbose_set",
            schema_type: SCHEMA_TYPES[2],
            created_with: None,
            update_with: Some("d".repeat(MAX_DESCRIPTION_LEN)),
            expected: Some("d".repeat(MAX_DESCRIPTION_LEN)),
        },
    ];

    // Every dataset is created first, so the one clock-owed transition out of CREATING is
    // polled for once per dataset rather than serialised row by row.
    let mut seeded: Vec<(&PatchRow, String)> = Vec::new();
    for row in &rows {
        let id = emulator
            .seed_dataset(row.case, row.name, row.schema_type, row.created_with)
            .await;
        seeded.push((row, id));
    }

    for (row, id) in &seeded {
        let case = row.case;
        let before = emulator.poll_until_status(case, id, "ACTIVE").await;

        let mut body = json!({});
        if let Some(description) = row.update_with.clone() {
            body["description"] = json!(description);
        }
        let updated = emulator.update_dataset(id, &body).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateDataset, got {} and body {}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.text("datasetId"),
            *id,
            "case {case:?}: expected the updated datasetId, got {}",
            updated.text("datasetId")
        );
        assert_eq!(
            updated.text("datasetArn"),
            before.text("datasetArn"),
            "case {case:?}: expected the dataset's own ARN, got {}",
            updated.text("datasetArn")
        );
        let updated_at = updated.seconds("updatedAt");
        assert!(
            updated_at >= before.seconds("createdAt"),
            "case {case:?}: expected updatedAt at or after the dataset was created, got \
             {updated_at} against createdAt {}",
            before.seconds("createdAt")
        );

        let read = emulator.get_dataset(id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetDataset, got {} and body {}",
            read.status, read.body
        );
        match row.expected.as_deref() {
            Some(expected) => assert_eq!(
                read.text("description"),
                expected,
                "case {case:?}: expected the stored description, got {}",
                read.text("description")
            ),
            None => assert!(
                !read.has("description"),
                "case {case:?}: expected no description member at all, got {}",
                read.body
            ),
        }
        assert_eq!(
            read.seconds("updatedAt"),
            updated_at,
            "case {case:?}: expected the read to report the updatedAt the update answered, \
             got {}",
            read.seconds("updatedAt")
        );
        // The update is a patch of METADATA: it changes nothing else about the dataset.
        for member in ["datasetName", "schemaType", "status", "draftStatus"] {
            assert_eq!(
                read.text(member),
                before.text(member),
                "case {case:?}: expected {member} to be left as it was ({}), got {}",
                before.text(member),
                read.text(member)
            );
        }
        assert_eq!(
            read.seconds("createdAt"),
            before.seconds("createdAt"),
            "case {case:?}: expected createdAt to be left as it was, got {}",
            read.seconds("createdAt")
        );
        assert_eq!(
            read.body["exampleCount"], before.body["exampleCount"],
            "case {case:?}: expected the example count to be left as it was ({}), got {}",
            before.body["exampleCount"], read.body["exampleCount"]
        );
        assert_eq!(
            read.text("datasetVersion"),
            "DRAFT",
            "case {case:?}: expected the read to still resolve to the DRAFT, got {}",
            read.text("datasetVersion")
        );
    }
}

/// A dataset whose own ingestion or last mutation is still in flight blocks every write,
/// so an update of it is the modelled conflict and persists nothing.
///
/// Both statuses that block one are driven: `CREATING`, straight after the create, and
/// `UPDATING`, straight after a version is published from the dataset.
#[tokio::test]
async fn update_dataset_is_refused_while_the_dataset_is_busy() {
    let emulator = Emulator::new();

    let id = emulator
        .seed_dataset("a busy dataset", "busy_set", SCHEMA_TYPES[0], Some("as created"))
        .await;

    // Straight after the create the dataset is CREATING, in which "All writes are
    // blocked" (`smithy_query shape=DatasetStatus`).
    let while_creating = emulator
        .update_dataset(&id, &json!({"description": "written while creating"}))
        .await;
    assert_eq!(
        while_creating.status, 409,
        "case {:?}: expected HTTP 409 from UpdateDataset, got {} and body {}",
        "CREATING", while_creating.status, while_creating.body
    );
    assert_eq!(
        while_creating.error_code(),
        "ConflictException",
        "case {:?}: expected the modelled ConflictException, got {} and body {}",
        "CREATING",
        while_creating.error_code(),
        while_creating.body
    );
    assert!(
        while_creating.text("message").contains("CREATING"),
        "case {:?}: expected the refusal to name the status it found, got {:?}",
        "CREATING",
        while_creating.text("message")
    );

    let active = emulator.poll_until_status("CREATING", &id, "ACTIVE").await;
    assert_eq!(
        active.text("description"),
        "as created",
        "case {:?}: a refused update persists nothing, got {}",
        "CREATING",
        active.text("description")
    );

    // Publishing a version puts the dataset in UPDATING, in which "All writes are
    // blocked" too.
    let published = emulator
        .create_dataset_version(&id, &json!({"clientToken": client_token("publish")}))
        .await;
    assert_eq!(
        published.status, 202,
        "case {:?}: expected HTTP 202 from CreateDatasetVersion, got {} and body {}",
        "UPDATING", published.status, published.body
    );
    let while_updating = emulator
        .update_dataset(&id, &json!({"description": "written while updating"}))
        .await;
    assert_eq!(
        while_updating.status, 409,
        "case {:?}: expected HTTP 409 from UpdateDataset, got {} and body {}",
        "UPDATING", while_updating.status, while_updating.body
    );
    assert_eq!(
        while_updating.error_code(),
        "ConflictException",
        "case {:?}: expected the modelled ConflictException, got {} and body {}",
        "UPDATING",
        while_updating.error_code(),
        while_updating.body
    );
    assert!(
        while_updating.text("message").contains("UPDATING"),
        "case {:?}: expected the refusal to name the status it found, got {:?}",
        "UPDATING",
        while_updating.text("message")
    );

    // Once the dataset is stable again the same update is served.
    emulator.poll_until_status("UPDATING", &id, "ACTIVE").await;
    let served = emulator
        .update_dataset(&id, &json!({"description": "written once stable"}))
        .await;
    assert_eq!(
        served.status, 200,
        "case {:?}: expected HTTP 200 from UpdateDataset once the dataset is ACTIVE, got {} \
         and body {}",
        "ACTIVE", served.status, served.body
    );
    let read = emulator.get_dataset(&id).await;
    assert_eq!(
        read.text("description"),
        "written once stable",
        "case {:?}: expected the served update to be the one that persisted, got {}",
        "ACTIVE",
        read.text("description")
    );
}

/// What `UpdateDataset` refuses besides a busy dataset: an id naming no stored dataset,
/// and a description past the member's modelled ceiling. Both leave the store as it was.
#[tokio::test]
async fn update_dataset_refuses_an_unknown_id_and_a_description_past_the_bound() {
    let emulator = Emulator::new();

    let missing = emulator
        .update_dataset("no_such_dataset", &json!({"description": "nowhere"}))
        .await;
    assert_eq!(
        missing.status, 404,
        "case {:?}: expected HTTP 404 from UpdateDataset, got {} and body {}",
        "an unknown datasetId", missing.status, missing.body
    );
    assert_eq!(
        missing.error_code(),
        "ResourceNotFoundException",
        "case {:?}: expected the modelled ResourceNotFoundException, got {} and body {}",
        "an unknown datasetId",
        missing.error_code(),
        missing.body
    );

    let id = emulator
        .seed_dataset(
            "a description past the bound",
            "bounded_set",
            SCHEMA_TYPES[0],
            Some("as created"),
        )
        .await;
    emulator
        .poll_until_status("a description past the bound", &id, "ACTIVE")
        .await;

    // `length max 200`: the bound itself is served (the table above), one past it is the
    // modelled refusal.
    let too_long = emulator
        .update_dataset(
            &id,
            &json!({"description": "d".repeat(MAX_DESCRIPTION_LEN + 1)}),
        )
        .await;
    assert_eq!(
        too_long.status, 400,
        "case {:?}: expected HTTP 400 from UpdateDataset, got {} and body {}",
        "one character past the bound", too_long.status, too_long.body
    );
    assert_eq!(
        too_long.error_code(),
        "ValidationException",
        "case {:?}: expected the modelled ValidationException, got {} and body {}",
        "one character past the bound",
        too_long.error_code(),
        too_long.body
    );
    let read = emulator.get_dataset(&id).await;
    assert_eq!(
        read.text("description"),
        "as created",
        "case {:?}: a refused update persists nothing, got {}",
        "one character past the bound",
        read.text("description")
    );
}

/// A `clientToken` replayed against `UpdateDataset` applies ONE update: the second
/// request is ignored and answers what the first one wrote.
#[tokio::test]
async fn a_replayed_client_token_applies_one_update() {
    let emulator = Emulator::new();

    let case = "a replayed clientToken";
    let id = emulator
        .seed_dataset(case, "replayed_set", SCHEMA_TYPES[1], None)
        .await;
    emulator.poll_until_status(case, &id, "ACTIVE").await;

    let token = client_token("update");
    let first = emulator
        .update_dataset(
            &id,
            &json!({"clientToken": token, "description": "the first wording"}),
        )
        .await;
    assert_eq!(
        first.status, 200,
        "case {case:?}: expected HTTP 200 from UpdateDataset, got {} and body {}",
        first.status, first.body
    );

    let replayed = emulator
        .update_dataset(
            &id,
            &json!({"clientToken": token, "description": "the second wording"}),
        )
        .await;
    assert_eq!(
        replayed.status, 200,
        "case {case:?}: a replayed token is not an error, got {} and body {}",
        replayed.status, replayed.body
    );
    assert_eq!(
        replayed.seconds("updatedAt"),
        first.seconds("updatedAt"),
        "case {case:?}: expected the replay to answer the updatedAt the first update wrote, \
         got {}",
        replayed.seconds("updatedAt")
    );

    let read = emulator.get_dataset(&id).await;
    assert_eq!(
        read.text("description"),
        "the first wording",
        "case {case:?}: expected the replayed request to be ignored, got {}",
        read.text("description")
    );
}
