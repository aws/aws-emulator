// SPDX-License-Identifier: CC0-1.0

//! MemoryDB parameter groups, end to end over the wire.
//!
//! Every case drives the SAME path a client does: a `POST` through the multi-service
//! router, so what is asserted is the emulator's HTTP behaviour and not a handler called
//! directly. This service's model declares TWO wire protocols side by side
//! (`smithy_query query="service_info"` -> `awsJson1_1`, `rpcv2Cbor`), so a client may
//! call it in either: the cases up to the last section drive the awsJson1_1 framing
//! (`POST /` with `X-Amz-Target: AmazonMemoryDB.<Operation>` and a JSON body), and that
//! section drives the same calls over the Smithy RPC v2 CBOR framing.
//!
//! The rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * `CreateParameterGroup` persists the group and answers the stored record: `Name`
//!   and `Family` folded to lower case (a MemoryDB resource name is a case-insensitive
//!   identifier), `Description` as given or the service's default when none was given,
//!   and an `ARN` scoped to the region the request was signed for;
//! * `DescribeParameterGroups` answers from that same state — the one group a request
//!   names, or every stored group in name order — so a read reports exactly what the
//!   create persisted;
//! * a read changes nothing a later caller sees: two describes of the same store agree;
//! * re-creating a stored group is the modelled already-exists fault and nothing in the
//!   store moves; naming a group that is not stored is the modelled not-found fault;
//! * a listing pages over the whole set in name order for any `MaxResults`, and a page
//!   size or token that names no page is the modelled invalid-value error;
//! * `DeleteParameterGroup` removes the group from that state — it answers the record it
//!   removed and both readers then answer the modelled not-found fault for the name —
//!   while a group MemoryDB itself owns, or one a cluster still references, is refused
//!   and left in place;
//! * and every one of those answers is the SAME answer whichever of the two protocols
//!   the model declares carried the call, each rendered in that protocol's own envelope:
//!   two encodings of one service over one store.

use std::collections::BTreeMap;
use std::sync::Arc;

use aws_smithy_cbor::data::Type;
use aws_smithy_cbor::{Decoder, Encoder};
use serde_json::{json, Value};
use service_com_amazonaws_memorydb_amazonmemorydb::state::{
    default_parameter_groups, PARAMETER_PAGE_SIZE_WINDOW,
};
use service_com_amazonaws_memorydb_amazonmemorydb::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's requests are signed for. Every ARN the emulator mints must
/// carry it, rather than a constant baked into the service.
const REGION: &str = "us-west-2";

/// The page sizes this listing serves: `DescribeParameterGroups` reads the
/// parameter-group surface, so its window is that surface's own.
const WINDOW_MIN: i32 = PARAMETER_PAGE_SIZE_WINDOW.min;
const WINDOW_MAX: i32 = PARAMETER_PAGE_SIZE_WINDOW.max;

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// One `CreateParameterGroup` request, and what the service must report for it.
///
/// The rows differ along everything `CreateParameterGroup`'s rule generalises over: the
/// group name, the parameter group family, whether a description was supplied, and
/// whether tags were. The families are the ones the engine catalog publishes
/// (`DescribeEngineVersions$ParameterGroupFamily`), which is what the service resolves
/// `Family` against — a family it does not publish carries no engine defaults, and the
/// refusal for one has its own case below.
struct Case {
    name: &'static str,
    family: &'static str,
    description: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
}

const CASES: &[Case] = &[
    // No description and no tags: both optional members absent.
    Case {
        name: "alpha-group",
        family: "memorydb_redis7",
        description: None,
        tags: &[],
    },
    // A description, no tags.
    Case {
        name: "beta-group",
        family: "memorydb_redis6",
        description: Some("tuned for the batch tier"),
        tags: &[],
    },
    // A description AND tags.
    Case {
        name: "gamma-group",
        family: "memorydb_valkey7",
        description: Some("multi-tag group"),
        tags: &[("team", "platform"), ("env", "test")],
    },
    // An EMPTY description is still a description: it must round-trip as `""`, not as
    // an absent member.
    Case {
        name: "delta-group",
        family: "memorydb_valkey7",
        description: Some(""),
        tags: &[("only", "one")],
    },
    // A long, digit- and hyphen-bearing name, sorting LAST of these rows: the listing
    // order the pagination cases rely on is the store's, not the insertion order.
    Case {
        name: "zeta-group-0042-with-a-much-longer-name",
        family: "memorydb_redis7",
        description: Some("long name"),
        tags: &[],
    },
];

/// A router with just this service mounted, on one shared engine — what
/// `core/router` builds, narrowed to the service under test.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation, signed for `region`, and return `(status, body)`.
async fn call_signed(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://memory-db.{region}.amazonaws.com/"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AmazonMemoryDB.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{region}/memorydb/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| json!({ "raw": String::from_utf8_lossy(&bytes) }))
    };
    (status, parsed)
}

/// Call one operation signed for [`REGION`], and return `(status, body)`.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call_signed(router, REGION, operation, body).await
}

/// Call an operation that must succeed, returning its body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value, case: &str) -> Value {
    let (status, body) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} with {body}"
    );
    body
}

/// The wire error code of a failure body (`__type`, with any namespace prefix dropped).
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `CreateParameterGroup` request body for one case.
fn create_body(case: &Case) -> Value {
    let mut body = json!({ "ParameterGroupName": case.name, "Family": case.family });
    if let Some(description) = case.description {
        body["Description"] = json!(description);
    }
    if !case.tags.is_empty() {
        body["Tags"] = Value::Array(
            case.tags
                .iter()
                .map(|(key, value)| json!({ "Key": key, "Value": value }))
                .collect(),
        );
    }
    body
}

/// What the service must report for one case, in the region it was created in.
fn expected_group(case: &Case, region: &str) -> Value {
    json!({
        "Name": case.name,
        "Family": case.family,
        // A group created without a `Description` carries the service's own default
        // (`crate::state::DEFAULT_PARAMETER_GROUP_DESCRIPTION`), not an absent member.
        "Description": case.description.unwrap_or(" "),
        "ARN": format!("arn:aws:memorydb:{region}:{ACCOUNT}:parametergroup/{}", case.name),
    })
}

/// `ParameterGroups[*].Name` of a `DescribeParameterGroups` body, in answer order.
fn group_names(body: &Value, case: &str) -> Vec<String> {
    body.get("ParameterGroups")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected ParameterGroups in {body}"))
        .iter()
        .map(|group| {
            group
                .get("Name")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: expected Name in {group}"))
                .to_string()
        })
        .collect()
}

/// Every name a full listing must report, in name order: the groups a row created plus
/// the `default.*` groups MemoryDB itself provides in every account (one per family of
/// the engine catalog, plus the search baseline of every line that has one), which
/// exist without being created.
fn expected_listing(created: &[&str]) -> Vec<String> {
    let mut names: Vec<String> = created.iter().map(|name| name.to_string()).collect();
    names.extend(
        default_parameter_groups()
            .into_iter()
            .map(|group| group.name),
    );
    names.sort();
    names
}

/// A create persists the group, and a later describe answers from that state: the same
/// name, family, description and ARN come back, whichever members the request carried.
#[tokio::test]
async fn a_created_parameter_group_is_reported_by_a_later_describe() {
    let router = router();

    for case in CASES {
        let created = ok(&router, "CreateParameterGroup", &create_body(case), case.name).await;
        assert_eq!(
            created["ParameterGroup"],
            expected_group(case, REGION),
            "case {:?}: the create must answer the record it persisted, got {created}",
            case.name
        );
    }

    // Read every one of them back through the implemented reader, one at a time.
    for case in CASES {
        let described = ok(
            &router,
            "DescribeParameterGroups",
            &json!({ "ParameterGroupName": case.name }),
            case.name,
        )
        .await;
        let groups = described["ParameterGroups"]
            .as_array()
            .unwrap_or_else(|| panic!("case {:?}: expected a list, got {described}", case.name));
        assert_eq!(
            groups.len(),
            1,
            "case {:?}: naming one group must answer exactly that group, got {described}",
            case.name
        );
        assert_eq!(
            groups[0],
            expected_group(case, REGION),
            "case {:?}: the describe must report what the create persisted, got {described}",
            case.name
        );
    }

    // And the unfiltered listing is every stored group, in name order — the groups
    // these cases created AND the AWS-owned default group of every family, which the
    // account already had.
    let listed = ok(&router, "DescribeParameterGroups", &json!({}), "all").await;
    let expected = expected_listing(&CASES.iter().map(|case| case.name).collect::<Vec<_>>());
    assert_eq!(
        group_names(&listed, "all"),
        expected,
        "case {:?}: the listing must report every stored group in name order, got {listed}",
        "all"
    );
}

/// A group belongs to the region the request that created it was signed for, and it
/// keeps that region: its ARN does not follow a later caller's region.
#[tokio::test]
async fn a_parameter_group_keeps_the_region_it_was_created_in() {
    const OTHER_REGION: &str = "eu-west-1";
    let router = router();

    for region in [REGION, OTHER_REGION] {
        let case = &CASES[1];
        let name = format!("{}-{region}", case.name);
        let mut body = create_body(case);
        body["ParameterGroupName"] = json!(name);
        let (status, created) = call_signed(&router, region, "CreateParameterGroup", &body).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200, got {status} with {created}"
        );
        assert_eq!(
            created["ParameterGroup"]["ARN"],
            json!(format!(
                "arn:aws:memorydb:{region}:{ACCOUNT}:parametergroup/{name}"
            )),
            "case {name:?}: the ARN must carry the request's region, got {created}"
        );

        // Described from the OTHER region, the group reports the same ARN.
        let elsewhere = if region == REGION { OTHER_REGION } else { REGION };
        let (status, described) = call_signed(
            &router,
            elsewhere,
            "DescribeParameterGroups",
            &json!({ "ParameterGroupName": name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 from {elsewhere}, got {status} with {described}"
        );
        assert_eq!(
            described["ParameterGroups"][0]["ARN"],
            json!(format!(
                "arn:aws:memorydb:{region}:{ACCOUNT}:parametergroup/{name}"
            )),
            "case {name:?}: a describe signed for {elsewhere} must not move the group, \
             got {described}"
        );
    }
}

/// A describe changes nothing a later caller sees: two reads of the same store agree,
/// and the create's own answer is still what the second read reports.
#[tokio::test]
async fn describing_a_parameter_group_changes_nothing() {
    let router = router();
    for case in CASES {
        ok(&router, "CreateParameterGroup", &create_body(case), case.name).await;
    }

    let first = ok(&router, "DescribeParameterGroups", &json!({}), "first read").await;
    let second = ok(&router, "DescribeParameterGroups", &json!({}), "second read").await;
    assert_eq!(
        first, second,
        "case {:?}: a read must not change what the next read sees, got {first} then {second}",
        "repeat read"
    );
    for case in CASES {
        let described = ok(
            &router,
            "DescribeParameterGroups",
            &json!({ "ParameterGroupName": case.name }),
            case.name,
        )
        .await;
        assert_eq!(
            described["ParameterGroups"][0],
            expected_group(case, REGION),
            "case {:?}: the third read must still report the stored record, got {described}",
            case.name
        );
    }
}

/// The listing pages over the whole set in name order for any page size the service's
/// window admits, and the pages together are exactly the unpaged listing.
#[tokio::test]
async fn the_listing_pages_over_every_group_in_name_order() {
    let router = router();
    for case in CASES {
        ok(&router, "CreateParameterGroup", &create_body(case), case.name).await;
    }
    let mut created: Vec<String> = CASES.iter().map(|case| case.name.to_string()).collect();
    // More groups than the SMALLEST page the service serves, so a page size at the bottom
    // of the page-size window has to walk several pages to cover the set.
    for index in 0..WINDOW_MIN {
        let name = format!("paged-group-{index:02}");
        ok(
            &router,
            "CreateParameterGroup",
            &json!({ "ParameterGroupName": name, "Family": CASES[0].family }),
            &name,
        )
        .await;
        created.push(name);
    }
    let mut expected: Vec<String> = expected_listing(
        &created
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
    );
    expected.sort();

    // At the bottom of the page-size window, one past it, and at the top: a size larger
    // than the set is one page with no token.
    for page_size in [WINDOW_MIN, WINDOW_MIN + 1, WINDOW_MAX] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // At most one page per group, plus one: a page size of at least 1 always makes
        // progress, so this cannot loop forever.
        for _ in 0..=expected.len() {
            let mut body = json!({ "MaxResults": page_size });
            if let Some(token) = token.as_deref() {
                body["NextToken"] = json!(token);
            }
            let page = ok(&router, "DescribeParameterGroups", &body, &case).await;
            let names = group_names(&page, &case);
            assert!(
                i32::try_from(names.len()).unwrap_or(i32::MAX) <= page_size,
                "case {case:?}: a page must hold at most {page_size} groups, got {page}"
            );
            seen.extend(names);
            token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {case:?}: the listing must end, still holding token {token:?}"
        );
        assert_eq!(
            seen, expected,
            "case {case:?}: the pages together must be the whole listing in name order"
        );
    }
}

/// `DeleteParameterGroup` removes the group from state: it answers the record it
/// removed, and the readers then answer the modelled not-found fault for that name — for
/// every row of the table. The two rules the operation documents are refusals that leave
/// the group in place: a group MemoryDB itself owns is not the caller's to delete, and a
/// group a cluster still references cannot go away under it.
#[tokio::test]
async fn deleting_a_parameter_group_removes_it_from_state() {
    let router = router();
    for case in CASES {
        ok(&router, "CreateParameterGroup", &create_body(case), case.name).await;
    }

    for case in CASES {
        let request = json!({ "ParameterGroupName": case.name });
        let deleted = ok(&router, "DeleteParameterGroup", &request, case.name).await;
        assert_eq!(
            deleted["ParameterGroup"],
            expected_group(case, REGION),
            "case {:?}: the delete must answer the record it removed, got {deleted}",
            case.name
        );
        // Read back through BOTH implemented readers: the group is gone from the state
        // they answer from, not merely from the one listing.
        for operation in ["DescribeParameterGroups", "DescribeParameters"] {
            let (status, refused) = call(&router, operation, &request).await;
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (400, "ParameterGroupNotFoundFault"),
                "case {:?}: {operation} must answer the modelled not-found fault for a \
                 deleted group, got {status} with {refused}",
                case.name
            );
        }
        // A second delete of the same name is that same fault: the removal stuck.
        let (status, refused) = call(&router, "DeleteParameterGroup", &request).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ParameterGroupNotFoundFault"),
            "case {:?}: deleting it twice must answer the modelled not-found fault, got \
             {status} with {refused}",
            case.name
        );
    }

    // Every group MemoryDB itself owns is refused, whichever family it belongs to, and
    // is still there afterwards.
    for group in default_parameter_groups() {
        let name = group.name;
        let request = json!({ "ParameterGroupName": name });
        let (status, refused) = call(&router, "DeleteParameterGroup", &request).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "InvalidParameterValueException"),
            "case {name:?}: a group the service owns must be refused, got {status} with \
             {refused}"
        );
        ok(&router, "DescribeParameterGroups", &request, &name).await;
    }

    // A group a cluster references cannot be deleted while it does, and is still there
    // afterwards.
    let in_use = "in-use-group";
    ok(
        &router,
        "CreateParameterGroup",
        &json!({ "ParameterGroupName": in_use, "Family": CASES[0].family }),
        in_use,
    )
    .await;
    ok(
        &router,
        "CreateCluster",
        &json!({
            "ClusterName": "pg-holder",
            "NodeType": "db.t4g.small",
            "ACLName": "open-access",
            "ParameterGroupName": in_use,
        }),
        in_use,
    )
    .await;
    let request = json!({ "ParameterGroupName": in_use });
    let (status, refused) = call(&router, "DeleteParameterGroup", &request).await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "InvalidParameterGroupStateFault"),
        "case {in_use:?}: a group a cluster references must be refused, got {status} with \
         {refused}"
    );
    ok(&router, "DescribeParameterGroups", &request, in_use).await;
}

/// Re-creating a stored group is refused with the modelled already-exists fault, and
/// the stored group is left exactly as it was.
#[tokio::test]
async fn recreating_a_stored_group_is_refused_and_changes_nothing() {
    let router = router();

    for case in CASES {
        ok(&router, "CreateParameterGroup", &create_body(case), case.name).await;
    }

    for case in CASES {
        // The second create differs from the first in family and description, so a
        // refusal that silently overwrote the record would be visible below.
        let body = json!({
            "ParameterGroupName": case.name,
            "Family": "memorydb_redis6",
            "Description": "an overwrite that must not happen",
        });
        let (status, refused) = call(&router, "CreateParameterGroup", &body).await;
        assert_eq!(
            status, 400,
            "case {:?}: re-creating a stored group must answer 400, got {status} with {refused}",
            case.name
        );
        assert_eq!(
            error_code(&refused),
            "ParameterGroupAlreadyExistsFault",
            "case {:?}: expected the modelled already-exists fault, got {refused}",
            case.name
        );
        let described = ok(
            &router,
            "DescribeParameterGroups",
            &json!({ "ParameterGroupName": case.name }),
            case.name,
        )
        .await;
        assert_eq!(
            described["ParameterGroups"][0],
            expected_group(case, REGION),
            "case {:?}: the refused create must leave the group unchanged, got {described}",
            case.name
        );
    }
}

/// Naming a group that is not in state is the modelled not-found fault, and a page size
/// or token that names no page is the modelled invalid-value error.
#[tokio::test]
async fn a_request_that_names_nothing_is_refused() {
    let router = router();
    ok(
        &router,
        "CreateParameterGroup",
        &create_body(&CASES[0]),
        CASES[0].name,
    )
    .await;

    // Unknown names, including one that is a prefix of a stored name, one that extends
    // it and one that differs by a trailing space: a name is folded to lower case
    // before it is looked up, but nothing else about it is normalised.
    for name in [
        "no-such-group",
        "alpha",
        "alpha-group-2",
        "alpha-group ",
    ] {
        let (status, refused) = call(
            &router,
            "DescribeParameterGroups",
            &json!({ "ParameterGroupName": name }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {name:?}: an unknown group must answer 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ParameterGroupNotFoundFault",
            "case {name:?}: expected the modelled not-found fault, got {refused}"
        );
    }

    // A page size outside the service's window names no page.
    for max_results in [0, -7, WINDOW_MIN - 1, WINDOW_MAX + 1] {
        let (status, refused) = call(
            &router,
            "DescribeParameterGroups",
            &json!({ "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {max_results:?}: a page size outside the window must answer 400, got \
             {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidParameterValueException",
            "case {max_results:?}: expected the modelled invalid-value error, got {refused}"
        );
    }

    // A token this listing never minted names no position in it.
    for token in ["zzzz-not-a-position", "~"] {
        let (status, refused) = call(
            &router,
            "DescribeParameterGroups",
            &json!({ "NextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {token:?}: a token past the listing must answer 400, got {status} with \
             {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidParameterValueException",
            "case {token:?}: expected the modelled invalid-value error, got {refused}"
        );
    }
}

// ---------------------------------------------------------------------------
// The same calls over the Smithy RPC v2 CBOR framing
// ---------------------------------------------------------------------------
//
// The model declares `awsJson1_1` AND `rpcv2Cbor`, so which framing carries one call is
// named by the request itself: the awsJson framing posts to `/` with an `X-Amz-Target`
// header and a JSON body, while the CBOR framing posts to
// `/service/{Service}/operation/{Operation}`, declares itself with
// `smithy-protocol: rpc-v2-cbor` and carries a CBOR body and no target header. Both
// decode to the same modelled input and are served by the same operation over the same
// store, so a group written over one framing is reported by the other and a refusal is
// worded the same way in either — in that framing's own envelope.
//
// The CBOR bodies below are written and read with the CBOR implementation the framework's
// own codec is built on, so the bytes a case sends are the bytes a client of that framing
// sends.

/// The `{Service}` label of this service's RPC v2 CBOR paths: the local name of the
/// model's service shape (`AmazonMemoryDB`), which is also the `X-Amz-Target` prefix of
/// its awsJson framing.
const SERVICE_LABEL: &str = "AmazonMemoryDB";

/// The header the RPC v2 CBOR specification requires on every request and response of
/// that framing, and the value it carries.
const SMITHY_PROTOCOL: (&str, &str) = ("smithy-protocol", "rpc-v2-cbor");

/// The media type that framing carries its bodies under.
const CBOR_MEDIA_TYPE: &str = "application/cbor";

/// The media type the awsJson1_1 framing carries its bodies under.
const JSON_MEDIA_TYPE: &str = "application/x-amz-json-1.1";

/// The framing one request is sent in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Framing {
    /// The awsJson1_1 framing: `POST /` with `X-Amz-Target` and a JSON body.
    AwsJson,
    /// Smithy RPC v2 CBOR: the operation in the path, the body as CBOR.
    RpcV2Cbor,
}

/// How a CBOR request writes its container headers. Both forms are CBOR and both are what
/// some client sends, so the cases send each of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lengths {
    /// Every map and array declares its entry count up front.
    Declared,
    /// Every map and array is opened and closed by a break, as the framework's own
    /// serializer writes them.
    Indefinite,
}

/// `value` as the CBOR item carrying the same members, with container lengths written as
/// `lengths` says.
fn cbor_bytes(value: &Value, lengths: Lengths) -> Vec<u8> {
    fn write(encoder: &mut Encoder, value: &Value, lengths: Lengths) {
        match value {
            Value::Null => {
                encoder.null();
            }
            Value::Bool(flag) => {
                encoder.boolean(*flag);
            }
            Value::Number(number) => match number.as_i64() {
                Some(whole) => {
                    encoder.long(whole);
                }
                None => {
                    encoder.double(number.as_f64().unwrap_or_default());
                }
            },
            Value::String(text) => {
                encoder.str(text);
            }
            Value::Array(elements) => {
                match lengths {
                    Lengths::Declared => encoder.array(elements.len()),
                    Lengths::Indefinite => encoder.begin_array(),
                };
                for element in elements {
                    write(encoder, element, lengths);
                }
                if lengths == Lengths::Indefinite {
                    encoder.end();
                }
            }
            Value::Object(members) => {
                match lengths {
                    Lengths::Declared => encoder.map(members.len()),
                    Lengths::Indefinite => encoder.begin_map(),
                };
                for (name, held) in members {
                    encoder.str(name);
                    write(encoder, held, lengths);
                }
                if lengths == Lengths::Indefinite {
                    encoder.end();
                }
            }
        }
    }
    let mut encoder = Encoder::new(Vec::new());
    write(&mut encoder, value, lengths);
    encoder.into_writer()
}

/// One value a response body carries, whichever framing carries it: the two framings
/// decode to the same shape, so every assertion below is written once.
///
/// A map's entries are held in KEY order rather than in the order the body wrote them,
/// because the order two codecs write one structure's members in is each codec's own and
/// is not part of the payload.
#[derive(Debug, Clone, PartialEq)]
enum Decoded {
    Text(String),
    Number(f64),
    Flag(bool),
    Bytes(Vec<u8>),
    List(Vec<Decoded>),
    Map(BTreeMap<String, Decoded>),
    Null,
}

impl Decoded {
    /// The value under `key`, for a map.
    fn get(&self, key: &str) -> Option<&Decoded> {
        match self {
            Self::Map(entries) => entries.get(key),
            _ => None,
        }
    }

    /// The text under `key`, or the empty string when the body carries none there — so an
    /// assertion reports what the body held rather than panicking on its absence.
    fn text_at(&self, key: &str) -> &str {
        match self.get(key) {
            Some(Self::Text(text)) => text,
            _ => "",
        }
    }

    /// The elements under `key`, or none when the body carries no list there.
    fn list_at(&self, key: &str) -> &[Decoded] {
        match self.get(key) {
            Some(Self::List(elements)) => elements,
            _ => &[],
        }
    }

    /// The error code a refusal names, as a client branches on it: the `__type` without
    /// its namespace.
    fn refusal_code(&self) -> String {
        let raw = self.text_at("__type");
        raw.rsplit(['#', ':']).next().unwrap_or(raw).to_string()
    }
}

/// A JSON response body as the shape above.
fn decoded_json(body: &[u8]) -> Decoded {
    fn convert(value: Value) -> Decoded {
        match value {
            Value::Null => Decoded::Null,
            Value::Bool(flag) => Decoded::Flag(flag),
            Value::Number(number) => Decoded::Number(number.as_f64().unwrap_or_default()),
            Value::String(text) => Decoded::Text(text),
            Value::Array(elements) => Decoded::List(elements.into_iter().map(convert).collect()),
            Value::Object(members) => Decoded::Map(
                members
                    .into_iter()
                    .map(|(key, held)| (key, convert(held)))
                    .collect(),
            ),
        }
    }
    match serde_json::from_slice::<Value>(body) {
        Ok(value) => convert(value),
        Err(_) => Decoded::Null,
    }
}

/// A CBOR response body as the shape above, decoded rather than scanned as text, so what
/// is asserted is the WHOLE decoded value a client of that framing reads.
fn decoded_cbor(body: &[u8]) -> Decoded {
    fn read(decoder: &mut Decoder<'_>) -> Decoded {
        let datatype = decoder
            .datatype()
            .unwrap_or_else(|error| panic!("the body ends where an item was expected: {error}"));
        match datatype {
            Type::String | Type::StringIndef => Decoded::Text(
                decoder
                    .string()
                    .unwrap_or_else(|error| panic!("a text item does not read: {error}")),
            ),
            Type::Bytes | Type::BytesIndef => Decoded::Bytes(
                decoder
                    .blob()
                    .unwrap_or_else(|error| panic!("a byte string does not read: {error}"))
                    .into_inner(),
            ),
            Type::Bool => Decoded::Flag(
                decoder
                    .boolean()
                    .unwrap_or_else(|error| panic!("a boolean does not read: {error}")),
            ),
            Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::I8 | Type::I16 | Type::I32
            | Type::I64 | Type::Int => Decoded::Number(
                decoder
                    .long()
                    .unwrap_or_else(|error| panic!("a whole number does not read: {error}"))
                    as f64,
            ),
            Type::F16 | Type::F32 | Type::F64 => Decoded::Number(
                decoder
                    .double()
                    .unwrap_or_else(|error| panic!("a number does not read: {error}")),
            ),
            Type::Null => {
                decoder.null().expect("a null reads");
                Decoded::Null
            }
            // An `@timestamp` member: the CBOR framing carries it as an instant under the
            // tag that framing reserves for one.
            Type::Tag => Decoded::Number(
                decoder
                    .timestamp()
                    .unwrap_or_else(|error| panic!("a timestamp does not read: {error}"))
                    .as_secs_f64(),
            ),
            Type::Array | Type::ArrayIndef => {
                let declared = decoder
                    .list()
                    .unwrap_or_else(|error| panic!("an array header does not read: {error}"));
                let mut elements = Vec::new();
                while !at_end(decoder, declared, elements.len()) {
                    elements.push(read(decoder));
                }
                end_container(decoder, declared);
                Decoded::List(elements)
            }
            Type::Map | Type::MapIndef => {
                let declared = decoder
                    .map()
                    .unwrap_or_else(|error| panic!("a map header does not read: {error}"));
                let mut entries = BTreeMap::new();
                while !at_end(decoder, declared, entries.len()) {
                    let key = decoder
                        .string()
                        .unwrap_or_else(|error| panic!("a map key is not text: {error}"));
                    entries.insert(key, read(decoder));
                }
                end_container(decoder, declared);
                Decoded::Map(entries)
            }
            other => panic!("a response body carries no item of type {other:?}"),
        }
    }

    /// Whether a container that declared `declared` entries (or none, for an indefinite
    /// one) ends here, `read` entries in.
    fn at_end(decoder: &mut Decoder<'_>, declared: Option<u64>, read: usize) -> bool {
        match declared {
            Some(entries) => read as u64 == entries,
            None => matches!(
                decoder
                    .datatype()
                    .unwrap_or_else(|error| panic!("a container ends mid-entry: {error}")),
                Type::Break
            ),
        }
    }

    /// Consume the break that terminates an indefinite-length container.
    fn end_container(decoder: &mut Decoder<'_>, declared: Option<u64>) {
        if declared.is_none() {
            decoder
                .skip()
                .unwrap_or_else(|error| panic!("the break of a container does not read: {error}"));
        }
    }

    let mut decoder = Decoder::new(body);
    read(&mut decoder)
}

/// One answer a call earns: the status, the framing markers the response carries, and the
/// decoded body.
#[derive(Debug, Clone, PartialEq)]
struct Answer {
    status: u16,
    content_type: String,
    smithy_protocol: String,
    body: Decoded,
}

/// Call `operation` in `framing` with the input `body` names, and answer what the service
/// replied.
///
/// Both framings are signed for [`REGION`] with the same SigV4 credential scope: that is
/// how the router picks this service whichever framing a client calls it in, and how the
/// handler learns the region a resource it mints belongs to.
async fn framed_call(
    router: &MultiServiceRouter,
    framing: Framing,
    lengths: Lengths,
    operation: &str,
    body: &Value,
) -> Answer {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{REGION}/memorydb/\
         aws4_request, SignedHeaders=host, Signature=0000"
    );
    let endpoint = format!("https://memory-db.{REGION}.amazonaws.com");
    let request = match framing {
        Framing::AwsJson => http::Request::builder()
            .method("POST")
            .uri(format!("{endpoint}/"))
            .header("content-type", JSON_MEDIA_TYPE)
            .header("x-amz-target", format!("{SERVICE_LABEL}.{operation}"))
            .header("authorization", &authorization)
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("build the awsJson request"),
        Framing::RpcV2Cbor => http::Request::builder()
            .method("POST")
            .uri(format!(
                "{endpoint}/service/{SERVICE_LABEL}/operation/{operation}"
            ))
            .header(SMITHY_PROTOCOL.0, SMITHY_PROTOCOL.1)
            .header("content-type", CBOR_MEDIA_TYPE)
            .header("accept", CBOR_MEDIA_TYPE)
            .header("authorization", &authorization)
            .body(bytes::Bytes::from(cbor_bytes(body, lengths)))
            .expect("build the rpc-v2-cbor request"),
    };

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    let content_type = header("content-type");
    let smithy_protocol = header(SMITHY_PROTOCOL.0);
    let payload = response.into_body();
    let body = match framing {
        Framing::AwsJson => decoded_json(&payload),
        Framing::RpcV2Cbor if payload.is_empty() => Decoded::Null,
        Framing::RpcV2Cbor => decoded_cbor(&payload),
    };
    Answer {
        status,
        content_type,
        smithy_protocol,
        body,
    }
}

/// A call in `framing` that must succeed, returning its decoded body.
async fn framed_ok(
    router: &MultiServiceRouter,
    framing: Framing,
    lengths: Lengths,
    operation: &str,
    body: &Value,
    case: &str,
) -> Decoded {
    let answer = framed_call(router, framing, lengths, operation, body).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected {operation} to answer 200 in {framing:?}, got {answer:?}"
    );
    match framing {
        Framing::AwsJson => assert_eq!(
            answer.content_type, JSON_MEDIA_TYPE,
            "case {case:?}: an awsJson1_1 answer carries that media type, got {answer:?}"
        ),
        Framing::RpcV2Cbor => {
            assert_eq!(
                answer.content_type, CBOR_MEDIA_TYPE,
                "case {case:?}: an rpc-v2-cbor answer carries CBOR bytes, got {answer:?}"
            );
            assert_eq!(
                answer.smithy_protocol, SMITHY_PROTOCOL.1,
                "case {case:?}: every rpc-v2-cbor response declares the framing, got {answer:?}"
            );
        }
    }
    answer.body
}

/// The one group a `DescribeParameterGroups` answer reports, read from either framing's
/// decoded body.
fn only_group(described: &Decoded, case: &str) -> Decoded {
    let groups = described.list_at("ParameterGroups");
    assert_eq!(
        groups.len(),
        1,
        "case {case:?}: naming one group must answer exactly that group, got {described:?}"
    );
    groups[0].clone()
}

/// A group written in either framing is reported by both, member for member: the two
/// framings are two encodings of ONE service over ONE store.
///
/// The rows differ along what the rule generalises over: which framing carries the write,
/// how a CBOR request writes its container lengths, and which members the request carries
/// (a bare create, one with a description, and one with a list of tag structures — a
/// nested list of maps on the wire).
#[tokio::test]
async fn a_parameter_group_written_in_either_framing_is_reported_by_both() {
    struct Row {
        case: &'static str,
        wrote_in: Framing,
        lengths: Lengths,
        input: Value,
        expected: &'static [(&'static str, &'static str)],
    }

    let rows = [
        Row {
            case: "a bare create over rpc-v2-cbor, lengths declared",
            wrote_in: Framing::RpcV2Cbor,
            lengths: Lengths::Declared,
            input: json!({ "ParameterGroupName": "cbor-declared", "Family": "memorydb_redis7" }),
            expected: &[
                ("Name", "cbor-declared"),
                ("Family", "memorydb_redis7"),
                // No `Description` was sent, so the service's own default is stored.
                ("Description", " "),
            ],
        },
        Row {
            case: "a create carrying a description and tags over rpc-v2-cbor, lengths \
                   indefinite",
            wrote_in: Framing::RpcV2Cbor,
            lengths: Lengths::Indefinite,
            input: json!({
                "ParameterGroupName": "cbor-indefinite",
                "Family": "memorydb_valkey7",
                "Description": "written over the CBOR framing",
                "Tags": [{ "Key": "team", "Value": "platform" }],
            }),
            expected: &[
                ("Name", "cbor-indefinite"),
                ("Family", "memorydb_valkey7"),
                ("Description", "written over the CBOR framing"),
            ],
        },
        Row {
            case: "a create over awsJson, read over rpc-v2-cbor",
            wrote_in: Framing::AwsJson,
            lengths: Lengths::Declared,
            input: json!({
                "ParameterGroupName": "json-written",
                "Family": "memorydb_redis6",
                "Description": "written over the awsJson framing",
            }),
            expected: &[
                ("Name", "json-written"),
                ("Family", "memorydb_redis6"),
                ("Description", "written over the awsJson framing"),
            ],
        },
    ];

    let router = router();

    for row in &rows {
        let created = framed_ok(
            &router,
            row.wrote_in,
            row.lengths,
            "CreateParameterGroup",
            &row.input,
            row.case,
        )
        .await;
        let name = created
            .get("ParameterGroup")
            .unwrap_or_else(|| panic!("case {:?}: expected ParameterGroup in {created:?}", row.case))
            .text_at("Name")
            .to_string();
        assert_eq!(
            name,
            row.input["ParameterGroupName"].as_str().unwrap_or_default(),
            "case {:?}: the create must answer the group it persisted, got {created:?}",
            row.case
        );

        // Read back in BOTH framings: one store, so the two answers are the same value.
        let request = json!({ "ParameterGroupName": name });
        let read_as_cbor = only_group(
            &framed_ok(
                &router,
                Framing::RpcV2Cbor,
                row.lengths,
                "DescribeParameterGroups",
                &request,
                row.case,
            )
            .await,
            row.case,
        );
        let read_as_json = only_group(
            &framed_ok(
                &router,
                Framing::AwsJson,
                row.lengths,
                "DescribeParameterGroups",
                &request,
                row.case,
            )
            .await,
            row.case,
        );
        assert_eq!(
            read_as_cbor, read_as_json,
            "case {:?}: the two framings must report ONE group, got {read_as_cbor:?} over CBOR \
             and {read_as_json:?} over awsJson",
            row.case
        );
        for body in [&read_as_cbor, &read_as_json] {
            for (member, expected) in row.expected {
                assert_eq!(
                    body.text_at(member),
                    *expected,
                    "case {:?}: expected {member} {expected:?}, got {body:?}",
                    row.case
                );
            }
            assert_eq!(
                body.text_at("ARN"),
                format!("arn:aws:memorydb:{REGION}:{ACCOUNT}:parametergroup/{name}"),
                "case {:?}: every framing reports the stored ARN, got {body:?}",
                row.case
            );
        }
        // The tags a CBOR body carried as a list of maps reached the store as tags.
        if let Some(tags) = row.input.get("Tags").and_then(Value::as_array) {
            let listed = framed_ok(
                &router,
                Framing::RpcV2Cbor,
                row.lengths,
                "ListTags",
                &json!({ "ResourceArn": read_as_cbor.text_at("ARN") }),
                row.case,
            )
            .await;
            let reported: Vec<(String, String)> = listed
                .list_at("TagList")
                .iter()
                .map(|tag| {
                    (
                        tag.text_at("Key").to_string(),
                        tag.text_at("Value").to_string(),
                    )
                })
                .collect();
            let expected: Vec<(String, String)> = tags
                .iter()
                .map(|tag| {
                    (
                        tag["Key"].as_str().unwrap_or_default().to_string(),
                        tag["Value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect();
            assert_eq!(
                reported, expected,
                "case {:?}: the tags the CBOR create carried must be the stored tags, got \
                 {listed:?}",
                row.case
            );
        }
    }
}

/// The listing is ONE listing whichever framing pages it: a page size the CBOR framing
/// carried as a CBOR integer bounds the page the same way, and a cursor minted in one
/// framing continues the listing in the other.
#[tokio::test]
async fn the_listing_pages_the_same_way_in_either_framing() {
    let router = router();
    for case in CASES {
        framed_ok(
            &router,
            Framing::RpcV2Cbor,
            Lengths::Declared,
            "CreateParameterGroup",
            &create_body(case),
            case.name,
        )
        .await;
    }
    let expected = expected_listing(&CASES.iter().map(|case| case.name).collect::<Vec<_>>());

    /// The `Name` of every group a decoded page reports, in answer order.
    fn names(page: &Decoded) -> Vec<String> {
        page.list_at("ParameterGroups")
            .iter()
            .map(|group| group.text_at("Name").to_string())
            .collect()
    }

    // A page size at the bottom of the window and one at the top, each carried as a CBOR
    // integer, and each page continued in the OTHER framing.
    for page_size in [WINDOW_MIN, WINDOW_MAX] {
        for (case, lengths) in [
            ("declared lengths", Lengths::Declared),
            ("indefinite lengths", Lengths::Indefinite),
        ] {
            let case = format!("MaxResults={page_size} with {case}");
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut framing = Framing::RpcV2Cbor;
            // At most one page per group, plus one: a page size of at least 1 always makes
            // progress, so this cannot loop forever.
            for _ in 0..=expected.len() {
                let mut body = json!({ "MaxResults": page_size });
                if let Some(token) = token.as_deref() {
                    body["NextToken"] = json!(token);
                }
                let page = framed_ok(
                    &router,
                    framing,
                    lengths,
                    "DescribeParameterGroups",
                    &body,
                    &case,
                )
                .await;
                let page_names = names(&page);
                assert!(
                    i32::try_from(page_names.len()).unwrap_or(i32::MAX) <= page_size,
                    "case {case:?}: a page must hold at most {page_size} groups, got {page:?}"
                );
                seen.extend(page_names);
                token = match page.get("NextToken") {
                    Some(Decoded::Text(cursor)) => Some(cursor.clone()),
                    _ => None,
                };
                if token.is_none() {
                    break;
                }
                // The next page is asked for in the other framing: the cursor belongs to
                // the store, not to the encoding that minted it.
                framing = match framing {
                    Framing::RpcV2Cbor => Framing::AwsJson,
                    Framing::AwsJson => Framing::RpcV2Cbor,
                };
            }
            assert!(
                token.is_none(),
                "case {case:?}: the listing must end, still holding token {token:?}"
            );
            assert_eq!(
                seen, expected,
                "case {case:?}: the pages together must be the whole listing in name order"
            );
        }
    }
}

/// A refused request is refused the same way in either framing, each in that framing's own
/// error envelope.
///
/// The rows differ along what the rule generalises over: a fault the handler words about
/// state (not found, already exists), a value the handler refuses (a page size outside the
/// service's window), and a value the MODEL refuses at the boundary (a name breaking the
/// pattern the model declares), which this service words itself
/// (`crate::service::override_refusal`).
#[tokio::test]
async fn a_refusal_is_worded_the_same_way_in_either_framing() {
    struct Row {
        case: &'static str,
        operation: &'static str,
        input: Value,
        expected_code: &'static str,
    }

    let rows = [
        Row {
            case: "a group name no group answers to",
            operation: "DescribeParameterGroups",
            input: json!({ "ParameterGroupName": "no-such-group" }),
            expected_code: "ParameterGroupNotFoundFault",
        },
        Row {
            case: "a group that is already stored",
            operation: "CreateParameterGroup",
            input: json!({ "ParameterGroupName": CASES[0].name, "Family": CASES[0].family }),
            expected_code: "ParameterGroupAlreadyExistsFault",
        },
        Row {
            case: "a page size outside the window",
            operation: "DescribeParameterGroups",
            input: json!({ "MaxResults": WINDOW_MAX + 1 }),
            expected_code: "InvalidParameterValueException",
        },
        Row {
            case: "a name the model's own pattern refuses",
            operation: "CreateParameterGroup",
            input: json!({ "ParameterGroupName": "-starts-with-a-hyphen", "Family": CASES[0].family }),
            expected_code: "InvalidParameterValueException",
        },
    ];

    let router = router();
    framed_ok(
        &router,
        Framing::AwsJson,
        Lengths::Declared,
        "CreateParameterGroup",
        &create_body(&CASES[0]),
        "seed",
    )
    .await;

    for row in &rows {
        let mut answers = Vec::new();
        for framing in [Framing::AwsJson, Framing::RpcV2Cbor] {
            let answer = framed_call(
                &router,
                framing,
                Lengths::Declared,
                row.operation,
                &row.input,
            )
            .await;
            assert_eq!(
                answer.status, 400,
                "case {:?}: expected {} to be refused with 400 in {framing:?}, got {answer:?}",
                row.case, row.operation
            );
            assert_eq!(
                answer.body.refusal_code(),
                row.expected_code,
                "case {:?}: expected a {} in {framing:?}, got {answer:?}",
                row.case,
                row.expected_code
            );
            if framing == Framing::RpcV2Cbor {
                assert_eq!(
                    answer.smithy_protocol, SMITHY_PROTOCOL.1,
                    "case {:?}: an rpc-v2-cbor refusal declares the framing too, got {answer:?}",
                    row.case
                );
            }
            answers.push((framing, answer));
        }

        // One service, one refusal: the sentence is the same text whichever framing asked.
        let (first_framing, first) = &answers[0];
        let (second_framing, second) = &answers[1];
        let message = |answer: &Answer| {
            let body = &answer.body;
            match body.get("message").or_else(|| body.get("Message")) {
                Some(Decoded::Text(text)) => text.clone(),
                _ => String::new(),
            }
        };
        assert_eq!(
            message(first),
            message(second),
            "case {:?}: {first_framing:?} and {second_framing:?} must word one refusal the same \
             way, got {first:?} and {second:?}",
            row.case
        );
        assert!(
            !message(first).is_empty(),
            "case {:?}: a refusal states why, got {first:?}",
            row.case
        );
    }
}

/// A request that names NEITHER of the protocols the model declares names no wire form
/// this service can read, and is refused before any operation is asked for it.
///
/// With two protocols declared, which one carries a call follows from the request's own
/// marker — the awsJson target header or media type, or the CBOR header or path — so a
/// request carrying none of them is not a call of this service at all.
#[tokio::test]
async fn a_request_naming_no_declared_protocol_is_refused() {
    let router = router();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://memory-db.{REGION}.amazonaws.com/"))
        // A media type neither framing is named by, and no target header and no CBOR path.
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{REGION}/memorydb/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from_static(b"{}"))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(&response.into_body()).to_string();
    assert_eq!(
        status, 404,
        "case {:?}: a request naming no declared protocol must not be served, got {status} \
         with {body:?}",
        "no protocol marker"
    );
}

/// A `@timestamp` member is carried in each framing's OWN form and reports the same
/// instant in both: the CBOR framing tags it as an instant, the awsJson one writes epoch
/// seconds, and a store written over either framing reports one event log to both.
///
/// `DescribeEvents` is the reader that carries one (`Event$Date`), and a parameter-group
/// create is one of the changes the log records, so the rows differ along which framing
/// made the change and which framing reads the log.
#[tokio::test]
async fn an_event_instant_is_carried_in_each_framings_own_form() {
    /// How far apart two encodings of ONE stored instant may read: they are the same
    /// value, so this is float slack and not a timing allowance.
    const SAME_INSTANT: f64 = 1.0;

    let router = router();
    for (framing, case) in [
        (Framing::RpcV2Cbor, "a change made over rpc-v2-cbor"),
        (Framing::AwsJson, "a change made over awsJson"),
    ] {
        let name = format!("event-group-{}", if framing == Framing::AwsJson { 1 } else { 0 });
        framed_ok(
            &router,
            framing,
            Lengths::Declared,
            "CreateParameterGroup",
            &json!({ "ParameterGroupName": &name, "Family": CASES[0].family }),
            case,
        )
        .await;

        // Read the log in BOTH framings: one log, so the two answers name the same events.
        let request = json!({ "SourceType": "parameter-group", "SourceName": &name });
        let mut instants = Vec::new();
        for reader in [Framing::RpcV2Cbor, Framing::AwsJson] {
            let listed = framed_ok(
                &router,
                reader,
                Lengths::Declared,
                "DescribeEvents",
                &request,
                case,
            )
            .await;
            let events = listed.list_at("Events");
            assert_eq!(
                events.len(),
                1,
                "case {case:?}: {reader:?} must report the one change to {name:?}, got {listed:?}"
            );
            assert_eq!(
                events[0].text_at("SourceName"),
                name,
                "case {case:?}: {reader:?} must name the changed group, got {listed:?}"
            );
            assert_eq!(
                events[0].text_at("Message"),
                format!("Create parameter group {name} operation completed."),
                "case {case:?}: {reader:?} must word the event the same way, got {listed:?}"
            );
            let instant = match events[0].get("Date") {
                Some(Decoded::Number(seconds)) => *seconds,
                other => panic!("case {case:?}: {reader:?} reports no instant, got {other:?}"),
            };
            assert!(
                instant > 0.0,
                "case {case:?}: {reader:?} must report the instant of the change, got {instant}"
            );
            instants.push(instant);
        }
        assert!(
            (instants[0] - instants[1]).abs() < SAME_INSTANT,
            "case {case:?}: both framings must report ONE stored instant, got {:?} over CBOR \
             and {:?} over awsJson",
            instants[0],
            instants[1]
        );
    }
}
