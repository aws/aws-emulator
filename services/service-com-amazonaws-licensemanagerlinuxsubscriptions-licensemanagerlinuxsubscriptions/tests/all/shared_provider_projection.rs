// SPDX-License-Identifier: CC0-1.0

//! The ONE projection of a stored registration onto the members a registration is REPORTED
//! with, driven through both operations that report one in full.
//!
//! The rule: the six members a registration is reported with
//! (`SubscriptionProviderArn`, `SubscriptionProviderSource`, `SecretArn`,
//! `SubscriptionProviderStatus`, `SubscriptionProviderStatusMessage`,
//! `LastSuccessfulDataRetrievalTime` — `smithy_query(query="operation",
//! operation="GetRegisteredSubscriptionProvider,ListRegisteredSubscriptionProviders")`) are
//! derived from the stored record by ONE mapping, so
//! `GetRegisteredSubscriptionProvider` and `ListRegisteredSubscriptionProviders` report the
//! same stored registration identically, member by member: what a registration reports is a
//! property of the record, not of the operation that reported it.
//!
//! This suite pins that by driving the shared projection
//! (`state::registered_subscription_provider`) through an operation on each side: the read
//! of one registration and the listing that projects every page element. The rows span what
//! the mapping generalises over — which `SubscriptionProviderStatus` the record holds,
//! whether it has anything to report about the token, whether it has a retrieval time to
//! report, and the form of the `SecretArn` it names — and one test drives the same two
//! operations over a registration the service itself created and then moved on the clock,
//! so the projection is pinned over records no test placed as well.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};

use crate::harness::{
    get_provider, harness, is_iso8601_utc, list_providers, provider_arn, provider_arn_for,
    register, Harness, Reply, ACCOUNT, ACTIVE, INVALID, PENDING, RED_HAT,
};
use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::state::{
    iso8601, SubscriptionProviderRecord,
};
use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState as ServiceState;

/// The six members a registration is reported with, in the model's names.
const REPORTED_MEMBERS: [&str; 6] = [
    "SubscriptionProviderArn",
    "SubscriptionProviderSource",
    "SecretArn",
    "SubscriptionProviderStatus",
    "SubscriptionProviderStatusMessage",
    "LastSuccessfulDataRetrievalTime",
];

/// How long this suite is willing to wait for a CONDITION the clock decides — the terminal
/// status a fresh registration owes, and the agreement of the two reporting operations
/// across the instant that status falls due — and how often it looks again.
///
/// Generous on purpose: what is waited for is the condition and never a span of wall clock,
/// so a loaded or a faster host cannot decide the outcome, and each wait ends as soon as the
/// condition holds.
const AGREEMENT_DEADLINE: Duration = Duration::from_secs(60);
/// See [`AGREEMENT_DEADLINE`].
const LOOK_AGAIN_AFTER: Duration = Duration::from_millis(25);

/// One row: a stored registration, stated member by member.
struct Row {
    case: &'static str,
    secret_arn: String,
    status: &'static str,
    status_message: Option<String>,
    retrieval_time: Option<String>,
}

/// The rows: every `SubscriptionProviderStatus` the model declares
/// (`one of: ACTIVE | INVALID | PENDING`), with and without something to report about the
/// token, with and without a retrieval time, over `SecretArn` forms the modelled pattern
/// `^arn:[a-z0-9-\.]{1,63}:secretsmanager:[a-z0-9-\.]{0,63}:[a-z0-9-\.]{0,63}:secret:[^/]{1,1023}$`
/// admits (a plain one, another partition, the empty region and account segments, and a
/// name using characters `[^/]` allows).
fn rows() -> Vec<Row> {
    let retrieved = iso8601(SystemTime::now() - Duration::from_secs(3600));
    vec![
        Row {
            case: "usable, nothing to report about the token",
            secret_arn: format!("arn:aws:secretsmanager:us-west-2:{ACCOUNT}:secret:rhel-token"),
            status: ACTIVE,
            status_message: None,
            retrieval_time: Some(retrieved.clone()),
        },
        Row {
            case: "usable, with a message about the token",
            secret_arn: "arn:aws-us-gov:secretsmanager:us-gov-west-1:123456789012:secret:rhsm"
                .to_string(),
            status: ACTIVE,
            status_message: Some("token refreshed".to_string()),
            retrieval_time: Some(retrieved.clone()),
        },
        Row {
            case: "not settled yet: no message and no retrieval",
            secret_arn: "arn:aws:secretsmanager:::secret:token".to_string(),
            status: PENDING,
            status_message: None,
            retrieval_time: None,
        },
        Row {
            case: "rejected token, with the message and no retrieval",
            secret_arn: "arn:aws:secretsmanager:eu-west-1:000000000000:secret:RedHat_Token=1"
                .to_string(),
            status: INVALID,
            status_message: Some("The subscription provider token is invalid or expired"
                .to_string()),
            retrieval_time: None,
        },
        Row {
            case: "rejected token that had retrieved data before",
            secret_arn: "arn:aws:secretsmanager:ap-southeast-2:123456789012:secret:rhel.token+2"
                .to_string(),
            status: INVALID,
            status_message: Some("access denied by the subscription provider".to_string()),
            retrieval_time: Some(retrieved),
        },
    ]
}

/// The member set a row expects to be reported with, as the projection of its stored record.
fn expected(row: &Row, arn: &str) -> BTreeMap<&'static str, Option<String>> {
    BTreeMap::from([
        ("SubscriptionProviderArn", Some(arn.to_string())),
        ("SubscriptionProviderSource", Some(RED_HAT.to_string())),
        ("SecretArn", Some(row.secret_arn.clone())),
        ("SubscriptionProviderStatus", Some(row.status.to_string())),
        (
            "SubscriptionProviderStatusMessage",
            row.status_message.clone(),
        ),
        (
            "LastSuccessfulDataRetrievalTime",
            row.retrieval_time.clone(),
        ),
    ])
}

/// The six reported members of one body — a `GetRegisteredSubscriptionProvider` response or
/// one element of a listing — read the same way on both sides, so the two are comparable.
fn reported(body: &Value, case: &str) -> BTreeMap<&'static str, Option<String>> {
    REPORTED_MEMBERS
        .iter()
        .map(|name| {
            let value = match body.get(name) {
                None | Some(Value::Null) => None,
                Some(value) => Some(
                    value
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: {name} must be a string, got {value:?}")
                        })
                        .to_string(),
                ),
            };
            (*name, value)
        })
        .collect()
}

/// Place one registration straight into the store, exactly as the row states it, and answer
/// its ARN. The store seam is the one the handlers read (`engine.state::<…>()`), so a
/// placed record is a record both reporting operations resolve.
///
/// `sequence` gives the resource id, in the lowercase-hex UUID shape this service mints, so
/// the records are held in a stable order a single listing reports in full.
fn seed(h: &Harness, sequence: usize, row: &Row) -> String {
    let arn = provider_arn_for(&format!("00000000-0000-4000-8000-{:012x}", sequence + 1));
    let mut record = SubscriptionProviderRecord::settled(
        arn.clone(),
        RED_HAT.to_string(),
        row.secret_arn.clone(),
        row.status.to_string(),
        HashMap::new(),
        SystemTime::now(),
    );
    // Stated by the row rather than by how the record was placed: these two members are
    // exactly what the projection has to carry across.
    record.status_message = row.status_message.clone();
    record.last_successful_data_retrieval_time = row.retrieval_time.clone();
    let state = h.engine.state::<ServiceState>();
    state.lock().providers.insert(arn.clone(), record);
    arn
}

/// One element of a listing, by the ARN it reports.
fn listed_entry(listing: &Reply, arn: &str, case: &str) -> Value {
    listing
        .ok(case)
        .get("RegisteredSubscriptionProviders")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: no RegisteredSubscriptionProviders in {:?}",
                listing.body
            )
        })
        .iter()
        .find(|entry| entry.get("SubscriptionProviderArn") == Some(&json!(arn)))
        .unwrap_or_else(|| panic!("case {case:?}: {arn} is missing from {:?}", listing.body))
        .clone()
}

/// Both reporting operations project the SAME stored record onto the same six members.
#[tokio::test]
async fn the_read_and_the_listing_report_a_stored_registration_identically() {
    let h = harness();
    let rows = rows();
    let arns: Vec<String> = rows
        .iter()
        .enumerate()
        .map(|(sequence, row)| seed(&h, sequence, row))
        .collect();

    // One listing reports every seeded registration (an absent `MaxResults` selects the
    // whole list), so the two sides are compared over the same store.
    let listing = list_providers(&h, &json!({})).await;

    for (row, arn) in rows.iter().zip(&arns) {
        let case = row.case;
        let got = get_provider(&h, arn).await;
        let read = reported(got.ok(case), case);
        let listed = reported(&listed_entry(&listing, arn, case), case);
        let expected = expected(row, arn);
        assert_eq!(
            read, expected,
            "case {case:?}: expected the read of {arn} to report {expected:?}, got {read:?}"
        );
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the listing of {arn} to report {expected:?}, got \
             {listed:?}"
        );
        assert_eq!(
            read, listed,
            "case {case:?}: the read reported {read:?} and the listing {listed:?} for the one \
             stored registration"
        );
    }
}

/// The same rule over a registration the SERVICE created and then moved on the clock: the
/// members the read reports are the members the listing reports, before and after the
/// transition, including the retrieval time the usable status carries.
#[tokio::test]
async fn the_read_and_the_listing_agree_over_a_registration_the_service_created() {
    let h = harness();
    let case = "a registration the service created";
    let created = register(
        &h,
        &json!({
            "SubscriptionProviderSource": RED_HAT,
            "SecretArn": format!("arn:aws:secretsmanager:us-west-2:{ACCOUNT}:secret:rhel-token"),
        }),
    )
    .await;
    let arn = provider_arn(&created, case);

    // Straight after the create: both operations report the same members, whether or not
    // the transition the clock owes this registration has already fallen due (which status
    // a fresh registration reports is `provider_status_transition.rs`'s rule, not this
    // suite's).
    let before = agreed_members(&h, &arn, case).await;
    let reported_status = before
        .get("SubscriptionProviderStatus")
        .and_then(Option::as_deref)
        .unwrap_or_else(|| panic!("case {case:?}: no status reported, got {before:?}"));
    assert!(
        [PENDING, ACTIVE].contains(&reported_status),
        "case {case:?}: expected one of {:?}, got {reported_status:?}",
        [PENDING, ACTIVE]
    );

    // Poll until the transition the clock owes has been materialized — the emulator never
    // sleeps for it, so the condition is what the test waits on, under a deadline generous
    // enough that a loaded host still reaches it.
    let deadline = Instant::now() + AGREEMENT_DEADLINE;
    let after = loop {
        let members = agreed_members(&h, &arn, case).await;
        if members.get("SubscriptionProviderStatus") == Some(&Some(ACTIVE.to_string())) {
            break members;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {ACTIVE} before the deadline, last reported {members:?}"
        );
        std::thread::sleep(LOOK_AGAIN_AFTER);
    };

    let retrieved = after
        .get("LastSuccessfulDataRetrievalTime")
        .and_then(Option::as_deref)
        .unwrap_or_else(|| panic!("case {case:?}: a usable registration reports a retrieval \
             time, got {after:?}"));
    assert!(
        is_iso8601_utc(retrieved),
        "case {case:?}: expected an ISO-8601 UTC instant, got {retrieved:?}"
    );
    assert_eq!(
        after.get("SubscriptionProviderStatusMessage"),
        Some(&None),
        "case {case:?}: a usable registration has nothing to report about its token, got \
         {after:?}"
    );
    assert_eq!(
        after.get("SecretArn"),
        before.get("SecretArn"),
        "case {case:?}: the secret the registration names cannot change with its status, got \
         {after:?} after {before:?}"
    );
}

/// The members both reporting operations answer for `arn`, asserted to be the same set on
/// both sides and answered once — the agreement is the rule, so a caller of this helper
/// reads one member set rather than two.
///
/// The two sides are two calls at two INSTANTS, and a transition the clock owes this
/// registration is materialized by whichever call reports a status first, so a pair that
/// straddles the due instant differs in WHEN the record was observed rather than in HOW it
/// was projected. The pair is therefore observed again until the two sides agree, under a
/// deadline far above any span this service owes: a projection that really differed between
/// the two operations never agrees, and the wait that runs out names the last pair it read.
///
/// The pause between looks is `std::thread::sleep` rather than `tokio::time::sleep`: this
/// workspace enables tokio's `macros`/`rt` features only, and nothing else is pending on
/// the test's runtime while it waits.
async fn agreed_members(
    h: &Harness,
    arn: &str,
    case: &str,
) -> BTreeMap<&'static str, Option<String>> {
    let deadline = Instant::now() + AGREEMENT_DEADLINE;
    loop {
        let read = reported(get_provider(h, arn).await.ok(case), case);
        let listing = list_providers(h, &json!({})).await;
        let listed = reported(&listed_entry(&listing, arn, case), case);
        if read == listed {
            return read;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the read reported {read:?} and the listing {listed:?} for {arn}"
        );
        std::thread::sleep(LOOK_AGAIN_AFTER);
    }
}
