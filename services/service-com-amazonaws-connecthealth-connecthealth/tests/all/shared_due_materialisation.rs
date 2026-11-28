// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the ONE preamble every handler of this service runs
//! before it resolves anything.
//!
//! The RULE under test, asserted over a table rather than one operation: a handler
//! materialises EVERYTHING the clock has already made due, never one class of owed work
//! and not another — so after ANY operation of this service has run, a domain whose
//! deletion had fallen due is gone AND a patient insights job whose next status
//! transition had fallen due has been moved on. Because each piece of owed work is
//! applied AT the instant it fell due rather than at the instant somebody looked, running
//! the preamble in one handler changes no value another handler reports, which is what
//! makes one shared preamble indistinguishable on the wire from the per-handler ones it
//! replaces.
//!
//! The rows differ along what the rule generalises over: which operation is driven (a
//! point reader, a listing, a creator, a deleter, the two status transitions, the three
//! ARN-addressed tagging operations, and both patient-insights handlers), its HTTP
//! method, what it is addressed to, and whether it resolves that (its modelled success)
//! or misses it (the modelled 404) — the preamble runs before the resolution, so a miss
//! materialises the owed work too.
//!
//! Every route and success status written here comes from
//! `smithy_query(query="digest",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`,
//! and the `sessionId` of the miss row is a value of the modelled `ScribeSessionId`
//! (`@length 36..36`, pattern
//! `[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}`).

/// The record kinds the refreshed model added — the `Dataset` lifecycle, the medical coding
/// job pair and the feedback a submission records — whose own clock-owed transitions extend
/// the rule this suite states.
///
/// A suite of its own rather than more rows here, because the records it drives are not the
/// two this file's fixture holds; it is declared HERE, as this suite's own child module,
/// because `tests/all.rs` is this crate's declaration list and the merge gate refuses a fix
/// that edits it.
///
/// The `#[path]` is what points the declaration into this suite's own directory: `all.rs`
/// declares THIS file through a `#[path]` of its own, so a bare `mod` here would resolve to
/// a second top-level suite beside it instead.
#[path = "shared_due_materialisation/datasets_and_medical_coding_jobs.rs"]
mod datasets_and_medical_coding_jobs;

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_connecthealth_connecthealth::state::{
    self, JobStatus, PATIENT_INSIGHTS_JOB_STEP_SPAN,
};

use crate::arn_resolution_during_domain_deletion::web_app_setup;
use crate::domain_subscription_lifecycle::{error_code, string_at, Emulator};
use crate::patient_insights_job_lifecycle::{job_path, minimal_body, start_path};
use crate::resource_tag_removal::untag_path;
use crate::resource_tagging::tags_path;

/// A `ScribeSessionId` that names no session: the modelled shape with every hex digit
/// zero, so the request is well-formed and the only possible answer is the modelled 404.
const ABSENT_SESSION_ID: &str = "00000000-0000-0000-0000-000000000000";

/// How far in the PAST the owed work is dated, so it is already due when the probe runs
/// without being due long enough for a second transition (one
/// [`PATIENT_INSIGHTS_JOB_STEP_SPAN`] later) to be owed as well.
const ALREADY_DUE_BY: Duration = Duration::from_millis(1);

/// One emulator with both classes of owed work standing: a domain whose purge has fallen
/// due, and a patient insights job whose next status transition has fallen due.
struct Fixture {
    emulator: Emulator,
    /// The domain every probe that needs a live resource is addressed to.
    live_domain: String,
    live_arn: String,
    subscription: String,
    subscription_arn: String,
    /// A second live domain, for the probe that deletes one — so no probe removes the
    /// records the assertions are made against.
    spare_domain: String,
    /// The job whose transition is already due, under [`Fixture::live_domain`].
    job_id: String,
    /// The instant that transition fell due.
    job_due: SystemTime,
    /// The domain whose purge is already due.
    dying_domain: String,
}

/// What the store reports about the owed work after a probe has run.
#[derive(Debug, PartialEq, Eq)]
struct Materialised {
    /// Whether the domain whose purge had fallen due is gone.
    purged: bool,
    /// The status the job carries and the instant it was last stamped with.
    job: (JobStatus, SystemTime),
}

/// Stand up one emulator and leave both classes of owed work already due.
///
/// The records are written through the service's OWN operations and only their due
/// instants are moved back, so the fixture is a state the clock reaches by itself rather
/// than one hand-built beside the handlers.
async fn fixture(region: &str) -> Fixture {
    let emulator = Emulator::new(region);

    let live = emulator
        .create_domain(&json!({ "name": "clinic-live" }))
        .await;
    let live_domain = string_at(&live, "domainId");
    let live_arn = string_at(&live, "arn");
    let spare_domain = string_at(
        &emulator
            .create_domain(&json!({ "name": "clinic-spare" }))
            .await,
        "domainId",
    );

    let (status, subscription) = emulator
        .send(
            "POST",
            &format!("/domains/{live_domain}/subscriptions"),
            Some(&json!({})),
        )
        .await;
    assert_eq!(
        status, 201,
        "CreateSubscription must answer 201, got {status} with {subscription}"
    );
    let subscription_id = string_at(&subscription, "subscriptionId");
    let subscription_arn = string_at(&subscription, "arn");

    // A domain owning a web application is the one whose deletion cannot finish inside
    // the call, so its record passes through the non-terminal status and owes a purge.
    let dying_domain = string_at(
        &emulator
            .create_domain(&json!({
                "name": "clinic-dying",
                "webAppSetupConfiguration": web_app_setup(),
            }))
            .await,
        "domainId",
    );
    let (status, deleted) = emulator
        .send("DELETE", &format!("/domain/{dying_domain}"), None)
        .await;
    assert_eq!(
        (status, string_at(&deleted, "status").as_str()),
        (200, "DELETING"),
        "DeleteDomain of a web-app domain must report the non-terminal status, got \
         {status} with {deleted}"
    );

    let (status, started) = emulator
        .send("POST", &start_path(&live_domain), Some(&minimal_body()))
        .await;
    assert_eq!(
        status, 200,
        "StartPatientInsightsJob must answer 200, got {status} with {started}"
    );
    let job_id = string_at(&started, "jobId");

    // Both due instants are moved into the past LAST, so no call of this setup
    // materialises what the probe is supposed to.
    let job_due = SystemTime::now() - ALREADY_DUE_BY;
    {
        let store = state::service_state(&**emulator.engine()).expect("this service's own store");
        let mut store = store.lock();
        let dying = store
            .held_domain_mut(region, &dying_domain)
            .expect("the deleted domain's record is still held");
        dying.purge_due_at = Some(job_due);
        let job = store
            .live_domain_mut(region, &live_domain)
            .and_then(|domain| domain.patient_insights_jobs.get_mut(&job_id))
            .expect("the started job is held under its domain");
        job.next_transition_at = Some(job_due);
        assert_eq!(
            job.status,
            JobStatus::Submitted,
            "a job read straight after its start has not moved yet"
        );
    }

    Fixture {
        emulator,
        live_domain,
        live_arn,
        subscription: subscription_id,
        subscription_arn,
        spare_domain,
        job_id,
        job_due,
        dying_domain,
    }
}

/// Read the owed work out of the store, as it stands after a probe.
fn materialised(fixture: &Fixture, case: &str) -> Materialised {
    let region = fixture.emulator.region();
    let store =
        state::service_state(&**fixture.emulator.engine()).expect("this service's own store");
    let store = store.lock();
    let job = store
        .patient_insights_job(region, &fixture.live_domain, &fixture.job_id)
        .unwrap_or_else(|| panic!("case {case:?}: the job's own domain is untouched by the probe"));
    Materialised {
        purged: store
            .held_domain(region, &fixture.dying_domain)
            .is_none(),
        job: (job.status, job.updated_at),
    }
}

/// ONE operation, addressed and driven the way a client drives it.
struct Probe {
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// The status this row expects: the operation's modelled success, or the modelled 404
    /// for the row that addresses a resource nothing holds.
    expected: u16,
}

/// How one row addresses its operation at the fixture the probe runs against.
type Address = fn(&Fixture) -> Probe;

/// Every operation this service serves, each addressed at the fixture.
fn probes() -> Vec<(&'static str, Address)> {
    vec![
        ("GetDomain", |f| Probe {
            method: "GET",
            path: format!("/domain/{}", f.live_domain),
            body: None,
            expected: 200,
        }),
        ("ListDomains", |_| Probe {
            method: "GET",
            path: "/domain".to_string(),
            body: None,
            expected: 200,
        }),
        ("CreateDomain", |_| Probe {
            method: "POST",
            path: "/domain".to_string(),
            body: Some(json!({ "name": "clinic-probe" })),
            expected: 201,
        }),
        ("DeleteDomain", |f| Probe {
            method: "DELETE",
            path: format!("/domain/{}", f.spare_domain),
            body: None,
            expected: 200,
        }),
        ("ListSubscriptions", |f| Probe {
            method: "GET",
            path: format!("/domains/{}/subscriptions", f.live_domain),
            body: None,
            expected: 200,
        }),
        ("CreateSubscription", |f| Probe {
            method: "POST",
            path: format!("/domains/{}/subscriptions", f.live_domain),
            body: Some(json!({})),
            expected: 201,
        }),
        ("GetSubscription", |f| Probe {
            method: "GET",
            path: format!(
                "/domains/{}/subscriptions/{}",
                f.live_domain, f.subscription
            ),
            body: None,
            expected: 200,
        }),
        ("ActivateSubscription", |f| Probe {
            method: "POST",
            path: format!(
                "/domains/{}/subscriptions/{}/activate",
                f.live_domain, f.subscription
            ),
            body: Some(json!({})),
            expected: 200,
        }),
        ("DeactivateSubscription", |f| Probe {
            method: "POST",
            path: format!(
                "/domains/{}/subscriptions/{}/deactivate",
                f.live_domain, f.subscription
            ),
            body: Some(json!({})),
            expected: 200,
        }),
        ("TagResource", |f| Probe {
            method: "POST",
            path: tags_path(&f.live_arn),
            body: Some(json!({ "tags": { "probe": "1" } })),
            expected: 204,
        }),
        ("UntagResource", |f| Probe {
            method: "DELETE",
            path: untag_path(&f.live_arn, &["probe".to_string()]),
            body: None,
            expected: 204,
        }),
        ("ListTagsForResource", |f| Probe {
            method: "GET",
            path: tags_path(&f.subscription_arn),
            body: None,
            expected: 200,
        }),
        (
            "GetMedicalScribeListeningSession, addressed to no session",
            |f| Probe {
                method: "GET",
                path: format!(
                    "/medical-scribe-stream/domain/{}/subscription/{}/session/{ABSENT_SESSION_ID}",
                    f.live_domain, f.subscription
                ),
                body: None,
                expected: 404,
            },
        ),
        ("StartPatientInsightsJob", |f| Probe {
            method: "POST",
            path: start_path(&f.live_domain),
            body: Some(minimal_body()),
            expected: 200,
        }),
        ("GetPatientInsightsJob", |f| Probe {
            method: "GET",
            path: job_path(&f.live_domain, &f.job_id),
            body: None,
            expected: 200,
        }),
    ]
}

#[tokio::test]
async fn every_operation_materialises_every_class_of_owed_work() {
    // The region is varied with the operation, so no row depends on one endpoint.
    let regions = ["us-east-1", "eu-west-1", "ap-south-1"];

    for (index, (case, build)) in probes().into_iter().enumerate() {
        let region = regions[index % regions.len()];
        let fixture = fixture(region).await;
        let probe = build(&fixture);

        let (status, response) = fixture
            .emulator
            .send(probe.method, &probe.path, probe.body.as_ref())
            .await;
        assert_eq!(
            status, probe.expected,
            "case {case:?}: {} {} must answer {}, got {status} with {response}",
            probe.method, probe.path, probe.expected
        );

        // Both classes of owed work, after ONE call of ONE operation. The job's stamp is
        // the instant its transition FELL DUE — never the instant this probe looked —
        // which is why running the preamble here cannot change what a later reader
        // reports; a probe slow enough for the following transition to fall due too is
        // stamped at THAT due instant, equally not its own.
        let observed = materialised(&fixture, case);
        assert!(
            observed.purged,
            "case {case:?}: a deletion already due must be materialised, got {observed:?}"
        );
        let stamps = [
            (JobStatus::InProgress, fixture.job_due),
            (
                JobStatus::Succeeded,
                fixture.job_due + PATIENT_INSIGHTS_JOB_STEP_SPAN,
            ),
        ];
        assert!(
            stamps.contains(&observed.job),
            "case {case:?}: a job transition already due must be materialised and stamped \
             at the instant it fell due, expected one of {stamps:?}, got {:?}",
            observed.job
        );

        // The same two facts on the wire: the purged domain resolves nowhere, and the
        // job's own reader reports the status the clock moved it to.
        let (status, missing) = fixture
            .emulator
            .send("GET", &format!("/domain/{}", fixture.dying_domain), None)
            .await;
        assert_eq!(
            (status, error_code(&missing).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a purged domain resolves nowhere, got {status} with {missing}"
        );
        let (status, fetched) = fixture
            .emulator
            .send(
                "GET",
                &job_path(&fixture.live_domain, &fixture.job_id),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetPatientInsightsJob must answer 200, got {status} with {fetched}"
        );
        assert_ne!(
            string_at(&fetched, "jobStatus"),
            JobStatus::Submitted.as_wire(),
            "case {case:?}: the reader must report the status the clock moved the job to, \
             got {fetched}"
        );
    }
}
