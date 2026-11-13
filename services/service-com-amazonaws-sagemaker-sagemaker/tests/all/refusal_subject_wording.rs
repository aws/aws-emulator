// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the SUBJECT a SageMaker refusal names, and for the one
//! change an archived model card does not take.
//!
//! Three rules, each asserted over a TABLE rather than one recorded call:
//!
//! 1. `AddTags`, `ListTags` and `DeleteTags` address ANY resource kind by ARN, and the
//!    refusal for an ARN this account holds no resource under is the OWNING resource
//!    family's own not-found sentence — the same text that family's reader answers — not
//!    one generic prose for every kind;
//! 2. the refusals worded `Cannot find <subject>: <arn>` name WHAT is missing and end on
//!    the ARN BARE, with no quoting and no closing period, whichever subject they name;
//! 3. `ModelCardStatus`'s `Archived` member means "No more updates should be made to the
//!    model card", so `UpdateModelCard` against an archived card is refused and the card
//!    keeps every version, status and document it held.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) on the harness the `inference_deployment_lifecycle` suite stands up, so
//! what these tests exercise is what a real SDK client gets.

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};
use crate::model_card_governance::{
    content_with_description, create_body, describe_body, number_field, MODEL_CARD_STATUSES,
};

/// An `arn:aws:sagemaker:<region>:<account>:<kind>/<name>` ARN, as this service mints one.
fn arn(region: &str, kind: &str, name: &str) -> String {
    format!("arn:aws:sagemaker:{region}:{ACCOUNT}:{kind}/{name}")
}

/// The three operations that address a resource by `ResourceArn` alone, with a body for
/// each: the rule holds for the reader and for both writers.
fn tag_requests(resource_arn: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "AddTags",
            json!({ "ResourceArn": resource_arn, "Tags": [{ "Key": "env", "Value": "prod" }] }),
        ),
        ("ListTags", json!({ "ResourceArn": resource_arn })),
        (
            "DeleteTags",
            json!({ "ResourceArn": resource_arn, "TagKeys": ["env"] }),
        ),
    ]
}

/// Call each tag operation on `resource_arn`, signed for `region`, and require that all
/// three refuse with HTTP 400 `ValidationException` and EXACTLY `expected`.
async fn assert_every_tag_operation_refuses(
    emulator: &Emulator,
    case: &str,
    region: &str,
    resource_arn: &str,
    expected: &str,
) {
    for (operation, body) in tag_requests(resource_arn) {
        let (status, refused) = emulator.call_in(region, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?} {operation}: expected HTTP 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?} {operation}: expected ValidationException, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            expected,
            "case {case:?} {operation}: expected message {expected:?}, got {:?}",
            message(&refused)
        );
    }
}

/// RULE: a tag operation that cannot resolve its `ResourceArn` refuses with the not-found
/// sentence of the resource family the ARN's resource-kind segment names — the entity
/// label, the identifier and the quoting that family's own operations use — and falls
/// back to the family-agnostic `Resource '<arn>' does not exist.` for an ARN naming
/// another Region or account, and for the families whose recorded refusal quotes the
/// identifier as the caller sent it (a tag request sends no name).
#[tokio::test]
async fn a_tag_refusal_words_the_owning_resource_familys_sentence() {
    // Rows differ along the resource KIND the ARN names — which is what the rule
    // generalises over — and so along the sentence's entity label, its quoting and
    // whether it closes with a period. Each expected text is the one that family's own
    // helper in `crate::state` renders (`does_not_exist`, `entity_does_not_exist`,
    // `model_card_not_found`, `notebook_instance_does_not_exist`, `image_does_not_exist`),
    // i.e. the text its reader answers for the same resource.
    struct Row {
        case: &'static str,
        kind: &'static str,
        name: &'static str,
        /// `true` when the family quotes the ARN in single quotes.
        quoted: bool,
        entity: &'static str,
        /// The whole sentence, when the family words one of its own.
        sentence: Option<fn(&str) -> String>,
    }

    let rows = [
        Row {
            case: "a lineage action",
            kind: "action",
            name: "absent-action",
            quoted: true,
            entity: "Action",
            sentence: None,
        },
        Row {
            case: "a lineage artifact",
            kind: "artifact",
            name: "0123456789abcdef0123456789abcdef",
            quoted: true,
            entity: "Artifact",
            sentence: None,
        },
        Row {
            case: "a lineage context",
            kind: "context",
            name: "absent-context",
            quoted: true,
            entity: "Context",
            sentence: None,
        },
        Row {
            case: "a lineage group",
            kind: "lineage-group",
            name: "absent-group",
            quoted: true,
            entity: "LineageGroup",
            sentence: None,
        },
        Row {
            case: "an experiment",
            kind: "experiment",
            name: "absent-experiment",
            quoted: true,
            entity: "Experiment",
            sentence: None,
        },
        Row {
            case: "an experiment trial",
            kind: "experiment-trial",
            name: "absent-trial",
            quoted: true,
            entity: "Trial",
            sentence: None,
        },
        Row {
            case: "an experiment trial component",
            kind: "experiment-trial-component",
            name: "absent-component",
            quoted: true,
            entity: "Trial component",
            sentence: None,
        },
        Row {
            case: "a pipeline",
            kind: "pipeline",
            name: "absent-pipeline",
            quoted: true,
            entity: "Pipeline",
            sentence: None,
        },
        Row {
            case: "an algorithm",
            kind: "algorithm",
            name: "absent-algorithm",
            quoted: false,
            entity: "Algorithm",
            sentence: None,
        },
        Row {
            case: "a model package group",
            kind: "model-package-group",
            name: "absent-mpg",
            quoted: false,
            entity: "ModelPackageGroup",
            sentence: None,
        },
        Row {
            case: "a model package",
            kind: "model-package",
            name: "absent-package",
            quoted: false,
            entity: "ModelPackage",
            sentence: None,
        },
        Row {
            case: "a model card",
            kind: "model-card",
            name: "absent-card",
            quoted: false,
            entity: "Modelcard",
            // One word, and NO closing period: the model-card family's own sentence.
            sentence: Some(|resource_arn| format!("Modelcard {resource_arn} does not exist")),
        },
        Row {
            case: "a notebook instance",
            kind: "notebook-instance",
            name: "absent-notebook",
            quoted: false,
            entity: "Notebook Instance",
            sentence: Some(|resource_arn| {
                format!("Notebook Instance {resource_arn} does not exist")
            }),
        },
        Row {
            case: "a custom image",
            kind: "image",
            name: "absent-image",
            quoted: false,
            entity: "Image",
            sentence: Some(|resource_arn| format!("Image with ARN {resource_arn} does not exist")),
        },
        Row {
            case: "a code repository",
            kind: "code-repository",
            name: "absent-repository",
            quoted: false,
            entity: "CodeRepository",
            // The `Cannot find <subject>: <arn>` family: bare ARN, no closing period.
            sentence: Some(|resource_arn| format!("Cannot find CodeRepository: {resource_arn}")),
        },
        Row {
            case: "a cluster policy",
            kind: "cluster-scheduler-config",
            name: "absent-scheduler-config",
            quoted: false,
            entity: "Cluster scheduler config",
            sentence: Some(|resource_arn| {
                format!("Cluster scheduler config with arn {resource_arn} could not be found")
            }),
        },
        Row {
            case: "a compute quota",
            kind: "compute-quota",
            name: "absent-quota",
            quoted: false,
            entity: "Compute Quota",
            sentence: Some(|resource_arn| {
                format!("Compute Quota with arn {resource_arn} could not be found")
            }),
        },
        Row {
            case: "an app image configuration",
            kind: "app-image-config",
            name: "absent-app-image-config",
            quoted: false,
            entity: "AppImageConfig",
            // The Studio family brackets the ARN and closes with no period.
            sentence: Some(|resource_arn| {
                format!("AppImageConfig [{resource_arn}] does not exist")
            }),
        },
        Row {
            case: "a notebook instance lifecycle configuration",
            kind: "notebook-instance-lifecycle-config",
            name: "absent-lifecycle-config",
            quoted: false,
            entity: "notebook instance lifecycle configuration",
            sentence: Some(|resource_arn| {
                format!(
                    "Could not find notebook instance lifecycle configuration \
                     \"{resource_arn}\"."
                )
            }),
        },
        // Two families whose sentence names the NAME inside the ARN, which is what their
        // own readers reduce an ARN form to.
        Row {
            case: "a JumpStart hub",
            kind: "hub",
            name: "absent-hub",
            quoted: false,
            entity: "Hub",
            sentence: Some(|resource_arn| {
                format!(
                    "Hub with name {} does not exist.",
                    resource_arn.rsplit('/').next().unwrap_or_default()
                )
            }),
        },
        Row {
            case: "a feature group",
            kind: "feature-group",
            name: "absent-feature-group",
            quoted: false,
            entity: "FeatureGroup",
            sentence: Some(|resource_arn| {
                format!(
                    "Resource Not Found: Amazon SageMaker can't find a FeatureGroup with \
                     name {}",
                    resource_arn.rsplit('/').next().unwrap_or_default()
                )
            }),
        },
        // The fallback: the families whose recorded refusal quotes the identifier AS THE
        // CALLER SENT IT keep the family-agnostic entity, because a tag request sends no
        // name. Three of them, so the fallback is not one special case.
        Row {
            case: "a hosted model",
            kind: "model",
            name: "absent-model",
            quoted: true,
            entity: "Resource",
            sentence: None,
        },
        Row {
            case: "an endpoint",
            kind: "endpoint",
            name: "absent-endpoint",
            quoted: true,
            entity: "Resource",
            sentence: None,
        },
        Row {
            case: "an AI workload configuration",
            kind: "ai-workload-config",
            name: "absent-workload-config",
            quoted: true,
            entity: "Resource",
            sentence: None,
        },
    ];

    let emulator = Emulator::new();
    // The signed Region is an axis too: the kind is read off an ARN of THIS account in
    // the Region the request signed for, so every row is driven in two Regions.
    for region in ["us-east-1", "eu-west-1"] {
        for row in &rows {
            let resource_arn = arn(region, row.kind, row.name);
            let expected = match row.sentence {
                Some(render) => render(&resource_arn),
                None if row.quoted => {
                    format!("{} '{resource_arn}' does not exist.", row.entity)
                }
                None => format!("{} {resource_arn} does not exist.", row.entity),
            };
            let case = format!("{} in {region}", row.case);
            assert_every_tag_operation_refuses(
                &emulator,
                &case,
                region,
                &resource_arn,
                &expected,
            )
            .await;
        }
    }

    // An ARN this account cannot hold a resource under names no family either: its kind
    // is not resolved, so the family-agnostic sentence is what remains. Rows differ along
    // WHICH fixed segment misses — the Region the request signed for, the account, the
    // partition.
    let foreign = [
        (
            "an algorithm ARN of another Region",
            arn("eu-west-1", "algorithm", "absent-algorithm"),
        ),
        (
            "an algorithm ARN of another account",
            "arn:aws:sagemaker:us-east-1:210987654321:algorithm/absent-algorithm".to_string(),
        ),
        (
            "an artifact ARN of another partition",
            format!("arn:aws-us-gov:sagemaker:us-east-1:{ACCOUNT}:artifact/0123456789abcdef"),
        ),
    ];
    for (case, resource_arn) in &foreign {
        let expected = format!("Resource '{resource_arn}' does not exist.");
        assert_every_tag_operation_refuses(&emulator, case, "us-east-1", resource_arn, &expected)
            .await;
    }
}

/// RULE: because the tag path words the family's own sentence, a resource that was
/// created and then deleted is refused by the tag operations in the SAME words its own
/// reader refuses it with — one fact, one text.
#[tokio::test]
async fn a_tag_refusal_matches_what_the_familys_own_reader_answers() {
    let emulator = Emulator::new();

    // A lineage artifact, created and then released: the tag operations and
    // `DescribeArtifact` must agree about the sentence afterwards.
    let created = emulator
        .ok(
            "CreateArtifact",
            &json!({
                "ArtifactName": "tagged-artifact",
                "ArtifactType": "DataSet",
                "Source": { "SourceUri": "s3://bucket/tagged-artifact.csv" },
            }),
        )
        .await;
    let artifact_arn = str_field(&created, "ArtifactArn");
    // While it exists the tag reader answers its (empty) tag set rather than refusing.
    emulator
        .ok("ListTags", &json!({ "ResourceArn": artifact_arn }))
        .await;
    emulator
        .ok("DeleteArtifact", &json!({ "ArtifactArn": artifact_arn }))
        .await;

    let (status, refused_read) = emulator
        .call("DescribeArtifact", &json!({ "ArtifactArn": artifact_arn }))
        .await;
    assert_eq!(
        status, 400,
        "case deleted artifact: expected DescribeArtifact to refuse, got {status} with \
         {refused_read}"
    );
    let family_sentence = message(&refused_read);
    assert_eq!(
        family_sentence,
        format!("Artifact '{artifact_arn}' does not exist."),
        "case deleted artifact: expected the artifact family's sentence, got \
         {family_sentence:?}"
    );
    assert_every_tag_operation_refuses(
        &emulator,
        "deleted artifact",
        "us-east-1",
        &artifact_arn,
        &family_sentence,
    )
    .await;

    // An algorithm nothing created: `DescribeAlgorithm` resolves either form of
    // `ArnOrName`, so the same ARN reaches the reader and the tag operations, and both
    // must word it the algorithm family's way — the ARN BARE.
    let algorithm_arn = arn("us-east-1", "algorithm", "never-created-algorithm");
    let (status, refused_read) = emulator
        .call(
            "DescribeAlgorithm",
            &json!({ "AlgorithmName": algorithm_arn }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case absent algorithm: expected DescribeAlgorithm to refuse, got {status} with \
         {refused_read}"
    );
    let family_sentence = message(&refused_read);
    assert_eq!(
        family_sentence,
        format!("Algorithm {algorithm_arn} does not exist."),
        "case absent algorithm: expected the algorithm family's sentence, got \
         {family_sentence:?}"
    );
    assert_every_tag_operation_refuses(
        &emulator,
        "absent algorithm",
        "us-east-1",
        &algorithm_arn,
        &family_sentence,
    )
    .await;
}

/// RULE: the refusals SageMaker words `Cannot find <subject>: <arn>` name what is missing
/// — a resource kind, or the resource POLICY of a resource — and end on the ARN the
/// request resolves to BARE: no quoting around it and no closing period.
#[tokio::test]
async fn a_subject_that_cannot_be_found_is_stated_bare_and_unpunctuated() {
    // Rows differ along the SUBJECT (a model package group, a group's resource policy, a
    // code repository), the operation that states it, why it is missing (never created,
    // or created and its policy released), the signed Region and the name's shape.
    struct Row {
        case: &'static str,
        region: &'static str,
        name: &'static str,
        /// Create the group first, so the row's refusal is about the POLICY rather than
        /// the group.
        seed_group: bool,
        operation: &'static str,
        body: fn(&str) -> Value,
        subject: &'static str,
        kind: &'static str,
    }

    let put_body: fn(&str) -> Value = |name| {
        json!({
            "ModelPackageGroupName": name,
            "ResourcePolicy": r#"{"Version":"2012-10-17","Statement":[]}"#,
        })
    };
    let group_body: fn(&str) -> Value = |name| json!({ "ModelPackageGroupName": name });
    let repository_body: fn(&str) -> Value = |name| json!({ "CodeRepositoryName": name });

    let rows = [
        Row {
            case: "a put naming no stored group",
            region: "us-east-1",
            name: "absent-group",
            seed_group: false,
            operation: "PutModelPackageGroupPolicy",
            body: put_body,
            subject: "Model Package Group",
            kind: "model-package-group",
        },
        Row {
            case: "a put naming no stored group, another Region and a one-character name",
            region: "ap-southeast-2",
            name: "g",
            seed_group: false,
            operation: "PutModelPackageGroupPolicy",
            body: put_body,
            subject: "Model Package Group",
            kind: "model-package-group",
        },
        Row {
            case: "a read of a group that carries no policy",
            region: "us-east-1",
            name: "fresh-group",
            seed_group: true,
            operation: "GetModelPackageGroupPolicy",
            body: group_body,
            subject: "resource policy for",
            kind: "model-package-group",
        },
        Row {
            case: "a read naming no stored group",
            region: "eu-west-1",
            name: "Mixed-Case-0123",
            seed_group: false,
            operation: "GetModelPackageGroupPolicy",
            body: group_body,
            subject: "resource policy for",
            kind: "model-package-group",
        },
        Row {
            case: "a release of a policy nothing attached",
            region: "eu-west-1",
            name: "fresh-group",
            seed_group: true,
            operation: "DeleteModelPackageGroupPolicy",
            body: group_body,
            subject: "resource policy for",
            kind: "model-package-group",
        },
        Row {
            case: "a read naming no stored repository",
            region: "us-east-1",
            name: "absent-repository",
            seed_group: false,
            operation: "DescribeCodeRepository",
            body: repository_body,
            subject: "CodeRepository",
            kind: "code-repository",
        },
        Row {
            case: "a delete naming no stored repository",
            region: "ap-southeast-2",
            name: "absent-repository",
            seed_group: false,
            operation: "DeleteCodeRepository",
            body: repository_body,
            subject: "CodeRepository",
            kind: "code-repository",
        },
    ];

    for row in &rows {
        let emulator = Emulator::new();
        if row.seed_group {
            emulator
                .call_in(
                    row.region,
                    "CreateModelPackageGroup",
                    &json!({ "ModelPackageGroupName": row.name }),
                )
                .await;
        }

        let (status, refused) = emulator
            .call_in(row.region, row.operation, &(row.body)(row.name))
            .await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400, got {status} with {refused}",
            row.case
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {:?}: expected ValidationException, got {refused:?}",
            row.case
        );
        let expected = format!(
            "Cannot find {}: {}",
            row.subject,
            arn(row.region, row.kind, row.name)
        );
        let answered = message(&refused);
        assert_eq!(
            answered, expected,
            "case {:?}: expected message {expected:?}, got {answered:?}",
            row.case
        );
        // The two properties the sentence is judged on, asserted on their own so a
        // regression says which one moved.
        assert!(
            !answered.ends_with('.'),
            "case {:?}: the sentence must not close with a period, got {answered:?}",
            row.case
        );
        assert!(
            !answered.contains('"') && !answered.contains('\''),
            "case {:?}: the ARN must be bare, got {answered:?}",
            row.case
        );
    }
}

/// RULE: `ModelCardStatus`'s `Archived` member means "No more updates should be made to
/// the model card, but it can still be exported", so `UpdateModelCard` against a card
/// whose latest version is archived is refused with HTTP 400 `ValidationException` and
/// the fixed message `Model card is in archived state`, whichever aspect the update
/// carries — while a card in any other status still takes the same update.
#[tokio::test]
async fn an_archived_card_takes_no_further_update() {
    // Rows differ along what the update asks for: new content (which would append a
    // version) and each status it could be moved to (including `Archived` itself, and
    // the move back out of it a caller might attempt).
    let stored = content_with_description("the content an archived card keeps");
    let mut updates: Vec<(String, Value)> = vec![(
        "new content".to_string(),
        json!({ "Content": content_with_description("a revision after archiving") }),
    )];
    for status in MODEL_CARD_STATUSES {
        updates.push((
            format!("a move to {status}"),
            json!({ "ModelCardStatus": status }),
        ));
    }

    for (change, update) in &updates {
        // The second axis: a card archived AT CREATION and one archived by a later status
        // update are the same archived card.
        for archived_at_creation in [true, false] {
            let case = format!("{change}, archived_at_creation={archived_at_creation}");
            let emulator = Emulator::new();
            let name = "archived-card";
            let created_in = if archived_at_creation {
                "Archived"
            } else {
                "Draft"
            };
            emulator
                .ok("CreateModelCard", &create_body(name, created_in, &stored))
                .await;
            if !archived_at_creation {
                emulator
                    .ok(
                        "UpdateModelCard",
                        &json!({ "ModelCardName": name, "ModelCardStatus": "Archived" }),
                    )
                    .await;
            }

            let mut body = update.clone();
            body["ModelCardName"] = json!(name);
            let (status, refused) = emulator.call("UpdateModelCard", &body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {refused:?}"
            );
            assert_eq!(
                message(&refused),
                "Model card is in archived state",
                "case {case:?}: expected the archived-state refusal, got {:?}",
                message(&refused)
            );

            // Nothing moved: the card still holds one version, still archived, still
            // carrying the document it was created with — and it is still readable, which
            // is what "it can still be exported" rests on.
            let described = emulator.ok("DescribeModelCard", &describe_body(name, None)).await;
            assert_eq!(
                number_field(&described, "ModelCardVersion"),
                1.0,
                "case {case:?}: expected the refused update to append no version, got \
                 {described:?}"
            );
            assert_eq!(
                str_field(&described, "ModelCardStatus"),
                "Archived",
                "case {case:?}: expected the status to stay Archived, got {described:?}"
            );
            assert_eq!(
                str_field(&described, "Content"),
                stored,
                "case {case:?}: expected the stored content to survive, got {described:?}"
            );
        }

        // The control row: the same update against a card in each status that is NOT
        // archived is accepted, so the refusal above is the archived status and nothing
        // else.
        for created_in in MODEL_CARD_STATUSES {
            if created_in == "Archived" {
                continue;
            }
            let case = format!("{change} against a {created_in} card");
            let emulator = Emulator::new();
            let name = "open-card";
            emulator
                .ok("CreateModelCard", &create_body(name, created_in, &stored))
                .await;
            let mut body = update.clone();
            body["ModelCardName"] = json!(name);
            let (status, answered) = emulator.call("UpdateModelCard", &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update to be accepted, got {status} with \
                 {answered}"
            );
            assert_eq!(
                str_field(&answered, "ModelCardArn"),
                arn("us-east-1", "model-card", name),
                "case {case:?}: expected the card's stored ARN, got {answered:?}"
            );
        }
    }
}
