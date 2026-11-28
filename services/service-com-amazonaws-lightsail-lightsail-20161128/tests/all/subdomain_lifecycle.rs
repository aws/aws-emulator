// SPDX-License-Identifier: CC0-1.0

//! The Lightsail subdomain lifecycle — check, create, read, list, associate,
//! disassociate, transfer and delete — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: a subdomain is a stored resource of the account named by
//! one or two lowercase labels, published at that name under
//! `amazonlightsail.com`, which `CreateSubdomain` creates, `GetSubdomains`
//! reports, `UpdateSubdomainAssociation` moves onto and off a resource through
//! the transient states the clock settles, and `DeleteSubdomain` removes —
//! leaving a top-level name whose nested subdomains remain held for the account
//! in the `IMPLICIT` state.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

// The harness, the wire error code, the minted-id check, the not-found wording,
// the instance stand-up and the distribution reads this test target already
// exposes: a sibling suite drives the service through the same front door rather
// than standing up a second one.
use crate::disk_attachment::create_running_instance;
use crate::distribution_certificate_attachment::{distribution, wait_for_distribution_status};
use crate::instance_lifecycle::{does_not_exist_message, error_code, is_uuid_shaped, Emulator};

/// The `ResourceType` a subdomain reports, from the modelled `ResourceType`
/// enum's own `Subdomain` member.
const RESOURCE_TYPE_SUBDOMAIN: &str = "Subdomain";

/// The one Region the subdomain API is served from, which is also the Region a
/// subdomain reports in its location (`crate::state::SUBDOMAIN_REGION_NAME`).
const SUBDOMAIN_REGION: &str = "us-east-1";

/// A Region that is NOT the one those APIs are served from, for the case that
/// asserts the documented restriction.
const OTHER_REGION: &str = "us-west-2";

/// The domain a subdomain's web address is published under, from the modelled
/// `Subdomain` documentation ("Its web address is the subdomain followed by
/// `amazonlightsail.com`").
const WEB_ADDRESS_DOMAIN: &str = "amazonlightsail.com";

/// The `SubdomainState` values this suite asserts, from the modelled enum.
const CREATING: &str = "CREATING";
const UNATTACHED: &str = "UNATTACHED";
const ATTACHING: &str = "ATTACHING";
const ATTACHED: &str = "ATTACHED";
const TRANSFERRING: &str = "TRANSFERRING";
const IMPLICIT: &str = "IMPLICIT";

/// The `LightsailDistributionSubdomainStateEnum` values this suite asserts.
const DISTRIBUTION_SUBDOMAIN_ATTACHED: &str = "Attached";

/// The value Lightsail puts in a not-found body's `code` member, which a
/// Lightsail SDK client branches on rather than on the HTTP status.
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// The bundle every fixture distribution is created on, an id of the service's
/// published CDN plans (`GetDistributionBundles`).
const DISTRIBUTION_BUNDLE_ID: &str = "small_1_0";

/// The `status` a distribution settles into once its configuration is rolled
/// out.
const DEPLOYED: &str = "Deployed";

// ---------------------------------------------------------------------------
// Reading a subdomain back
// ---------------------------------------------------------------------------

/// The HTTP status `GetSubdomains` answers for the subdomain `name`, and the
/// record it reports — `Value::Null` when this account holds no subdomain of
/// that name.
async fn subdomain_entry(emulator: &Emulator, name: &str) -> (u16, Value) {
    let (status, body) = emulator
        .call("GetSubdomains", json!({"subdomainName": name}))
        .await;
    let record = body["subdomains"]
        .as_array()
        .and_then(|subdomains| subdomains.first())
        .cloned()
        .unwrap_or(Value::Null);
    (status, record)
}

/// The subdomain `name`, as `GetSubdomains` reports it when a request names it —
/// the read that carries the subdomain's tags.
async fn subdomain(emulator: &Emulator, name: &str) -> Value {
    let body = emulator
        .ok("GetSubdomains", json!({"subdomainName": name}))
        .await;
    body["subdomains"]
        .as_array()
        .and_then(|subdomains| subdomains.first())
        .cloned()
        .unwrap_or(Value::Null)
}

/// Poll `GetSubdomains` until the subdomain `name` reports `want`, answering the
/// whole record.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for_subdomain_state(emulator: &Emulator, name: &str, want: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let reported = subdomain(emulator, name).await;
        if reported["state"].as_str() == Some(want) {
            return reported;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for subdomain {name}: expected '{want}' within 10s, got {reported:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Create the subdomain `name` with `tags` and wait until it is ready and
/// associated with nothing.
async fn create_ready_subdomain(emulator: &Emulator, name: &str, tags: Value) -> Value {
    let mut body = json!({"subdomainName": name});
    if !tags.is_null() {
        body["tags"] = tags;
    }
    emulator.ok("CreateSubdomain", body).await;
    wait_for_subdomain_state(emulator, name, UNATTACHED).await
}

/// Create the distribution `name` over the origin `origin` and wait until it is
/// deployed, answering its ARN.
async fn create_deployed_distribution(emulator: &Emulator, name: &str, origin: &str) -> String {
    emulator
        .ok(
            "CreateDistribution",
            json!({
                "distributionName": name,
                "origin": {"name": origin, "regionName": SUBDOMAIN_REGION},
                "defaultCacheBehavior": {"behavior": "cache"},
                "bundleId": DISTRIBUTION_BUNDLE_ID,
            }),
        )
        .await;
    let deployed = wait_for_distribution_status(emulator, name, DEPLOYED).await;
    deployed["arn"]
        .as_str()
        .expect("a created distribution carries an ARN")
        .to_string()
}

// ---------------------------------------------------------------------------
// The name rule
// ---------------------------------------------------------------------------

/// One subdomain name and whether Lightsail serves it.
///
/// The rows differ along everything the documented name rule generalises over:
/// the number of labels, the length (at the published bound and one past it),
/// which characters a label holds, where a hyphen or a digit sits in a label,
/// the reserved `xn--` prefix, and a web address sent where a subdomain belongs.
/// SOURCE: the modelled `CheckSubdomainAvailabilityRequest$subdomainName`
/// documentation.
struct NameRow {
    case: &'static str,
    name: &'static str,
    served: bool,
}

/// The longest name the rule admits: 39 characters including the period.
const LONGEST_NAME: &str = "blog0123456789012345.mysite012345678901";
/// One character past that bound.
const TOO_LONG_NAME: &str = "blog0123456789012345.mysite0123456789012";

const NAME_ROWS: &[NameRow] = &[
    NameRow {
        case: "one label",
        name: "mysite",
        served: true,
    },
    NameRow {
        case: "two labels",
        name: "blog.mysite",
        served: true,
    },
    NameRow {
        case: "digits and an inner hyphen",
        name: "my-site2.a1-b2",
        served: true,
    },
    NameRow {
        case: "a single character",
        name: "x",
        served: true,
    },
    NameRow {
        case: "exactly the published length bound",
        name: LONGEST_NAME,
        served: true,
    },
    NameRow {
        case: "one character past the published length bound",
        name: TOO_LONG_NAME,
        served: false,
    },
    NameRow {
        case: "three labels",
        name: "a.blog.mysite",
        served: false,
    },
    NameRow {
        case: "an upper-case letter",
        name: "MySite",
        served: false,
    },
    NameRow {
        case: "a leading hyphen",
        name: "-mysite",
        served: false,
    },
    NameRow {
        case: "a trailing hyphen",
        name: "mysite-",
        served: false,
    },
    NameRow {
        case: "an underscore",
        name: "my_site",
        served: false,
    },
    NameRow {
        case: "an empty label",
        name: "blog.",
        served: false,
    },
    NameRow {
        case: "the reserved xn-- prefix",
        name: "xn--mysite",
        served: false,
    },
    NameRow {
        case: "a web address rather than a subdomain",
        name: "mysite.amazonlightsail.com",
        served: false,
    },
];

/// RULE: a subdomain name carries one or two lowercase labels of letters,
/// digits and inner hyphens within the published length, so every other name is
/// refused with the modelled `InvalidInputException` and creates nothing —
/// whichever call it is sent to.
#[tokio::test]
async fn only_a_documented_subdomain_name_is_served() {
    let emulator = Emulator::new();

    for row in NAME_ROWS {
        let case = row.case;
        for operation in ["CheckSubdomainAvailability", "CreateSubdomain", "GetSubdomains"] {
            let (status, body) = emulator
                .call(operation, json!({"subdomainName": row.name}))
                .await;
            if row.served {
                // A name the rule admits is SERVED — whether the account holds
                // such a subdomain is the operation's own answer, which the
                // other cases pin: the read of a name nothing was created
                // under is the not-found envelope, never the name refusal.
                let refused_for_the_name = status == 400
                    && error_code(&body) == "InvalidInputException";
                assert!(
                    !refused_for_the_name,
                    "case {case} on {operation}: the name '{}' is one Lightsail serves, got \
                     {status} {body:?}",
                    row.name
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {case} on {operation}: expected HTTP 400, got {status} {body:?}"
                );
                assert_eq!(
                    error_code(&body),
                    "InvalidInputException",
                    "case {case} on {operation}: expected the modelled input refusal, got \
                     {body:?}"
                );
                let message = body["message"].as_str().unwrap_or_default();
                assert!(
                    message.contains(row.name),
                    "case {case} on {operation}: the refusal must name the subdomain it \
                     refused, got {message:?}"
                );
            }
        }
        // Nothing the refused names asked for was created.
        if !row.served {
            let (status, body) = emulator
                .call("GetSubdomains", json!({}))
                .await;
            assert_eq!(status, 200, "case {case}: listing must succeed, got {body:?}");
            let listed: Vec<&str> = body["subdomains"]
                .as_array()
                .expect("the listing reports an array")
                .iter()
                .filter_map(|entry| entry["subdomainName"].as_str())
                .collect();
            assert!(
                !listed.contains(&row.name),
                "case {case}: a refused create stored nothing, got {listed:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Create and read back
// ---------------------------------------------------------------------------

/// One `CreateSubdomain` request, and what the rule says the stored subdomain
/// then reports. The rows differ along what the rule generalises over: the depth
/// of the name (and so its parent and the implicit record a nested name leaves),
/// the form of the labels, and the number of tags.
struct CreateRow {
    case: &'static str,
    name: &'static str,
    tags: &'static [(&'static str, &'static str)],
    depth: i64,
    parent: Option<&'static str>,
}

const CREATE_ROWS: &[CreateRow] = &[
    CreateRow {
        case: "a top-level subdomain with no tags",
        name: "mysite",
        tags: &[],
        depth: 1,
        parent: None,
    },
    CreateRow {
        case: "a top-level subdomain with two tags",
        name: "shop-2",
        tags: &[("env", "prod"), ("team", "web")],
        depth: 1,
        parent: None,
    },
    CreateRow {
        case: "a nested subdomain under a name the account already holds",
        name: "blog.mysite",
        tags: &[("env", "staging")],
        depth: 2,
        parent: Some("mysite"),
    },
    CreateRow {
        case: "a nested subdomain under a name the account does not hold",
        name: "docs.handbook",
        tags: &[],
        depth: 2,
        parent: Some("handbook"),
    },
];

/// RULE: `CreateSubdomain` stores the name and tags the request carried and
/// assigns the ARN, the creation time, the web address, the depth and the
/// location, answers the subdomain being created together with one non-terminal
/// operation-log entry, and the clock settles it into the state of a subdomain
/// that exists and is associated with nothing — with the top-level parent of a
/// nested name held for the account where the account did not hold it already.
#[tokio::test]
async fn a_created_subdomain_reads_back_as_it_was_created() {
    let emulator = Emulator::new();

    for row in CREATE_ROWS {
        let case = row.case;
        let tags: Vec<Value> = row
            .tags
            .iter()
            .map(|(key, value)| json!({"key": key, "value": value}))
            .collect();
        let mut body = json!({"subdomainName": row.name});
        if !tags.is_empty() {
            body["tags"] = json!(tags);
        }
        let created = emulator.ok("CreateSubdomain", body).await;

        // The create's own answer: the subdomain is being created, and the log
        // entry for it is not terminal yet.
        let answered = &created["subdomain"];
        assert_eq!(
            answered["subdomainName"].as_str(),
            Some(row.name),
            "case {case}: the create answers the subdomain it created, got {created:?}"
        );
        assert_eq!(
            answered["state"].as_str(),
            Some(CREATING),
            "case {case}: a fresh subdomain is being created, got {created:?}"
        );
        assert_eq!(
            answered["fqdn"].as_str(),
            Some(format!("{}.{WEB_ADDRESS_DOMAIN}", row.name).as_str()),
            "case {case}: the web address is the subdomain under the published domain, got \
             {created:?}"
        );
        assert_eq!(
            answered["depth"].as_i64(),
            Some(row.depth),
            "case {case}: the depth counts the labels, got {created:?}"
        );
        assert_eq!(
            answered["parentFqdn"].as_str(),
            row.parent
                .map(|parent| format!("{parent}.{WEB_ADDRESS_DOMAIN}"))
                .as_deref(),
            "case {case}: only a nested subdomain reports a parent web address, got {created:?}"
        );
        assert_eq!(
            answered["location"]["regionName"].as_str(),
            Some(SUBDOMAIN_REGION),
            "case {case}: a subdomain lives in the Region its API is served from, got {created:?}"
        );
        let arn = answered["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: a created subdomain carries an ARN"));
        let id = arn
            .rsplit('/')
            .next()
            .unwrap_or_else(|| panic!("case {case}: an ARN ends in the resource id"));
        assert!(
            arn.contains(&format!(":lightsail:{SUBDOMAIN_REGION}:"))
                && arn.contains(&format!(":{RESOURCE_TYPE_SUBDOMAIN}/"))
                && is_uuid_shaped(id),
            "case {case}: the ARN names this service, Region and resource type with a minted \
             id, got {arn}"
        );
        assert!(
            answered["createdAt"].as_f64().is_some(),
            "case {case}: a created subdomain carries its creation time, got {created:?}"
        );
        // "This parameter is returned only by the GetSubdomains action when you
        // specify a subdomain", so the create answers none.
        assert!(
            created["subdomain"]["tags"].is_null(),
            "case {case}: the create answers no tags, got {created:?}"
        );

        let operation = &created["operation"];
        assert_eq!(
            (
                operation["operationType"].as_str(),
                operation["resourceType"].as_str(),
                operation["resourceName"].as_str(),
                operation["status"].as_str(),
                operation["isTerminal"].as_bool(),
            ),
            (
                Some("CreateSubdomain"),
                Some(RESOURCE_TYPE_SUBDOMAIN),
                Some(row.name),
                Some("Started"),
                Some(false),
            ),
            "case {case}: the log entry records the create of this subdomain, got {created:?}"
        );

        // The clock owes the subdomain its settled state.
        let settled = wait_for_subdomain_state(&emulator, row.name, UNATTACHED).await;
        assert_eq!(
            settled["arn"].as_str(),
            Some(arn),
            "case {case}: the settled record is the same resource, got {settled:?}"
        );
        assert_eq!(
            settled["tags"],
            if tags.is_empty() {
                json!([])
            } else {
                json!(tags)
            },
            "case {case}: a read of one subdomain reports the tags its create carried, got \
             {settled:?}"
        );
        assert!(
            settled["attachedResource"].is_null() && settled["attachingResource"].is_null(),
            "case {case}: a subdomain nothing has associated reports no resource, got {settled:?}"
        );

        // A nested name leaves its top-level parent held for the account, with
        // no ARN of its own until a create asks for that name.
        if let Some(parent) = row.parent {
            let held = subdomain(&emulator, parent).await;
            let state = held["state"].as_str();
            assert!(
                state == Some(IMPLICIT) || state == Some(UNATTACHED),
                "case {case}: the parent {parent} is held for the account, got {held:?}"
            );
            if state == Some(IMPLICIT) {
                assert!(
                    held["arn"].is_null(),
                    "case {case}: an implicit parent has no ARN yet, got {held:?}"
                );
            }
        }
    }
}

/// RULE: a name the account already holds a subdomain under is not one it can
/// create again, and the refusal changes nothing; a name it holds only
/// IMPLICITLY is exactly the name a create turns into a subdomain of its own.
#[tokio::test]
async fn a_held_name_cannot_be_created_again_and_an_implicit_one_can() {
    let emulator = Emulator::new();

    create_ready_subdomain(&emulator, "news.weekly", Value::Null).await;
    let implicit = subdomain(&emulator, "weekly").await;
    assert_eq!(
        implicit["state"].as_str(),
        Some(IMPLICIT),
        "case the parent of a nested create: expected the implicit state, got {implicit:?}"
    );

    // The nested name is held, so creating it again is refused.
    let (status, body) = emulator
        .call("CreateSubdomain", json!({"subdomainName": "news.weekly"}))
        .await;
    assert_eq!(
        status, 400,
        "case re-creating a held subdomain: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "case re-creating a held subdomain: expected the modelled input refusal, got {body:?}"
    );

    // The implicit parent is available, and creating it leaves a real
    // subdomain carrying an ARN, with the nested subdomain still there.
    let created = emulator
        .ok("CreateSubdomain", json!({"subdomainName": "weekly"}))
        .await;
    assert!(
        created["subdomain"]["arn"].as_str().is_some(),
        "case creating an implicit name: the subdomain now has an ARN, got {created:?}"
    );
    let settled = wait_for_subdomain_state(&emulator, "weekly", UNATTACHED).await;
    assert_eq!(
        settled["subdomainName"].as_str(),
        Some("weekly"),
        "case creating an implicit name: the record is the name that was created, got {settled:?}"
    );
    let nested = subdomain(&emulator, "news.weekly").await;
    assert_eq!(
        nested["state"].as_str(),
        Some(UNATTACHED),
        "case creating an implicit name: its nested subdomain is untouched, got {nested:?}"
    );
}

// ---------------------------------------------------------------------------
// Availability and reservations
// ---------------------------------------------------------------------------

/// RULE: a subdomain is available for this account to create exactly while the
/// account holds no subdomain of that name — a name it holds only IMPLICITLY is
/// available — and a reservation holds an available name for the account until
/// it expires, without being extended by a later check, and is released by the
/// create it was holding the name for.
#[tokio::test]
async fn availability_follows_the_store_and_a_reservation_holds_a_free_name() {
    let emulator = Emulator::new();

    // A name nothing holds is available, and no reservation is reported for it
    // until one is asked for.
    let free = emulator
        .ok(
            "CheckSubdomainAvailability",
            json!({"subdomainName": "fresh.site"}),
        )
        .await;
    assert_eq!(
        (
            free["available"].as_bool(),
            free["subdomainName"].as_str(),
            free["reservationExpiresAt"].as_f64()
        ),
        (Some(true), Some("fresh.site"), None),
        "case checking a free name: expected it available with no reservation, got {free:?}"
    );

    // Asking for the reservation reports when it expires; a later check of the
    // same subdomain answers the SAME expiry, because "checking the subdomain
    // again doesn't extend the reservation".
    let reserved = emulator
        .ok(
            "CheckSubdomainAvailability",
            json!({"subdomainName": "fresh.site", "reserve": true}),
        )
        .await;
    let expires_at = reserved["reservationExpiresAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("case reserving a free name: expected an expiry, got {reserved:?}"));
    let again = emulator
        .ok(
            "CheckSubdomainAvailability",
            json!({"subdomainName": "fresh.site", "reserve": true}),
        )
        .await;
    assert_eq!(
        again["reservationExpiresAt"].as_f64(),
        Some(expires_at),
        "case checking a reserved name again: the reservation is not extended, got {again:?}"
    );
    // The reservation is held under the TOP-LEVEL name, so the top-level name
    // reports it too.
    let top_level = emulator
        .ok(
            "CheckSubdomainAvailability",
            json!({"subdomainName": "site"}),
        )
        .await;
    assert_eq!(
        top_level["reservationExpiresAt"].as_f64(),
        Some(expires_at),
        "case checking the top-level name: a nested reservation holds it, got {top_level:?}"
    );

    // Creating the subdomain the reservation was holding releases it.
    create_ready_subdomain(&emulator, "fresh.site", Value::Null).await;
    let after_create = emulator
        .ok(
            "CheckSubdomainAvailability",
            json!({"subdomainName": "fresh.site"}),
        )
        .await;
    assert_eq!(
        (
            after_create["available"].as_bool(),
            after_create["reservationExpiresAt"].as_f64()
        ),
        (Some(false), None),
        "case checking a created name: expected it unavailable with the reservation released, \
         got {after_create:?}"
    );

    // A name the account holds only implicitly is available to create.
    let implicit = emulator
        .ok(
            "CheckSubdomainAvailability",
            json!({"subdomainName": "site"}),
        )
        .await;
    assert_eq!(
        implicit["available"].as_bool(),
        Some(true),
        "case checking an implicit name: expected it available, got {implicit:?}"
    );
}

/// RULE: the account holds at most the published number of active reservations
/// at a time, and one more is refused with the modelled
/// `OperationFailureException`.
#[tokio::test]
async fn the_published_reservation_quota_is_served() {
    /// "You can have up to 5 active reservations at a time" (the modelled
    /// `reserve` documentation).
    const MAX_RESERVATIONS: usize = 5;

    let emulator = Emulator::new();

    for index in 0..MAX_RESERVATIONS {
        let name = format!("hold{index}");
        let reserved = emulator
            .ok(
                "CheckSubdomainAvailability",
                json!({"subdomainName": name, "reserve": true}),
            )
            .await;
        assert!(
            reserved["reservationExpiresAt"].as_f64().is_some(),
            "case reserving {name}: expected an expiry, got {reserved:?}"
        );
    }

    let (status, body) = emulator
        .call(
            "CheckSubdomainAvailability",
            json!({"subdomainName": "onemore", "reserve": true}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case one reservation past the quota: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "OperationFailureException",
        "case one reservation past the quota: expected the modelled operation failure, got \
         {body:?}"
    );

    // A check that does NOT ask for a reservation is still served at the quota.
    let checked = emulator
        .ok(
            "CheckSubdomainAvailability",
            json!({"subdomainName": "onemore"}),
        )
        .await;
    assert_eq!(
        checked["available"].as_bool(),
        Some(true),
        "case checking without reserving at the quota: expected the name available, got \
         {checked:?}"
    );
}

// ---------------------------------------------------------------------------
// Listing
// ---------------------------------------------------------------------------

/// RULE: a listing that names no subdomain reports the account's TOP-LEVEL
/// subdomains, each carrying its nested subdomains, one page at a time in the
/// store's own order; a listing that names one reports that subdomain with its
/// tags and no children; and the two members cannot be sent together.
#[tokio::test]
async fn listing_reports_top_level_subdomains_with_their_children() {
    /// How many subdomains one page carries
    /// (`crate::state::SUBDOMAIN_PAGE_SIZE`).
    const PAGE_SIZE: usize = 10;
    /// Enough top-level subdomains to need a second page.
    const TOP_LEVEL: usize = PAGE_SIZE + 2;

    let emulator = Emulator::new();

    let mut expected: BTreeSet<String> = BTreeSet::new();
    for index in 0..TOP_LEVEL {
        let name = format!("site{index:02}");
        create_ready_subdomain(&emulator, &name, Value::Null).await;
        expected.insert(name);
    }
    // One nested subdomain, which the listing reports under its parent rather
    // than as an entry of its own.
    create_ready_subdomain(&emulator, "blog.site00", Value::Null).await;

    let mut listed: Vec<String> = Vec::new();
    let mut page_sizes: Vec<usize> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let body = match &token {
            Some(token) => emulator.ok("GetSubdomains", json!({"pageToken": token})).await,
            None => emulator.ok("GetSubdomains", json!({})).await,
        };
        let page = body["subdomains"]
            .as_array()
            .unwrap_or_else(|| panic!("case listing subdomains: expected an array, got {body:?}"))
            .clone();
        page_sizes.push(page.len());
        for entry in &page {
            let name = entry["subdomainName"]
                .as_str()
                .expect("every listed subdomain carries a name")
                .to_string();
            // "This parameter is returned only by the GetSubdomains action when
            // you specify a subdomain": a listing reports no tags.
            assert!(
                entry["tags"].is_null(),
                "case listing subdomains: a listing reports no tags, got {entry:?}"
            );
            let children: Vec<&str> = entry["children"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case listing subdomains: expected a children array, got {entry:?}")
                })
                .iter()
                .filter_map(|child| child["subdomainName"].as_str())
                .collect();
            if name == "site00" {
                assert_eq!(
                    children,
                    vec!["blog.site00"],
                    "case listing subdomains: a top-level subdomain carries its nested ones, \
                     got {entry:?}"
                );
            } else {
                assert!(
                    children.is_empty(),
                    "case listing subdomains: {name} has no nested subdomains, got {entry:?}"
                );
            }
            listed.push(name);
        }
        match body["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
        assert!(
            page_sizes.len() < 100,
            "case listing subdomains: the walk did not terminate after {} pages",
            page_sizes.len()
        );
    }
    assert_eq!(
        listed.iter().cloned().collect::<BTreeSet<String>>(),
        expected,
        "case listing subdomains: expected every top-level subdomain once, got {listed:?}"
    );
    assert_eq!(
        page_sizes,
        vec![PAGE_SIZE, TOP_LEVEL - PAGE_SIZE],
        "case listing subdomains: expected full pages then the remainder, got {page_sizes:?}"
    );

    // A named read reports that subdomain with its tags and no children.
    let named = subdomain(&emulator, "blog.site00").await;
    assert_eq!(
        (named["subdomainName"].as_str(), named["children"].clone()),
        (Some("blog.site00"), Value::Null),
        "case reading one subdomain: expected that record with no children, got {named:?}"
    );

    // "You can't specify this parameter together with the pageToken parameter."
    let (status, body) = emulator
        .call(
            "GetSubdomains",
            json!({"subdomainName": "site00", "pageToken": "site01"}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case naming a subdomain and a page token: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "case naming a subdomain and a page token: expected the modelled input refusal, got \
         {body:?}"
    );
}

// ---------------------------------------------------------------------------
// Association
// ---------------------------------------------------------------------------

/// RULE: associating a subdomain with a distribution leaves the subdomain in a
/// transient state the clock settles into `ATTACHED` with that resource on its
/// record, and the distribution reports the subdomain it is reachable at;
/// disassociating it releases both; and a transfer moves it from one
/// distribution to another.
#[tokio::test]
async fn an_association_moves_the_subdomain_onto_and_off_a_distribution() {
    let emulator = Emulator::new();

    create_running_instance(&emulator, "cdn-origin", &format!("{SUBDOMAIN_REGION}a")).await;
    let first_arn = create_deployed_distribution(&emulator, "first-cdn", "cdn-origin").await;
    let second_arn = create_deployed_distribution(&emulator, "second-cdn", "cdn-origin").await;
    create_ready_subdomain(&emulator, "store", Value::Null).await;

    // ASSOCIATE: the answer reports the transient state and the resource the
    // subdomain is being associated with.
    let associated = emulator
        .ok(
            "UpdateSubdomainAssociation",
            json!({
                "subdomainName": "store",
                "action": "ASSOCIATE",
                "targetResourceArn": first_arn,
            }),
        )
        .await;
    assert_eq!(
        (
            associated["subdomain"]["state"].as_str(),
            associated["subdomain"]["attachingResource"]["arn"].as_str(),
            associated["subdomain"]["attachingResource"]["name"].as_str(),
            associated["operation"]["operationType"].as_str(),
            associated["operation"]["isTerminal"].as_bool(),
        ),
        (
            Some(ATTACHING),
            Some(first_arn.as_str()),
            Some("first-cdn"),
            Some("UpdateSubdomainAssociation"),
            Some(false),
        ),
        "case associating a subdomain: expected the transient state and the target, got \
         {associated:?}"
    );

    let settled = wait_for_subdomain_state(&emulator, "store", ATTACHED).await;
    assert_eq!(
        (
            settled["attachedResource"]["arn"].as_str(),
            settled["attachedResource"]["name"].as_str(),
            settled["attachingResource"].clone(),
        ),
        (Some(first_arn.as_str()), Some("first-cdn"), Value::Null),
        "case an associated subdomain: expected the resource on its record, got {settled:?}"
    );
    assert!(
        settled["attachedResource"]["attachedAt"].as_f64().is_some(),
        "case an associated subdomain: expected the instant it was associated, got {settled:?}"
    );

    // The distribution reports the subdomain it is reachable at.
    let reported = distribution(&emulator, "first-cdn").await;
    assert_eq!(
        (
            reported["subdomain"]["name"].as_str(),
            reported["subdomain"]["state"].as_str(),
        ),
        (Some("store"), Some(DISTRIBUTION_SUBDOMAIN_ATTACHED)),
        "case an associated distribution: expected the subdomain it is reachable at, got \
         {reported:?}"
    );

    // TRANSFER: the subdomain moves to the other distribution, reporting the
    // resource it is moving from and the one it is moving to.
    let transferred = emulator
        .ok(
            "UpdateSubdomainAssociation",
            json!({
                "subdomainName": "store",
                "action": "TRANSFER",
                "targetResourceArn": second_arn,
            }),
        )
        .await;
    assert_eq!(
        (
            transferred["subdomain"]["state"].as_str(),
            transferred["subdomain"]["attachedResource"]["arn"].as_str(),
            transferred["subdomain"]["attachingResource"]["arn"].as_str(),
        ),
        (
            Some(TRANSFERRING),
            Some(first_arn.as_str()),
            Some(second_arn.as_str()),
        ),
        "case transferring a subdomain: expected both resources, got {transferred:?}"
    );
    let moved = wait_for_subdomain_state(&emulator, "store", ATTACHED).await;
    assert_eq!(
        moved["attachedResource"]["arn"].as_str(),
        Some(second_arn.as_str()),
        "case a transferred subdomain: expected the new resource, got {moved:?}"
    );
    let left_behind = distribution(&emulator, "first-cdn").await;
    assert!(
        left_behind["subdomain"].is_null(),
        "case the distribution a subdomain left: expected no subdomain, got {left_behind:?}"
    );

    // A second transfer within the published window is refused.
    let (status, body) = emulator
        .call(
            "UpdateSubdomainAssociation",
            json!({
                "subdomainName": "store",
                "action": "TRANSFER",
                "targetResourceArn": first_arn,
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case a second transfer within 24 hours: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "OperationFailureException",
        "case a second transfer within 24 hours: expected the modelled operation failure, got \
         {body:?}"
    );

    // DISASSOCIATE: the subdomain is released from the resource and is ready
    // again, and the distribution reports no subdomain.
    let disassociated = emulator
        .ok(
            "UpdateSubdomainAssociation",
            json!({"subdomainName": "store", "action": "DISASSOCIATE"}),
        )
        .await;
    assert_eq!(
        disassociated["subdomain"]["state"].as_str(),
        Some("DETACHING"),
        "case disassociating a subdomain: expected the transient state, got {disassociated:?}"
    );
    let released = wait_for_subdomain_state(&emulator, "store", UNATTACHED).await;
    assert!(
        released["attachedResource"].is_null() && released["attachingResource"].is_null(),
        "case a disassociated subdomain: expected no resource, got {released:?}"
    );
    let freed = distribution(&emulator, "second-cdn").await;
    assert!(
        freed["subdomain"].is_null(),
        "case a disassociated distribution: expected no subdomain, got {freed:?}"
    );
}

/// One association request the service's own rules do not allow, and the
/// modelled failure it is answered with.
struct RefusalRow {
    case: &'static str,
    action: &'static str,
    /// Whether the request names the fixture distribution as its target.
    target: bool,
    /// Which subdomain the request names: the one associated with the
    /// distribution, the one that is free, or a name nothing holds.
    subdomain: &'static str,
    error: &'static str,
}

const ASSOCIATION_REFUSALS: &[RefusalRow] = &[
    RefusalRow {
        case: "associating a subdomain that is already associated",
        action: "ASSOCIATE",
        target: true,
        subdomain: "taken",
        error: "OperationFailureException",
    },
    RefusalRow {
        case: "associating without naming the resource",
        action: "ASSOCIATE",
        target: false,
        subdomain: "free",
        error: "InvalidInputException",
    },
    RefusalRow {
        case: "transferring without naming the resource",
        action: "TRANSFER",
        target: false,
        subdomain: "taken",
        error: "InvalidInputException",
    },
    RefusalRow {
        case: "disassociating a subdomain that is associated with nothing",
        action: "DISASSOCIATE",
        target: false,
        subdomain: "free",
        error: "OperationFailureException",
    },
    RefusalRow {
        case: "associating a subdomain the account does not hold",
        action: "ASSOCIATE",
        target: true,
        subdomain: "nothing-holds-this",
        error: "NotFoundException",
    },
    RefusalRow {
        case: "associating a resource that is already associated with a subdomain",
        action: "ASSOCIATE",
        target: true,
        subdomain: "free",
        error: "OperationFailureException",
    },
];

/// RULE: a request the two resources' states do not allow is refused with the
/// modelled failure for that rule and changes nothing — the subdomain still
/// reports the state and the resource it did before the call.
#[tokio::test]
async fn an_association_the_rules_do_not_allow_changes_nothing() {
    let emulator = Emulator::new();

    create_running_instance(&emulator, "refusal-origin", &format!("{SUBDOMAIN_REGION}a")).await;
    let arn = create_deployed_distribution(&emulator, "refusal-cdn", "refusal-origin").await;
    create_ready_subdomain(&emulator, "taken", Value::Null).await;
    create_ready_subdomain(&emulator, "free", Value::Null).await;
    emulator
        .ok(
            "UpdateSubdomainAssociation",
            json!({
                "subdomainName": "taken",
                "action": "ASSOCIATE",
                "targetResourceArn": arn,
            }),
        )
        .await;
    wait_for_subdomain_state(&emulator, "taken", ATTACHED).await;

    for row in ASSOCIATION_REFUSALS {
        let case = row.case;
        let (_, before) = subdomain_entry(&emulator, row.subdomain).await;
        let mut body = json!({"subdomainName": row.subdomain, "action": row.action});
        if row.target {
            body["targetResourceArn"] = json!(arn);
        }
        let (status, answered) = emulator.call("UpdateSubdomainAssociation", body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            row.error,
            "case {case}: expected {}, got {answered:?}",
            row.error
        );
        let (_, after) = subdomain_entry(&emulator, row.subdomain).await;
        assert_eq!(
            (after["state"].clone(), after["attachedResource"].clone()),
            (before["state"].clone(), before["attachedResource"].clone()),
            "case {case}: a refused request changes nothing, got {after:?}"
        );
    }

    // A target ARN that names no distribution of this account is the
    // `DoesNotExist` envelope.
    let (status, body) = emulator
        .call(
            "UpdateSubdomainAssociation",
            json!({
                "subdomainName": "free",
                "action": "ASSOCIATE",
                "targetResourceArn": "arn:aws:lightsail:us-east-1:000000000000:Distribution/none",
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body), body["code"].as_str()),
        (400, "NotFoundException".to_string(), Some(DOES_NOT_EXIST)),
        "case a target ARN naming nothing: expected the DoesNotExist envelope, got {body:?}"
    );
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

/// RULE: a subdomain is deletable only while it is associated with nothing, a
/// delete removes it from every read, and deleting a top-level subdomain that
/// has nested subdomains leaves the name held for the account in the `IMPLICIT`
/// state and answers that record.
#[tokio::test]
async fn deleting_a_subdomain_removes_it_and_keeps_a_parent_of_nested_ones() {
    let emulator = Emulator::new();

    create_running_instance(&emulator, "delete-origin", &format!("{SUBDOMAIN_REGION}a")).await;
    let arn = create_deployed_distribution(&emulator, "delete-cdn", "delete-origin").await;
    create_ready_subdomain(&emulator, "attached", Value::Null).await;
    create_ready_subdomain(&emulator, "plain", Value::Null).await;
    create_ready_subdomain(&emulator, "parent", Value::Null).await;
    create_ready_subdomain(&emulator, "child.parent", Value::Null).await;
    emulator
        .ok(
            "UpdateSubdomainAssociation",
            json!({
                "subdomainName": "attached",
                "action": "ASSOCIATE",
                "targetResourceArn": arn,
            }),
        )
        .await;
    wait_for_subdomain_state(&emulator, "attached", ATTACHED).await;

    // An associated subdomain is not deletable.
    let (status, body) = emulator
        .call("DeleteSubdomain", json!({"subdomainName": "attached"}))
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "OperationFailureException".to_string()),
        "case deleting an associated subdomain: expected the modelled operation failure, got \
         {body:?}"
    );
    assert_eq!(
        subdomain(&emulator, "attached").await["state"].as_str(),
        Some(ATTACHED),
        "case deleting an associated subdomain: the refusal changed nothing"
    );

    // A subdomain with no nested subdomains is removed outright, and the delete
    // answers a terminal log entry and no subdomain.
    let deleted = emulator
        .ok("DeleteSubdomain", json!({"subdomainName": "plain"}))
        .await;
    assert_eq!(
        (
            deleted["subdomain"].clone(),
            deleted["operation"]["operationType"].as_str(),
            deleted["operation"]["resourceType"].as_str(),
            deleted["operation"]["isTerminal"].as_bool(),
            deleted["operation"]["status"].as_str(),
        ),
        (
            Value::Null,
            Some("DeleteSubdomain"),
            Some(RESOURCE_TYPE_SUBDOMAIN),
            Some(true),
            Some("Succeeded"),
        ),
        "case deleting a plain subdomain: expected no subdomain and a terminal entry, got \
         {deleted:?}"
    );
    let (status, body) = emulator
        .call("GetSubdomains", json!({"subdomainName": "plain"}))
        .await;
    assert_eq!(
        (
            status,
            error_code(&body),
            body["code"].as_str(),
            body["message"].as_str()
        ),
        (
            400,
            "NotFoundException".to_string(),
            Some(DOES_NOT_EXIST),
            Some(does_not_exist_message(RESOURCE_TYPE_SUBDOMAIN, "plain").as_str())
        ),
        "case reading a deleted subdomain: expected the DoesNotExist envelope, got {body:?}"
    );
    // The name is available again.
    let available = emulator
        .ok(
            "CheckSubdomainAvailability",
            json!({"subdomainName": "plain"}),
        )
        .await;
    assert_eq!(
        available["available"].as_bool(),
        Some(true),
        "case checking a deleted name: expected it available again, got {available:?}"
    );

    // A top-level subdomain with nested subdomains stays held, implicitly, and
    // the nested subdomain keeps working.
    let deleted_parent = emulator
        .ok("DeleteSubdomain", json!({"subdomainName": "parent"}))
        .await;
    assert_eq!(
        (
            deleted_parent["subdomain"]["subdomainName"].as_str(),
            deleted_parent["subdomain"]["state"].as_str(),
            deleted_parent["subdomain"]["arn"].clone(),
        ),
        (Some("parent"), Some(IMPLICIT), Value::Null),
        "case deleting a parent of nested subdomains: expected the implicit record, got \
         {deleted_parent:?}"
    );
    assert_eq!(
        subdomain(&emulator, "child.parent").await["state"].as_str(),
        Some(UNATTACHED),
        "case deleting a parent of nested subdomains: the nested one continues to work"
    );
    // An implicit subdomain is one no call can delete.
    let (status, body) = emulator
        .call("DeleteSubdomain", json!({"subdomainName": "parent"}))
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "OperationFailureException".to_string()),
        "case deleting an implicit subdomain: expected the modelled operation failure, got \
         {body:?}"
    );

    // A name the account does not hold at all is the DoesNotExist envelope on
    // every subdomain call that addresses one.
    for operation in ["DeleteSubdomain", "GetSubdomains"] {
        let (status, body) = emulator
            .call(operation, json!({"subdomainName": "never-held"}))
            .await;
        assert_eq!(
            (status, error_code(&body), body["code"].as_str()),
            (400, "NotFoundException".to_string(), Some(DOES_NOT_EXIST)),
            "case {operation} on a name nothing holds: expected the DoesNotExist envelope, got \
             {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The Region the subdomain API is served from
// ---------------------------------------------------------------------------

/// RULE: the subdomain calls are served by the N. Virginia endpoint alone, so a
/// request signed for another Region is refused with the modelled
/// `InvalidInputException` naming that Region, whichever subdomain call it is.
#[tokio::test]
async fn the_subdomain_api_is_served_from_one_region() {
    let emulator = Emulator::new();
    create_ready_subdomain(&emulator, "regional", Value::Null).await;

    for (operation, body) in [
        (
            "CheckSubdomainAvailability",
            json!({"subdomainName": "regional"}),
        ),
        ("CreateSubdomain", json!({"subdomainName": "elsewhere"})),
        ("GetSubdomains", json!({})),
        (
            "UpdateSubdomainAssociation",
            json!({"subdomainName": "regional", "action": "DISASSOCIATE"}),
        ),
        ("DeleteSubdomain", json!({"subdomainName": "regional"})),
    ] {
        let (status, answered) = emulator
            .call_in(Some(OTHER_REGION), operation, body)
            .await;
        assert_eq!(
            status, 400,
            "case {operation} in {OTHER_REGION}: expected HTTP 400, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "InvalidInputException",
            "case {operation} in {OTHER_REGION}: expected the modelled input refusal, got \
             {answered:?}"
        );
        assert!(
            answered["message"]
                .as_str()
                .unwrap_or_default()
                .contains(SUBDOMAIN_REGION),
            "case {operation} in {OTHER_REGION}: the refusal names the Region these APIs are \
             served from, got {answered:?}"
        );
    }

    // The subdomain is still there, and still ready: nothing the refused calls
    // asked for was done.
    assert_eq!(
        subdomain(&emulator, "regional").await["state"].as_str(),
        Some(UNATTACHED),
        "case the refused calls: the subdomain is untouched"
    );
}
