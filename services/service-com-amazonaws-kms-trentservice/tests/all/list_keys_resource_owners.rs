// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHOSE keys `ListKeys` lists, driven over the wire through the
//! shared [`super::harness`].
//!
//! The RULE asserted here: **`ListKeys` lists the keys of the owners `ResourceOwners`
//! names — `SELF` the keys the account this endpoint serves owns, `OTHER_ACCOUNTS_VIA_RAM`
//! the keys another account shares with it, the own keys first when both are named, and
//! the own keys alone when the member is omitted** — with every page of a selection drawn
//! from that selection alone, so a marker only resumes a listing of the same owners.
//!
//! Nothing in this store is a key of another account, so the shared selection is empty:
//! "a response can contain fewer KMS keys than the `Limit` value, or none"
//! (`smithy_query(query="doc",shape="ListKeysRequest",member="ResourceOwners")`).
//!
//! The rows are the shapes the member admits — absent, each single value, and both values
//! in either order — plus each published bound of `ResourceOwnersList` (`@length(1..2)`)
//! and one value its `ResourceOwnerType` does not declare.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    call, create_key, error_code, error_message, failure, ok, one_violation, router, REGION,
};

/// Every value `ResourceOwnerType` declares
/// (`smithy_query(query="shape",shape="ResourceOwnerType")`).
const SELF: &str = "SELF";
const OTHER_ACCOUNTS_VIA_RAM: &str = "OTHER_ACCOUNTS_VIA_RAM";

/// How many keys each test seeds, so a `Limit` smaller than the total has pages to walk.
const SEEDED_KEYS: usize = 5;

/// Seed [`SEEDED_KEYS`] keys and report the `KeyListEntry` each one is listed as, in
/// creation order — the order a listing reports.
async fn seeded(router: &MultiServiceRouter) -> Vec<Value> {
    let mut expected = Vec::new();
    for index in 0..SEEDED_KEYS {
        let metadata = create_key(router, &json!({"Description": format!("owned-{index}")})).await;
        expected.push(json!({"KeyId": metadata["KeyId"], "KeyArn": metadata["Arn"]}));
    }
    expected
}

/// The `ResourceOwners` value of a request, and whether it selects the keys this account
/// owns.
struct OwnersCase {
    case: &'static str,
    /// What the request carries, or `None` for a request that omits the member.
    owners: Option<&'static [&'static str]>,
    /// Whether the selection holds the keys the serving account owns.
    lists_own: bool,
}

/// Every shape `ResourceOwners` admits.
const OWNERS_CASES: &[OwnersCase] = &[
    OwnersCase {
        case: "the member omitted",
        owners: None,
        lists_own: true,
    },
    OwnersCase {
        case: "SELF alone",
        owners: Some(&[SELF]),
        lists_own: true,
    },
    OwnersCase {
        case: "OTHER_ACCOUNTS_VIA_RAM alone",
        owners: Some(&[OTHER_ACCOUNTS_VIA_RAM]),
        lists_own: false,
    },
    OwnersCase {
        case: "both values, own account first",
        owners: Some(&[SELF, OTHER_ACCOUNTS_VIA_RAM]),
        lists_own: true,
    },
    // "If you specify both values, in either order, ListKeys returns the KMS keys in your
    // Amazon Web Services account first".
    OwnersCase {
        case: "both values, the other order",
        owners: Some(&[OTHER_ACCOUNTS_VIA_RAM, SELF]),
        lists_own: true,
    },
];

/// A `ListKeys` request carrying `owners` and whatever else `extra` states.
fn request(owners: Option<&[&str]>, extra: Value) -> Value {
    let mut body = extra;
    if let Some(owners) = owners {
        body["ResourceOwners"] = json!(owners);
    }
    body
}

/// Each shape of `ResourceOwners` lists exactly the keys of the owners it names: the keys
/// this account owns when it names `SELF` (or names nothing), and none when it names only
/// the keys another account shares — which this store holds none of.
#[tokio::test]
async fn resource_owners_selects_whose_keys_are_listed() {
    let router = router();
    let owned = seeded(&router).await;

    for row in OWNERS_CASES {
        let case = row.case;
        let expected: Vec<Value> = if row.lists_own {
            owned.clone()
        } else {
            Vec::new()
        };

        let listed = ok(&router, "ListKeys", &request(row.owners, json!({}))).await;
        assert_eq!(
            listed["Keys"],
            json!(expected),
            "case {case}: expected {} keys, got {listed}",
            expected.len()
        );
        assert_eq!(
            listed["Truncated"],
            json!(false),
            "case {case}: a complete page is not truncated, got {listed}"
        );
        assert!(
            listed["NextMarker"].is_null(),
            "case {case}: an untruncated page carries no marker, got {listed}"
        );

        // A listing is a read: the same request answers the same page afterwards.
        let again = ok(&router, "ListKeys", &request(row.owners, json!({}))).await;
        assert_eq!(
            again, listed,
            "case {case}: a listing must not change what a later listing sees, got {again}"
        );
    }
}

/// A selection pages over its own keys and nothing else: walking it with a `Limit` below
/// the total visits every key of that selection exactly once, in creation order, and an
/// empty selection is one untruncated page whatever the `Limit`.
#[tokio::test]
async fn each_selection_pages_over_its_own_keys() {
    let router = router();
    let owned = seeded(&router).await;

    for row in OWNERS_CASES {
        let expected: Vec<Value> = if row.lists_own {
            owned.clone()
        } else {
            Vec::new()
        };

        for limit in [1usize, 2, SEEDED_KEYS, SEEDED_KEYS + 2] {
            let case = format!("{}: limit {limit}", row.case);
            let mut collected: Vec<Value> = Vec::new();
            let mut marker: Option<String> = None;
            let mut pages = 0;

            loop {
                let mut body = request(row.owners, json!({"Limit": limit}));
                if let Some(marker) = marker.as_ref() {
                    body["Marker"] = json!(marker);
                }
                let page = ok(&router, "ListKeys", &body).await;
                pages += 1;
                assert!(
                    pages <= expected.len() + 1,
                    "case {case}: pagination must terminate, but took {pages} pages"
                );

                let entries = page["Keys"].as_array().cloned().unwrap_or_default();
                assert!(
                    entries.len() <= limit,
                    "case {case}: a page holds at most Limit entries, got {page}"
                );
                collected.extend(entries);

                let truncated = page["Truncated"] == json!(true);
                assert_eq!(
                    truncated,
                    collected.len() < expected.len(),
                    "case {case}: Truncated must say whether keys remain, got {page}"
                );
                if !truncated {
                    assert!(
                        page["NextMarker"].is_null(),
                        "case {case}: the last page carries no marker, got {page}"
                    );
                    break;
                }
                marker = Some(
                    page["NextMarker"]
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case {case}: a truncated page carries a marker: {page}")
                        })
                        .to_string(),
                );
            }

            assert_eq!(
                collected, expected,
                "case {case}: paging must visit every key of the selection exactly once, in \
                 creation order"
            );
        }
    }
}

/// "Use a `NextMarker` value only in a request that has the same `ResourceOwners` value as
/// the request that returned it. Otherwise, the request can fail with an
/// `InvalidMarkerException`": a marker a listing of this account's keys issued names no
/// position in the shared selection, so a request that carries it there is refused.
#[tokio::test]
async fn a_marker_of_another_selection_is_refused() {
    let router = router();
    seeded(&router).await;

    let page = ok(&router, "ListKeys", &json!({"Limit": 1})).await;
    let marker = page["NextMarker"]
        .as_str()
        .unwrap_or_else(|| panic!("case a first page of the own keys: expected a marker: {page}"))
        .to_string();

    let (status, code) = failure(
        &router,
        "ListKeys",
        &json!({"Marker": marker, "ResourceOwners": [OTHER_ACCOUNTS_VIA_RAM]}),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidMarkerException"),
        "case a marker carried into another selection: expected HTTP 400 \
         InvalidMarkerException, got {status} {code}"
    );

    // The same marker still resumes the listing it was issued for.
    let resumed = ok(&router, "ListKeys", &json!({"Limit": 1, "Marker": marker})).await;
    assert_eq!(
        resumed["Keys"].as_array().map(Vec::len),
        Some(1),
        "case the marker in its own selection: expected the next page, got {resumed}"
    );
}

/// One row of `ResourceOwners` the member does not admit: what the request carries, and
/// the clause the boundary refuses it with.
struct RefusedOwners {
    case: &'static str,
    owners: Value,
    clause: String,
}

/// Every way a `ResourceOwners` value can fall outside what the model declares: each
/// published bound of `ResourceOwnersList` (`@length(1..2)`) crossed, and a value
/// `ResourceOwnerType` does not declare.
fn refused_owners() -> Vec<RefusedOwners> {
    vec![
        RefusedOwners {
            case: "one below the published minimum: the empty list",
            owners: json!([]),
            clause: "Value at 'resourceOwners' failed to satisfy constraint: Member must have \
                     length greater than or equal to 1"
                .to_string(),
        },
        RefusedOwners {
            case: "one past the published maximum: three values",
            owners: json!([SELF, OTHER_ACCOUNTS_VIA_RAM, SELF]),
            clause: "Value at 'resourceOwners' failed to satisfy constraint: Member must have \
                     length less than or equal to 2"
                .to_string(),
        },
        RefusedOwners {
            case: "a value the enum does not declare",
            owners: json!(["EVERYONE"]),
            clause: format!(
                "Value 'EVERYONE' at 'resourceOwners.1.member' failed to satisfy constraint: \
                 Member must satisfy enum value set: [{SELF}, {OTHER_ACCOUNTS_VIA_RAM}]"
            ),
        },
    ]
}

/// A `ResourceOwners` value outside what the model declares is refused by the boundary
/// with `ValidationException`, in the envelope a constraint refusal is stated in — and the
/// listing is unchanged afterwards.
#[tokio::test]
async fn a_resource_owners_value_outside_the_model_is_refused() {
    let router = router();
    let owned = seeded(&router).await;

    for row in refused_owners() {
        let case = row.case;
        let (status, payload) = call(
            &router,
            REGION,
            "ListKeys",
            &json!({"ResourceOwners": row.owners}),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (400, "ValidationException"),
            "case {case}: expected HTTP 400 ValidationException, got {status} with {payload}"
        );
        assert_eq!(
            error_message(&payload),
            one_violation(&row.clause),
            "case {case}: expected the constraint envelope, got {payload}"
        );
    }

    let listed = ok(&router, "ListKeys", &json!({})).await;
    assert_eq!(
        listed["Keys"],
        json!(owned),
        "case after every refusal: a refused listing must leave the keys as they were, got \
         {listed}"
    );
}
