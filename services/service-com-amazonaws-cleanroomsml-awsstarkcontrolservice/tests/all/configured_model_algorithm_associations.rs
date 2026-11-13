// SPDX-License-Identifier: CC0-1.0

//! The Clean Rooms ML configured model algorithm association, end to end over HTTP.
//!
//! The rule under test: `CreateConfiguredModelAlgorithmAssociation` associates a
//! configured model algorithm this regional endpoint holds with the collaboration the
//! membership in its path belongs to — the collaboration is READ from the membership Clean
//! Rooms persisted, never derived from the identifier the request carried — persisting
//! exactly what the request carried under an ARN it mints that nests the association under
//! that membership, so the reads of that association (its own, the collaboration's, the
//! membership-scoped list, the collaboration-scoped list and its tags) all compute their
//! answers from that one record; a membership or an algorithm this endpoint does not hold
//! is refused `ResourceNotFoundException` (404) instead.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The memberships the cases associate under are created through Clean
//! Rooms' own `CreateCollaboration`/`CreateMembership` on the SAME engine, so an
//! association is only ever created under a membership that service really holds.
//!
//! The rows of each table differ along what the rule generalises over: which optional
//! members the create carries, how many tags there are, the name's and the description's
//! length, which membership and which collaboration the association lands in, and the
//! region the request is signed for.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_cleanroomsml_awsstarkcontrolservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

// The HTTP harness every suite in this crate drives the router through, defined once in
// the sibling suite rather than copied here.
use crate::training_dataset_audience_model_lifecycle::{
    assert_member_round_trips, is_timestamp, label, signed_call, timestamp_seconds, unix_now,
    Answer,
};

/// The region the requests in this suite are signed for, unless a case signs for another.
const REGION: &str = "us-east-1";

/// A second endpoint, for the cases that pin that a Clean Rooms ML endpoint serves the
/// resources created under its own region and no others.
const OTHER_REGION: &str = "ap-northeast-1";

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`),
/// spelled out here rather than imported so a wrong account in the handler fails.
const ACCOUNT: &str = "000000000000";

/// Another account id, twelve digits as `AccountId` declares, for the privacy
/// configuration that allows two accounts to read a log.
const OTHER_ACCOUNT: &str = "210987654321";

/// An IAM role ARN matching `IamRoleArn`'s `^arn:aws[-a-z]*:iam::[0-9]{12}:role/.+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/CleanRoomsMLRole";

/// The largest `maxResults` `ListConfiguredModelAlgorithmAssociationsRequest$maxResults`
/// admits (`@range(min: 1, max: 100)`), spelled out rather than imported so a wrong
/// ceiling in the handler fails.
const MAX_RESULTS_CEILING: i64 = 100;

/// One emulator: a router carrying Clean Rooms ML and Clean Rooms over one shared engine.
///
/// Both services are registered because a Clean Rooms ML association is created under a
/// Clean Rooms membership: the membership a case associates under is the one Clean Rooms'
/// own create persisted, not a record written behind the handler's back.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_cleanrooms_awsbastioncontrolplaneservicelambda::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// One request to CLEAN ROOMS (not Clean Rooms ML): the SigV4 credential scope is the
/// routing key, so this one names `cleanrooms` where the sibling harness names
/// `cleanrooms-ml`.
async fn cleanrooms_call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let host = format!("cleanrooms.{region}.amazonaws.com");
    let request = ::http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/cleanrooms/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {path} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// A collaboration and a membership in it, both created through Clean Rooms in `region`.
/// Answers `(collaboration identifier, membership identifier)`.
async fn collaboration_with_membership(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
) -> (String, String) {
    let created = cleanrooms_call(
        router,
        region,
        "POST",
        "/collaborations",
        Some(json!({
            "name": name,
            "creatorDisplayName": "creator",
            "creatorMemberAbilities": ["CAN_QUERY"],
            "members": [],
            "queryLogStatus": "ENABLED",
        })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "CreateCollaboration {name:?} failed: {:?}",
        created.body
    );
    let collaboration = created.at("collaboration/id").as_str().unwrap().to_string();

    let joined = cleanrooms_call(
        router,
        region,
        "POST",
        "/memberships",
        Some(json!({
            "collaborationIdentifier": collaboration,
            "queryLogStatus": "ENABLED",
        })),
    )
    .await;
    assert_eq!(
        joined.status, 200,
        "CreateMembership in {collaboration} failed: {:?}",
        joined.body
    );
    let membership = joined.at("membership/id").as_str().unwrap().to_string();

    (collaboration, membership)
}

async fn create_algorithm(router: &MultiServiceRouter, region: &str, name: &str) -> String {
    let created = signed_call(
        router,
        region,
        "POST",
        "/configured-model-algorithms",
        Some(json!({ "name": name, "roleArn": ROLE_ARN })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "CreateConfiguredModelAlgorithm {name:?} failed: {:?}",
        created.body
    );
    created
        .at("configuredModelAlgorithmArn")
        .as_str()
        .unwrap()
        .to_string()
}

async fn create_association(
    router: &MultiServiceRouter,
    region: &str,
    membership: &str,
    request: Value,
) -> Answer {
    signed_call(
        router,
        region,
        "POST",
        &format!("/memberships/{membership}/configured-model-algorithm-associations"),
        Some(request),
    )
    .await
}

async fn get_association(
    router: &MultiServiceRouter,
    region: &str,
    membership: &str,
    arn: &str,
) -> Answer {
    signed_call(
        router,
        region,
        "GET",
        &format!(
            "/memberships/{membership}/configured-model-algorithm-associations/{}",
            label(arn)
        ),
        None,
    )
    .await
}

async fn get_collaboration_association(
    router: &MultiServiceRouter,
    region: &str,
    collaboration: &str,
    arn: &str,
) -> Answer {
    signed_call(
        router,
        region,
        "GET",
        &format!(
            "/collaborations/{collaboration}/configured-model-algorithm-associations/{}",
            label(arn)
        ),
        None,
    )
    .await
}

async fn list_associations(
    router: &MultiServiceRouter,
    region: &str,
    membership: &str,
    query: &str,
) -> Answer {
    signed_call(
        router,
        region,
        "GET",
        &format!("/memberships/{membership}/configured-model-algorithm-associations{query}"),
        None,
    )
    .await
}

async fn list_collaboration_associations(
    router: &MultiServiceRouter,
    region: &str,
    collaboration: &str,
) -> Answer {
    signed_call(
        router,
        region,
        "GET",
        &format!("/collaborations/{collaboration}/configured-model-algorithm-associations"),
        None,
    )
    .await
}

async fn list_tags(router: &MultiServiceRouter, region: &str, arn: &str) -> Answer {
    signed_call(router, region, "GET", &format!("/tags/{}", label(arn)), None).await
}

/// The association ARNs a list answered, in the order it answered them.
fn listed_arns(answer: &Answer, member: &str) -> Vec<String> {
    answer.body[member]
        .as_array()
        .unwrap_or_else(|| panic!("a list must return a {member} list: {:?}", answer.body))
        .iter()
        .map(|summary| {
            summary["configuredModelAlgorithmAssociationArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The tag set an answer reports, ordered so two answers compare by content.
fn reported_tags(answer: &Answer) -> BTreeMap<String, String> {
    answer
        .at("tags")
        .as_object()
        .map(|tags| {
            tags.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// True when `value` is the `ConfiguredModelAlgorithmAssociationArn` of an association
/// created under `membership` in `region`, from that shape's pattern
/// `^arn:aws[-a-z]*:cleanrooms-ml:[-a-z0-9]+:[0-9]{12}:membership/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/configured-model-algorithm-association/[-a-zA-Z0-9_/.]+$`.
///
/// Checked here rather than through the crate's own predicate, so a wrong predicate
/// cannot make this test pass.
fn is_association_arn(value: &str, membership: &str, region: &str) -> bool {
    let prefix = format!(
        "arn:aws:cleanrooms-ml:{region}:{ACCOUNT}:membership/{membership}/\
         configured-model-algorithm-association/"
    );
    let Some(id) = value.strip_prefix(&prefix) else {
        return false;
    };
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/' | '.'))
}

/// A `privacyConfiguration` whose policies name `receivers` for `policies`: the shape is
/// `PrivacyConfiguration$policies` (`trainedModels`, `trainedModelExports`,
/// `trainedModelInferenceJobs`), from
/// `smithy_query(query="shape", shape="PrivacyConfigurationPolicies")`.
fn privacy_configuration(policies: Value) -> Value {
    json!({ "policies": policies })
}

/// `CreateConfiguredModelAlgorithmAssociation` persists what it was given under the
/// collaboration its membership belongs to, and every read of the association computes its
/// answer from that one record.
#[tokio::test]
async fn a_created_association_is_reported_from_what_was_stored() {
    let router = emulator();
    // Two memberships in two different collaborations, so a reported collaboration can
    // only have come from the membership the create named.
    let (collaboration_a, membership_a) =
        collaboration_with_membership(&router, REGION, "assoc-collab-a").await;
    let (collaboration_b, membership_b) =
        collaboration_with_membership(&router, REGION, "assoc-collab-b").await;
    assert_ne!(
        collaboration_a, collaboration_b,
        "the two seeded collaborations must be distinct: {collaboration_a} {collaboration_b}"
    );
    let algorithm = create_algorithm(&router, REGION, "cma-for-associations").await;
    let other_algorithm = create_algorithm(&router, REGION, "cma-second").await;

    let longest_name = "n".repeat(63);
    let longest_description = "d".repeat(255);
    let many_tags: Value = json!((0..5)
        .map(|index| (format!("key-{index}"), json!(format!("value-{index}"))))
        .collect::<serde_json::Map<String, Value>>());

    // Each row: the case name, the membership (hence the collaboration) it lands in, and
    // the request body.
    let cases: Vec<(&str, &str, &str, Value)> = vec![
        (
            "only the required members",
            &membership_a,
            &collaboration_a,
            json!({ "name": "assoc-minimal", "configuredModelAlgorithmArn": algorithm }),
        ),
        (
            "a one-character name and a description",
            &membership_a,
            &collaboration_a,
            json!({
                "name": "a",
                "configuredModelAlgorithmArn": algorithm,
                "description": "the shortest name NameString admits",
            }),
        ),
        (
            "the longest name and description the model admits",
            &membership_a,
            &collaboration_a,
            json!({
                "name": longest_name,
                "configuredModelAlgorithmArn": algorithm,
                "description": longest_description,
            }),
        ),
        (
            "a privacy configuration naming one policy",
            &membership_b,
            &collaboration_b,
            json!({
                "name": "assoc-privacy-one",
                "configuredModelAlgorithmArn": algorithm,
                "privacyConfiguration": privacy_configuration(json!({
                    "trainedModels": {
                        // `LogsConfigurationPolicyList` is bounded @length(min: 1, max: 5)
                        // and `LogsConfigurationPolicy$allowedAccountIds` likewise.
                        "containerLogs": [{
                            "allowedAccountIds": [ACCOUNT],
                            // Named because the model DEFAULTS `logType` to `ALL`, so the
                            // read reports it whether or not the create sent it.
                            "logType": "ALL",
                        }],
                    },
                })),
            }),
        ),
        (
            "a privacy configuration naming every policy, and tags",
            &membership_b,
            &collaboration_b,
            json!({
                "name": "assoc-privacy-all",
                "configuredModelAlgorithmArn": other_algorithm,
                "description": "every optional member present",
                "privacyConfiguration": privacy_configuration(json!({
                    "trainedModels": {
                        "containerLogs": [{
                            "allowedAccountIds": [ACCOUNT],
                            // Catalog: model shape `LogType` (ALL | ERROR_SUMMARY).
                            "logType": "ERROR_SUMMARY",
                            "filterPattern": "ERROR",
                        }],
                        // Catalog: model shape `NoiseLevelType`
                        // (HIGH | MEDIUM | LOW | NONE).
                        "containerMetrics": { "noiseLevel": "MEDIUM" },
                        // `TrainedModelArtifactMaxSizeUnitType` admits only GB, and the
                        // value is @range(min: 0.01, max: 100.0).
                        "maxArtifactSize": { "unit": "GB", "value": 100.0 },
                    },
                    "trainedModelExports": {
                        // `TrainedModelExportsMaxSizeValue` is @range(min: 0.01, max: 50.0).
                        "maxSize": { "unit": "GB", "value": 0.01 },
                        // Catalog: model shape `TrainedModelExportFileType`
                        // (MODEL | OUTPUT).
                        "filesToExport": ["MODEL", "OUTPUT"],
                    },
                    "trainedModelInferenceJobs": {
                        "containerLogs": [{
                            "allowedAccountIds": [ACCOUNT, OTHER_ACCOUNT],
                            "logType": "ALL",
                        }],
                        "maxOutputSize": { "unit": "GB", "value": 2.5 },
                    },
                })),
                "tags": many_tags,
            }),
        ),
        (
            "one tag and nothing else optional",
            &membership_b,
            &collaboration_b,
            json!({
                "name": "assoc-one-tag",
                "configuredModelAlgorithmArn": algorithm,
                "tags": { "owner": "clean-rooms-ml" },
            }),
        ),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, membership, collaboration, request) in &cases {
        let before = unix_now();
        let created = create_association(&router, REGION, membership, (*request).clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200, got {} with {:?}",
            created.status, created.body
        );
        let arn = created
            .at("configuredModelAlgorithmAssociationArn")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_association_arn(&arn, membership, REGION),
            "case {case:?}: expected an association ARN nested under {membership} in \
             {REGION}, got {arn:?}"
        );
        assert!(
            !minted.contains(&arn),
            "case {case:?}: each create must mint a name of its own, got {arn:?} again"
        );
        minted.push(arn.clone());

        // The association's own read reports every member the create carried, the
        // membership it was created under and the collaboration that membership belongs
        // to — which this case never sent.
        let read = get_association(&router, REGION, membership, &arn).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetConfiguredModelAlgorithmAssociation answered {} with {:?}",
            read.status, read.body
        );
        for (member, expected) in request.as_object().unwrap() {
            if member == "tags" {
                continue;
            }
            assert_member_round_trips(case, member, expected, &read.at(member));
        }
        assert_eq!(
            read.at("configuredModelAlgorithmAssociationArn")
                .as_str()
                .unwrap_or_default(),
            arn,
            "case {case:?}: the read must report the ARN the create minted, got {:?}",
            read.at("configuredModelAlgorithmAssociationArn")
        );
        assert_eq!(
            read.at("membershipIdentifier").as_str().unwrap_or_default(),
            *membership,
            "case {case:?}: expected membership {membership}, got {:?}",
            read.at("membershipIdentifier")
        );
        assert_eq!(
            read.at("collaborationIdentifier")
                .as_str()
                .unwrap_or_default(),
            *collaboration,
            "case {case:?}: expected the membership's collaboration {collaboration}, got {:?}",
            read.at("collaborationIdentifier")
        );
        for member in ["createTime", "updateTime"] {
            let reported = read.at(member);
            assert!(
                is_timestamp(&reported),
                "case {case:?}: {member} must be a modelled timestamp, got {reported:?}"
            );
            let seconds = timestamp_seconds(&reported);
            assert!(
                seconds >= before - 1.0 && seconds <= unix_now() + 1.0,
                "case {case:?}: {member} {seconds} is outside the window the create ran in"
            );
        }
        // An optional member the create omitted is reported absent rather than invented.
        for member in ["description", "privacyConfiguration"] {
            if request.get(member).is_none() {
                assert_eq!(
                    read.at(member),
                    Value::Null,
                    "case {case:?}: {member} was never sent but is reported as {:?}",
                    read.at(member)
                );
            }
        }

        // The tags the create carried are the association's own, reported by its read and
        // by `ListTagsForResource` — one copy, two readers.
        let expected_tags: BTreeMap<String, String> = request
            .get("tags")
            .and_then(Value::as_object)
            .map(|tags| {
                tags.iter()
                    .map(|(key, value)| {
                        (key.clone(), value.as_str().unwrap_or_default().to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            reported_tags(&read),
            expected_tags,
            "case {case:?}: the read reported tags {:?}",
            read.at("tags")
        );
        let tags = list_tags(&router, REGION, &arn).await;
        assert_eq!(
            tags.status, 200,
            "case {case:?}: ListTagsForResource answered {} with {:?}",
            tags.status, tags.body
        );
        assert_eq!(
            reported_tags(&tags),
            expected_tags,
            "case {case:?}: ListTagsForResource reported {:?}",
            tags.at("tags")
        );

        // The collaboration's own read sees the same record, plus the account of the
        // member that created it.
        let collaboration_read =
            get_collaboration_association(&router, REGION, collaboration, &arn).await;
        assert_eq!(
            collaboration_read.status, 200,
            "case {case:?}: GetCollaborationConfiguredModelAlgorithmAssociation answered {} \
             with {:?}",
            collaboration_read.status, collaboration_read.body
        );
        assert_eq!(
            collaboration_read.at("name"),
            read.at("name"),
            "case {case:?}: the two reads of one record disagree on name"
        );
        assert_eq!(
            collaboration_read.at("configuredModelAlgorithmArn"),
            read.at("configuredModelAlgorithmArn"),
            "case {case:?}: the two reads of one record disagree on the algorithm"
        );
        assert_eq!(
            collaboration_read
                .at("creatorAccountId")
                .as_str()
                .unwrap_or_default(),
            ACCOUNT,
            "case {case:?}: expected creator account {ACCOUNT}, got {:?}",
            collaboration_read.at("creatorAccountId")
        );
    }

    // Each list reports exactly the associations of the scope it was asked about, in the
    // one stable order the store's ARN keys give.
    for (membership, collaboration) in [
        (&membership_a, &collaboration_a),
        (&membership_b, &collaboration_b),
    ] {
        let expected: Vec<String> = cases
            .iter()
            .zip(&minted)
            .filter(|((_, case_membership, _, _), _)| *case_membership == membership)
            .map(|(_, arn)| arn.clone())
            .collect();

        let listed = list_associations(&router, REGION, membership, "").await;
        assert_eq!(
            listed.status, 200,
            "case membership {membership}: ListConfiguredModelAlgorithmAssociations answered \
             {} with {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed_arns(&listed, "configuredModelAlgorithmAssociations"),
            expected,
            "case membership {membership}: expected {expected:?}, got {:?}",
            listed.body
        );

        let collaboration_listed =
            list_collaboration_associations(&router, REGION, collaboration).await;
        assert_eq!(
            collaboration_listed.status, 200,
            "case collaboration {collaboration}: the collaboration-scoped list answered {} \
             with {:?}",
            collaboration_listed.status, collaboration_listed.body
        );
        assert_eq!(
            listed_arns(
                &collaboration_listed,
                "collaborationConfiguredModelAlgorithmAssociations"
            ),
            expected,
            "case collaboration {collaboration}: expected {expected:?}, got {:?}",
            collaboration_listed.body
        );
    }
}

/// The membership-scoped list pages in ARN order: every page size from one to more than
/// the membership holds walks the same records once, and a membership holding none is an
/// empty list rather than a refusal.
#[tokio::test]
async fn the_membership_list_pages_in_arn_order() {
    let router = emulator();
    let (_, membership) = collaboration_with_membership(&router, REGION, "assoc-paging").await;
    let (_, empty_membership) = collaboration_with_membership(&router, REGION, "assoc-empty").await;
    let algorithm = create_algorithm(&router, REGION, "cma-for-paging").await;

    let mut all: Vec<String> = Vec::new();
    for index in 0..5 {
        let created = create_association(
            &router,
            REGION,
            &membership,
            json!({
                "name": format!("paged-{index}"),
                "configuredModelAlgorithmArn": algorithm,
            }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "seeding association {index} failed: {:?}",
            created.body
        );
        all.push(
            created
                .at("configuredModelAlgorithmAssociationArn")
                .as_str()
                .unwrap()
                .to_string(),
        );
    }
    all.sort();

    for page_size in [1i64, 2, 3, 5, 6, MAX_RESULTS_CEILING] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let query = match &token {
                Some(token) => format!("?maxResults={page_size}&nextToken={}", label(token)),
                None => format!("?maxResults={page_size}"),
            };
            let page = list_associations(&router, REGION, &membership, &query).await;
            assert_eq!(
                page.status, 200,
                "case maxResults={page_size}: expected 200, got {} with {:?}",
                page.status, page.body
            );
            let arns = listed_arns(&page, "configuredModelAlgorithmAssociations");
            assert!(
                arns.len() as i64 <= page_size,
                "case maxResults={page_size}: a page carried {} rows",
                arns.len()
            );
            seen.extend(arns);
            token = page.at("nextToken").as_str().map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, all,
            "case maxResults={page_size}: expected {all:?}, got {seen:?}"
        );
    }

    let empty = list_associations(&router, REGION, &empty_membership, "").await;
    assert_eq!(
        empty.status, 200,
        "case a membership holding none: expected 200, got {} with {:?}",
        empty.status, empty.body
    );
    assert_eq!(
        listed_arns(&empty, "configuredModelAlgorithmAssociations"),
        Vec::<String>::new(),
        "case a membership holding none: expected an empty list, got {:?}",
        empty.body
    );

    // A token this list never issued is refused rather than silently serving page one.
    let refused = list_associations(
        &router,
        REGION,
        &membership,
        "?nextToken=not-an-association-arn",
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case a token this list never issued: expected 400, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ValidationException",
        "case a token this list never issued: expected ValidationException, got {:?}",
        refused.body
    );
}

/// A create whose membership or whose algorithm this endpoint holds no record for is the
/// modelled `ResourceNotFoundException` (404), naming the kind of resource that was not
/// found and the identifier the request carried — and it persists nothing.
#[tokio::test]
async fn a_create_naming_a_resource_this_endpoint_does_not_hold_is_refused() {
    let router = emulator();
    let (_, membership) = collaboration_with_membership(&router, REGION, "assoc-refusals").await;
    let (_, other_region_membership) =
        collaboration_with_membership(&router, OTHER_REGION, "assoc-other-region").await;
    let algorithm = create_algorithm(&router, REGION, "cma-for-refusals").await;
    let other_region_algorithm =
        create_algorithm(&router, OTHER_REGION, "cma-other-region").await;

    // A `UUID` no Clean Rooms membership was ever created for.
    let unknown_membership = "deadbeef-1111-4222-8333-444444444444";
    // A well-formed `ConfiguredModelAlgorithmArn` naming an algorithm nothing minted.
    let unknown_algorithm = format!(
        "arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:configured-model-algorithm/\
         00000000-0000-4000-8000-000000000000"
    );

    // Each row: the case, the region the create is signed for, the membership and the
    // algorithm ARN it names, and the message the refusal must carry. The sentence is the
    // KIND's: a membership that was not found is reported `<kind> <id> does not exist.`,
    // and a configured model algorithm `<kind>: <arn> not found.`
    let cases: Vec<(&str, &str, String, String, String)> = vec![
        (
            "a membership no Clean Rooms membership exists for",
            REGION,
            unknown_membership.to_string(),
            algorithm.clone(),
            format!("Membership {unknown_membership} does not exist."),
        ),
        (
            "a membership created in another region",
            REGION,
            other_region_membership.clone(),
            algorithm.clone(),
            format!("Membership {other_region_membership} does not exist."),
        ),
        (
            "an algorithm nothing minted",
            REGION,
            membership.clone(),
            unknown_algorithm.clone(),
            format!("Configured Model Algorithm: {unknown_algorithm} not found."),
        ),
        (
            "an algorithm created in another region",
            REGION,
            membership.clone(),
            other_region_algorithm.clone(),
            format!("Configured Model Algorithm: {other_region_algorithm} not found."),
        ),
        (
            "this region's algorithm, through the other region's endpoint",
            OTHER_REGION,
            other_region_membership.clone(),
            algorithm.clone(),
            format!("Configured Model Algorithm: {algorithm} not found."),
        ),
    ];

    for (case, region, case_membership, case_algorithm, message) in &cases {
        let refused = create_association(
            &router,
            region,
            case_membership,
            json!({
                "name": "assoc-refused",
                "configuredModelAlgorithmArn": case_algorithm,
            }),
        )
        .await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected 404, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            refused.body
        );
        assert_eq!(
            refused.at("message").as_str().unwrap_or_default(),
            *message,
            "case {case:?}: expected the refusal to say {message:?}, got {:?}",
            refused.at("message")
        );
    }

    // Nothing was persisted by any of the refusals: both memberships still hold none.
    for (region, scope) in [(REGION, &membership), (OTHER_REGION, &other_region_membership)] {
        let listed = list_associations(&router, region, scope, "").await;
        assert_eq!(
            listed_arns(&listed, "configuredModelAlgorithmAssociations"),
            Vec::<String>::new(),
            "case membership {scope}: a refused create persisted a record: {:?}",
            listed.body
        );
    }
}

/// An association is addressed by its ARN AND the scope in the path: a read through
/// another membership, another collaboration or another region's endpoint is the modelled
/// `ResourceNotFoundException` (404) rather than the record.
#[tokio::test]
async fn an_association_is_only_read_through_its_own_scope() {
    let router = emulator();
    let (collaboration, membership) =
        collaboration_with_membership(&router, REGION, "assoc-scopes").await;
    let (other_collaboration, other_membership) =
        collaboration_with_membership(&router, REGION, "assoc-other-scope").await;
    let algorithm = create_algorithm(&router, REGION, "cma-for-scopes").await;

    let created = create_association(
        &router,
        REGION,
        &membership,
        json!({ "name": "assoc-scoped", "configuredModelAlgorithmArn": algorithm }),
    )
    .await;
    assert_eq!(created.status, 200, "the create failed: {:?}", created.body);
    let arn = created
        .at("configuredModelAlgorithmAssociationArn")
        .as_str()
        .unwrap()
        .to_string();

    let expected_message = format!("Configured Model Algorithm Association {arn} does not exist.");
    // Each row: the case, and the answer a read outside the association's own scope gave.
    let cases: Vec<(&str, Answer)> = vec![
        (
            "its own read, through another membership",
            get_association(&router, REGION, &other_membership, &arn).await,
        ),
        (
            "its own read, through another region's endpoint",
            get_association(&router, OTHER_REGION, &membership, &arn).await,
        ),
        (
            "the collaboration read, through another collaboration",
            get_collaboration_association(&router, REGION, &other_collaboration, &arn).await,
        ),
        (
            "the collaboration read, through another region's endpoint",
            get_collaboration_association(&router, OTHER_REGION, &collaboration, &arn).await,
        ),
    ];

    for (case, answer) in &cases {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.at("message").as_str().unwrap_or_default(),
            expected_message,
            "case {case:?}: expected the refusal to say {expected_message:?}, got {:?}",
            answer.at("message")
        );
    }

    // Its own scope still reads it back: the refusals above changed nothing.
    let read = get_association(&router, REGION, &membership, &arn).await;
    assert_eq!(
        read.status, 200,
        "the association must still read back in its own scope, got {} with {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.at("collaborationIdentifier").as_str().unwrap_or_default(),
        collaboration,
        "the association must still report its own collaboration, got {:?}",
        read.at("collaborationIdentifier")
    );
}
