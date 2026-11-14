// SPDX-License-Identifier: CC0-1.0

//! The zero-ETL integration family, end to end over the awsQuery wire.
//!
//! The rule under test: **an integration is a record of this account's control
//! plane, minted under the ARN this service assigns, whose TARGET is resolved
//! against the namespaces this account holds before anything is recorded — so
//! `DescribeIntegrations`, `DescribeInboundIntegrations`, `ModifyIntegration` and
//! `DeleteIntegration` all answer `IntegrationNotFoundFault` for an ARN that names
//! no integration, rather than an empty report or a code outside the model.** The
//! integration a create records is `creating` and becomes `active` a provisioning
//! span later; a modify applies the members the request carries and leaves the rest
//! as they stood; a delete takes the record out of every later report.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter` the
//! production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The rows differ along what the rule generalises over: the source service the
//! integration reads from (Amazon S3 and DynamoDB, the two the member's own pattern
//! admits), whether the create names a description, an encryption key or tags, and
//! which filter a read narrows the report by.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator, REGION};

/// The cluster whose namespace every row integrates INTO.
const CLUSTER: &str = "integration-target";

/// An integration ARN of the form the member's own pattern admits, naming no
/// integration this account holds.
const UNKNOWN_ARN: &str =
    "arn:aws:redshift:us-west-2:000000000000:integration:ffffffff-ffff-ffff-ffff-ffffffffffff";

/// Drive one action with owned parameters.
async fn call(emulator: &Emulator, action: &str, params: &[(String, String)]) -> Answer {
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    emulator.call(action, &borrowed).await
}

/// An emulator holding an AVAILABLE cluster, whose namespace ARN is the target
/// every row names: "the specified cluster is not in the available state" is its
/// own refusal, so the rows that are about the integration wait past the
/// cluster's provisioning span first.
async fn seeded() -> (Emulator, String) {
    let emulator = Emulator::new();
    let created = emulator.create_minimal(CLUSTER).await;
    assert_eq!(
        created.status, 200,
        "case seed {CLUSTER}: expected 200, got {} with body {}",
        created.status, created.body
    );
    let namespace = created
        .text("ClusterNamespaceArn")
        .expect("case seed: the cluster reports its namespace ARN");
    // Crossed by ADVANCING this emulator's own clock, so the cluster is serving
    // before the first row runs however loaded the host is — and the clock then
    // stands still, which is what keeps a record a later row means to read in
    // its transient state in it.
    emulator.advance_clock(state::PROVISIONING_SPAN);
    (emulator, namespace)
}

/// One row of the create table.
struct Row {
    case: &'static str,
    name: &'static str,
    source: &'static str,
    description: Option<&'static str>,
    kms_key_id: Option<&'static str>,
    tags: Vec<(&'static str, &'static str)>,
}

#[tokio::test]
async fn an_integration_is_recorded_against_the_namespace_it_replicates_into() {
    let (emulator, namespace) = seeded().await;

    let rows: Vec<Row> = vec![
        Row {
            case: "an Amazon S3 source, nothing optional",
            name: "s3-plain",
            source: "arn:aws:s3:::analytics-landing-zone",
            description: None,
            kms_key_id: None,
            tags: Vec::new(),
        },
        Row {
            case: "a DynamoDB source with a description and a key",
            name: "ddb-described",
            source: "arn:aws:dynamodb:us-west-2:000000000000:table/orders",
            description: Some("orders into the warehouse"),
            kms_key_id: Some("alias/aws/redshift"),
            tags: Vec::new(),
        },
        Row {
            case: "a tagged integration",
            name: "s3-tagged",
            source: "arn:aws:s3:::clickstream",
            description: Some("clickstream"),
            kms_key_id: None,
            tags: vec![("team", "analytics")],
        },
    ];

    let mut arns: Vec<String> = Vec::new();
    for row in &rows {
        let mut params: Vec<(String, String)> = vec![
            ("IntegrationName".to_string(), row.name.to_string()),
            ("SourceArn".to_string(), row.source.to_string()),
            ("TargetArn".to_string(), namespace.clone()),
        ];
        if let Some(description) = row.description {
            params.push(("Description".to_string(), description.to_string()));
        }
        if let Some(key) = row.kms_key_id {
            params.push(("KMSKeyId".to_string(), key.to_string()));
        }
        for (position, (key, value)) in row.tags.iter().enumerate() {
            params.push((
                format!("TagList.Tag.{}.Key", position + 1),
                (*key).to_string(),
            ));
            params.push((
                format!("TagList.Tag.{}.Value", position + 1),
                (*value).to_string(),
            ));
        }
        let created = call(&emulator, "CreateIntegration", &params).await;
        assert_eq!(
            created.status, 200,
            "case {}: expected 200, got {} with body {}",
            row.case, created.status, created.body
        );
        // The ARN is the one this service mints for the integration's own name,
        // so it is stable across reads rather than drawn per run.
        let expected_arn = state::integration_arn(row.name, REGION);
        assert_eq!(
            created.text("IntegrationArn").as_deref(),
            Some(expected_arn.as_str()),
            "case {}: expected the minted ARN, got {:?}",
            row.case,
            created.text("IntegrationArn")
        );
        assert_eq!(
            (
                created.text("IntegrationName").as_deref(),
                created.text("SourceArn").as_deref(),
                created.text("TargetArn").as_deref(),
                created.text("Status").as_deref()
            ),
            (
                Some(row.name),
                Some(row.source),
                Some(namespace.as_str()),
                Some(state::INTEGRATION_STATUS_CREATING)
            ),
            "case {}: expected the recorded integration back in the transient status, got body {}",
            row.case,
            created.body
        );
        assert_eq!(
            created.text("Description").as_deref(),
            row.description,
            "case {}: expected the declared description, got {:?}",
            row.case,
            created.text("Description")
        );
        assert_eq!(
            created.text("KMSKeyId").as_deref(),
            row.kms_key_id,
            "case {}: expected the declared key, got {:?}",
            row.case,
            created.text("KMSKeyId")
        );

        // A second create under one name is refused and records nothing.
        let duplicate = call(
            &emulator,
            "CreateIntegration",
            &[
                ("IntegrationName".to_string(), row.name.to_string()),
                ("SourceArn".to_string(), row.source.to_string()),
                ("TargetArn".to_string(), namespace.clone()),
            ],
        )
        .await;
        assert_eq!(
            (duplicate.status, duplicate.error_code().as_deref()),
            (400, Some("IntegrationAlreadyExistsFault")),
            "case {}: expected 400 IntegrationAlreadyExistsFault, got {} / {:?}",
            row.case,
            duplicate.status,
            duplicate.error_code()
        );
        arns.push(expected_arn);
    }

    // ROUND TRIP: every integration the account holds, in creation order, and the
    // ONE an ARN names.
    let listed = emulator.call("DescribeIntegrations", &[]).await;
    assert_eq!(
        all_text(&listed.body, "IntegrationArn"),
        arns,
        "case list every integration: expected creation order, got body {}",
        listed.body
    );
    let one = call(
        &emulator,
        "DescribeIntegrations",
        &[("IntegrationArn".to_string(), arns[1].clone())],
    )
    .await;
    assert_eq!(
        all_text(&one.body, "IntegrationName"),
        vec![rows[1].name.to_string()],
        "case read one integration: expected only it, got body {}",
        one.body
    );

    // The clock owes each integration its `active` status.
    emulator.advance_clock(state::PROVISIONING_SPAN);
    let settled = emulator.call("DescribeIntegrations", &[]).await;
    assert_eq!(
        all_text(&settled.body, "Status"),
        vec![state::INTEGRATION_STATUS_ACTIVE.to_string(); rows.len()],
        "case settled integrations: expected active once the span has passed, got body {}",
        settled.body
    );

    // Each filter selects the integrations whose own value is one of its values.
    let by_source = call(
        &emulator,
        "DescribeIntegrations",
        &[
            (
                "Filters.DescribeIntegrationsFilter.1.Name".to_string(),
                "source-arn".to_string(),
            ),
            (
                "Filters.DescribeIntegrationsFilter.1.Values.Value.1".to_string(),
                rows[2].source.to_string(),
            ),
        ],
    )
    .await;
    assert_eq!(
        all_text(&by_source.body, "IntegrationName"),
        vec![rows[2].name.to_string()],
        "case filter by source: expected only that integration, got body {}",
        by_source.body
    );
    let by_source_type = call(
        &emulator,
        "DescribeIntegrations",
        &[
            (
                "Filters.DescribeIntegrationsFilter.1.Name".to_string(),
                "source-types".to_string(),
            ),
            (
                "Filters.DescribeIntegrationsFilter.1.Values.Value.1".to_string(),
                "dynamodb".to_string(),
            ),
        ],
    )
    .await;
    assert_eq!(
        all_text(&by_source_type.body, "IntegrationName"),
        vec![rows[1].name.to_string()],
        "case filter by source type: expected only the DynamoDB integration, got body {}",
        by_source_type.body
    );
    let by_status = call(
        &emulator,
        "DescribeIntegrations",
        &[
            (
                "Filters.DescribeIntegrationsFilter.1.Name".to_string(),
                "status".to_string(),
            ),
            (
                "Filters.DescribeIntegrationsFilter.1.Values.Value.1".to_string(),
                state::INTEGRATION_STATUS_ACTIVE.to_string(),
            ),
        ],
    )
    .await;
    assert_eq!(
        all_text(&by_status.body, "IntegrationArn").len(),
        rows.len(),
        "case filter by status: expected every active integration, got body {}",
        by_status.body
    );

    // The INBOUND view of the same integrations: the ones whose target is a
    // namespace this account holds.
    let inbound = emulator.call("DescribeInboundIntegrations", &[]).await;
    assert_eq!(
        all_text(&inbound.body, "IntegrationArn"),
        arns,
        "case list inbound integrations: expected the same records, got body {}",
        inbound.body
    );
    let inbound_by_target = call(
        &emulator,
        "DescribeInboundIntegrations",
        &[("TargetArn".to_string(), namespace.clone())],
    )
    .await;
    assert_eq!(
        all_text(&inbound_by_target.body, "IntegrationArn"),
        arns,
        "case inbound by target: expected the namespace's integrations, got body {}",
        inbound_by_target.body
    );

    // A MODIFY applies the members the request carries and leaves the rest.
    let modified = call(
        &emulator,
        "ModifyIntegration",
        &[
            ("IntegrationArn".to_string(), arns[0].clone()),
            ("Description".to_string(), "the landing zone".to_string()),
        ],
    )
    .await;
    assert_eq!(
        modified.status, 200,
        "case modify the description: expected 200, got {} with body {}",
        modified.status, modified.body
    );
    assert_eq!(
        (
            modified.text("Description").as_deref(),
            modified.text("Status").as_deref(),
            modified.text("SourceArn").as_deref()
        ),
        (
            Some("the landing zone"),
            Some(state::INTEGRATION_STATUS_MODIFYING),
            Some(rows[0].source)
        ),
        "case modify the description: expected the change in progress and the source kept, got body {}",
        modified.body
    );
    let read_back = call(
        &emulator,
        "DescribeIntegrations",
        &[("IntegrationArn".to_string(), arns[0].clone())],
    )
    .await;
    assert_eq!(
        read_back.text("Description").as_deref(),
        Some("the landing zone"),
        "case modify the description: expected it to persist, got body {}",
        read_back.body
    );
    // A rename onto a name another integration carries is refused.
    let collision = call(
        &emulator,
        "ModifyIntegration",
        &[
            ("IntegrationArn".to_string(), arns[0].clone()),
            ("IntegrationName".to_string(), rows[1].name.to_string()),
        ],
    )
    .await;
    assert_eq!(
        (collision.status, collision.error_code().as_deref()),
        (400, Some("IntegrationAlreadyExistsFault")),
        "case rename onto a taken name: expected 400 IntegrationAlreadyExistsFault, got {} / {:?}",
        collision.status,
        collision.error_code()
    );

    // A DELETE reports the record in the status the deletion put it in, and takes
    // it out of every later report.
    let deleted = call(
        &emulator,
        "DeleteIntegration",
        &[("IntegrationArn".to_string(), arns[0].clone())],
    )
    .await;
    assert_eq!(
        (deleted.status, deleted.text("Status").as_deref()),
        (200, Some(state::INTEGRATION_STATUS_DELETING)),
        "case delete an integration: expected 200 and the deleting status, got {} with body {}",
        deleted.status,
        deleted.body
    );
    let after_delete = emulator.call("DescribeIntegrations", &[]).await;
    assert_eq!(
        all_text(&after_delete.body, "IntegrationArn"),
        arns[1..].to_vec(),
        "case delete an integration: expected only that one to go, got body {}",
        after_delete.body
    );
}

#[tokio::test]
async fn an_arn_that_names_no_integration_is_refused_by_every_operation() {
    let (emulator, namespace) = seeded().await;

    for (action, extra) in [
        ("DescribeIntegrations", None),
        ("DescribeInboundIntegrations", None),
        ("DeleteIntegration", None),
        ("ModifyIntegration", Some(("Description", "whatever"))),
    ] {
        let mut params: Vec<(String, String)> =
            vec![("IntegrationArn".to_string(), UNKNOWN_ARN.to_string())];
        if let Some((name, value)) = extra {
            params.push((name.to_string(), value.to_string()));
        }
        let answer = call(&emulator, action, &params).await;
        assert_eq!(
            (answer.status, answer.error_code().as_deref()),
            (404, Some("IntegrationNotFoundFault")),
            "case {action} with an unknown ARN: expected 404 IntegrationNotFoundFault, got {} / {:?} with body {}",
            answer.status,
            answer.error_code(),
            answer.body
        );
        assert_eq!(
            answer.text("Message").as_deref(),
            Some(state::integration_not_found_message(UNKNOWN_ARN).as_str()),
            "case {action} with an unknown ARN: expected the message to name it, got {:?}",
            answer.text("Message")
        );
    }

    // A TARGET that names no namespace this account holds is no target at all.
    let unknown_namespace = "arn:aws:redshift:us-west-2:000000000000:namespace/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let refused = call(
        &emulator,
        "CreateIntegration",
        &[
            ("IntegrationName".to_string(), "no-target".to_string()),
            (
                "SourceArn".to_string(),
                "arn:aws:s3:::analytics-landing-zone".to_string(),
            ),
            ("TargetArn".to_string(), unknown_namespace.to_string()),
        ],
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code().as_deref()),
        (404, Some("IntegrationTargetNotFoundFault")),
        "case an unknown target: expected 404 IntegrationTargetNotFoundFault, got {} / {:?} with body {}",
        refused.status,
        refused.error_code(),
        refused.body
    );
    let inbound = call(
        &emulator,
        "DescribeInboundIntegrations",
        &[("TargetArn".to_string(), unknown_namespace.to_string())],
    )
    .await;
    assert_eq!(
        (inbound.status, inbound.error_code().as_deref()),
        (400, Some("InvalidNamespaceFault")),
        "case inbound read of an unknown namespace: expected 400 InvalidNamespaceFault, got {} / {:?}",
        inbound.status,
        inbound.error_code()
    );

    // A cluster that is not available cannot take an integration yet.
    let coming_up = emulator.create_minimal("integration-coming-up").await;
    assert_eq!(
        coming_up.status, 200,
        "case seed a creating cluster: expected 200, got {} with body {}",
        coming_up.status, coming_up.body
    );
    let creating_namespace = coming_up
        .text("ClusterNamespaceArn")
        .expect("the creating cluster reports its namespace ARN");
    let too_early = call(
        &emulator,
        "CreateIntegration",
        &[
            ("IntegrationName".to_string(), "too-early".to_string()),
            (
                "SourceArn".to_string(),
                "arn:aws:s3:::analytics-landing-zone".to_string(),
            ),
            ("TargetArn".to_string(), creating_namespace),
        ],
    )
    .await;
    assert_eq!(
        (too_early.status, too_early.error_code().as_deref()),
        (400, Some("InvalidClusterState")),
        "case a target still coming up: expected 400 InvalidClusterState, got {} / {:?} with body {}",
        too_early.status,
        too_early.error_code(),
        too_early.body
    );

    // The namespace the available cluster holds is a target that resolves, which
    // is what makes the refusals above about the target rather than about the
    // operation.
    let accepted = call(
        &emulator,
        "CreateIntegration",
        &[
            ("IntegrationName".to_string(), "resolves".to_string()),
            (
                "SourceArn".to_string(),
                "arn:aws:s3:::analytics-landing-zone".to_string(),
            ),
            ("TargetArn".to_string(), namespace),
        ],
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case a target that resolves: expected 200, got {} / {:?} with body {}",
        accepted.status,
        accepted.error_code(),
        accepted.body
    );
}
