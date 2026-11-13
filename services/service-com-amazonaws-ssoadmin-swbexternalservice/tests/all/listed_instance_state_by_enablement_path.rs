// SPDX-License-Identifier: CC0-1.0

//! The `State` view an enumerated IAM Identity Center instance carries, over the way the
//! SESSION itself established that instance, on the `awsJson1_1` wire.
//!
//! THE RULE: the instance projection this service serves is read off the stored instance
//! record, so every entry of every `ListInstances` page — and the `Instance` envelope
//! `DescribeInstance` repeats for the same ARN — carries a `State` structure whose single
//! `Name` member is that record's own `Status`, whatever call put the record in the store.
//!
//! The sibling suites pin the rule over records the harness SEEDED, or over one instance a
//! single `CreateInstance` minted: `instance_state_view.rs` that the member is served and
//! mirrors the status through the lifecycle, `listed_instance_state_source.rs` the record it
//! is read from, `listed_instance_state_every_entry.rs` the cardinality of the enumeration,
//! `listed_instance_state_through_a_call_sequence.rs` the length and order of a call
//! sequence and `listed_instance_projection_matches_describe.rs` the two projections against
//! each other. The axis this suite adds is the ENABLEMENT PATH — how the account came to
//! hold the instance the listing then serves:
//!
//! * the ambient instance a client discovers with no create at all, where the enumeration
//!   is itself the first call of the session;
//! * `CreateInstance` carrying no `Name` at all, the member being optional
//!   (`smithy_query(query="operation",operation="CreateInstance")`: `Name` targets
//!   `NameType`, `length 0..255`, `pattern ^[\w+=,.@-]+$`);
//! * `CreateInstance` carrying a caller-generated word of that class;
//! * `CreateInstance` carrying a `Name` at that member's published upper length bound;
//! * `CreateInstance` carrying a `ClientToken` (`length 1..64`, `pattern ^[!-~]+$`) plus the
//!   idempotent RETRY of the same create, so the listing follows two creates and one record;
//! * `CreateInstance` followed by a second create the account's one-instance quota refuses,
//!   so the listing follows a served create AND a refused one.
//!
//! Each path is read at every page size the rows span — both published bounds of the
//! operation's `MaxResults` (`range 1..100`) and the request that names none — and, where
//! the session minted the instance, on both sides of the activation the create owes. A
//! projection that serves the member only for a record the harness planted, or only for the
//! account's ambient instance, fails here.
//!
//! Every call goes through `MultiServiceRouter::dispatch`; the harness is the one
//! `ssoadmin_permission_sets.rs` owns, the enumeration and the rule assertion the ones
//! `instance_state_view.rs` owns, and the empty account and the held activation instant the
//! ones `instance_create.rs` owns. Where a phase lives on the far side of a due instant the
//! test moves the STORED instant rather than sleeping — the transition is still applied by
//! the service's own `materialize_due`, exactly as in production.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::instance_create::{empty_account_router, hold_activation};
use crate::instance_state_view::{assert_state_mirrors_status, listed};
use crate::ssoadmin_permission_sets::{
    assert_refused, call_in_region, router_over, served_in_region,
};

/// The Region every instance below is enabled in and every call is signed for: a
/// `RegionName` (`^([a-z]+-){2,3}\d$`) none of the sibling suites drives.
const REGION: &str = "ca-central-1";

/// The page sizes the rows ask for: both published bounds of the operation's `MaxResults`
/// (`range 1..100`), and the request that names none.
const PAGE_SIZES: [Option<i64>; 3] = [None, Some(1), Some(100)];

/// `NameType`'s published upper length bound (`length 0..255`), the length the
/// longest-named row creates its instance with.
const NAME_MAX: usize = 255;

/// The `ClientToken` the idempotent-retry row sends twice: a word of `ClientToken`'s own
/// class (`length 1..64`, `pattern ^[!-~]+$`).
const CLIENT_TOKEN: &str = "enablement-path-token-1";

/// How the session established the instance the enumeration then serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Enablement {
    /// No create at all: the account's ambient instance, discovered by the first call of
    /// the session.
    AmbientDiscovery,
    /// `CreateInstance` carrying no `Name` member.
    CreateWithoutName,
    /// `CreateInstance` carrying a caller-generated word of `NameType`'s class.
    CreateWithName,
    /// `CreateInstance` carrying a `Name` at `NameType`'s upper length bound.
    CreateWithNameAtUpperBound,
    /// `CreateInstance` carrying a `ClientToken`, then the idempotent retry of that very
    /// request, which answers with the instance the first call created.
    CreateThenIdempotentRetry,
    /// `CreateInstance`, then a second create the account's one-instance quota refuses.
    CreateThenRefusedSecondCreate,
}

/// The rows, in the order they run.
const PATHS: [Enablement; 6] = [
    Enablement::AmbientDiscovery,
    Enablement::CreateWithoutName,
    Enablement::CreateWithName,
    Enablement::CreateWithNameAtUpperBound,
    Enablement::CreateThenIdempotentRetry,
    Enablement::CreateThenRefusedSecondCreate,
];

impl Enablement {
    /// Whether this path MINTS the instance through `CreateInstance` — the paths that leave
    /// an activation owed on the record, so the entry can be read on both sides of it.
    fn mints(self) -> bool {
        self != Enablement::AmbientDiscovery
    }

    /// The `Name` the create sends, or `None` for the request that carries none.
    ///
    /// The words are a caller's own, a prefix and a hex suffix of `NameType`'s
    /// `^[\w+=,.@-]+$`, so nothing the view reports can be keyed on a name the service
    /// chose.
    fn created_name(self) -> Option<String> {
        match self {
            Enablement::AmbientDiscovery | Enablement::CreateWithoutName => None,
            Enablement::CreateWithName => Some("enabled-idc-3f0a91c7".to_string()),
            Enablement::CreateWithNameAtUpperBound => {
                let prefix = "enabled-idc-at-the-bound-";
                Some(format!(
                    "{prefix}{}",
                    "b".repeat(NAME_MAX - prefix.chars().count())
                ))
            }
            Enablement::CreateThenIdempotentRetry => Some("enabled-idc-retried-7c21".to_string()),
            Enablement::CreateThenRefusedSecondCreate => {
                Some("enabled-idc-first-of-two".to_string())
            }
        }
    }

    /// The `Name` the entry must report: what the create sent, or the ambient instance's
    /// own name for the path that creates nothing.
    fn expected_name(self) -> Option<String> {
        match self {
            Enablement::AmbientDiscovery => Some(state::SEED_INSTANCE_NAME.to_string()),
            other => other.created_name(),
        }
    }
}

/// What a row's session left behind: the router it was driven over, the engine under it and
/// the instance the account then holds.
struct Established {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    instance_arn: String,
}

/// Run the row's session and answer what it established.
///
/// Everything here goes over the wire except the account state a create is documented for
/// (an account holding no instance), which is the sibling suite's own helper.
async fn establish(case: &str, path: Enablement) -> Established {
    if path == Enablement::AmbientDiscovery {
        // Nothing is seeded and nothing has decided the account's enablement, so the first
        // call of the session is what enables the account's ambient instance — and that
        // first call is the enumeration this suite is about.
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        return Established {
            router: router_over(Arc::clone(&engine)),
            engine,
            instance_arn: state::instance_arn(state::SEED_INSTANCE_ID),
        };
    }

    let (router, engine) = empty_account_router();
    let mut request = json!({});
    if let Some(name) = path.created_name() {
        request["Name"] = json!(name);
    }
    if path == Enablement::CreateThenIdempotentRetry {
        request["ClientToken"] = json!(CLIENT_TOKEN);
    }
    let created = served_in_region(case, &router, REGION, "CreateInstance", &request).await;
    let instance_arn = created["InstanceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no InstanceArn in {created}"))
        .to_string();

    match path {
        Enablement::CreateThenIdempotentRetry => {
            let again = served_in_region(case, &router, REGION, "CreateInstance", &request).await;
            assert_eq!(
                again["InstanceArn"].as_str(),
                Some(instance_arn.as_str()),
                "case {case:?}: the retry of the same create answers with the instance it \
                 created, got {again}"
            );
        }
        Enablement::CreateThenRefusedSecondCreate => {
            let (status, body) = call_in_region(
                &router,
                REGION,
                "CreateInstance",
                &json!({"Name": "enabled-idc-second-of-two"}),
            )
            .await;
            assert_refused(
                case,
                "a second CreateInstance on an account that already holds one",
                "ServiceQuotaExceededException",
                status,
                &body,
            );
        }
        _ => {}
    }

    Established {
        router,
        engine,
        instance_arn,
    }
}

/// THE RULE on the one instance the session established, read at `max_results`: the entry
/// the enumeration serves carries a `State` structure whose `Name` is that entry's own
/// `Status`, and the single-instance read of the same ARN repeats that very structure.
async fn assert_state_served(
    case: &str,
    established: &Established,
    expected_name: Option<&str>,
    expected_status: &str,
    max_results: Option<i64>,
) {
    let entries = listed(case, &established.router, REGION, max_results).await;
    assert_eq!(
        entries.len(),
        1,
        "case {case:?}: the account's one instance is enumerated, got {entries:?}"
    );
    let entry = &entries[0];
    assert_eq!(
        entry["InstanceArn"].as_str(),
        Some(established.instance_arn.as_str()),
        "case {case:?}: expected the instance the session established, got {entry}"
    );
    assert_eq!(
        entry.get("Name").and_then(Value::as_str),
        expected_name,
        "case {case:?}: expected Name {expected_name:?}, got {entry}"
    );
    assert_state_mirrors_status(case, entry);
    assert_eq!(
        entry["Status"].as_str(),
        Some(expected_status),
        "case {case:?}: expected Status {expected_status:?}, got {entry}"
    );
    assert_eq!(
        entry["State"]["Name"].as_str(),
        Some(expected_status),
        "case {case:?}: expected State.Name {expected_status:?}, got {entry}"
    );

    let described = served_in_region(
        case,
        &established.router,
        REGION,
        "DescribeInstance",
        &json!({"InstanceArn": established.instance_arn}),
    )
    .await;
    // The envelope's own view has to hold on its own merits, not only by agreeing with the
    // listed entry: a pair of reads that both omitted `State` would agree about nothing.
    assert_eq!(
        described["Instance"]["State"]["Name"].as_str(),
        Some(expected_status),
        "case {case:?}: expected the envelope's State.Name {expected_status:?}, got {described}"
    );
    assert_eq!(
        &described["Instance"], entry,
        "case {case:?}: the listed entry and the described envelope are one projection, got \
         {} beside {entry}",
        described["Instance"]
    );
}

/// THE RULE over the enablement paths: however the account came to hold its instance, every
/// entry the enumeration serves for it carries its `State` view, at every page size the rows
/// span and on both sides of the activation a create owes.
#[tokio::test]
async fn every_enablement_path_is_listed_with_its_state_view() {
    for path in PATHS {
        let case = format!("{path:?}");
        let established = establish(&case, path).await;
        let expected_name = path.expected_name();

        // Where the session MINTED the instance the clock still owes it an activation, so
        // the entry is read while the creation is in flight and again once the activation
        // has fallen due; the ambient instance has nothing owed on it and is read as the
        // session established it.
        let phases: &[(&str, &str)] = if path.mints() {
            &[
                ("creation in flight", state::STATUS_CREATE_IN_PROGRESS),
                ("activation due", state::STATUS_ACTIVE),
            ]
        } else {
            &[("as the session established it", state::STATUS_ACTIVE)]
        };

        for (phase, expected_status) in phases {
            if path.mints() {
                // A generous instant either side of now: the suite never waits for a span
                // to pass, it moves the stored instant the service's own clock compares
                // against.
                let due_at = if *expected_status == state::STATUS_CREATE_IN_PROGRESS {
                    SystemTime::now() + Duration::from_secs(3_600)
                } else {
                    SystemTime::now() - Duration::from_secs(3_600)
                };
                hold_activation(&established.engine, due_at);
            }
            for max_results in PAGE_SIZES {
                let case = format!("{case} / {phase} / MaxResults {max_results:?}");
                assert_state_served(
                    &case,
                    &established,
                    expected_name.as_deref(),
                    expected_status,
                    max_results,
                )
                .await;
            }
        }
    }
}
