// SPDX-License-Identifier: CC0-1.0

//! What `CreateImportJob` does to the set of import jobs an account holds, end to end
//! over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization are
//! all exercised, and reaches the engine's own state to seed the jobs the account
//! already holds and to count what a call left behind.
//!
//! The rules these cases pin:
//!
//! * a create is served while the account holds fewer import jobs than the store bounds
//!   it to ([`MAX_IMPORT_JOBS`]) and, once it holds that many, is refused with the error
//!   the model binds for too many instances of a resource — `LimitExceededException`,
//!   HTTP 400 — leaving the jobs it already holds untouched;
//! * each served create mints an id the account holds no import job under and stores one
//!   more job, so two identical creates are two jobs, each of which reads back under its
//!   own id with the destination and the data source its own create supplied.
//!
//! The tables span what the rules generalise over: holdings of none, one, half the
//! bound, one short of it and the bound itself, and both members of `ImportDestination`
//! created twice over from the same request.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::ops::types::{
    ImportDataSource, ImportDestination, SuppressionListDestination,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    ComAmazonawsSesv2SimpleemailserviceV2State, ImportJobRecord, MAX_IMPORT_JOBS,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region the cases execute in: the one their `Authorization` credential scope
/// names. An import job belongs to the account, so one Region is enough.
const HOME: &str = "us-east-1";

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

    /// The message the error carried.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
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

/// One create request the cases drive, as its document.
fn create_request(destination: &Value, s3_url: &str, data_format: &str) -> Value {
    json!({
        "ImportDestination": destination,
        "ImportDataSource": { "S3Url": s3_url, "DataFormat": data_format },
    })
}

/// The emulated service behind its router, plus the shared engine so a case can seed
/// jobs the account holds and count what it holds after a call.
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

    /// Send one `CreateImportJob` request.
    async fn create(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/import-jobs", Some(body)).await
    }

    /// Send one `GetImportJob` request.
    async fn get(&self, job_id: &str) -> Reply {
        self.call("GET", &format!("/v2/email/import-jobs/{job_id}"), None)
            .await
    }

    /// Create the contact list `name`, so a contact-list destination can name it.
    async fn create_contact_list(&self, name: &str) {
        let reply = self
            .call(
                "POST",
                "/v2/email/contact-lists",
                Some(json!({ "ContactListName": name })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {name:?}: the destination list must be created, got {} {:?}",
            reply.status, reply.body
        );
    }

    /// How many import jobs the account holds.
    fn stored_count(&self) -> usize {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        store.import_jobs.len()
    }

    /// Whether the account holds an import job under `job_id`.
    fn holds(&self, job_id: &str) -> bool {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        store.import_jobs.contains_key(job_id)
    }

    /// Seed the store with `count` import jobs, as creates of them would leave it.
    fn seed(&self, count: usize) {
        let now = SystemTime::now();
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        for index in 0..count {
            let job_id = format!("seeded-{index:08}");
            store.import_jobs.insert(
                job_id.clone(),
                ImportJobRecord::new(
                    job_id,
                    ImportDestination {
                        suppression_list_destination: Some(SuppressionListDestination {
                            suppression_list_import_action: "PUT".to_string(),
                        }),
                        contact_list_destination: None,
                    },
                    ImportDataSource {
                        s3_url: format!("s3://ses-imports/seeded/{index:08}.csv"),
                        data_format: "CSV".to_string(),
                    },
                    now,
                ),
            );
        }
    }
}

/// One holding of import jobs a create is driven against.
struct Holding {
    case: &'static str,
    /// How many import jobs the account already holds.
    held: usize,
    /// Whether a create is served at that holding.
    served: bool,
}

#[tokio::test]
async fn a_create_is_served_until_the_account_holds_the_jobs_the_store_bounds_it_to() {
    let holdings = [
        Holding {
            case: "no jobs held",
            held: 0,
            served: true,
        },
        Holding {
            case: "one job held",
            held: 1,
            served: true,
        },
        Holding {
            case: "half the bound held",
            held: MAX_IMPORT_JOBS / 2,
            served: true,
        },
        Holding {
            case: "one short of the bound",
            held: MAX_IMPORT_JOBS - 1,
            served: true,
        },
        Holding {
            case: "the bound held",
            held: MAX_IMPORT_JOBS,
            served: false,
        },
    ];

    let destination = json!({
        "SuppressionListDestination": { "SuppressionListImportAction": "PUT" },
    });

    for holding in holdings {
        let case = holding.case;
        let ses = Ses::new();
        ses.seed(holding.held);
        assert_eq!(
            ses.stored_count(),
            holding.held,
            "case {case:?}: expected {} jobs held before the create, got {}",
            holding.held,
            ses.stored_count()
        );

        let reply = ses
            .create(create_request(
                &destination,
                "s3://ses-imports/suppression.csv",
                "CSV",
            ))
            .await;

        if holding.served {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: a create below the bound is served, got {} {:?}",
                reply.status, reply.body
            );
            let job_id = reply.text("JobId");
            assert!(
                !job_id.is_empty(),
                "case {case:?}: expected the minted JobId, got {:?}",
                reply.body
            );
            assert_eq!(
                ses.stored_count(),
                holding.held + 1,
                "case {case:?}: a served create stores one more job, got {}",
                ses.stored_count()
            );
            assert!(
                ses.holds(&job_id),
                "case {case:?}: the minted job must be held under {job_id:?}, got {} jobs",
                ses.stored_count()
            );
            // The minted job is the one the read resolves, beside the jobs the account
            // already held.
            let read = ses.get(&job_id).await;
            assert_eq!(
                (read.status, read.text("JobId").as_str()),
                (200, job_id.as_str()),
                "case {case:?}: expected the minted job read back, got {} {:?}",
                read.status,
                read.body
            );
        } else {
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (400, "LimitExceededException"),
                "case {case:?}: expected 400 LimitExceededException at the bound, got {} {:?}",
                reply.status,
                reply.body
            );
            assert!(
                reply.message().contains(&MAX_IMPORT_JOBS.to_string()),
                "case {case:?}: expected the bound in the message, got {:?}",
                reply.message()
            );
            assert_eq!(
                ses.stored_count(),
                holding.held,
                "case {case:?}: a refused create persists nothing, got {} jobs",
                ses.stored_count()
            );
        }
    }
}

/// One create driven twice over, and what the job each of them mints must carry.
struct Repeated {
    case: &'static str,
    /// The `ImportDestination` document both creates carry.
    destination: Value,
    /// The contact list the destination names, for a contact-list destination.
    contact_list: Option<&'static str>,
    /// `ImportDataSource$S3Url`.
    s3_url: &'static str,
    /// `ImportDataSource$DataFormat`.
    data_format: &'static str,
}

#[tokio::test]
async fn each_create_mints_a_job_id_the_account_does_not_already_hold() {
    let repeats = vec![
        Repeated {
            case: "suppression list destination",
            destination: json!({
                "SuppressionListDestination": { "SuppressionListImportAction": "PUT" },
            }),
            contact_list: None,
            s3_url: "s3://ses-imports/suppression.csv",
            data_format: "CSV",
        },
        Repeated {
            case: "contact list destination",
            destination: json!({
                "ContactListDestination": {
                    "ContactListName": "newsletter",
                    "ContactListImportAction": "DELETE",
                },
            }),
            contact_list: Some("newsletter"),
            s3_url: "s3://ses-imports/2026/11/contacts.json",
            data_format: "JSON",
        },
    ];

    let ses = Ses::new();
    for repeat in &repeats {
        if let Some(name) = repeat.contact_list {
            ses.create_contact_list(name).await;
        }
    }

    // Every create that was served, in the order it ran: which row it came from and the
    // id it was answered with.
    let mut minted: Vec<(&'static str, usize, String)> = Vec::new();
    for repeat in &repeats {
        // The SAME request twice: an id is minted per job, so the second create is a
        // second job rather than the first one again.
        for attempt in 1..=2usize {
            let case = repeat.case;
            let reply = ses
                .create(create_request(
                    &repeat.destination,
                    repeat.s3_url,
                    repeat.data_format,
                ))
                .await;
            assert_eq!(
                reply.status, 200,
                "case {case:?} attempt {attempt}: a create of a modelled destination is served, got {} {:?}",
                reply.status, reply.body
            );
            let job_id = reply.text("JobId");
            assert!(
                !job_id.is_empty(),
                "case {case:?} attempt {attempt}: expected the minted JobId, got {:?}",
                reply.body
            );
            minted.push((case, attempt, job_id));
        }
    }

    let distinct: BTreeSet<&String> = minted.iter().map(|(_, _, job_id)| job_id).collect();
    assert_eq!(
        distinct.len(),
        minted.len(),
        "expected one distinct JobId per create, got {:?}",
        minted
    );
    assert_eq!(
        ses.stored_count(),
        minted.len(),
        "expected one held job per create, got {}",
        ses.stored_count()
    );

    for (index, (case, attempt, job_id)) in minted.iter().enumerate() {
        let repeat = &repeats[index / 2];
        let read = ses.get(job_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?} attempt {attempt}: a read of a stored job is served, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.text("JobId"),
            *job_id,
            "case {case:?} attempt {attempt}: expected the id the job was minted with, got {:?}",
            read.body
        );
        assert_eq!(
            read.body.get("ImportDestination"),
            Some(&repeat.destination),
            "case {case:?} attempt {attempt}: expected the destination its own create supplied, got {:?}",
            read.body.get("ImportDestination")
        );
        assert_eq!(
            read.body.get("ImportDataSource"),
            Some(&json!({ "S3Url": repeat.s3_url, "DataFormat": repeat.data_format })),
            "case {case:?} attempt {attempt}: expected the data source its own create supplied, got {:?}",
            read.body.get("ImportDataSource")
        );
    }
}
