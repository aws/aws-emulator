// SPDX-License-Identifier: CC0-1.0

//! The awsQuery error envelope, end to end over the wire: every failed call answers an
//! `<ErrorResponse>` carrying a `<RequestId>`.
//!
//! The rule under test is the PROTOCOL's and belongs to no one operation: an awsQuery
//! error response is
//! `<ErrorResponse><Error><Type/><Code/><Message/></Error><RequestId/></ErrorResponse>`,
//! so whichever layer refused the call — a handler's modelled error, a handler refusal,
//! the boundary validator, the scaffold's 501, the routing table — the answer carries a
//! request id. That element is what an SDK's error object exposes as the request id of a
//! failure and what correlates a failed call against server-side records, exactly as a
//! success carries `ResponseMetadata/RequestId`.
//!
//! The table below is therefore keyed on the CLASS of failure rather than on an
//! operation, and each row names the status and the wire code it expects as well, so a
//! row cannot pass by failing for a different reason. The recorded cases this suite grew
//! from (`ModifyCluster` and `DeleteCluster` against an absent cluster,
//! `CreateClusterParameterGroup` for a name in use, `DescribeClusterParameters` for an
//! absent group, `CreateIntegration`) are four of its rows, not its subject.
//!
//! The success path is checked too: it must keep its single `ResponseMetadata/RequestId`
//! and grow no error envelope, because the writer under test rewrites error documents
//! only.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Emulator};

/// One row of the error-envelope table: a request that must fail, and the status and
/// wire code it must fail with.
struct ErrorCase {
    /// What class of failure this row drives.
    case: &'static str,
    action: &'static str,
    params: Vec<(String, String)>,
    status: u16,
    code: &'static str,
}

/// `Emulator::call` takes borrowed pairs; a row that builds owned parameters (a name read
/// out of the store's own seed, a value at a modelled bound) borrows them through this.
fn borrowed(params: &[(String, String)]) -> Vec<(&str, &str)> {
    params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

/// Build one row's parameters from `&str` pairs.
fn params(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

/// A `CreateIntegration` request whose members all satisfy the model, so the call reaches
/// the (still scaffolded) handler instead of being refused at the boundary.
///
/// The ARNs satisfy the `SourceArn` / `TargetArn` patterns the model declares
/// (`smithy_query(query="operation",operation="CreateIntegration")`).
const INTEGRATION_SOURCE_ARN: &str = "arn:aws:s3:::integration-source-bucket";
const INTEGRATION_TARGET_ARN: &str =
    "arn:aws:redshift:us-west-2:123456789012:namespace/abc-12345678";

/// The longest `IntegrationName` the model admits is 63 characters (`IntegrationName`,
/// `length 1..63`), so a 64-character one is one past the published bound and must be
/// refused by the boundary validator rather than reaching a handler.
fn integration_name_past_the_bound() -> String {
    std::iter::once('n')
        .chain(std::iter::repeat_n('x', 63))
        .collect()
}

/// The parameter group [`seed_existing_group`] creates, so the already-exists row of the
/// table refuses a name that really is in use.
///
/// A created group rather than the account's seeded `default.…` one: the documented
/// `ParameterGroupName` syntax admits no dot, so a request naming a default group is
/// refused on the syntax rule before any name is looked up.
const EXISTING_GROUP: &str = "envelope-existing";

/// Create [`EXISTING_GROUP`], the one piece of state the table's already-exists row needs.
async fn seed_existing_group(emulator: &Emulator) {
    let family = state::parameter_group_family(state::DEFAULT_CLUSTER_VERSION);
    let answer = emulator
        .call(
            "CreateClusterParameterGroup",
            &[
                ("ParameterGroupName", EXISTING_GROUP),
                ("ParameterGroupFamily", family.as_str()),
                ("Description", "the group a duplicate create collides with"),
            ],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "seeding {EXISTING_GROUP:?} must succeed, got {} with {:?}",
        answer.status, answer.body
    );
}

/// Every class of failure this service can answer, one row each.
///
/// The codes are the ones each layer names: a modelled error renders its `awsQueryError`
/// code (`ClusterNotFound`, `ClusterParameterGroupNotFound`,
/// `ClusterParameterGroupAlreadyExists` —
/// `smithy_query(query="operation",operation="ModifyCluster,DeleteCluster,DescribeClusterParameters,CreateClusterParameterGroup")`),
/// a handler refusal the model cannot express the query-protocol code the service
/// itself answers (`InvalidParameterCombination` for members that cannot stand
/// together), a body the codec
/// could not read `MalformedInput`, a value outside a declared bound `ValidationError`, a
/// scaffolded operation `NotImplemented`, and an `Action` naming no modelled operation
/// `InvalidAction`.
fn error_cases() -> Vec<ErrorCase> {
    // The family of the engine version this service publishes, read off the store's own
    // seed rather than spelled here.
    let existing_family = state::parameter_group_family(state::DEFAULT_CLUSTER_VERSION);

    vec![
        ErrorCase {
            case: "a modelled 404: a modify naming no cluster",
            action: "ModifyCluster",
            params: params(&[("ClusterIdentifier", "no-such-cluster")]),
            status: 404,
            code: "ClusterNotFound",
        },
        ErrorCase {
            case: "a modelled 404: a delete naming no cluster",
            action: "DeleteCluster",
            params: params(&[
                ("ClusterIdentifier", "no-such-cluster"),
                ("SkipFinalClusterSnapshot", "true"),
            ]),
            status: 404,
            code: "ClusterNotFound",
        },
        ErrorCase {
            case: "a modelled 404: a parameter read naming no group",
            action: "DescribeClusterParameters",
            params: params(&[("ParameterGroupName", "no-such-group")]),
            status: 404,
            code: "ClusterParameterGroupNotFound",
        },
        ErrorCase {
            case: "a modelled 400: a create for a name already in use",
            action: "CreateClusterParameterGroup",
            params: vec![
                (
                    "ParameterGroupName".to_string(),
                    EXISTING_GROUP.to_string(),
                ),
                ("ParameterGroupFamily".to_string(), existing_family),
                ("Description".to_string(), "a duplicate".to_string()),
            ],
            status: 400,
            code: "ClusterParameterGroupAlreadyExists",
        },
        ErrorCase {
            case: "a handler refusal the model cannot express: a delete naming neither a \
                   final snapshot nor a skip",
            action: "DeleteCluster",
            params: params(&[("ClusterIdentifier", "any-cluster")]),
            status: 400,
            code: "InvalidParameterCombination",
        },
        ErrorCase {
            case: "an absent required member, which the codec refuses",
            action: "ModifyCluster",
            params: Vec::new(),
            status: 400,
            code: "MalformedInput",
        },
        ErrorCase {
            case: "a value one past a declared length bound",
            action: "CreateIntegration",
            params: vec![
                (
                    "IntegrationName".to_string(),
                    integration_name_past_the_bound(),
                ),
                ("SourceArn".to_string(), INTEGRATION_SOURCE_ARN.to_string()),
                ("TargetArn".to_string(), INTEGRATION_TARGET_ARN.to_string()),
            ],
            status: 400,
            code: "ValidationError",
        },
        ErrorCase {
            case: "a scaffolded operation, which answers the 501 channel",
            action: "ModifySnapshotSchedule",
            params: vec![
                ("ScheduleIdentifier".to_string(), "nightly".to_string()),
                (
                    "ScheduleDefinitions.ScheduleDefinition.1".to_string(),
                    "rate(12 hours)".to_string(),
                ),
            ],
            status: 501,
            code: "NotImplemented",
        },
        ErrorCase {
            case: "an Action naming no modelled operation",
            action: "NotAnActionAtAll",
            params: Vec::new(),
            status: 400,
            code: "InvalidAction",
        },
    ]
}

/// One row of a table whose action must FAIL: the case, the action, the parameters the
/// request carries, and the status and wire code it must fail with.
type RefusedCase<'a> = (&'a str, &'a str, Vec<(&'a str, &'a str)>, u16, &'a str);

/// One row of a table whose action must SUCCEED: the case, the action, and the owned
/// parameters the request carries.
type SucceedingCase<'a> = (&'a str, &'a str, Vec<(String, String)>);

/// The request id of an error response, checked to be where AWS puts it: exactly one
/// `<RequestId>`, a sibling AFTER `<Error>` and inside `<ErrorResponse>`.
fn error_envelope_request_id(case: &str, body: &str) -> String {
    assert!(
        body.contains("<ErrorResponse"),
        "case {case:?}: expected an <ErrorResponse> envelope, got {body:?}"
    );
    let ids = all_text(body, "RequestId");
    assert_eq!(
        ids.len(),
        1,
        "case {case:?}: expected exactly one <RequestId> in the envelope, got {ids:?} from \
         {body:?}"
    );
    let error_close = body
        .rfind("</Error>")
        .unwrap_or_else(|| panic!("case {case:?}: expected an <Error> element, got {body:?}"));
    let request_id_open = body
        .find("<RequestId>")
        .unwrap_or_else(|| panic!("case {case:?}: expected a <RequestId> element, got {body:?}"));
    let envelope_close = body.rfind("</ErrorResponse>").unwrap_or_else(|| {
        panic!("case {case:?}: expected a closed <ErrorResponse>, got {body:?}")
    });
    assert!(
        error_close < request_id_open && request_id_open < envelope_close,
        "case {case:?}: expected <RequestId> between </Error> and </ErrorResponse>, got {body:?}"
    );
    let request_id = ids.into_iter().next().expect("one id");
    assert!(
        !request_id.is_empty(),
        "case {case:?}: expected a non-empty request id, got {body:?}"
    );
    request_id
}

/// A request id has the 8-4-4-4-12 lowercase-hex form AWS answers with, whether it came
/// from an error envelope or from a successful response's `ResponseMetadata`.
fn assert_request_id_shape(case: &str, request_id: &str) {
    let groups: Vec<&str> = request_id.split('-').collect();
    let widths = [8usize, 4, 4, 4, 12];
    assert_eq!(
        groups.len(),
        widths.len(),
        "case {case:?}: expected five hyphen-separated groups, got {request_id:?}"
    );
    for (group, width) in groups.iter().zip(widths) {
        assert!(
            group.len() == width
                && group
                    .chars()
                    .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "case {case:?}: expected a {width}-character lowercase-hex group, got {group:?} in \
             {request_id:?}"
        );
    }
}

/// Every class of failure answers an envelope carrying a request id — and still answers
/// the status and the wire code it did before.
#[tokio::test]
async fn every_error_response_carries_a_request_id() {
    let emulator = Emulator::new();
    seed_existing_group(&emulator).await;

    for case in error_cases() {
        let answer = emulator
            .call(case.action, &borrowed(&case.params))
            .await;
        assert_eq!(
            answer.status, case.status,
            "case {:?}: expected HTTP {}, got {} with {:?}",
            case.case, case.status, answer.status, answer.body
        );
        assert_eq!(
            answer.error_code().as_deref(),
            Some(case.code),
            "case {:?}: expected <Code>{}</Code>, got {:?}",
            case.case,
            case.code,
            answer.body
        );
        let request_id = error_envelope_request_id(case.case, &answer.body);
        assert_request_id_shape(case.case, &request_id);
    }
}

/// A request id names ONE request: two failures, of the same class or of different ones,
/// never report the same id.
#[tokio::test]
async fn each_error_response_carries_its_own_request_id() {
    let emulator = Emulator::new();
    seed_existing_group(&emulator).await;

    let cases = error_cases();
    let mut seen: Vec<String> = Vec::new();
    // Every row twice: the same request repeated must still report a fresh id, which a
    // value derived from the request could not do.
    for case in cases.iter().chain(cases.iter()) {
        let answer = emulator
            .call(case.action, &borrowed(&case.params))
            .await;
        let request_id = error_envelope_request_id(case.case, &answer.body);
        assert!(
            !seen.contains(&request_id),
            "case {:?}: expected a fresh request id, got the already-reported {request_id:?}",
            case.case
        );
        seen.push(request_id);
    }
}

/// A refusal that depends on STORED state answers the same envelope: a cluster created
/// and then deleted is gone, and the refusals its absence (and its presence) produce
/// carry a request id too.
#[tokio::test]
async fn a_state_dependent_refusal_carries_a_request_id() {
    let emulator = Emulator::new();

    let created = emulator.create_minimal("envelope-cluster").await;
    assert_eq!(
        created.status, 200,
        "the create must succeed, got {} with {:?}",
        created.status, created.body
    );
    let snapshot = emulator
        .call(
            "CreateClusterSnapshot",
            &[
                ("SnapshotIdentifier", "envelope-snapshot"),
                ("ClusterIdentifier", "envelope-cluster"),
            ],
        )
        .await;
    assert_eq!(
        snapshot.status, 200,
        "the snapshot must be taken, got {} with {:?}",
        snapshot.status, snapshot.body
    );

    // (case, action, params, status, code)
    let rows: Vec<RefusedCase<'_>> = vec![
        (
            "a duplicate create",
            "CreateCluster",
            vec![
                ("ClusterIdentifier", "envelope-cluster"),
                ("NodeType", "ra3.xlplus"),
                ("MasterUsername", "admin"),
            ],
            400,
            "ClusterAlreadyExists",
        ),
        (
            "a snapshot identifier already in use",
            "CreateClusterSnapshot",
            vec![
                ("SnapshotIdentifier", "envelope-snapshot"),
                ("ClusterIdentifier", "envelope-cluster"),
            ],
            400,
            "ClusterSnapshotAlreadyExists",
        ),
        (
            "a read of a snapshot nothing took",
            "DescribeClusterSnapshots",
            vec![("SnapshotIdentifier", "no-such-snapshot")],
            404,
            "ClusterSnapshotNotFound",
        ),
    ];

    for (case, action, call_params, status, code) in rows {
        let answer = emulator.call(action, &call_params).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code().as_deref(),
            Some(code),
            "case {case:?}: expected <Code>{code}</Code>, got {:?}",
            answer.body
        );
        let request_id = error_envelope_request_id(case, &answer.body);
        assert_request_id_shape(case, &request_id);
    }
}

/// The error envelope a handler's error is serialized into is rooted at the model's
/// `<ErrorResponse xmlns="...">`, and the added `<RequestId>` sits inside that root.
#[tokio::test]
async fn the_error_envelope_carries_the_service_namespace() {
    let emulator = Emulator::new();
    let namespace = modelled_xml_namespace();

    let answer = emulator
        .call("ModifyCluster", &[("ClusterIdentifier", "no-such-cluster")])
        .await;
    assert_eq!(
        answer.status, 404,
        "the modify must be refused, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        answer
            .body
            .contains(&format!("<ErrorResponse xmlns=\"{namespace}\">")),
        "case \"a modelled error\": expected the model's namespace on the envelope root, got \
         {:?}",
        answer.body
    );
    let request_id = error_envelope_request_id("a modelled error", &answer.body);
    assert_request_id_shape("a modelled error", &request_id);
}

/// The service's `@xmlNamespace` URI, read out of the shipped model rather than restated
/// here (`smithy_query(query="service_info")` -> `xmlNamespace.uri`).
fn modelled_xml_namespace() -> String {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-redshift-redshiftserviceversion20121201.json"
    );
    let model: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("the shipped model"))
            .expect("the model parses as JSON");
    model["shapes"]["com.amazonaws.redshift#RedshiftServiceVersion20121201"]["traits"]
        ["smithy.api#xmlNamespace"]["uri"]
        .as_str()
        .expect("the service shape declares an @xmlNamespace uri")
        .to_string()
}

/// The success path is untouched: a successful response keeps its one
/// `ResponseMetadata/RequestId` and grows no error envelope.
#[tokio::test]
async fn a_successful_response_keeps_its_response_metadata_request_id() {
    let emulator = Emulator::new();
    let family = state::parameter_group_family(state::DEFAULT_CLUSTER_VERSION);

    // (case, action, params)
    let rows: Vec<SucceedingCase<'_>> = vec![
        ("an empty describe", "DescribeClusters", Vec::new()),
        (
            "a describe of the seeded parameter groups",
            "DescribeClusterParameterGroups",
            Vec::new(),
        ),
        (
            "a create",
            "CreateClusterParameterGroup",
            vec![
                ("ParameterGroupName".to_string(), "envelope-wlm".to_string()),
                ("ParameterGroupFamily".to_string(), family),
                ("Description".to_string(), "queue assignment".to_string()),
            ],
        ),
        (
            "a read that reads it back",
            "DescribeClusterParameters",
            params(&[("ParameterGroupName", "envelope-wlm")]),
        ),
    ];

    for (case, action, call_params) in rows {
        let answer = emulator.call(action, &borrowed(&call_params)).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            !answer.body.contains("<ErrorResponse"),
            "case {case:?}: a success must carry no error envelope, got {:?}",
            answer.body
        );
        let ids = all_text(&answer.body, "RequestId");
        assert_eq!(
            ids.len(),
            1,
            "case {case:?}: expected one ResponseMetadata/RequestId, got {ids:?} from {:?}",
            answer.body
        );
        assert_request_id_shape(case, &ids[0]);
    }
}
