// SPDX-License-Identifier: CC0-1.0

//! The Multi-Region Access Point SUB-RESOURCES — the policy, the policy status
//! and the routing configuration — and the envelope every refusal of this
//! service answers in, driven end to end over HTTP.
//!
//! Three rules are under test here.
//!
//! * A route whose URI template puts LITERAL segments after a greedy
//!   `@httpLabel` is matched by reading those literals off the end of the path
//!   first, so the label binds the resource between the leading literals and
//!   them. Every such route of this model is a Multi-Region Access Point
//!   sub-resource, and the table of them is re-derived from the model below, so
//!   the rule is asserted over the whole class rather than over the one route
//!   that showed it.
//! * A refusal states the CLASS of failure in fixed words and the INSTANCE in
//!   the members beside the message — the name the request named and the
//!   account it named it under — and every refusal names the request it answers
//!   with a `RequestId` and a `HostId` beside `<Error>`, mirrored in the two
//!   `x-amz-` correlation headers.
//! * An asynchronous request token is RESOLVED against the requests this
//!   service holds rather than checked against a shape: whatever its text, a
//!   token that names no request is answered the one not-found refusal.
//!
//! Every row below differs along something those rules generalise over: the
//! length and shape of the access point name, the account that owns it, the
//! Region the call is signed for, which sub-resource is read, how many buckets
//! the access point associates, the bytes of the policy document put on it, and
//! the form of the request token presented.
//!
//! The router, the request builder and the XML readers are the sibling suites',
//! reused rather than copied.

use std::collections::BTreeSet;

use crate::access_point_lifecycle::{
    assert_foreign_account_refused, call_in_region, xml_escape, Emulator, Reply, ACCOUNT,
    OTHER_ACCOUNT, REGION, XMLNS,
};
use crate::multi_region_access_point_lifecycle::{create, describe, settled_wait, CreateCase};
use crate::storage_lens_configuration::{leaves_of, Leaf};

use service_com_amazonaws_s3control_awss3controlservicev20180820::service::routes::{
    GreedySuffixRoute, GREEDY_SUFFIX_ROUTES,
};
use service_com_amazonaws_s3control_awss3controlservicev20180820::state::{
    NO_SUCH_ASYNC_REQUEST_MESSAGE, TRAFFIC_DIAL_ACTIVE,
};

/// A second signing Region, so no row's answer can depend on the one the
/// sibling suites sign for.
const OTHER_REGION: &str = "eu-central-1";

/// The message every absent-Multi-Region-Access-Point refusal carries: fixed
/// text, naming neither the access point nor the account — those travel in the
/// members beside it.
const NO_SUCH_MRAP_MESSAGE: &str = "The specified Multi-Region Access Point does not exist.";

/// The code every absent-Multi-Region-Access-Point refusal carries.
const NO_SUCH_MRAP_CODE: &str = "NoSuchMultiRegionAccessPoint";

/// The code and message a routing read answers for an `Mrap` it cannot resolve.
const INVALID_REQUEST_CODE: &str = "InvalidRequest";
const INVALID_REQUEST_MESSAGE: &str = "Request invalid";

/// The code a request token that names no asynchronous request is refused with.
const NO_SUCH_ASYNC_REQUEST_CODE: &str = "NoSuchAsyncRequest";

/// The longest name the modelled `MultiRegionAccessPointName` admits (`len
/// 0..50`, pattern `^[a-z0-9][-a-z0-9]{1,48}[a-z0-9]$`).
const LONGEST_NAME: &str = "mrapsubresourcemrapsubresourcemrapsubresource50abc";

/// The shortest name that pattern admits.
const SHORTEST_NAME: &str = "abc";

/// One response read as this service's refusal envelope.
struct Refusal {
    status: u16,
    code: String,
    message: String,
    /// Every leaf inside `<Error>` that is not `Type`, `Code` or `Message`:
    /// the members the refusal names the instance in.
    members: Vec<Leaf>,
    request_id: String,
    host_id: String,
}

/// `reply` read as a refusal envelope. The elements are read by PATH, so a
/// member inside `<Error>` is told apart from the correlation pair beside it.
fn refusal_of(case: &str, reply: &Reply) -> Refusal {
    let leaves = leaves_of(&reply.body);
    let text = |path: &str| {
        leaves
            .iter()
            .find(|(leaf, _)| leaf == path)
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    };
    assert!(
        reply.body.contains("<ErrorResponse"),
        "case {case:?}: expected an ErrorResponse envelope, got {} with body {}",
        reply.status,
        reply.body
    );
    Refusal {
        status: reply.status,
        code: text("Error/Code"),
        message: text("Error/Message"),
        members: leaves
            .iter()
            .filter(|(path, _)| {
                path.starts_with("Error/")
                    && !matches!(path.as_str(), "Error/Type" | "Error/Code" | "Error/Message")
            })
            .cloned()
            .collect(),
        request_id: text("RequestId"),
        host_id: text("HostId"),
    }
}

/// Whether every character of `value` is a hex digit of `case`.
fn is_hex(value: &str, upper: bool) -> bool {
    !value.is_empty()
        && value.chars().all(|character| {
            character.is_ascii_hexdigit()
                && (character.is_ascii_digit()
                    || (upper && character.is_ascii_uppercase())
                    || (!upper && character.is_ascii_lowercase()))
        })
}

/// Assert that `reply` names the request it answers: the two elements beside
/// `<Error>`, in the shape an S3-family service renders them, mirrored by the
/// two correlation headers.
fn assert_names_the_request(case: &str, reply: &Reply, refusal: &Refusal) {
    assert_eq!(
        (refusal.request_id.len(), is_hex(&refusal.request_id, true)),
        (16, true),
        "case {case:?}: expected a 16-character upper-case hex RequestId, got {:?} in {}",
        refusal.request_id,
        reply.body
    );
    assert_eq!(
        (refusal.host_id.len(), is_hex(&refusal.host_id, false)),
        (64, true),
        "case {case:?}: expected a 64-character lower-case hex HostId, got {:?} in {}",
        refusal.host_id,
        reply.body
    );
    assert_eq!(
        reply.header("x-amz-request-id").as_deref(),
        Some(refusal.request_id.as_str()),
        "case {case:?}: the x-amz-request-id header must mirror the element, got {:?}",
        reply.header("x-amz-request-id")
    );
    assert_eq!(
        reply.header("x-amz-id-2").as_deref(),
        Some(refusal.host_id.as_str()),
        "case {case:?}: the x-amz-id-2 header must mirror the element, got {:?}",
        reply.header("x-amz-id-2")
    );
}

/// The members an absent-access-point refusal must name: the name the request
/// named, and the account it named it under.
fn named_members(name: &str, account: &str) -> Vec<Leaf> {
    vec![
        (
            "Error/MultiRegionAccessPointName".to_string(),
            name.to_string(),
        ),
        ("Error/AccountId".to_string(), account.to_string()),
    ]
}

async fn get_access_point(emulator: &Emulator, region: &str, account: &str, name: &str) -> Reply {
    call_in_region(
        &emulator.router,
        region,
        "GET",
        &format!("/v20180820/mrap/instances/{name}"),
        account,
        "",
    )
    .await
}

async fn get_policy(emulator: &Emulator, region: &str, account: &str, name: &str) -> Reply {
    call_in_region(
        &emulator.router,
        region,
        "GET",
        &format!("/v20180820/mrap/instances/{name}/policy"),
        account,
        "",
    )
    .await
}

async fn get_policy_status(emulator: &Emulator, region: &str, account: &str, name: &str) -> Reply {
    call_in_region(
        &emulator.router,
        region,
        "GET",
        &format!("/v20180820/mrap/instances/{name}/policystatus"),
        account,
        "",
    )
    .await
}

/// The routing configuration of the Multi-Region Access Point `mrap` resolves
/// to, read through its own route.
///
/// `pub(crate)`, with [`route_entries`] and [`leaf`] below: the sibling suite
/// that SUBMITS routing changes reads them back through the same call rather
/// than building a second request.
pub(crate) async fn get_routes(emulator: &Emulator, region: &str, account: &str, mrap: &str) -> Reply {
    call_in_region(
        &emulator.router,
        region,
        "GET",
        &format!("/v20180820/mrap/instances/{mrap}/routes"),
        account,
        "",
    )
    .await
}

async fn put_policy(
    emulator: &Emulator,
    region: &str,
    account: &str,
    name: &str,
    client_token: &str,
    document: &str,
) -> Reply {
    let body = format!(
        "<PutMultiRegionAccessPointPolicyRequest xmlns=\"{XMLNS}\">\
         <ClientToken>{}</ClientToken><Details><Name>{}</Name><Policy>{}</Policy></Details>\
         </PutMultiRegionAccessPointPolicyRequest>",
        xml_escape(client_token),
        xml_escape(name),
        xml_escape(document)
    );
    call_in_region(
        &emulator.router,
        region,
        "POST",
        "/v20180820/async-requests/mrap/put-policy",
        account,
        &body,
    )
    .await
}

async fn delete_access_point(
    emulator: &Emulator,
    region: &str,
    account: &str,
    name: &str,
    client_token: &str,
) -> Reply {
    let body = format!(
        "<DeleteMultiRegionAccessPointRequest xmlns=\"{XMLNS}\">\
         <ClientToken>{}</ClientToken><Details><Name>{}</Name></Details>\
         </DeleteMultiRegionAccessPointRequest>",
        xml_escape(client_token),
        xml_escape(name)
    );
    call_in_region(
        &emulator.router,
        region,
        "POST",
        "/v20180820/async-requests/mrap/delete",
        account,
        &body,
    )
    .await
}

/// The text of the one leaf at `path`, or `None` when the answer carries none.
pub(crate) fn leaf(leaves: &[Leaf], path: &str) -> Option<String> {
    leaves
        .iter()
        .find(|(leaf_path, _)| leaf_path == path)
        .map(|(_, value)| value.clone())
}

/// The routes a routing read reported, as (bucket, Region, traffic dial) in
/// document order.
///
/// A `MultiRegionAccessPointRoute` is written as `<Route><Bucket/><Region/>
/// <TrafficDialPercentage/></Route>`, so the entries are read from the leaf
/// PATHS: a new entry starts at each `Bucket`, the first member the model
/// declares.
pub(crate) fn route_entries(leaves: &[Leaf]) -> Vec<(String, String, String)> {
    let mut entries: Vec<(String, String, String)> = Vec::new();
    for (path, value) in leaves {
        let Some(member) = path.strip_prefix("Routes/Route/") else {
            continue;
        };
        match member {
            "Bucket" => entries.push((value.clone(), String::new(), String::new())),
            "Region" => {
                if let Some(entry) = entries.last_mut() {
                    entry.1 = value.clone();
                }
            }
            "TrafficDialPercentage" => {
                if let Some(entry) = entries.last_mut() {
                    entry.2 = value.clone();
                }
            }
            _ => {}
        }
    }
    entries
}

/// One (account, Region, name) a read is made under. No row is merely a rename:
/// each differs in the length or shape of the name, the owning account, or the
/// Region the call is signed for.
fn absent_rows() -> Vec<(&'static str, &'static str, &'static str, String)> {
    vec![
        (
            "a name of the shortest admitted length",
            ACCOUNT,
            REGION,
            SHORTEST_NAME.to_string(),
        ),
        (
            "a name of the longest admitted length",
            ACCOUNT,
            REGION,
            LONGEST_NAME.to_string(),
        ),
        (
            "a name carrying digits and hyphens",
            ACCOUNT,
            REGION,
            "mrap-7-9".to_string(),
        ),
        (
            "a name whose digits and hyphens sit at its ends",
            ACCOUNT,
            REGION,
            "7-mrap-9".to_string(),
        ),
        (
            "a read signed for another Region",
            ACCOUNT,
            OTHER_REGION,
            "never-created-mrap".to_string(),
        ),
    ]
}

#[tokio::test]
async fn an_absent_access_point_is_refused_alike_through_every_route_that_names_one() {
    assert_eq!(
        LONGEST_NAME.len(),
        50,
        "the longest-name row must sit on the modelled maximum, not near it"
    );
    let emulator = Emulator::start();
    let mut correlation: BTreeSet<String> = BTreeSet::new();

    for (case, account, region, name) in absent_rows() {
        // Every route that names an access point BY NAME: the access point
        // itself, its policy, its policy status, and the delete that names it in
        // its request document.
        let reads: Vec<(String, Reply)> = vec![
            (
                format!("{case}: the access point"),
                get_access_point(&emulator, region, account, &name).await,
            ),
            (
                format!("{case}: its policy"),
                get_policy(&emulator, region, account, &name).await,
            ),
            (
                format!("{case}: its policy status"),
                get_policy_status(&emulator, region, account, &name).await,
            ),
            (
                format!("{case}: its delete"),
                delete_access_point(&emulator, region, account, &name, "t-absent").await,
            ),
        ];
        for (read, reply) in &reads {
            let refusal = refusal_of(read, reply);
            assert_eq!(
                (
                    refusal.status,
                    refusal.code.as_str(),
                    refusal.message.as_str()
                ),
                (404, NO_SUCH_MRAP_CODE, NO_SUCH_MRAP_MESSAGE),
                "case {read:?}: expected the absent-access-point refusal, got body {}",
                reply.body
            );
            assert_eq!(
                refusal.members,
                named_members(&name, account),
                "case {read:?}: expected the refusal to name the access point and the account, \
                 got body {}",
                reply.body
            );
            assert_names_the_request(read, reply, &refusal);
            correlation.insert(refusal.request_id.clone());
            correlation.insert(refusal.host_id.clone());
        }

        // The routing read resolves an `Mrap` rather than a name, so a value it
        // cannot resolve is a request this service will not carry out.
        let routes = get_routes(&emulator, region, account, &name).await;
        let refusal = refusal_of(case, &routes);
        assert_eq!(
            (
                refusal.status,
                refusal.code.as_str(),
                refusal.message.as_str()
            ),
            (400, INVALID_REQUEST_CODE, INVALID_REQUEST_MESSAGE),
            "case {case:?}: expected the unresolvable-Mrap refusal, got body {}",
            routes.body
        );
        assert_names_the_request(case, &routes, &refusal);
        correlation.insert(refusal.request_id.clone());
        correlation.insert(refusal.host_id.clone());
    }

    // Each response names ONE request: no two of the refusals above share an id.
    assert_eq!(
        correlation.len(),
        absent_rows().len() * 5 * 2,
        "expected every refusal to carry its own correlation pair, got {} distinct ids",
        correlation.len()
    );
}

/// The creates the sub-resource reads below are run over: the rows differ in how
/// many buckets the access point associates, whether a bucket names its owning
/// account, the account that owns the access point and the Region the create is
/// signed for.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase::new("one bucket", "sub-one-mrap", "t-sub-one", &["reports"]),
        CreateCase::new(
            "three buckets",
            "sub-three-mrap",
            "t-sub-three",
            &["reports", "finance", "partner"],
        ),
        CreateCase {
            regions: vec![("reports", None), ("partner", Some(OTHER_ACCOUNT))],
            ..CreateCase::new(
                "a cross-account bucket",
                "sub-partner-mrap",
                "t-sub-partner",
                &[],
            )
        },
        CreateCase {
            region: OTHER_REGION,
            ..CreateCase::new(
                "another Region",
                "sub-other-mrap",
                "t-sub-other",
                &["reports-central"],
            )
        },
        CreateCase::new(
            "a name of the longest admitted length",
            LONGEST_NAME,
            "t-sub-longest",
            &["reports"],
        ),
    ]
}

/// The alias `GetMultiRegionAccessPoint` reports for a created access point —
/// the `MultiRegionAccessPointId` its routing configuration is read by.
pub(crate) async fn alias_of(emulator: &Emulator, case: &CreateCase) -> String {
    let read = get_access_point(emulator, case.region, case.account, case.name).await;
    assert_eq!(
        read.status, 200,
        "case {:?}: expected the created access point to be readable, got {} with body {}",
        case.name, read.status, read.body
    );
    leaf(&leaves_of(&read.body), "AccessPoint/Alias").unwrap_or_else(|| {
        panic!(
            "case {:?}: expected the read to report an alias, got body {}",
            case.name, read.body
        )
    })
}

#[tokio::test]
async fn a_created_access_point_reports_its_sub_resources_through_their_own_routes() {
    let emulator = Emulator::start();

    for case in create_cases() {
        let created = create(&emulator, &case).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected the create to be accepted, got {} with body {}",
            case.name, created.status, created.body
        );
        let alias = alias_of(&emulator, &case).await;

        // An access point nobody has put a policy on carries neither an
        // established nor a proposed document.
        let policy = get_policy(&emulator, case.region, case.account, case.name).await;
        let leaves = leaves_of(&policy.body);
        assert_eq!(
            (
                policy.status,
                leaf(&leaves, "Policy/Established/Policy"),
                leaf(&leaves, "Policy/Proposed/Policy")
            ),
            (200, None, None),
            "case {:?}: expected a policy read of an access point with no policy to report \
             neither document, got body {}",
            case.name,
            policy.body
        );

        // No policy grants anybody anything, so the access point is not public.
        let status = get_policy_status(&emulator, case.region, case.account, case.name).await;
        assert_eq!(
            (
                status.status,
                leaf(&leaves_of(&status.body), "Established/IsPublic")
            ),
            (200, Some("false".to_string())),
            "case {:?}: expected a policy status of not public, got body {}",
            case.name,
            status.body
        );

        // The routing configuration: one ACTIVE route per associated bucket, in
        // the order the create listed them, each in the Region the create was
        // signed for.
        let routes = get_routes(&emulator, case.region, case.account, &alias).await;
        let leaves = leaves_of(&routes.body);
        let expected: Vec<(String, String, String)> = case
            .regions
            .iter()
            .map(|(bucket, _)| {
                (
                    (*bucket).to_string(),
                    case.region.to_string(),
                    TRAFFIC_DIAL_ACTIVE.to_string(),
                )
            })
            .collect();
        assert_eq!(
            (
                routes.status,
                leaf(&leaves, "Mrap"),
                route_entries(&leaves)
            ),
            (200, Some(alias.clone()), expected),
            "case {:?}: expected one active route per associated bucket, got body {}",
            case.name,
            routes.body
        );

        // A routing read is by `MultiRegionAccessPointId`, which the NAME is
        // not: the name resolves to no access point here, exactly as an invented
        // alias would.
        let by_name = get_routes(&emulator, case.region, case.account, case.name).await;
        let refusal = refusal_of(case.name, &by_name);
        assert_eq!(
            (refusal.status, refusal.code.as_str()),
            (400, INVALID_REQUEST_CODE),
            "case {:?}: expected a routing read by name to resolve nothing, got body {}",
            case.name,
            by_name.body
        );

        // A request addressed in another account's control plane is refused
        // before any sub-resource is resolved, so neither the alias nor the
        // name tells another account anything.
        for (label, reply) in [
            (
                "the routing configuration by its alias",
                get_routes(&emulator, case.region, OTHER_ACCOUNT, &alias).await,
            ),
            (
                "its policy",
                get_policy(&emulator, case.region, OTHER_ACCOUNT, case.name).await,
            ),
            (
                "its policy status",
                get_policy_status(&emulator, case.region, OTHER_ACCOUNT, case.name).await,
            ),
        ] {
            assert_foreign_account_refused(
                &format!("{}: {label}", case.name),
                OTHER_ACCOUNT,
                &reply,
            );
        }
    }
}

/// The policy documents the put/read round trip is run over: the rows differ in
/// what the document grants, who to, and under which condition — which is what
/// the public-access derivation generalises over.
fn policy_documents() -> Vec<(&'static str, String, bool)> {
    vec![
        (
            "a grant to one named principal",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":"s3:GetObject","Resource":"*"}]}"#
                .to_string(),
            false,
        ),
        (
            "a grant to every principal",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"s3:GetObject","Resource":"*"}]}"#
                .to_string(),
            true,
        ),
        (
            "a grant to every principal of one account",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"s3:GetObject","Resource":"*","Condition":{"StringEquals":{"aws:PrincipalAccount":"123456789012"}}}]}"#
                .to_string(),
            false,
        ),
        (
            "a denial to every principal",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Principal":"*","Action":"s3:*","Resource":"*"}]}"#
                .to_string(),
            false,
        ),
        (
            "a document carrying the five XML entities in its text",
            r#"{"Version":"2012-10-17","Statement":[{"Sid":"a&b<c>d\"e'f","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":"s3:GetObject","Resource":"*"}]}"#
                .to_string(),
            false,
        ),
    ]
}

#[tokio::test]
async fn a_policy_put_is_proposed_then_established_and_the_status_follows_it() {
    let emulator = Emulator::start();

    for (index, (case, document, expected_public)) in policy_documents().into_iter().enumerate() {
        // One access point per row, so no row reads what another put.
        let name = format!("policy-mrap-{index}");
        let create_case = CreateCase::new("the access point the policy is put on", "", "", &[]);
        emulator.provision_bucket(case, "reports");
        let created = call_in_region(
            &emulator.router,
            create_case.region,
            "POST",
            "/v20180820/async-requests/mrap/create",
            create_case.account,
            &format!(
                "<CreateMultiRegionAccessPointRequest xmlns=\"{XMLNS}\">\
                 <ClientToken>t-policy-{index}</ClientToken><Details><Name>{name}</Name>\
                 <Regions><Region><Bucket>reports</Bucket></Region></Regions></Details>\
                 </CreateMultiRegionAccessPointRequest>"
            ),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the create to be accepted, got {} with body {}",
            created.status, created.body
        );

        let put = put_policy(
            &emulator,
            REGION,
            ACCOUNT,
            &name,
            &format!("t-put-{index}"),
            &document,
        )
        .await;
        let token = leaf(&leaves_of(&put.body), "RequestTokenARN");
        assert_eq!(
            (put.status, token.is_some()),
            (200, true),
            "case {case:?}: expected the put to be accepted with a request token, got {} \
             with body {}",
            put.status,
            put.body
        );
        let token = token.unwrap_or_default();

        // Until the clock owes the request its outcome the document is the
        // PROPOSED one, and nothing is established yet.
        let proposed = get_policy(&emulator, REGION, ACCOUNT, &name).await;
        let leaves = leaves_of(&proposed.body);
        assert_eq!(
            (
                leaf(&leaves, "Policy/Proposed/Policy"),
                leaf(&leaves, "Policy/Established/Policy")
            ),
            (Some(document.clone()), None),
            "case {case:?}: expected the submitted document to be proposed and nothing \
             established, got body {}",
            proposed.body
        );
        let status = get_policy_status(&emulator, REGION, ACCOUNT, &name).await;
        assert_eq!(
            leaf(&leaves_of(&status.body), "Established/IsPublic"),
            Some("false".to_string()),
            "case {case:?}: a policy nothing has established yet grants nobody anything, \
             got body {}",
            status.body
        );

        // The request this put made is reported under its own token.
        let described = describe(&emulator, REGION, ACCOUNT, &token).await;
        let leaves = leaves_of(&described.body);
        assert_eq!(
            (
                described.status,
                leaf(&leaves, "AsyncOperation/Operation"),
                leaf(
                    &leaves,
                    "AsyncOperation/RequestParameters/\
                     PutMultiRegionAccessPointPolicyRequest/Policy"
                )
            ),
            (
                200,
                Some("PutMultiRegionAccessPointPolicy".to_string()),
                Some(document.clone())
            ),
            "case {case:?}: expected the token to report the put it was handed for, got body {}",
            described.body
        );

        // Once the clock owes it, the proposed document IS the established one,
        // and the status is derived from the document that is now in force.
        tokio::time::sleep(settled_wait()).await;
        let established = get_policy(&emulator, REGION, ACCOUNT, &name).await;
        let leaves = leaves_of(&established.body);
        assert_eq!(
            (
                leaf(&leaves, "Policy/Established/Policy"),
                leaf(&leaves, "Policy/Proposed/Policy")
            ),
            (Some(document.clone()), None),
            "case {case:?}: expected the proposed document to become the established one, \
             got body {}",
            established.body
        );
        let status = get_policy_status(&emulator, REGION, ACCOUNT, &name).await;
        assert_eq!(
            leaf(&leaves_of(&status.body), "Established/IsPublic"),
            Some(expected_public.to_string()),
            "case {case:?}: expected the status of the established document, got body {}",
            status.body
        );
    }
}

#[tokio::test]
async fn a_policy_put_the_service_refuses_establishes_nothing() {
    let emulator = Emulator::start();
    let case = CreateCase::new("the access point the puts name", "put-mrap", "t-put", &["reports"]);
    let created = create(&emulator, &case).await;
    assert_eq!(
        created.status, 200,
        "expected the create to be accepted, got {} with body {}",
        created.status, created.body
    );

    // A document that is not JSON at all is not a resource policy.
    let malformed = put_policy(&emulator, REGION, ACCOUNT, case.name, "t-bad", "not a policy").await;
    let refusal = refusal_of("a document that is not JSON", &malformed);
    assert_eq!(
        (refusal.status, refusal.code.as_str()),
        (400, "MalformedPolicy"),
        "case \"a document that is not JSON\": expected a malformed-policy refusal, got body {}",
        malformed.body
    );
    assert_names_the_request("a document that is not JSON", &malformed, &refusal);

    // A put naming an access point this account does not hold is the same
    // absent-resource refusal every other route answers.
    let absent = put_policy(
        &emulator,
        REGION,
        ACCOUNT,
        "never-created-mrap",
        "t-absent-put",
        r#"{"Version":"2012-10-17","Statement":[]}"#,
    )
    .await;
    let refusal = refusal_of("a put naming an absent access point", &absent);
    assert_eq!(
        (
            refusal.status,
            refusal.code.as_str(),
            refusal.message.as_str()
        ),
        (404, NO_SUCH_MRAP_CODE, NO_SUCH_MRAP_MESSAGE),
        "case \"a put naming an absent access point\": expected the absent-access-point \
         refusal, got body {}",
        absent.body
    );
    assert_eq!(
        refusal.members,
        named_members("never-created-mrap", ACCOUNT),
        "case \"a put naming an absent access point\": expected the refusal to name the \
         access point and the account, got body {}",
        absent.body
    );

    // Neither refusal left a document behind.
    let policy = get_policy(&emulator, REGION, ACCOUNT, case.name).await;
    let leaves = leaves_of(&policy.body);
    assert_eq!(
        (
            leaf(&leaves, "Policy/Established/Policy"),
            leaf(&leaves, "Policy/Proposed/Policy")
        ),
        (None, None),
        "expected a refused put to establish nothing, got body {}",
        policy.body
    );
}

/// The request tokens a poll is run over. Each names no asynchronous request,
/// and they differ in the one thing the resolution generalises over: the form of
/// the token — one this service could have minted, one that is no ARN at all,
/// one past the modelled length, and one carrying path separators.
fn unknown_tokens() -> Vec<(&'static str, String)> {
    vec![
        (
            "a token in the form this service mints",
            format!(
                "arn:aws:s3:{REGION}:{}:async-request/mrap/create/{}",
                ::emulator_server::emulator_account_id(),
                "00000000-0000-4000-8000-000000000000"
            ),
        ),
        ("a token that is not an ARN at all", "not-an-arn".to_string()),
        (
            "a token a probe invented",
            "sim2real-probe-not-an-arn".to_string(),
        ),
        (
            "a token past the modelled maximum length",
            format!("arn:{}", "x".repeat(1100)),
        ),
        (
            "a token carrying path separators",
            "mrap/create/0123456789".to_string(),
        ),
    ]
}

#[tokio::test]
async fn a_request_token_that_names_no_request_is_refused_as_not_found() {
    let emulator = Emulator::start();

    for (case, token) in unknown_tokens() {
        let described = describe(&emulator, REGION, ACCOUNT, &token).await;
        let refusal = refusal_of(case, &described);
        assert_eq!(
            (
                refusal.status,
                refusal.code.as_str(),
                refusal.message.as_str()
            ),
            (
                404,
                NO_SUCH_ASYNC_REQUEST_CODE,
                NO_SUCH_ASYNC_REQUEST_MESSAGE
            ),
            "case {case:?}: expected the not-found refusal for an unknown token, got body {}",
            described.body
        );
        assert!(
            !refusal.message.contains(&token),
            "case {case:?}: the refusal must not echo the token it was given, got body {}",
            described.body
        );
        assert_names_the_request(case, &described, &refusal);
    }
}

#[test]
fn the_greedy_suffix_route_table_matches_the_model() {
    // Every route of this service's model whose `@http.uri` puts a literal
    // segment after a greedy label, read from the model itself rather than from
    // a second hand-written list.
    let model_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-s3control-awss3controlservicev20180820.json"
    );
    let model: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(model_path).expect("the service's Smithy model must be readable"),
    )
    .expect("the Smithy model must parse as JSON");
    let shapes = model["shapes"]
        .as_object()
        .expect("the model must carry a shapes object");

    let mut modelled: Vec<(String, String, String)> = shapes
        .iter()
        .filter(|(_, shape)| shape["type"] == "operation")
        .filter_map(|(id, shape)| {
            let http = shape["traits"].get("smithy.api#http")?;
            let uri = http["uri"].as_str()?;
            let method = http["method"].as_str()?;
            // A literal after a greedy label: the segment holding `+}` is not
            // the last one.
            let segments: Vec<&str> = uri.split('/').filter(|part| !part.is_empty()).collect();
            let greedy_at = segments.iter().position(|part| part.ends_with("+}"))?;
            (greedy_at + 1 < segments.len()).then(|| {
                (
                    method.to_string(),
                    uri.to_string(),
                    id.split('#')
                        .next_back()
                        .expect("a shape id names a shape")
                        .to_string(),
                )
            })
        })
        .collect();
    modelled.sort();

    let mut declared: Vec<(String, String, String)> = GREEDY_SUFFIX_ROUTES
        .iter()
        .map(|GreedySuffixRoute { method, uri, operation }| {
            (
                (*method).to_string(),
                (*uri).to_string(),
                (*operation).to_string(),
            )
        })
        .collect();
    declared.sort();

    assert_eq!(
        declared, modelled,
        "GREEDY_SUFFIX_ROUTES must name exactly the routes whose template puts a literal \
         after a greedy label; declared {declared:?}, the model binds {modelled:?}"
    );
}
