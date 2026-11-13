// SPDX-License-Identifier: CC0-1.0

//! IAM's decision in front of the two collection reads that report the account's whole API
//! inventory, end to end over HTTP: `ListApis` (`GET /v2/apis`, output member `apis`) and
//! `ListGraphqlApis` (`GET /v1/apis`, output member `graphqlApis`)
//! (`smithy_query(query="operation", operation="ListApis,ListGraphqlApis")`).
//!
//! The rule under test: a collection read names no single resource, so the identity-based
//! decision taken in FRONT of this service judges it against every resource of this service
//! in the account and region the request addresses; a caller whose own identity-based
//! documents do not allow that listing's action there is refused with the modelled
//! `AccessDeniedException` (HTTP 403) and IAM's refusal sentence, and the refusal carries
//! NOTHING of what the account holds — no page, no api id, no name, no endpoint hostname
//! and no ARN. A caller the documents do allow is served the inventory, and the decision is
//! taken before anything is read, so it is the same whether the account holds APIs or none.
//!
//! The tables span what the rule generalises over: WHICH listing is asked for (two URI
//! spaces, two output members, two actions), the query string it carries (none, a page
//! size, the filters `ListGraphqlApis` publishes), the KIND of principal (an IAM user, an
//! assumed-role session, the account's own credential), WHAT its documents say (nothing at
//! all, an allow of the other listing's action, an allow of this action on one API rather
//! than on the account's resources, an explicit deny beside a wildcard allow) and what the
//! account HOLDS (two APIs, none).
//!
//! The principals are made with IAM's and STS's own operations over the same engine, so the
//! documents this service reads are the ones those services wrote; the harness is the
//! identity-authorization suite's.

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::service::authorization::{
    ACTION_PREFIX, ANY_RESOURCE,
};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;

use crate::identity_authorization::{
    allow, allow_all_but, arn, default_deny_message, Emulator, ACCESS_DENIED,
    ACCESS_DENIED_STATUS, ACCOUNT_KEY,
};

/// The name the one Event API of the inventory is created under.
const EVENT_API_NAME: &str = "Events_inventory";

/// The name the one GraphQL API of the inventory is created under.
const GRAPHQL_API_NAME: &str = "Graphql_inventory";

/// Which of the account's two API families a listing is the collection of, so the positive
/// control asserts each listing against the resources it reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    Event,
    Graphql,
}

/// One collection read under test: the operation (which is also the IAM action short name),
/// the family it reports, the output member it carries its page in, and the request forms it
/// is asked in — the bare collection, and query strings built from the members its own input
/// publishes.
///
/// Source: `smithy_query(query="operation", operation="ListApis,ListGraphqlApis")` —
/// `ListApis` is `GET /v2/apis` answering `apis`, with `maxResults` and `nextToken` in its
/// input; `ListGraphqlApis` is `GET /v1/apis` answering `graphqlApis`, with `maxResults`,
/// `nextToken`, `apiType` and `owner`.
struct Listing {
    operation: &'static str,
    family: Family,
    member: &'static str,
    paths: &'static [&'static str],
}

const LISTINGS: &[Listing] = &[
    Listing {
        operation: "ListApis",
        family: Family::Event,
        member: "apis",
        paths: &["/v2/apis", "/v2/apis?maxResults=25"],
    },
    Listing {
        operation: "ListGraphqlApis",
        family: Family::Graphql,
        member: "graphqlApis",
        paths: &[
            "/v1/apis",
            "/v1/apis?maxResults=25",
            "/v1/apis?apiType=GRAPHQL&owner=CURRENT_ACCOUNT",
        ],
    },
];

/// The OTHER listing's action: what a document allowing it must still not serve this one,
/// since the decision is about the action the request names.
fn other_listing(operation: &str) -> &'static str {
    LISTINGS
        .iter()
        .map(|listing| listing.operation)
        .find(|name| *name != operation)
        .expect("the table holds both listings")
}

/// What a refused caller's documents say, built from the listing's own action and from one
/// API the account holds.
struct Refused {
    case: &'static str,
    document: fn(&str, &str) -> Option<String>,
    /// Whether the principal is an assumed-role session rather than an IAM user — which is
    /// also what decides the ARN form the refusal blames it by.
    assumed_role: bool,
    /// Whether a document DENIES the action, which is the clause the refusal names.
    explicit_deny: bool,
}

/// No document at all: IAM's default deny.
fn nothing(_operation: &str, _api_id: &str) -> Option<String> {
    None
}

/// An allow of the other listing's action on everything.
fn the_other_listings_action(operation: &str, _api_id: &str) -> Option<String> {
    Some(allow(
        &format!("{ACTION_PREFIX}:{}", other_listing(operation)),
        ANY_RESOURCE,
    ))
}

/// An allow of THIS action, but on one API's own ARN rather than on the resources a
/// collection read is judged against.
fn this_action_on_one_api(operation: &str, api_id: &str) -> Option<String> {
    Some(allow(
        &format!("{ACTION_PREFIX}:{operation}"),
        &arn(&state::api_resource(api_id)),
    ))
}

/// A wildcard allow over this service with an explicit deny of this action beside it.
fn an_explicit_deny(operation: &str, _api_id: &str) -> Option<String> {
    Some(allow_all_but(&format!("{ACTION_PREFIX}:{operation}")))
}

const REFUSED: &[Refused] = &[
    Refused {
        case: "an IAM user whose documents allow nothing",
        document: nothing,
        assumed_role: false,
        explicit_deny: false,
    },
    Refused {
        case: "an assumed-role session whose role allows nothing",
        document: nothing,
        assumed_role: true,
        explicit_deny: false,
    },
    Refused {
        case: "an allow of the other listing's action",
        document: the_other_listings_action,
        assumed_role: false,
        explicit_deny: false,
    },
    Refused {
        case: "an allow of this action on one API rather than on the account's resources",
        document: this_action_on_one_api,
        assumed_role: false,
        explicit_deny: false,
    },
    Refused {
        case: "an explicit deny beside a wildcard allow",
        document: an_explicit_deny,
        assumed_role: true,
        explicit_deny: true,
    },
];

/// The sentence the refusal carries: the principal that signed, the action it asked for, the
/// resources a collection read of this service is judged against, and which of IAM's rules
/// withheld the grant — the default deny for a request no document allows, and the
/// explicit-deny clause where a document denies it.
fn expected_message(principal: &str, operation: &str, explicit_deny: bool) -> String {
    if explicit_deny {
        format!(
            "User: {principal} is not authorized to perform: {ACTION_PREFIX}:{operation} on \
             resource: {} with an explicit deny in an identity-based policy",
            arn(ANY_RESOURCE)
        )
    } else {
        default_deny_message(principal, operation, &arn(ANY_RESOURCE))
    }
}

/// One Event API configuration whose only authorization provider is an API key.
fn event_config() -> Value {
    let modes = json!([{"authType": "API_KEY"}]);
    json!({
        "authProviders": [{"authType": "API_KEY"}],
        "connectionAuthModes": modes,
        "defaultPublishAuthModes": modes,
        "defaultSubscribeAuthModes": modes,
    })
}

/// What the account holds while the tables run, and every string of it a refusal must not
/// disclose.
struct Inventory {
    event_api_id: String,
    graphql_api_id: String,
    disclosures: Vec<(&'static str, String)>,
}

impl Inventory {
    /// The `apiId` the listing of `family` must report.
    fn id(&self, family: Family) -> &str {
        match family {
            Family::Event => &self.event_api_id,
            Family::Graphql => &self.graphql_api_id,
        }
    }
}

/// Create the one Event API and the one GraphQL API the tables are written about, with the
/// account's own credential, and collect what a refusal must withhold.
async fn inventory(emulator: &Emulator) -> Inventory {
    let event = emulator
        .call(
            ACCOUNT_KEY,
            "POST",
            "/v2/apis",
            Some(&json!({"name": EVENT_API_NAME, "eventConfig": event_config()})),
        )
        .await;
    assert_eq!(
        event.status, 200,
        "case \"the inventory\": expected CreateApi 200, got {} {:?} with {:?}",
        event.status, event.error_code, event.body
    );
    let graphql = emulator
        .call(
            ACCOUNT_KEY,
            "POST",
            "/v1/apis",
            Some(&json!({"name": GRAPHQL_API_NAME, "authenticationType": "API_KEY"})),
        )
        .await;
    assert_eq!(
        graphql.status, 200,
        "case \"the inventory\": expected CreateGraphqlApi 200, got {} {:?} with {:?}",
        graphql.status, graphql.error_code, graphql.body
    );

    let api = &event.body["api"];
    let graphql_api = &graphql.body["graphqlApi"];
    let mut disclosures = Vec::new();
    for (label, value) in [
        ("the Event API's id", api["apiId"].as_str()),
        ("the Event API's name", api["name"].as_str()),
        ("the Event API's ARN", api["apiArn"].as_str()),
        ("the Event API's HTTP endpoint host", api["dns"]["HTTP"].as_str()),
        (
            "the Event API's REALTIME endpoint host",
            api["dns"]["REALTIME"].as_str(),
        ),
        ("the GraphQL API's id", graphql_api["apiId"].as_str()),
        ("the GraphQL API's name", graphql_api["name"].as_str()),
        ("the GraphQL API's ARN", graphql_api["arn"].as_str()),
    ] {
        // A member the create answered nothing for is nothing to withhold.
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            disclosures.push((label, value.to_string()));
        }
    }

    let held = Inventory {
        event_api_id: api["apiId"].as_str().unwrap_or_default().to_string(),
        graphql_api_id: graphql_api["apiId"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        disclosures,
    };
    assert!(
        !held.event_api_id.is_empty() && !held.graphql_api_id.is_empty(),
        "case \"the inventory\": expected both creates to answer an apiId, got {:?} and {:?}",
        event.body,
        graphql.body
    );
    held
}

/// The `apiId`s one request form of `listing` reports to the caller behind `access_key_id`,
/// which must be a served 200.
async fn reported(
    emulator: &Emulator,
    access_key_id: &str,
    listing: &Listing,
    path: &str,
) -> Vec<String> {
    let reply = emulator.call(access_key_id, "GET", path, None).await;
    assert_eq!(
        reply.status, 200,
        "case {:?} at {path:?}: expected the listing served to a caller the documents allow, \
         got {} {:?} with {:?}",
        listing.operation, reply.status, reply.error_code, reply.body
    );
    reply.body[listing.member]
        .as_array()
        .map(|page| {
            page.iter()
                .filter_map(|api| api["apiId"].as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// RULE: each of the two inventory listings is served only to a caller whose identity-based
/// documents allow that listing's own action on the resources the collection read is judged
/// against; every other caller is refused with the modelled 403 `AccessDeniedException` and
/// IAM's refusal sentence, carrying nothing of what the account holds.
#[tokio::test]
async fn an_unauthorized_caller_is_refused_the_api_inventory() {
    let emulator = Emulator::new();
    let held = inventory(&emulator).await;
    let permitted = emulator
        .user(
            "inventory_reader",
            Some(&allow(&format!("{ACTION_PREFIX}:*"), ANY_RESOURCE)),
        )
        .await;

    // The positive control: a caller the documents allow IS served the account's inventory,
    // so a refusal below is the gate's answer and not an empty account.
    for listing in LISTINGS {
        for path in listing.paths {
            let page = reported(&emulator, &permitted.access_key_id, listing, path).await;
            assert!(
                page.iter().any(|id| id == held.id(listing.family)),
                "case {:?} at {path:?}: expected the permitted caller to be served {:?}, got \
                 {page:?}",
                listing.operation,
                held.id(listing.family)
            );
        }
    }

    for listing in LISTINGS {
        for (index, row) in REFUSED.iter().enumerate() {
            let label = format!("{}_{index}", listing.operation.to_lowercase());
            let document = (row.document)(listing.operation, &held.event_api_id);
            let principal = if row.assumed_role {
                emulator.assumed_role(&label, document.as_deref()).await
            } else {
                emulator.user(&label, document.as_deref()).await
            };
            for path in listing.paths {
                let case = format!("{} at {path:?} for {}", listing.operation, row.case);
                let reply = emulator
                    .call(&principal.access_key_id, "GET", path, None)
                    .await;
                assert_eq!(
                    (reply.status, reply.error_code.as_str()),
                    (ACCESS_DENIED_STATUS, ACCESS_DENIED),
                    "case {case}: expected {ACCESS_DENIED_STATUS} {ACCESS_DENIED}, got {} {:?} \
                     with {:?}",
                    reply.status,
                    reply.error_code,
                    reply.body
                );
                assert_eq!(
                    reply.message(),
                    expected_message(&principal.arn, listing.operation, row.explicit_deny),
                    "case {case}: expected IAM's refusal sentence about this action and the \
                     resources the listing is judged against, got {:?}",
                    reply.message()
                );
                assert!(
                    reply.body.get(listing.member).is_none(),
                    "case {case}: expected the refusal to carry no page, got {:?}",
                    reply.body
                );
                let answered = reply.body.to_string();
                for (what, value) in &held.disclosures {
                    assert!(
                        !answered.contains(value.as_str()),
                        "case {case}: expected the refusal to disclose nothing of what the \
                         account holds, got {what} in {answered}"
                    );
                }
            }
        }
    }

    // The refusals read nothing and wrote nothing: the permitted caller is served the same
    // inventory afterwards.
    for listing in LISTINGS {
        let path = listing.paths[0];
        let page = reported(&emulator, &permitted.access_key_id, listing, path).await;
        assert!(
            page.iter().any(|id| id == held.id(listing.family)),
            "case {:?} after the refusals: expected the inventory unchanged, got {page:?}",
            listing.operation
        );
    }
}

/// RULE: the decision is taken before the listing reads anything, so what the account holds
/// does not change it — a caller whose documents allow nothing is refused the same way over
/// an account holding no API at all, where an ungated read would answer an empty page, and
/// the account's own credential is served that empty page.
#[tokio::test]
async fn the_refusal_does_not_depend_on_what_the_account_holds() {
    let emulator = Emulator::new();
    let ungranted = emulator.user("empty_account_probe", None).await;

    for listing in LISTINGS {
        for path in listing.paths {
            let case = format!("{} at {path:?} over an empty account", listing.operation);
            let reply = emulator
                .call(&ungranted.access_key_id, "GET", path, None)
                .await;
            assert_eq!(
                (reply.status, reply.error_code.as_str()),
                (ACCESS_DENIED_STATUS, ACCESS_DENIED),
                "case {case}: expected {ACCESS_DENIED_STATUS} {ACCESS_DENIED} rather than an \
                 empty page, got {} {:?} with {:?}",
                reply.status,
                reply.error_code,
                reply.body
            );
            assert_eq!(
                reply.message(),
                expected_message(&ungranted.arn, listing.operation, false),
                "case {case}: expected IAM's default-deny sentence, got {:?}",
                reply.message()
            );

            let served = emulator.call(ACCOUNT_KEY, "GET", path, None).await;
            assert_eq!(
                (served.status, served.body[listing.member].as_array().map(Vec::len)),
                (200, Some(0)),
                "case {case}: expected the account's own credential to be served an empty \
                 page, got {} {:?} with {:?}",
                served.status,
                served.error_code,
                served.body
            );
        }
    }
}
