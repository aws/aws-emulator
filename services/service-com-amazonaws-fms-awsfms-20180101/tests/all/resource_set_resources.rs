// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListResourceSetResources`.
//!
//! Every test drives the service the way a client does: an awsJson1_1 `POST /`
//! through [`MultiServiceRouter::dispatch`], with the `X-Amz-Target` header and a
//! SigV4 credential scope, so the whole deserialize → validate → handle →
//! serialize walk runs and the assertions are about the WIRE response.
//!
//! The rule under test is one sentence: the operation REPORTS, in a stable total
//! order and without changing anything, exactly the resources currently
//! associated with the resource set the request names — each with the account it
//! belongs to, derived from its own URI — bounded by the page-size rule every
//! paginated operation of
//! this service shares, carrying a `NextToken` only while members remain after
//! the page, and refusing an identifier that names no stored set
//! (`ResourceNotFoundException`) or a token it did not mint
//! (`InvalidInputException`).
//!
//! The table in [`cases`] is the class that rule ranges over: the rows differ in
//! how many resources the set holds (none, one, several, more than the smallest
//! page), in how many resource types it declares, in the ARN forms of its
//! members (an id-only resource field, a `type/name` one, and one whose resource
//! field carries slashes), in which accounts of the organization those resources
//! belong to, and in whether the members are associated in or out of URI order —
//! including ARN forms whose owning service leaves the ACCOUNT field empty (an
//! API Gateway stage's, a global service's), whose resource belongs to the
//! account this emulator holds it in rather than to no account at all.
//!
//! Two rules this suite does not restate, because a shared suite already asserts
//! them over EVERY operation they hold for: the administrator gate
//! (`administrator_gated_operations`, whose table carries a row for this
//! operation) and the page-size rule itself (`page_size_resolution`, likewise).
//! What is asserted here is the paging of THIS collection, end to end.

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::state;
use smithy_server_runtime::MultiServiceRouter;

// The wire harness lives in one place for this test target: `policy_lifecycle`
// owns it, and the administrator helper lives with the gate suite — so no two
// suites drift in how a request is signed or an error envelope is read.
use crate::administrator_gated_operations::{associate_admin, error_message, with_store};
use crate::policy_lifecycle::{call, emulator_with_engine, error_code, ACCOUNT, REGION};
use crate::tagging_and_managing_admins::minimal_policy;

/// A `Base62Id` (the model's `com.amazonaws.fms#Base62Id`: 22 characters
/// matching `^[a-z0-9A-Z]{22}$`) that names no resource set.
const UNKNOWN_RESOURCE_SET_ID: &str = "0123456789AbCdEfGhIjKl";

/// Two further accounts of the organization, put there by the policy scope
/// [`seeded_emulator`] stores, so a member resource can belong to an account
/// other than the administrator's own and `AccountId` varies across the rows.
///
/// Both are `AWSAccountId`s as the model declares them
/// (`com.amazonaws.fms#AWSAccountId`: length 1..1024, pattern `^[0-9]+$`).
const MEMBER_ACCOUNTS: [&str; 2] = ["111111111111", "222222222222"];

/// An EC2 instance ARN, whose resource field names only an id.
///
/// `pub(crate)` so a sibling suite in this test target builds a member URI
/// through this one helper rather than a second copy of the ARN form.
pub(crate) fn instance(account: &str, index: usize) -> String {
    format!("arn:aws:ec2:{REGION}:{account}:instance/i-{index:017x}")
}

/// A Network Firewall ARN, whose resource field is a `type/name` pair.
fn firewall(account: &str, name: &str) -> String {
    format!("arn:aws:network-firewall:{REGION}:{account}:firewall/{name}")
}

/// An Application Load Balancer ARN, whose resource field carries several
/// slashes — so the URI is not a value a naive splitter keeps whole.
fn load_balancer(account: &str, name: &str) -> String {
    format!("arn:aws:elasticloadbalancing:{REGION}:{account}:loadbalancer/app/{name}/50dc6c495c0c9188")
}

/// An API Gateway stage ARN, whose ACCOUNT field that service leaves EMPTY
/// (`arn:aws:apigateway:<region>::/restapis/<id>/stages/<name>`) and whose
/// resource field is a path naming the type in the plural.
fn api_gateway_stage(api: &str, stage: &str) -> String {
    format!("arn:aws:apigateway:{REGION}::/restapis/{api}/stages/{stage}")
}

/// A CloudFront distribution ARN: CloudFront is global, so its Region field is
/// empty; `account` is written as given, so one row can carry both the form that
/// names an account and the form that leaves that field empty too.
fn distribution(account: &str, id: &str) -> String {
    format!("arn:aws:cloudfront::{account}:distribution/{id}")
}

/// One row of the membership table: a name for the assertion messages, the set's
/// `ResourceTypeList`, and the resources associated with it, in the order the
/// request associates them.
struct Case {
    name: &'static str,
    resource_types: Vec<&'static str>,
    members: Vec<String>,
}

impl Case {
    /// The `(URI, AccountId)` pairs the rule says the operation reports: every
    /// member, in ascending URI order, each carrying the account it BELONGS TO —
    /// the account field of its own ARN where the owning service writes one, and
    /// the account this emulator holds the resource in where that service leaves
    /// the field empty, which is the owner `ListDiscoveredResources` reports for
    /// the same resource.
    fn expected(&self) -> Vec<(String, Option<String>)> {
        let mut expected: Vec<(String, Option<String>)> = self
            .members
            .iter()
            .map(|uri| {
                let account = uri.split(':').nth(4).unwrap_or_default();
                let owner = if account.is_empty() {
                    ACCOUNT.to_string()
                } else {
                    account.to_string()
                };
                (uri.clone(), Some(owner))
            })
            .collect();
        expected.sort();
        expected.dedup();
        expected
    }
}

/// The class the reporting rule is asserted over.
///
/// The resource types are drawn from this service's own catalogue of the types
/// Firewall Manager works with (`state::RESOURCE_TYPE_*`, whose entries are the
/// `Policy$ResourceType` form), and each row's members match the types its set
/// declares, because `ResourceSet$ResourceTypeList` "determines the resources
/// that can be associated to the resource set" — so every row's members really
/// are its membership.
fn cases() -> Vec<Case> {
    vec![
        Case {
            // The smallest membership there is: the set exists and holds nothing.
            name: "holds-nothing",
            resource_types: vec![state::RESOURCE_TYPE_EC2_INSTANCE],
            members: Vec::new(),
        },
        Case {
            name: "holds-one-resource-of-the-administrators-account",
            resource_types: vec![state::RESOURCE_TYPE_EC2_INSTANCE],
            members: vec![instance(ACCOUNT, 0)],
        },
        Case {
            // Three accounts, so `AccountId` is not one constant.
            name: "holds-three-resources-of-three-accounts",
            resource_types: vec![state::RESOURCE_TYPE_EC2_INSTANCE],
            members: vec![
                instance(ACCOUNT, 1),
                instance(MEMBER_ACCOUNTS[0], 2),
                instance(MEMBER_ACCOUNTS[1], 3),
            ],
        },
        Case {
            // Three resource types, and three ARN forms of the resource field.
            name: "holds-three-resource-types",
            resource_types: vec![
                state::RESOURCE_TYPE_EC2_INSTANCE,
                state::RESOURCE_TYPE_NETWORK_FIREWALL_FIREWALL,
                state::RESOURCE_TYPE_ELBV2_LOAD_BALANCER,
            ],
            members: vec![
                load_balancer(MEMBER_ACCOUNTS[0], "web"),
                instance(ACCOUNT, 4),
                firewall(ACCOUNT, "edge"),
            ],
        },
        Case {
            // More members than the smallest page, associated in DESCENDING URI
            // order, so the reported order is the store's and not the caller's.
            name: "holds-seven-resources-associated-out-of-order",
            resource_types: vec![state::RESOURCE_TYPE_EC2_INSTANCE],
            members: (0..7)
                .rev()
                .map(|index| instance(MEMBER_ACCOUNTS[index % MEMBER_ACCOUNTS.len()], index))
                .collect(),
        },
        Case {
            // An ARN whose owning service leaves the ACCOUNT field empty: the
            // resource belongs to the account this emulator holds it in, which
            // is the owner the inventory reports for the same resource.
            name: "holds-an-account-less-resource",
            resource_types: vec![state::RESOURCE_TYPE_APIGATEWAY_STAGE],
            members: vec![api_gateway_stage("a1b2c3d4e5", "prod")],
        },
        Case {
            // The account-carrying and the account-less ARN forms side by side,
            // across three owning services: an ARN naming every field, a global
            // ARN naming an account, a global ARN naming none, and an
            // account-less ARN.
            name: "holds-account-less-and-account-carrying-resources",
            resource_types: vec![
                state::RESOURCE_TYPE_APIGATEWAY_STAGE,
                state::RESOURCE_TYPE_CLOUDFRONT_DISTRIBUTION,
                state::RESOURCE_TYPE_EC2_INSTANCE,
            ],
            members: vec![
                api_gateway_stage("zz9pza", "pre-release"),
                distribution(ACCOUNT, "E1EXAMPLE1EXAM"),
                distribution("", "E2EXAMPLE2EXAM"),
                instance(MEMBER_ACCOUNTS[1], 8),
            ],
        },
        Case {
            // Associating a URI twice is associating it once: the membership is a
            // SET, so the report carries it once too.
            name: "holds-a-resource-named-twice",
            resource_types: vec![state::RESOURCE_TYPE_EC2_INSTANCE],
            members: vec![instance(ACCOUNT, 9), instance(ACCOUNT, 9)],
        },
    ]
}

/// A fresh emulator with an administrator associated and a policy whose
/// `IncludeMap` names [`MEMBER_ACCOUNTS`], which is what makes them accounts of
/// the organization — and so makes their resources associable.
///
/// `pub(crate)` so a sibling suite in this test target stands the same
/// organization up through this one helper.
pub(crate) async fn seeded_emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>)
{
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;
    let mut scoped = minimal_policy("resource-set-resources-scope");
    scoped["IncludeMap"] = json!({ "ACCOUNT": MEMBER_ACCOUNTS });
    let (status, body) = call(&router, "PutPolicy", &json!({ "Policy": scoped })).await;
    assert_eq!(
        status, 200,
        "the scope policy must be stored, got {status} with {body}"
    );
    (router, engine)
}

/// Create a resource set declaring `resource_types` and associate `members` with
/// it, returning the minted `(Id, ResourceSetArn)`.
///
/// `pub(crate)` so a sibling suite in this test target creates a populated set
/// through this one writer rather than a second copy of it.
pub(crate) async fn create_set_with_members(
    router: &MultiServiceRouter,
    case_name: &str,
    resource_types: &[&str],
    members: &[String],
) -> (String, String) {
    let (status, created) = call(
        router,
        "PutResourceSet",
        &json!({
            "ResourceSet": {
                "Name": format!("set-{case_name}"),
                "ResourceTypeList": resource_types,
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case_name:?}: expected PutResourceSet to answer 200, got {status} with {created}"
    );
    let id = created["ResourceSet"]["Id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let arn = created["ResourceSetArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !id.is_empty() && !arn.is_empty(),
        "case {case_name:?}: expected a minted Id and ARN, got {created}"
    );

    if !members.is_empty() {
        let (status, body) = call(
            router,
            "BatchAssociateResource",
            &json!({ "ResourceSetIdentifier": id, "Items": members }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case_name:?}: expected BatchAssociateResource to answer 200, got {status} with \
             {body}"
        );
        assert_eq!(
            body["FailedItems"],
            json!([]),
            "case {case_name:?}: every seeded member must associate, got {body}"
        );
    }
    (id, arn)
}

/// The `(URI, AccountId)` pairs one `Items` page carries, in the order it served
/// them.
fn reported(case: &str, body: &Value) -> Vec<(String, Option<String>)> {
    body["Items"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: the response must carry an Items list, got {body}"))
        .iter()
        .map(|item| {
            let uri = item["URI"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: every item must carry a URI, got {body}"))
                .to_string();
            let account = item
                .get("AccountId")
                .and_then(Value::as_str)
                .map(str::to_string);
            (uri, account)
        })
        .collect()
}

/// Read every page of one resource set's membership at one page size, following
/// `NextToken`, and return the pairs in the order they were served.
///
/// Asserts the page invariants on EVERY page: a page never holds more than the
/// effective bound, a page that carries a token is exactly that bound long (so a
/// bound can never hide a member behind a short page), and the last page carries
/// no token.
async fn page_through(
    router: &MultiServiceRouter,
    case_name: &str,
    identifier: &str,
    max_results: Option<usize>,
) -> Vec<(String, Option<String>)> {
    // The largest page the model's `com.amazonaws.fms#PaginationMaxResults`
    // allows (range 1..100), read off the model rather than imported from
    // `state::MAX_PAGE_SIZE`, so the two disagreeing fails here.
    const MAX_PAGE_SIZE: usize = 100;
    let case = format!("{case_name} at MaxResults={max_results:?}");
    let bound = max_results.unwrap_or(MAX_PAGE_SIZE).min(MAX_PAGE_SIZE);
    let mut served: Vec<(String, Option<String>)> = Vec::new();
    let mut next_token: Option<String> = None;
    for page in 0..=MAX_PAGE_SIZE {
        let mut request = json!({ "Identifier": identifier });
        if let Some(max) = max_results {
            request["MaxResults"] = json!(max);
        }
        if let Some(token) = next_token.as_deref() {
            request["NextToken"] = json!(token);
        }
        let (status, body) = call(router, "ListResourceSetResources", &request).await;
        assert_eq!(
            status, 200,
            "case {case}: page {page} must answer 200, got {status} with {body}"
        );
        let items = reported(&case, &body);
        assert!(
            items.len() <= bound,
            "case {case}: page {page} must hold at most {bound} items, got {items:?}"
        );
        served.extend(items.iter().cloned());
        next_token = body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            return served;
        }
        assert_eq!(
            items.len(),
            bound,
            "case {case}: page {page} carried a NextToken, so it must be a full page of {bound}, \
             got {items:?}"
        );
    }
    panic!(
        "case {case}: NextToken never cleared, having served {} items",
        served.len()
    );
}

/// The operation reports exactly the set's membership, each resource carrying the
/// account its ARN names, in ascending URI order — for every row of the table,
/// and without reporting the membership of another set in the same store.
#[tokio::test]
async fn reports_exactly_the_membership_with_each_resources_account() {
    for case in cases() {
        let (router, _engine) = seeded_emulator().await;
        let (id, _) =
            create_set_with_members(&router, case.name, &case.resource_types, &case.members).await;
        // A second set holding a resource the first one does not, so a handler
        // that reported "every resource in the store" would fail here.
        let decoy_member = firewall(ACCOUNT, "decoy");
        let (decoy_id, _) = create_set_with_members(
            &router,
            "decoy",
            &[state::RESOURCE_TYPE_NETWORK_FIREWALL_FIREWALL],
            std::slice::from_ref(&decoy_member),
        )
        .await;

        let (status, body) = call(
            &router,
            "ListResourceSetResources",
            &json!({ "Identifier": id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected ListResourceSetResources to answer 200, got {status} with {body}",
            case.name
        );
        let expected = case.expected();
        assert_eq!(
            reported(case.name, &body),
            expected,
            "case {:?}: expected the membership {expected:?}, got {body}",
            case.name
        );
        assert!(
            body.get("NextToken").is_none_or(Value::is_null),
            "case {:?}: a page that holds the whole membership carries no NextToken, got {body}",
            case.name
        );

        let (status, decoy) = call(
            &router,
            "ListResourceSetResources",
            &json!({ "Identifier": decoy_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the second set must be reported too, got {status} with {decoy}",
            case.name
        );
        assert_eq!(
            reported("decoy", &decoy),
            vec![(decoy_member.clone(), Some(ACCOUNT.to_string()))],
            "case {:?}: each set reports its OWN membership, got {decoy}",
            case.name
        );
    }
}

/// Every page size serves the same membership once, in the same order, and the
/// members a bound keeps off a page are reachable through that page's
/// `NextToken`.
///
/// The sizes span the modelled range: its published minimum, sizes that divide
/// the membership evenly and unevenly, exactly the membership's size, one more
/// than it, the published maximum, and no bound at all.
#[tokio::test]
async fn every_page_size_serves_the_whole_membership_once() {
    for case in cases() {
        let (router, _engine) = seeded_emulator().await;
        let (id, _) =
            create_set_with_members(&router, case.name, &case.resource_types, &case.members).await;
        let expected = case.expected();
        let held = expected.len();

        for size in [
            Some(1),
            Some(2),
            Some(3),
            Some(held.max(1)),
            Some(held + 1),
            Some(100),
            None,
        ] {
            let served = page_through(&router, case.name, &id, size).await;
            assert_eq!(
                served, expected,
                "case {:?} at MaxResults={size:?}: every page size must serve the whole \
                 membership once, in ascending URI order",
                case.name
            );
        }
    }
}

/// What the operation reports follows the membership the batch operations write,
/// and reporting it changes nothing a later caller sees.
#[tokio::test]
async fn the_report_follows_the_membership_and_changes_nothing() {
    let (router, engine) = seeded_emulator().await;
    let members: Vec<String> = (0..3).map(|index| instance(ACCOUNT, index)).collect();
    let (id, _) = create_set_with_members(
        &router,
        "follows-membership",
        &[state::RESOURCE_TYPE_EC2_INSTANCE],
        &[],
    )
    .await;

    // Before any association the set reports an empty membership — the modelled
    // `Items` is required, so it is a list, not an absent member.
    let (status, body) = call(
        &router,
        "ListResourceSetResources",
        &json!({ "Identifier": id }),
    )
    .await;
    assert_eq!(
        (status, &body["Items"]),
        (200, &json!([])),
        "a set that holds nothing must report an empty Items list, got {status} with {body}"
    );

    let (status, body) = call(
        &router,
        "BatchAssociateResource",
        &json!({ "ResourceSetIdentifier": id, "Items": members }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the members must associate, got {status} with {body}"
    );
    let (_, body) = call(
        &router,
        "ListResourceSetResources",
        &json!({ "Identifier": id }),
    )
    .await;
    assert_eq!(
        reported("after-association", &body),
        members
            .iter()
            .map(|uri| (uri.clone(), Some(ACCOUNT.to_string())))
            .collect::<Vec<(String, Option<String>)>>(),
        "every associated member must be reported, got {body}"
    );

    // The read is answered from the store the batch operations write, so a
    // disassociation is reported with no propagation step.
    let (status, body) = call(
        &router,
        "BatchDisassociateResource",
        &json!({ "ResourceSetIdentifier": id, "Items": [members[1].clone()] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the member must disassociate, got {status} with {body}"
    );

    // Reporting is a READ: the document, its update token and its last-update
    // instant are the same after two listings as before them, and the second
    // listing answers exactly what the first did.
    let (_, before) = call(&router, "GetResourceSet", &json!({ "Identifier": id })).await;
    let (_, first) = call(
        &router,
        "ListResourceSetResources",
        &json!({ "Identifier": id }),
    )
    .await;
    let (_, again) = call(
        &router,
        "ListResourceSetResources",
        &json!({ "Identifier": id }),
    )
    .await;
    let (_, after) = call(&router, "GetResourceSet", &json!({ "Identifier": id })).await;
    assert_eq!(
        reported("after-disassociation", &first),
        vec![
            (members[0].clone(), Some(ACCOUNT.to_string())),
            (members[2].clone(), Some(ACCOUNT.to_string())),
        ],
        "the disassociated member must be gone from the report, got {first}"
    );
    assert_eq!(
        first, again,
        "reading the membership twice must answer the same page, got {again}"
    );
    assert_eq!(
        before, after,
        "reporting the membership must not change what GetResourceSet reports, got {after}"
    );
    // And it wrote nothing to the record the handlers share.
    let held = with_store(&engine, |store| {
        store.resource_sets[&id]
            .resources
            .iter()
            .cloned()
            .collect::<Vec<String>>()
    });
    assert_eq!(
        held,
        vec![members[0].clone(), members[2].clone()],
        "the stored membership must be untouched by the reads, got {held:?}"
    );
}

/// The resource set can be named by its `Id` or by its ARN — this operation's
/// `Identifier` is the model's 1..1024 `ResourceId`, not the 22-character
/// `Base62Id` `GetResourceSet` takes — and an identifier that names no stored set
/// is `ResourceNotFoundException`.
#[tokio::test]
async fn the_identifier_is_resolved_by_id_or_arn_and_an_unknown_one_is_not_found() {
    let (router, _engine) = seeded_emulator().await;
    let member = instance(ACCOUNT, 0);
    let (id, arn) = create_set_with_members(
        &router,
        "identifiers",
        &[state::RESOURCE_TYPE_EC2_INSTANCE],
        std::slice::from_ref(&member),
    )
    .await;

    for identifier in [id.as_str(), arn.as_str()] {
        let (status, body) = call(
            &router,
            "ListResourceSetResources",
            &json!({ "Identifier": identifier }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {identifier:?}: the set must be reportable by this identifier, got {status} \
             with {body}"
        );
        assert_eq!(
            reported(identifier, &body),
            vec![(member.clone(), Some(ACCOUNT.to_string()))],
            "case {identifier:?}: both identifiers must name the same membership, got {body}"
        );
    }

    // Well-formed identifiers that name nothing, drawn from each character class
    // the id's own pattern allows, plus a resource-set ARN of an unknown id and a
    // value that is neither.
    for unknown in [
        UNKNOWN_RESOURCE_SET_ID,
        "0000000000000000000001",
        "abcdefghijklmnopqrstuv",
        "ABCDEFGHIJKLMNOPQRSTUV",
        &state::resource_set_arn(REGION, UNKNOWN_RESOURCE_SET_ID),
        "no-such-resource-set",
    ] {
        let (status, body) = call(
            &router,
            "ListResourceSetResources",
            &json!({ "Identifier": unknown }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {unknown:?}: an unknown identifier must be ResourceNotFoundException, got \
             {status} with {body}"
        );
        assert_eq!(
            error_message(&body),
            state::RESOURCE_NOT_FOUND_MESSAGE,
            "case {unknown:?}: the refusal must carry the service's not-found message, got {body}"
        );
    }
}

/// A `MaxResults` outside the modelled range is refused by the boundary, and a
/// `NextToken` this service did not mint for a membership page is refused rather
/// than resumed from as an arbitrary key.
#[tokio::test]
async fn an_out_of_range_page_size_or_a_foreign_token_is_refused() {
    let (router, _engine) = seeded_emulator().await;
    let members: Vec<String> = (0..3).map(|index| instance(ACCOUNT, index)).collect();
    let (id, _) = create_set_with_members(
        &router,
        "tokens",
        &[state::RESOURCE_TYPE_EC2_INSTANCE],
        &members,
    )
    .await;

    // One past each published bound of MaxResults (1..100).
    for out_of_range in [0, 101] {
        let (status, body) = call(
            &router,
            "ListResourceSetResources",
            &json!({ "Identifier": id, "MaxResults": out_of_range }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case MaxResults={out_of_range}: a value outside the modelled 1..100 must be refused, \
             got {status} with {body}"
        );
    }

    // Tokens that are no membership boundary: a value that is not a token at
    // all, and well-formed tokens for OTHER page families (a resource set's
    // `Base62Id`, an account id, a `PolicyId`-shaped key) together with the hex
    // of a value that is not an ARN — none of which may be resumed from as a
    // member URI.
    for token in [
        "not-a-token-this-service-minted".to_string(),
        state::encode_key_token(UNKNOWN_RESOURCE_SET_ID),
        state::encode_key_token(ACCOUNT),
        state::encode_key_token("00000000-0000-0000-0000-000000000000"),
        state::encode_key_token("arn:aws"),
    ] {
        let (status, body) = call(
            &router,
            "ListResourceSetResources",
            &json!({ "Identifier": id, "NextToken": token }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidInputException"),
            "case {token:?}: a NextToken this service did not mint for a membership page must be \
             refused, got {status} with {body}"
        );
    }

    // A token the service DID mint round-trips, so the refusals above are not
    // refusing every token.
    let (status, body) = call(
        &router,
        "ListResourceSetResources",
        &json!({ "Identifier": id, "MaxResults": 1 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the first page must be served, got {status} with {body}"
    );
    let minted = body["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a bounded first page must carry a NextToken, got {body}"))
        .to_string();
    let (status, body) = call(
        &router,
        "ListResourceSetResources",
        &json!({ "Identifier": id, "MaxResults": 1, "NextToken": minted }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {minted:?}: a minted NextToken must name the next page, got {status} with {body}"
    );
    assert_eq!(
        reported("second-page", &body),
        vec![(members[1].clone(), Some(ACCOUNT.to_string()))],
        "the minted token must resume at the member after the first page, got {body}"
    );
}
