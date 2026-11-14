// SPDX-License-Identifier: CC0-1.0

//! The import-job listing (`ListImportJobs`), end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization are
//! all exercised, and reaches the engine's own state to seed the stored jobs a listing
//! reports and to move the instants the clock-owed transitions of a run fall due at.
//!
//! The rules these cases pin:
//!
//! * the listing is computed from the stored job records: every stored job is reported
//!   once, as the summary its record projects (its id, the destination the create
//!   supplied, the status of its run, when it was created and the records it really
//!   imported), most recently created first;
//! * `ImportDestinationType` narrows the listing to the jobs whose destination selects
//!   the value the request names, and a modelled value no stored job carries matches
//!   nothing;
//! * `PageSize` truncates a page and `NextToken` resumes the walk strictly after the
//!   position the previous page ended at, so a walk reports every matching job exactly
//!   once and terminates;
//! * a run's `CREATED` -> `PROCESSING` -> `COMPLETED` transitions are owed by the clock,
//!   not by being listed: a listing reports whichever of them were already due, and
//!   never moves a run on — or changes anything else about the job — because somebody
//!   looked;
//! * a token this listing never handed out is refused with the one input error the
//!   operation models, and a filter value outside its modelled enum is refused at the
//!   request boundary;
//! * a job a create minted is in the listing from the moment the create answers.
//!
//! The tables below span what the rules generalise over: both `ImportDestinationType`
//! destinations with every import action each models, both `DataFormat` values, the form
//! and length of a job id, every status a run reaches, the number of stored jobs, the
//! page size, and every refusal leg.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::ops::types::{
    ContactListDestination, ImportDataSource, ImportDestination, SuppressionListDestination,
    IMPORT_DESTINATION_TYPE_VALUES,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    encode_tagged_page_token, ComAmazonawsSesv2SimpleemailserviceV2State, ImportJobRecord,
    IMPORT_JOB_COMPLETION_SETTLE, IMPORT_JOB_PROCESSING_SETTLE, TENANTS_PAGE_TOKEN_TAG,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region the cases execute in: the one their `Authorization` credential scope
/// names. An import job belongs to the account, so the listing is the same from any
/// Region; one is enough.
const HOME: &str = "us-east-1";

/// Where this listing is addressed.
const LIST_PATH: &str = "/v2/email/import-jobs/list";

/// Long enough that a transition held this far out cannot fall due while a case runs.
const HELD: Duration = Duration::from_secs(3600);

/// Just enough to put a due instant in the past.
const ELAPSED: Duration = Duration::from_secs(1);

/// The instant the first seeded job was created at, as whole epoch seconds so the
/// timestamp a response carries compares exactly.
const FIRST_CREATED_AT_SECS: u64 = 1_761_580_651;

/// How far apart the seeded jobs were created, so creation order is unambiguous.
const CREATED_APART: Duration = Duration::from_secs(60);

/// `ImportDestinationType$CONTACT_LIST` — the destination type a contact-list
/// destination selects.
const CONTACT_LIST: &str = "CONTACT_LIST";

/// `ImportDestinationType$SUPPRESSION_LIST` — the destination type a suppression-list
/// destination selects.
const SUPPRESSION_LIST: &str = "SUPPRESSION_LIST";

/// One answered request: the status, the error shape when there was one, and the parsed
/// body.
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// The modelled error code the response names, or an empty string on success.
    ///
    /// restJson1 resolves the error shape from `x-amzn-errortype`; the JSON body's
    /// `__type` carries the same name, so either is read.
    fn error_code(&self) -> String {
        let raw = self
            .error_type
            .as_deref()
            .or_else(|| self.body.get("__type").and_then(Value::as_str))
            .unwrap_or_default();
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }

    /// The message the error carried, for an assertion that wants to show it.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The summaries the listing reported, in the order it reported them.
    fn jobs(&self) -> Vec<Value> {
        self.body
            .get("ImportJobs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The ids the listing reported, in the order it reported them.
    fn ids(&self) -> Vec<String> {
        self.jobs()
            .iter()
            .map(|job| {
                job.get("JobId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    /// The token the listing handed back, if any.
    fn next_token(&self) -> Option<String> {
        self.body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// A string member of the body, or `""` when absent.
    fn text(&self, member: &str) -> String {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// A member of one reported summary, as the document carries it: `None` when the member
/// is absent altogether.
fn member(job: &Value, name: &str) -> Option<Value> {
    job.get(name).cloned()
}

/// How far a seeded job's run has got.
///
/// The create stores a due instant per transition rather than sleeping, so a case drives
/// a run by moving those instants instead of waiting on the wall clock — which keeps
/// every assertion independent of how loaded the machine is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Run {
    /// The run has not started: both transitions are still ahead.
    Created,
    /// The run started and has not finished.
    Processing,
    /// The run finished.
    Completed,
}

impl Run {
    /// The modelled `JobStatus` a job in this state reports.
    fn status(self) -> &'static str {
        match self {
            Self::Created => "CREATED",
            Self::Processing => "PROCESSING",
            Self::Completed => "COMPLETED",
        }
    }
}

/// One seeded import job.
struct JobRow {
    case: &'static str,
    job_id: &'static str,
    /// The contact list the destination names, for a contact-list destination; `None`
    /// for a suppression-list destination.
    contact_list: Option<&'static str>,
    /// The import action the destination carries — `ContactListImportAction` for a
    /// contact-list destination, `SuppressionListImportAction` for the other.
    action: &'static str,
    /// `ImportDataSource$S3Url`.
    s3_url: &'static str,
    /// `ImportDataSource$DataFormat`.
    data_format: &'static str,
    run: Run,
}

impl JobRow {
    /// The `ImportDestinationType` this row's destination selects.
    fn destination_type(&self) -> &'static str {
        if self.contact_list.is_some() {
            CONTACT_LIST
        } else {
            SUPPRESSION_LIST
        }
    }

    /// The destination the stored job carries, as the create that minted it supplied it.
    fn destination(&self) -> ImportDestination {
        match self.contact_list {
            Some(name) => ImportDestination {
                suppression_list_destination: None,
                contact_list_destination: Some(ContactListDestination {
                    contact_list_name: name.to_string(),
                    contact_list_import_action: self.action.to_string(),
                }),
            },
            None => ImportDestination {
                suppression_list_destination: Some(SuppressionListDestination {
                    suppression_list_import_action: self.action.to_string(),
                }),
                contact_list_destination: None,
            },
        }
    }

    /// That same destination as the wire carries it in a reported summary.
    fn destination_document(&self) -> Value {
        match self.contact_list {
            Some(name) => json!({
                "ContactListDestination": {
                    "ContactListName": name,
                    "ContactListImportAction": self.action,
                },
            }),
            None => json!({
                "SuppressionListDestination": { "SuppressionListImportAction": self.action },
            }),
        }
    }

    /// The data source the stored job carries.
    fn data_source(&self) -> ImportDataSource {
        ImportDataSource {
            s3_url: self.s3_url.to_string(),
            data_format: self.data_format.to_string(),
        }
    }
}

/// The population the listing cases seed, oldest first.
///
/// The rows span what the rules generalise over: both members of `ImportDestination` and
/// so both values of `ImportDestinationType`
/// (`smithy_query(query="shape",shape="ImportDestinationType")`), every value of
/// `SuppressionListImportAction` and of `ContactListImportAction`, both `DataFormat`
/// values, every status a run of this emulator reaches, and `JobId` forms of different
/// shapes and lengths — the model bounds a job id only at one character or more.
const JOB_ROWS: &[JobRow] = &[
    JobRow {
        case: "a dashed id, suppression list, PUT, CSV, run not started",
        job_id: "7d1a0f8c-0f4a-4f1e-9d2f-2a1b3c4d5e6f",
        contact_list: None,
        action: "PUT",
        s3_url: "s3://ses-imports/suppression.csv",
        data_format: "CSV",
        run: Run::Created,
    },
    JobRow {
        case: "a one-character id, contact list, PUT, CSV, run not started",
        job_id: "a",
        contact_list: Some("newsletter"),
        action: "PUT",
        s3_url: "s3://ses-imports/contacts/",
        data_format: "CSV",
        run: Run::Created,
    },
    JobRow {
        case: "a hexadecimal id, suppression list, DELETE, JSON, run in progress",
        job_id: "0f3b2a1c9d8e7f60",
        contact_list: None,
        action: "DELETE",
        s3_url: "s3://ses-imports/2026/10/29/suppression-removals.json",
        data_format: "JSON",
        run: Run::Processing,
    },
    JobRow {
        case: "an id of digits, contact list, DELETE, JSON, run in progress",
        job_id: "20261027161731",
        contact_list: Some("product-announcements"),
        action: "DELETE",
        s3_url: "s3://ses-imports/contacts/unsubscribed.json",
        data_format: "JSON",
        run: Run::Processing,
    },
    JobRow {
        case: "a mixed-case id with separators, suppression list, PUT, JSON, run finished",
        job_id: "Import_Job-42",
        contact_list: None,
        action: "PUT",
        s3_url: "s3://ses-imports/bounces.json",
        data_format: "JSON",
        run: Run::Completed,
    },
    JobRow {
        case: "a long id, contact list, PUT, CSV, run finished",
        job_id: "contacts-0123456789-0123456789-0123456789-0123456789-0123456789",
        contact_list: Some("newsletter"),
        action: "PUT",
        s3_url: "s3://ses-imports/contacts/2026/all.csv",
        data_format: "CSV",
        run: Run::Completed,
    },
];

/// The emulated service behind its router, plus the shared engine so a case can seed the
/// stored jobs and move the instants their transitions fall due at.
struct Ses {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Ses {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise handler behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// Send one request to `path`, with `body` as its document when it carries one.
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let host = format!("email.{HOME}.amazonaws.com");
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/{HOME}/ses/aws4_request, SignedHeaders=host, Signature=00"
                ),
            )
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    /// Send one `ListImportJobs` request with `body` as its document.
    async fn list(&self, body: Value) -> Reply {
        self.call("POST", LIST_PATH, Some(body)).await
    }

    /// The whole listing, with no filter and no page size.
    async fn list_all(&self) -> Reply {
        let reply = self.list(json!({})).await;
        assert_eq!(
            reply.status, 200,
            "the listing must succeed, got {} {:?}",
            reply.status, reply.body
        );
        reply
    }

    /// Store one import job as a create would, then put its run where `row` says by
    /// moving the instants its transitions fall due at.
    fn seed(&self, row: &JobRow, created_at: SystemTime) {
        let now = SystemTime::now();
        let (processing_due_at, completion_due_at) = match row.run {
            Run::Created => (now + HELD, now + HELD + HELD),
            Run::Processing => (now - ELAPSED, now + HELD),
            Run::Completed => (now - ELAPSED - ELAPSED, now - ELAPSED),
        };
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let mut record = ImportJobRecord::new(
            row.job_id.to_string(),
            row.destination(),
            row.data_source(),
            created_at,
        );
        record.processing_due_at = processing_due_at;
        record.completion_due_at = completion_due_at;
        store.import_jobs.insert(row.job_id.to_string(), record);
    }

    /// Store one import job exactly as a create leaves it: just created, with both
    /// transitions of its run due at the spans the state module states.
    fn seed_fresh(&self, row: &JobRow, created_at: SystemTime) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        store.import_jobs.insert(
            row.job_id.to_string(),
            ImportJobRecord::new(
                row.job_id.to_string(),
                row.destination(),
                row.data_source(),
                created_at,
            ),
        );
    }

    /// Seed [`JOB_ROWS`] oldest first, each created [`CREATED_APART`] after the one
    /// before it.
    fn seed_population(&self) {
        for (index, row) in JOB_ROWS.iter().enumerate() {
            self.seed(row, created_at(index));
        }
    }

    /// Move the instants `job_id`'s transitions fall due at.
    fn set_due(&self, job_id: &str, processing_due_at: SystemTime, completion_due_at: SystemTime) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let record = store
            .import_jobs
            .get_mut(job_id)
            .unwrap_or_else(|| panic!("case {job_id:?}: the job must be in the store"));
        record.processing_due_at = processing_due_at;
        record.completion_due_at = completion_due_at;
    }

    /// Hold both of `job_id`'s transitions off, so an assertion about the status it was
    /// created in cannot race the span.
    fn hold_created(&self, job_id: &str) {
        let now = SystemTime::now();
        self.set_due(job_id, now + HELD, now + HELD + HELD);
    }

    /// Bring the start of `job_id`'s run due, without waiting for it, and hold its
    /// completion off.
    fn bring_processing_due(&self, job_id: &str) {
        let now = SystemTime::now();
        self.set_due(job_id, now - ELAPSED, now + HELD);
    }

    /// Bring the completion of `job_id`'s run due, without waiting for it.
    fn bring_completion_due(&self, job_id: &str) {
        let now = SystemTime::now();
        self.set_due(job_id, now - ELAPSED - ELAPSED, now - ELAPSED);
    }

    /// The spans the record stores between `job_id`'s creation and each of its
    /// transitions falling due.
    fn stored_spans(&self, job_id: &str) -> (Duration, Duration) {
        let record = self
            .stored(job_id)
            .unwrap_or_else(|| panic!("case {job_id:?}: the job must be in the store"));
        (
            record
                .processing_due_at
                .duration_since(record.created_at)
                .unwrap_or_default(),
            record
                .completion_due_at
                .duration_since(record.created_at)
                .unwrap_or_default(),
        )
    }

    /// The stored job `job_id` names, if the store holds one.
    fn stored(&self, job_id: &str) -> Option<ImportJobRecord> {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        store.import_jobs.get(job_id).cloned()
    }

    /// What a listing may not change about a stored job: the instants its transitions
    /// fall due at, and the records it has really imported.
    fn fingerprint(&self, job_id: &str) -> (SystemTime, SystemTime, SystemTime, i32, i32) {
        let record = self
            .stored(job_id)
            .unwrap_or_else(|| panic!("case {job_id:?}: the job must be in the store"));
        (
            record.created_at,
            record.processing_due_at,
            record.completion_due_at,
            record.processed_records_count,
            record.failed_records_count,
        )
    }

    /// Forget one stored job, so a case can show that a token names a position in the
    /// order rather than the job it was minted from.
    fn forget(&self, job_id: &str) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        store.import_jobs.remove(job_id);
    }
}

/// When the job seeded at `index` of [`JOB_ROWS`] was created.
fn created_at(index: usize) -> SystemTime {
    SystemTime::UNIX_EPOCH
        + Duration::from_secs(FIRST_CREATED_AT_SECS)
        + CREATED_APART * (index as u32)
}

/// The ids of [`JOB_ROWS`] in the order a listing reports them: most recently created
/// first.
fn newest_first() -> Vec<String> {
    JOB_ROWS
        .iter()
        .rev()
        .map(|row| row.job_id.to_string())
        .collect()
}

#[tokio::test]
async fn every_stored_job_is_reported_as_its_record_projects_it() {
    let ses = Ses::new();
    ses.seed_population();

    let reply = ses.list_all().await;
    assert_eq!(
        reply.ids(),
        newest_first(),
        "expected every stored job once, most recently created first, got {:?}",
        reply.body
    );
    assert_eq!(
        reply.next_token(),
        None,
        "the whole listing fits in one page, so there is nothing to resume from, got {:?}",
        reply.body
    );

    let reported = reply.jobs();
    for (index, row) in JOB_ROWS.iter().enumerate() {
        let case = row.case;
        // The listing is newest first, so the oldest row is reported last.
        let job = &reported[JOB_ROWS.len() - 1 - index];
        assert_eq!(
            member(job, "JobId"),
            Some(json!(row.job_id)),
            "case {case:?}: expected the stored job id, got {job:?}"
        );
        assert_eq!(
            member(job, "ImportDestination"),
            Some(row.destination_document()),
            "case {case:?}: expected the destination the create supplied, got {job:?}"
        );
        assert_eq!(
            member(job, "JobStatus"),
            Some(json!(row.run.status())),
            "case {case:?}: expected the status the run has reached, got {job:?}"
        );
        let expected_created = created_at(index)
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("seeded after the epoch")
            .as_secs();
        assert_eq!(
            member(job, "CreatedTimestamp"),
            Some(json!(expected_created)),
            "case {case:?}: expected the instant the job was created, got {job:?}"
        );
        // Nothing in this emulator reads the Amazon S3 object an import job's data
        // source names, so the counts a summary carries are the records it really
        // imported — none — rather than a number synthesised to match the status.
        assert_eq!(
            (
                member(job, "ProcessedRecordsCount"),
                member(job, "FailedRecordsCount"),
            ),
            (Some(json!(0)), Some(json!(0))),
            "case {case:?}: expected the counts the record holds, got {job:?}"
        );
    }
}

#[tokio::test]
async fn the_destination_type_filter_narrows_the_listing_to_the_jobs_that_carry_it() {
    // The filter's value set is the modelled enum's, so every one of its values is a
    // case.
    assert_eq!(
        IMPORT_DESTINATION_TYPE_VALUES,
        &[CONTACT_LIST, SUPPRESSION_LIST],
        "the destination types this case spans must be the modelled set, got \
         {IMPORT_DESTINATION_TYPE_VALUES:?}"
    );

    let ses = Ses::new();
    ses.seed_population();

    let filters: Vec<Option<&str>> = std::iter::once(None)
        .chain(IMPORT_DESTINATION_TYPE_VALUES.iter().copied().map(Some))
        .collect();
    for filter in filters {
        let case = format!("ImportDestinationType={filter:?}");
        let mut body = json!({});
        if let Some(filter) = filter {
            body["ImportDestinationType"] = json!(filter);
        }
        let reply = ses.list(body).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: a modelled filter value is served, got {} {:?}",
            reply.status, reply.body
        );
        let expected: Vec<String> = JOB_ROWS
            .iter()
            .rev()
            .filter(|row| filter.is_none_or(|wanted| row.destination_type() == wanted))
            .map(|row| row.job_id.to_string())
            .collect();
        assert_eq!(
            reply.ids(),
            expected,
            "case {case:?}: expected exactly the jobs whose destination selects the value, \
             got {:?}",
            reply.body
        );
    }
}

#[tokio::test]
async fn a_modelled_destination_type_no_stored_job_carries_matches_nothing() {
    let ses = Ses::new();
    // One job only, and its destination is the other kind, so the filter below is a
    // modelled value the account holds no job for.
    let row = &JOB_ROWS[0];
    ses.seed(row, created_at(0));

    let reply = ses
        .list(json!({"ImportDestinationType": CONTACT_LIST}))
        .await;
    assert_eq!(
        reply.status, 200,
        "a modelled filter value is served, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.jobs(),
        Vec::<Value>::new(),
        "case {:?}: a destination type no stored job carries matches nothing, got {:?}",
        row.case,
        reply.body
    );
}

#[tokio::test]
async fn a_page_walk_reports_every_matching_job_exactly_once() {
    let ses = Ses::new();
    ses.seed_population();
    let all = newest_first();

    // A page size below, at and above the number of stored jobs, plus the two forms
    // that truncate nothing: an absent size and a size of zero.
    let sizes = [
        Some(1),
        Some(2),
        Some(4),
        Some(JOB_ROWS.len()),
        Some(JOB_ROWS.len() + 3),
        Some(0),
        None,
    ];
    for size in sizes {
        let case = format!("PageSize={size:?}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One request per job is the most a terminating walk can need; the guard turns a
        // walk that fails to terminate into a failed assertion instead of a hang.
        for _ in 0..=JOB_ROWS.len() {
            let mut body = json!({});
            if let Some(size) = size {
                body["PageSize"] = json!(size);
            }
            if let Some(token) = &token {
                body["NextToken"] = json!(token);
            }
            let reply = ses.list(body).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: the page must be served, got {} {:?}",
                reply.status, reply.body
            );
            let page = reply.ids();
            if let Some(size) = size.filter(|size| *size > 0) {
                assert!(
                    page.len() <= size,
                    "case {case:?}: a page may not exceed the size asked for, got {page:?}"
                );
            }
            walked.extend(page);
            token = reply.next_token();
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            token, None,
            "case {case:?}: the walk must terminate, and it still offered {token:?}"
        );
        assert_eq!(
            walked, all,
            "case {case:?}: expected the whole listing, in order, exactly once"
        );
    }
}

#[tokio::test]
async fn a_page_walk_under_a_filter_reports_only_the_matching_jobs() {
    let ses = Ses::new();
    ses.seed_population();

    for wanted in IMPORT_DESTINATION_TYPE_VALUES {
        let case = format!("ImportDestinationType={wanted:?}, PageSize=1");
        let expected: Vec<String> = JOB_ROWS
            .iter()
            .rev()
            .filter(|row| row.destination_type() == *wanted)
            .map(|row| row.job_id.to_string())
            .collect();
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..=JOB_ROWS.len() {
            let mut body = json!({"ImportDestinationType": wanted, "PageSize": 1});
            if let Some(token) = &token {
                body["NextToken"] = json!(token);
            }
            let reply = ses.list(body).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: the page must be served, got {} {:?}",
                reply.status, reply.body
            );
            walked.extend(reply.ids());
            token = reply.next_token();
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, expected,
            "case {case:?}: expected exactly the matching jobs, in order, exactly once"
        );
    }
}

#[tokio::test]
async fn a_token_names_a_position_not_the_job_it_was_minted_from() {
    let ses = Ses::new();
    ses.seed_population();
    let all = newest_first();

    let first = ses.list(json!({"PageSize": 1})).await;
    assert_eq!(
        first.ids(),
        all[..1].to_vec(),
        "expected the newest job on the first page, got {:?}",
        first.body
    );
    let token = first
        .next_token()
        .expect("jobs remain behind the first page, so it carries a token");

    // The job the token was minted from is gone: the token is still a position in the
    // order, so the walk resumes exactly where it was.
    ses.forget(&all[0]);
    let second = ses.list(json!({"NextToken": token})).await;
    assert_eq!(
        second.status, 200,
        "a position whose job is gone is still a position, got {} {:?}",
        second.status, second.body
    );
    assert_eq!(
        second.ids(),
        all[1..].to_vec(),
        "expected the rest of the listing, got {:?}",
        second.body
    );
}

#[tokio::test]
async fn a_runs_transitions_fall_due_on_the_clock_and_never_on_a_listing() {
    let ses = Ses::new();
    let row = &JOB_ROWS[0];
    let job_id = row.job_id;
    ses.seed_fresh(row, SystemTime::now());

    let (processing_span, completion_span) = ses.stored_spans(job_id);
    assert_eq!(
        processing_span, IMPORT_JOB_PROCESSING_SETTLE,
        "expected the run to start {IMPORT_JOB_PROCESSING_SETTLE:?} after the job was created, \
         got {processing_span:?}"
    );
    assert_eq!(
        completion_span,
        IMPORT_JOB_PROCESSING_SETTLE + IMPORT_JOB_COMPLETION_SETTLE,
        "expected the run to finish {:?} after the job was created, got {completion_span:?}",
        IMPORT_JOB_PROCESSING_SETTLE + IMPORT_JOB_COMPLETION_SETTLE
    );

    // Nothing is due yet, and listing the job twice does not move its run on, nor change
    // anything else about it: the transition is owed by the clock, not by the read.
    ses.hold_created(job_id);
    let before = ses.fingerprint(job_id);
    for read in ["first", "second"] {
        let reply = ses.list_all().await;
        let job = reply
            .jobs()
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case {read:?}: the stored job must be reported"));
        assert_eq!(
            member(&job, "JobStatus"),
            Some(json!(Run::Created.status())),
            "case {read:?}: nothing is due yet, so the run has not started, got {job:?}"
        );
        assert_eq!(
            ses.fingerprint(job_id),
            before,
            "case {read:?}: a listing may not change the stored job"
        );
    }

    ses.bring_processing_due(job_id);
    let processing = ses.list_all().await;
    let job = processing
        .jobs()
        .first()
        .cloned()
        .expect("the stored job must be reported");
    assert_eq!(
        member(&job, "JobStatus"),
        Some(json!(Run::Processing.status())),
        "the start of the run was due, so the listing reports it processing, got {job:?}"
    );

    ses.bring_completion_due(job_id);
    let completed = ses.list_all().await;
    let job = completed
        .jobs()
        .first()
        .cloned()
        .expect("the stored job must be reported");
    assert_eq!(
        member(&job, "JobStatus"),
        Some(json!(Run::Completed.status())),
        "the completion of the run was due, so the listing reports it completed, got {job:?}"
    );
}

#[tokio::test]
async fn a_token_this_listing_never_handed_out_is_refused() {
    let ses = Ses::new();
    ses.seed_population();

    let cases: Vec<(&str, String)> = vec![
        (
            "a token that is not base64 at all",
            "not-a-token".to_string(),
        ),
        (
            "base64 of something this service never minted",
            aws_smithy_types::base64::encode("just some text"),
        ),
        (
            "another listing's token",
            encode_tagged_page_token(TENANTS_PAGE_TOKEN_TAG, "tenant-a"),
        ),
        ("an empty token", String::new()),
    ];

    for (case, token) in cases {
        let reply = ses.list(json!({"NextToken": token})).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected 400 BadRequestException, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.message(),
            "Invalid next token.",
            "case {case:?}: expected the refusal this service words for a token that is not a \
             position in the listing, got {:?}",
            reply.message()
        );
    }
}

#[tokio::test]
async fn a_destination_type_outside_its_modelled_enum_is_refused() {
    let ses = Ses::new();
    ses.seed_population();

    // Each row is a value outside `ImportDestinationType`'s modelled value set: a value
    // the enum does not carry, and ones that differ from a modelled value only in case
    // or in surrounding space.
    let cases = [
        "CONTACTS",
        "contact_list",
        " CONTACT_LIST",
        "SUPPRESSION_LIST ",
    ];

    for value in cases {
        let case = format!("ImportDestinationType={value:?}");
        let reply = ses.list(json!({"ImportDestinationType": value})).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected 400 BadRequestException, got {} {:?}",
            reply.status,
            reply.body
        );
        // The clause names the member and the value set it had to satisfy, and withholds
        // the value that violated them.
        let subject = "1 validation error detected: Value at 'importDestinationType' failed to \
                       satisfy constraint: Member must satisfy enum value set: [";
        let message = reply.message();
        assert!(
            message.starts_with(subject),
            "case {case:?}: expected the refusal to open {subject:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn an_account_holding_no_import_job_has_an_empty_listing() {
    let ses = Ses::new();

    let reply = ses.list_all().await;
    assert_eq!(
        reply.jobs(),
        Vec::<Value>::new(),
        "expected an empty list, got {:?}",
        reply.body
    );
    assert_eq!(
        reply.body.get("ImportJobs"),
        Some(&json!([])),
        "an empty list is a different answer from an absent member, got {:?}",
        reply.body
    );
    assert_eq!(
        reply.next_token(),
        None,
        "there is nothing to resume from, got {:?}",
        reply.body
    );
}

#[tokio::test]
async fn a_job_a_create_minted_is_in_the_listing() {
    let ses = Ses::new();

    // One create per destination kind, over the wire, so the listing is shown to report
    // the jobs this service really mints rather than only seeded records.
    let mut minted: Vec<(&str, String, Value)> = Vec::new();
    for row in JOB_ROWS.iter().take(2) {
        if let Some(name) = row.contact_list {
            let created = ses
                .call(
                    "POST",
                    "/v2/email/contact-lists",
                    Some(json!({"ContactListName": name})),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "case {:?}: the destination list must be created, got {} {:?}",
                row.case, created.status, created.body
            );
        }
        let reply = ses
            .call(
                "POST",
                "/v2/email/import-jobs",
                Some(json!({
                    "ImportDestination": row.destination_document(),
                    "ImportDataSource": {
                        "S3Url": row.s3_url,
                        "DataFormat": row.data_format,
                    },
                })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: the create must be served, got {} {:?}",
            row.case, reply.status, reply.body
        );
        let job_id = reply.text("JobId");
        assert!(
            !job_id.is_empty(),
            "case {:?}: expected the minted JobId, got {:?}",
            row.case,
            reply.body
        );
        minted.push((row.case, job_id, row.destination_document()));
    }

    let reply = ses.list_all().await;
    let ids = reply.ids();
    for (case, job_id, destination) in &minted {
        assert!(
            ids.contains(job_id),
            "case {case:?}: expected the minted job {job_id:?} in the listing, got {ids:?}"
        );
        let job = reply
            .jobs()
            .into_iter()
            .find(|job| member(job, "JobId") == Some(json!(job_id)))
            .unwrap_or_else(|| panic!("case {case:?}: the minted job must be reported"));
        assert_eq!(
            member(&job, "ImportDestination"),
            Some(destination.clone()),
            "case {case:?}: expected the destination the create supplied, got {job:?}"
        );
    }
    assert_eq!(
        ids.len(),
        minted.len(),
        "expected exactly the jobs the creates minted, got {ids:?}"
    );
}
