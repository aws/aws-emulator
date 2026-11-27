// SPDX-License-Identifier: CC0-1.0

//! A resource of this service exists in exactly ONE Region.
//!
//! The rule these cases assert: Connector for SCEP is a regional service, so a connector —
//! and the challenges and tags it owns — exists only in the Region named by its own ARN
//! (`ConnectorArn` / `ChallengeArn` carry a Region segment, smithy_query shape=ConnectorArn),
//! which is the Region the create was signed for. A call signed for any other Region
//! therefore neither lists nor resolves it: the listing omits it, and every operation that
//! identifies it by ARN answers as though it did not exist — a read with
//! `ResourceNotFoundException` (HTTP 404) carrying that ARN as `ResourceId` and the kind of
//! resource it names as `ResourceType`, a create with that same 404 naming the input member
//! that carried the reference it could not resolve, a tagging call with the tagging layer's
//! own not-found refusal, a delete with the accepted no-op its 202 output is — and writes
//! nothing.
//!
//! Every case drives the service the way a client does, one HTTP request per call through the
//! same `MultiServiceRouter` the emulator boots, with the Region in the request's own SigV4
//! credential scope. The table's rows differ along what the rule generalises over: which
//! Region owns the resource, which Region the foreign call is signed for (including a pair
//! whose names sort adjacently, so an ARN-ordered walk of one Region would run straight into
//! the other), the private CA the connector links, and how many challenges the connector
//! holds.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{self, label, new_router, Answer};
use service_com_amazonaws_pcaconnectorscep_pcaconnectorscep::state;

/// The canonical emulator account id every ARN of this service carries.
const ACCOUNT: &str = "000000000000";

/// The `ResourceType` of each of this service's two resource kinds, taken from the service's
/// own vocabulary rather than respelled here.
const RESOURCE_TYPE_CONNECTOR: &str = state::RESOURCE_TYPE_CONNECTOR;
const RESOURCE_TYPE_CHALLENGE: &str = state::RESOURCE_TYPE_CHALLENGE;

/// The name of `CreateChallenge`'s input member that references the connector a challenge is
/// minted under (smithy_query operation=CreateChallenge).
///
/// A create does not identify a resource of its own, so what it could not resolve is this
/// parameter, and that is what its 404 reports — where a read or a delete handed the same ARN
/// reports the resource KIND above.
const MEMBER_CONNECTOR_ARN: &str = "ConnectorArn";

/// The status a settled connector reports.
const STATUS_ACTIVE: &str = state::STATUS_ACTIVE;

/// The Region a request whose credential scope names none this service can file a record
/// under is served in.
const DEFAULT_REGION: &str = state::DEFAULT_REGION;

/// One Region pair: the Region a connector is created in, and a second Region the same
/// client then addresses it from.
#[derive(Debug)]
struct Row {
    name: &'static str,
    /// The Region the create is signed for, which is the only Region the connector exists in.
    home: &'static str,
    /// Another Region of the same account, signed for by every foreign call.
    foreign: &'static str,
    /// The uuid half of the private CA the connector links, so no two rows name one CA.
    ca_id: &'static str,
    /// How many challenge passwords the connector holds.
    challenges: usize,
}

/// Region pairs. Each name matches the Region segment both ARN patterns require
/// (`[a-z]+(-[a-z]+)+-[1-9]\d*`, smithy_query shape=ConnectorArn).
const ROWS: &[Row] = &[
    Row {
        name: "created in eu-west-2, addressed from us-east-1",
        home: "eu-west-2",
        foreign: "us-east-1",
        ca_id: "11111111-2222-3333-4444-555555555555",
        challenges: 1,
    },
    Row {
        name: "created in us-east-1, addressed from eu-west-2",
        home: "us-east-1",
        foreign: "eu-west-2",
        ca_id: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
        challenges: 2,
    },
    Row {
        name: "adjacent Region names: ap-southeast-3 and ap-southeast-4",
        home: "ap-southeast-3",
        foreign: "ap-southeast-4",
        ca_id: "0123456f-89ab-cdef-0123-456789abcdef",
        challenges: 1,
    },
    Row {
        name: "created in sa-east-1, addressed from af-south-1",
        home: "sa-east-1",
        foreign: "af-south-1",
        ca_id: "deadbeef-0000-1111-2222-333344445555",
        challenges: 3,
    },
    Row {
        name: "created in me-central-1, addressed from eu-central-1",
        home: "me-central-1",
        foreign: "eu-central-1",
        ca_id: "99999999-8888-7777-6666-555544443333",
        challenges: 0,
    },
];

impl Row {
    /// The private CA this row's connector links, in the Region that connector lives in.
    fn ca_arn(&self) -> String {
        format!(
            "arn:aws:acm-pca:{}:{ACCOUNT}:certificate-authority/{}",
            self.home, self.ca_id
        )
    }
}

async fn create_connector(router: &MultiServiceRouter, region: &str, body: Value) -> Answer {
    harness::call(router, region, "POST", "/connectors", Some(body)).await
}

async fn get_connector(router: &MultiServiceRouter, region: &str, arn: &str) -> Answer {
    harness::call(
        router,
        region,
        "GET",
        &format!("/connectors/{}", label(arn)),
        None,
    )
    .await
}

async fn delete_connector(router: &MultiServiceRouter, region: &str, arn: &str) -> Answer {
    harness::call(
        router,
        region,
        "DELETE",
        &format!("/connectors/{}", label(arn)),
        None,
    )
    .await
}

async fn list_connectors(router: &MultiServiceRouter, region: &str, query: &str) -> Answer {
    harness::call(router, region, "GET", &format!("/connectors{query}"), None).await
}

async fn create_challenge(router: &MultiServiceRouter, region: &str, connector_arn: &str) -> Answer {
    harness::call(
        router,
        region,
        "POST",
        "/challenges",
        Some(json!({ "ConnectorArn": connector_arn })),
    )
    .await
}

async fn get_challenge_password(router: &MultiServiceRouter, region: &str, arn: &str) -> Answer {
    harness::call(
        router,
        region,
        "GET",
        &format!("/challengePasswords/{}", label(arn)),
        None,
    )
    .await
}

async fn get_challenge_metadata(router: &MultiServiceRouter, region: &str, arn: &str) -> Answer {
    harness::call(
        router,
        region,
        "GET",
        &format!("/challengeMetadata/{}", label(arn)),
        None,
    )
    .await
}

async fn delete_challenge(router: &MultiServiceRouter, region: &str, arn: &str) -> Answer {
    harness::call(
        router,
        region,
        "DELETE",
        &format!("/challenges/{}", label(arn)),
        None,
    )
    .await
}

async fn list_challenge_metadata(
    router: &MultiServiceRouter,
    region: &str,
    connector_arn: &str,
) -> Answer {
    harness::call(
        router,
        region,
        "GET",
        &format!("/challengeMetadata?ConnectorArn={}", label(connector_arn)),
        None,
    )
    .await
}

async fn list_tags(router: &MultiServiceRouter, region: &str, arn: &str) -> Answer {
    harness::call(router, region, "GET", &format!("/tags/{}", label(arn)), None).await
}

async fn tag_resource(
    router: &MultiServiceRouter,
    region: &str,
    arn: &str,
    tags: Value,
) -> Answer {
    harness::call(
        router,
        region,
        "POST",
        &format!("/tags/{}", label(arn)),
        Some(json!({ "Tags": tags })),
    )
    .await
}

/// Create a connector in `region` and settle it, so it can mint challenge passwords.
async fn active_connector(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    region: &str,
    case: &str,
    ca_arn: &str,
) -> String {
    let created =
        create_connector(router, region, json!({ "CertificateAuthorityArn": ca_arn })).await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: expected 202 from CreateConnector in {region}, got {} / {:?}",
        created.status, created.body
    );
    let arn = created.text("ConnectorArn");
    assert!(
        arn.contains(&format!(":{region}:")),
        "case {case:?}: the ConnectorArn must name the Region the create was signed for \
         ({region}), got {arn:?}"
    );
    harness::release_pending_if_owed(engine, &arn);
    arn
}

/// The ARNs `ListConnectors` reports to a caller signed for `region`, walking every page.
async fn listed_arns(router: &MultiServiceRouter, region: &str, case: &str, page_size: u32) -> Vec<String> {
    let mut seen = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..64 {
        let query = match token.as_deref() {
            Some(next) => format!("?MaxResults={page_size}&NextToken={}", label(next)),
            None => format!("?MaxResults={page_size}"),
        };
        let page = list_connectors(router, region, &query).await;
        assert_eq!(
            page.status, 200,
            "case {case:?}: expected 200 from ListConnectors in {region}, got {} / {:?}",
            page.status, page.body
        );
        for entry in page.body["Connectors"].as_array().into_iter().flatten() {
            seen.push(
                entry["Arn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected an Arn in {entry:?}"))
                    .to_string(),
            );
        }
        token = page.body["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return seen;
        }
    }
    panic!("case {case:?}: paging ListConnectors in {region} never terminated, got {seen:?}");
}

/// Assert one answer is the 404 a resource that does not exist here gets.
fn assert_not_found(answer: &Answer, case: &str, what: &str, arn: &str, resource_type: &str) {
    assert_eq!(
        answer.status, 404,
        "case {case:?}: {what} on a Region that does not hold {arn:?} must answer 404, got {} / {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_type, "ResourceNotFoundException",
        "case {case:?}: {what} must answer ResourceNotFoundException, got {:?} / {:?}",
        answer.error_type, answer.body
    );
    assert_eq!(
        answer.body["ResourceId"],
        json!(arn),
        "case {case:?}: {what} must report the ARN it was asked about as ResourceId, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.body["ResourceType"],
        json!(resource_type),
        "case {case:?}: {what} must report ResourceType {resource_type}, got {:?}",
        answer.body
    );
}

/// Assert one answer is the accepted no-op a DELETE addressed at a Region that does not hold
/// the resource gets: the modelled 202, because the end state it asks for — that resource not
/// being here — is the state this Region is already in. That it changed nothing is asserted
/// afterwards, through the Region that DOES hold the resource.
fn assert_accepted(answer: &Answer, case: &str, what: &str, arn: &str) {
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (202, ""),
        "case {case:?}: {what} on a Region that does not hold {arn:?} must answer the modelled \
         202, got {} / {:?}",
        answer.status,
        answer.body
    );
}

/// A listing reports the connectors of the Region it is signed for and no others.
#[tokio::test]
async fn a_connector_is_listed_only_through_the_region_it_was_created_in() {
    for row in ROWS {
        let case = row.name;
        let (router, engine) = new_router();
        let arn = active_connector(&router, &engine, row.home, case, &row.ca_arn()).await;

        let home = listed_arns(&router, row.home, case, 10).await;
        assert_eq!(
            home,
            vec![arn.clone()],
            "case {case:?}: the Region the connector was created in ({}) must list exactly it, \
             got {home:?}",
            row.home
        );

        let foreign = listed_arns(&router, row.foreign, case, 10).await;
        assert!(
            foreign.is_empty(),
            "case {case:?}: a Region nothing was created in ({}) must list nothing, got \
             {foreign:?}",
            row.foreign
        );
    }
}

/// Every operation that identifies a resource by ARN answers for the Region it is signed for
/// only: a read is 404, a tagging call is the tagging layer's own not-found refusal, a delete
/// is the accepted no-op — and none of them changes anything, so the owning Region still
/// serves the resource untouched.
#[tokio::test]
async fn every_operation_refuses_an_arn_of_another_region() {
    for row in ROWS {
        let case = row.name;
        let (router, engine) = new_router();
        let connector_arn = active_connector(&router, &engine, row.home, case, &row.ca_arn()).await;

        // The connector's challenges, created where the connector lives.
        let mut challenge_arns = Vec::new();
        for index in 0..row.challenges {
            let created = create_challenge(&router, row.home, &connector_arn).await;
            assert_eq!(
                created.status, 202,
                "case {case:?}: expected 202 from CreateChallenge #{index} in {}, got {} / {:?}",
                row.home, created.status, created.body
            );
            challenge_arns.push(
                created.body["Challenge"]["Arn"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected a Challenge Arn in {:?}", created.body)
                    })
                    .to_string(),
            );
        }
        let tagged = tag_resource(
            &router,
            row.home,
            &connector_arn,
            json!({ "owner": "home-region" }),
        )
        .await;
        assert_eq!(
            tagged.status, 204,
            "case {case:?}: expected 204 from TagResource in {}, got {} / {:?}",
            row.home, tagged.status, tagged.body
        );

        // Every reader and writer of the connector, addressed at the foreign Region.
        let foreign = row.foreign;
        assert_not_found(
            &get_connector(&router, foreign, &connector_arn).await,
            case,
            "GetConnector",
            &connector_arn,
            RESOURCE_TYPE_CONNECTOR,
        );
        assert_not_found(
            &list_challenge_metadata(&router, foreign, &connector_arn).await,
            case,
            "ListChallengeMetadata",
            &connector_arn,
            RESOURCE_TYPE_CONNECTOR,
        );
        assert_not_found(
            &create_challenge(&router, foreign, &connector_arn).await,
            case,
            "CreateChallenge",
            &connector_arn,
            MEMBER_CONNECTOR_ARN,
        );
        harness::assert_tagging_not_found(
            &list_tags(&router, foreign, &connector_arn).await,
            &format!("{case}: ListTagsForResource"),
        );
        harness::assert_tagging_not_found(
            &tag_resource(&router, foreign, &connector_arn, json!({ "owner": "foreign" })).await,
            &format!("{case}: TagResource"),
        );
        assert_accepted(
            &delete_connector(&router, foreign, &connector_arn).await,
            case,
            "DeleteConnector",
            &connector_arn,
        );

        // And every reader and writer of its challenges.
        for challenge_arn in &challenge_arns {
            assert_not_found(
                &get_challenge_password(&router, foreign, challenge_arn).await,
                case,
                "GetChallengePassword",
                challenge_arn,
                RESOURCE_TYPE_CHALLENGE,
            );
            assert_not_found(
                &get_challenge_metadata(&router, foreign, challenge_arn).await,
                case,
                "GetChallengeMetadata",
                challenge_arn,
                RESOURCE_TYPE_CHALLENGE,
            );
            harness::assert_tagging_not_found(
                &list_tags(&router, foreign, challenge_arn).await,
                &format!("{case}: ListTagsForResource (challenge)"),
            );
            assert_accepted(
                &delete_challenge(&router, foreign, challenge_arn).await,
                case,
                "DeleteChallenge",
                challenge_arn,
            );
        }

        // None of those answers touched the resource: the Region that holds it still does.
        let read = get_connector(&router, row.home, &connector_arn).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the owning Region must still serve the connector, got {} / {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body["Connector"]["Status"],
            json!(STATUS_ACTIVE),
            "case {case:?}: a foreign-Region DeleteConnector must not move the connector's \
             status, got {:?}",
            read.body
        );
        let tags = list_tags(&router, row.home, &connector_arn).await;
        assert_eq!(
            tags.body["Tags"],
            json!({ "owner": "home-region" }),
            "case {case:?}: a foreign-Region TagResource must write nothing, got {:?}",
            tags.body
        );
        for challenge_arn in &challenge_arns {
            let password = get_challenge_password(&router, row.home, challenge_arn).await;
            assert_eq!(
                password.status, 200,
                "case {case:?}: a foreign-Region DeleteChallenge must not remove {challenge_arn:?}, \
                 got {} / {:?}",
                password.status, password.body
            );
        }
    }
}

/// The Region segment of an ARN names the only Region the resource exists in: an ARN whose
/// Region is rewritten resolves in neither Region.
#[tokio::test]
async fn an_arn_whose_region_segment_is_rewritten_resolves_nowhere() {
    for row in ROWS {
        let case = row.name;
        let (router, engine) = new_router();
        let arn = active_connector(&router, &engine, row.home, case, &row.ca_arn()).await;
        let rewritten = arn.replacen(&format!(":{}:", row.home), &format!(":{}:", row.foreign), 1);
        assert_ne!(
            rewritten, arn,
            "case {case:?}: rewriting the Region segment must change the ARN"
        );

        // Signed for the Region the rewritten ARN names: nothing was ever created there.
        assert_not_found(
            &get_connector(&router, row.foreign, &rewritten).await,
            case,
            "GetConnector on a rewritten ARN, signed for the Region it names",
            &rewritten,
            RESOURCE_TYPE_CONNECTOR,
        );
        // Signed for the Region that does hold the connector: the ARN names another Region's
        // resource, so it still resolves to nothing.
        assert_not_found(
            &get_connector(&router, row.home, &rewritten).await,
            case,
            "GetConnector on a rewritten ARN, signed for the owning Region",
            &rewritten,
            RESOURCE_TYPE_CONNECTOR,
        );
    }
}

/// A paginated walk of one Region's connectors visits exactly that Region's inventory, at
/// every page size, and a cursor one Region minted names no position in another.
#[tokio::test]
async fn paging_one_regions_listing_never_crosses_into_another() {
    for row in ROWS {
        let case = row.name;
        let (router, engine) = new_router();

        let mut home_arns = Vec::new();
        let mut foreign_arns = Vec::new();
        let home_ca = row.ca_arn();
        let foreign_ca = format!(
            "arn:aws:acm-pca:{}:{ACCOUNT}:certificate-authority/{}",
            row.foreign, row.ca_id
        );
        for _ in 0..3 {
            home_arns.push(active_connector(&router, &engine, row.home, case, &home_ca).await);
            foreign_arns
                .push(active_connector(&router, &engine, row.foreign, case, &foreign_ca).await);
        }
        home_arns.sort();
        foreign_arns.sort();

        for page_size in [1u32, 2, 3, 10] {
            let walked = listed_arns(&router, row.home, case, page_size).await;
            assert_eq!(
                walked, home_arns,
                "case {case:?}: paging {} at MaxResults={page_size} must visit exactly its own \
                 connectors, got {walked:?}",
                row.home
            );
            let walked_foreign = listed_arns(&router, row.foreign, case, page_size).await;
            assert_eq!(
                walked_foreign, foreign_arns,
                "case {case:?}: paging {} at MaxResults={page_size} must visit exactly its own \
                 connectors, got {walked_foreign:?}",
                row.foreign
            );
        }

        // The cursor one Region's listing minted is not a position in another's.
        let first = list_connectors(&router, row.home, "?MaxResults=1").await;
        let token = first.body["NextToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a NextToken in {:?}", first.body))
            .to_string();
        let resumed_elsewhere = list_connectors(
            &router,
            row.foreign,
            &format!("?NextToken={}", label(&token)),
        )
        .await;
        assert_eq!(
            resumed_elsewhere.status, 400,
            "case {case:?}: a NextToken minted in {} must not resume a listing in {}, got {} / {:?}",
            row.home, row.foreign, resumed_elsewhere.status, resumed_elsewhere.body
        );
        assert_eq!(
            resumed_elsewhere.error_type, "ValidationException",
            "case {case:?}: an unusable pagination token is a ValidationException, got {:?} / {:?}",
            resumed_elsewhere.error_type, resumed_elsewhere.body
        );
    }
}

/// The Region a record is filed under is one an ARN of this service can name: a credential
/// scope carrying anything else is served in the default Region rather than becoming a
/// partition of its own.
///
/// Rows are the ways a scope's Region field can fail the shape both ARN patterns require
/// (`[a-z]+(-[a-z]+)+-[1-9]\d*`, smithy_query shape=ConnectorArn): wrong case, too few
/// words, no trailing number, a leading zero in it, an empty word, a word that is not
/// lower-case letters, and one far longer than any Region name.
#[tokio::test]
async fn a_scope_naming_no_region_of_this_service_is_served_in_the_default_region() {
    let overlong = "a-".repeat(40) + "1";
    let scopes: Vec<String> = [
        "US-EAST-1",
        "useast1",
        "us-east",
        "us-east-0",
        "us-east-01",
        "us--east-1",
        "us-east1-x",
        "us-e@st-1",
    ]
    .iter()
    .map(|scope| (*scope).to_string())
    .chain(std::iter::once(overlong))
    .collect();

    let (router, _engine) = new_router();
    let mut minted = Vec::new();
    for scope in &scopes {
        let case = format!("credential scope Region {scope:?}");
        let created = harness::call_with_scope(
            &router,
            DEFAULT_REGION,
            scope,
            "POST",
            "/connectors",
            Some(json!({
                "CertificateAuthorityArn":
                    format!("arn:aws:acm-pca:{DEFAULT_REGION}:{ACCOUNT}:certificate-authority/11111111-2222-3333-4444-555555555555"),
            })),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: expected 202 from CreateConnector, got {} / {:?}",
            created.status, created.body
        );
        let arn = created.text("ConnectorArn");
        assert!(
            arn.contains(&format!(":{DEFAULT_REGION}:")),
            "case {case:?}: a scope naming no Region of this service must be served in \
             {DEFAULT_REGION}, got {arn:?}"
        );
        minted.push(arn);
    }

    // Every one of them landed in the ONE default partition, which a plain request signed
    // for that Region reads back — so no invented scope got a store of its own.
    minted.sort();
    let listed = listed_arns(&router, DEFAULT_REGION, "mis-shaped scopes", 10).await;
    assert_eq!(
        listed, minted,
        "case \"mis-shaped scopes\": every connector created under a mis-shaped scope must be \
         listed by {DEFAULT_REGION}, got {listed:?}"
    );
}

/// A `ClientToken` identifies a retry of a call made against ONE Region's endpoint: the same
/// token sent to two Regions asks for a connector in each.
#[tokio::test]
async fn one_client_token_creates_one_connector_per_region() {
    for row in ROWS {
        let case = row.name;
        let (router, _engine) = new_router();
        let ca = row.ca_arn();
        let request = json!({
            "CertificateAuthorityArn": ca,
            "ClientToken": "a-retry-token-0001",
        });

        let home = create_connector(&router, row.home, request.clone()).await;
        assert_eq!(
            home.status, 202,
            "case {case:?}: expected 202 from CreateConnector in {}, got {} / {:?}",
            row.home, home.status, home.body
        );
        let home_arn = home.text("ConnectorArn");

        let foreign = create_connector(&router, row.foreign, request.clone()).await;
        assert_eq!(
            foreign.status, 202,
            "case {case:?}: expected 202 from CreateConnector in {}, got {} / {:?}",
            row.foreign, foreign.status, foreign.body
        );
        let foreign_arn = foreign.text("ConnectorArn");

        assert_ne!(
            home_arn, foreign_arn,
            "case {case:?}: one token used in two Regions must mint one connector in each"
        );
        assert!(
            foreign_arn.contains(&format!(":{}:", row.foreign)),
            "case {case:?}: the second connector must live in {}, got {foreign_arn:?}",
            row.foreign
        );

        // Within its own Region the token still stands for the call it was first seen on.
        let retried = create_connector(&router, row.home, request).await;
        assert_eq!(
            retried.text("ConnectorArn"),
            home_arn,
            "case {case:?}: repeating the token in {} must answer with the connector it created",
            row.home
        );
    }
}
