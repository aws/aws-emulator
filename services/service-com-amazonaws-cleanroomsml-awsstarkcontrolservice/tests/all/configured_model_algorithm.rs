// SPDX-License-Identifier: CC0-1.0

//! The Clean Rooms ML configured model algorithm, end to end over HTTP.
//!
//! The rule under test: `CreateConfiguredModelAlgorithm` persists exactly what the request
//! carried under an ARN it mints in the region the request was signed for, and
//! `GetConfiguredModelAlgorithm` computes its whole answer from that one record — so an
//! optional member the create omitted is reported absent rather than invented, and an ARN
//! this regional endpoint holds no record for is refused as not found.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rows of each table differ along what the rule generalises over: which
//! optional members are present, how deep the container configuration goes, how many tags
//! and metric definitions there are, the name's length and characters, and the region the
//! request is signed for.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_cleanroomsml_awsstarkcontrolservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

// The HTTP harness every suite in this crate drives the router through, defined once in
// the sibling suite rather than copied here.
use crate::training_dataset_audience_model_lifecycle::{
    assert_member_round_trips, is_timestamp, label, matches_arn_pattern, signed_call,
    timestamp_seconds, unix_now, Answer,
};

/// The region the requests in this suite are signed for, unless a case signs for another.
const REGION: &str = "us-east-2";

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`),
/// spelled out here rather than imported so a wrong account in the handler fails.
const ACCOUNT: &str = "000000000000";

/// The resource segment a `ConfiguredModelAlgorithmArn` carries, from that shape's
/// pattern `^arn:aws[-a-z]*:cleanrooms-ml:[-a-z0-9]+:[0-9]{12}:configured-model-algorithm/[-a-zA-Z0-9_/.]+$`.
const RESOURCE: &str = "configured-model-algorithm";

/// An IAM role ARN matching `IamRoleArn`'s `^arn:aws[-a-z]*:iam::[0-9]{12}:role/.+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/CleanRoomsMLRole";

/// A KMS key ARN matching `KmsKeyArn`'s `^arn:aws[-a-z]*:kms:[-a-z0-9]+:[0-9]{12}:key/.+$`.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-2:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

async fn create_algorithm(router: &MultiServiceRouter, region: &str, request: Value) -> Answer {
    signed_call(
        router,
        region,
        "POST",
        "/configured-model-algorithms",
        Some(request),
    )
    .await
}

async fn get_algorithm(router: &MultiServiceRouter, region: &str, arn: &str) -> Answer {
    signed_call(
        router,
        region,
        "GET",
        &format!("/configured-model-algorithms/{}", label(arn)),
        None,
    )
    .await
}

/// A `ConfiguredModelAlgorithmArn` in `region` whose resource id is `id`: the name of a
/// resource this emulator never minted, used by the cases that read one.
fn unknown_arn(region: &str, account: &str, id: &str) -> String {
    format!("arn:aws:cleanrooms-ml:{region}:{account}:{RESOURCE}/{id}")
}

/// A create request carrying only the two required members.
fn minimal_algorithm(name: &str) -> Value {
    json!({ "name": name, "roleArn": ROLE_ARN })
}

/// `CreateConfiguredModelAlgorithm` persists what it was given and
/// `GetConfiguredModelAlgorithm` reports that record: for every input member the reported
/// value is the created one, the ARN names the resource type, the signed region and the
/// emulator account, and each create mints a name of its own.
#[tokio::test]
async fn a_created_configured_model_algorithm_is_reported_from_what_was_stored() {
    let longest_name = "n".repeat(63);
    let longest_description = "d".repeat(255);
    let cases: Vec<(&str, Value)> = vec![
        ("only the required members", minimal_algorithm("cma-minimal")),
        (
            "a training container carrying only the image it must carry",
            json!({
                "name": "cma-training-only",
                "roleArn": ROLE_ARN,
                "trainingContainerConfig": {
                    "imageUri": "000000000000.dkr.ecr.us-east-2.amazonaws.com/trainer:1",
                },
            }),
        ),
        (
            "an inference container and no training container",
            json!({
                "name": "cma-inference-only",
                "roleArn": ROLE_ARN,
                "inferenceContainerConfig": {
                    "imageUri": "000000000000.dkr.ecr.us-east-2.amazonaws.com/predictor:7",
                },
            }),
        ),
        (
            "both containers, every container option, a KMS key, a description and tags",
            json!({
                "name": "cma-full",
                "roleArn": ROLE_ARN,
                "trainingContainerConfig": {
                    "imageUri": "000000000000.dkr.ecr.us-east-2.amazonaws.com/trainer:2",
                    // `ContainerEntrypoint` and `ContainerArguments` are both bounded
                    // @length(min: 1, max: 100).
                    "entrypoint": ["python3", "/opt/ml/code/train.py"],
                    "arguments": ["--epochs", "10", "--batch-size", "32"],
                    // `MetricDefinitionList` is bounded @length(min: 0, max: 40).
                    "metricDefinitions": [
                        { "name": "train:loss", "regex": "loss: ([0-9\\.]+)" },
                        { "name": "validation:auc", "regex": "auc: ([0-9\\.]+)" },
                        { "name": "train:recall", "regex": "recall: ([0-9\\.]+)" },
                    ],
                },
                "inferenceContainerConfig": {
                    "imageUri": "000000000000.dkr.ecr.us-east-2.amazonaws.com/predictor:2",
                },
                "kmsKeyArn": KMS_KEY_ARN,
                "description": "a container pair for the demo collaboration",
                "tags": {"env": "test", "team": "ml", "cost-centre": "42"},
            }),
        ),
        (
            "the shortest name NameString allows and an empty tag map",
            json!({ "name": "a", "roleArn": ROLE_ARN, "tags": {} }),
        ),
        (
            "the longest name and description the model allows",
            json!({
                "name": longest_name,
                "roleArn": ROLE_ARN,
                "description": longest_description,
            }),
        ),
        (
            "a name carrying spaces and a role in another partition",
            json!({
                "name": "cma 2026 demo",
                // `IamRoleArn`'s pattern admits every `aws[-a-z]*` partition.
                "roleArn": "arn:aws-us-gov:iam::210987654321:role/CleanRoomsMLRole",
                "description": "",
            }),
        ),
    ];

    let router = emulator();
    let mut minted = BTreeSet::new();
    for (case, request) in cases {
        let before = unix_now();
        let created = create_algorithm(&router, REGION, request.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateConfiguredModelAlgorithm, got {} \
             with {:?}",
            created.status, created.body
        );
        let arn = created
            .at("configuredModelAlgorithmArn")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            matches_arn_pattern(&arn, RESOURCE, REGION),
            "case {case:?}: expected a {RESOURCE} ARN in {REGION} for account {ACCOUNT}, \
             got {arn:?}"
        );
        assert!(
            minted.insert(arn.clone()),
            "case {case:?}: every create must mint a new ARN, got the earlier {arn:?}"
        );

        let got = get_algorithm(&router, REGION, &arn).await;
        assert_eq!(
            got.status, 200,
            "case {case:?}: expected HTTP 200 from GetConfiguredModelAlgorithm, got {} \
             with {:?}",
            got.status, got.body
        );
        assert_eq!(
            got.at("configuredModelAlgorithmArn"),
            json!(arn),
            "case {case:?}: GetConfiguredModelAlgorithm must report the ARN it was asked \
             about, got {:?}",
            got.at("configuredModelAlgorithmArn")
        );

        // Every member the create carried lands on the same member of the Get answer.
        for (member, expected) in request.as_object().expect("each case is a JSON object") {
            assert_member_round_trips(case, member, expected, &got.at(member));
        }
        // A create that named no tags stored none, and that is what is reported.
        if request.get("tags").is_none() {
            assert_eq!(
                got.at("tags"),
                json!({}),
                "case {case:?}: an algorithm created with no tags carries none, got {:?}",
                got.at("tags")
            );
        }
        // An absent optional member is reported as absent, not as an invented value.
        for member in [
            "description",
            "kmsKeyArn",
            "trainingContainerConfig",
            "inferenceContainerConfig",
        ] {
            if request.get(member).is_none() {
                assert_eq!(
                    got.at(member),
                    Value::Null,
                    "case {case:?}: an unspecified {member} must not be invented, got {:?}",
                    got.at(member)
                );
            }
        }
        let after = unix_now();
        for member in ["createTime", "updateTime"] {
            let reported = got.at(member);
            assert!(
                is_timestamp(&reported),
                "case {case:?}: {member} must be a modelled timestamp, got {reported:?}"
            );
            let seconds = timestamp_seconds(&reported);
            assert!(
                (before..=after).contains(&seconds),
                "case {case:?}: {member} must be the instant the create ran, expected it in \
                 {before}..={after}, got {seconds}"
            );
        }

        // The read reported on the store without changing it: a second read of the same
        // resource answers exactly what the first one did.
        let again = get_algorithm(&router, REGION, &arn).await;
        assert_eq!(
            again.body, got.body,
            "case {case:?}: reading an algorithm must not change what a later caller sees, \
             got {:?} after {:?}",
            again.body, got.body
        );
    }
}

/// A minted ARN names the region the request was signed for, and a Clean Rooms ML endpoint
/// serves only the resources created under its own region: the same ARN read from any
/// other region is not found.
#[tokio::test]
async fn a_minted_arn_names_the_signed_region_and_only_that_endpoint_serves_it() {
    // Rows differ only in the signed region, which is what the rule generalises over.
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2", "us-gov-west-1"];
    let router = emulator();

    let mut created = Vec::new();
    for region in regions {
        let answer = create_algorithm(
            &router,
            region,
            // One name, used in every region: the name is not what scopes the resource.
            minimal_algorithm("one-name-per-region"),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {region:?}: expected HTTP 200 from CreateConfiguredModelAlgorithm, got {} \
             with {:?}",
            answer.status, answer.body
        );
        let arn = answer
            .at("configuredModelAlgorithmArn")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            matches_arn_pattern(&arn, RESOURCE, region),
            "case {region:?}: the minted ARN must name the signed region, got {arn:?}"
        );
        created.push((region, arn));
    }

    for (region, arn) in &created {
        let got = get_algorithm(&router, region, arn).await;
        assert_eq!(
            got.status, 200,
            "case {region:?}: its own endpoint must serve {arn:?}, got {} with {:?}",
            got.status, got.body
        );
        assert_eq!(
            got.at("name"),
            json!("one-name-per-region"),
            "case {region:?}: the stored name must be reported, got {:?}",
            got.at("name")
        );

        for (other, _) in &created {
            if other == region {
                continue;
            }
            let answer = get_algorithm(&router, other, arn).await;
            assert_eq!(
                answer.status, 404,
                "case {other:?}: reading {region:?}'s resource must answer HTTP 404, got {} \
                 with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {other:?}: expected ResourceNotFoundException for {region:?}'s \
                 resource, got {:?}",
                answer.body
            );
        }
    }
}

/// An ARN this endpoint holds no record for is the modelled `ResourceNotFoundException`
/// (404), and the refusal names the ARN that was asked about.
#[tokio::test]
async fn an_arn_naming_no_persisted_algorithm_is_not_found() {
    let router = emulator();
    // One real resource exists, so a miss is a miss on the store and not on an empty one.
    let created = create_algorithm(&router, REGION, minimal_algorithm("cma-present")).await;
    assert_eq!(
        created.status, 200,
        "the fixture create must answer HTTP 200, got {} with {:?}",
        created.status, created.body
    );
    let present = created
        .at("configuredModelAlgorithmArn")
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Rows differ along the shape of the resource id and the account the ARN names — every
    // one of them satisfies `ConfiguredModelAlgorithmArn`'s pattern and names nothing.
    let cases = [
        (
            "an id this emulator never minted",
            unknown_arn(REGION, ACCOUNT, "00000000-0000-0000-0000-000000000000"),
        ),
        (
            "a nested id of the dots and slashes the pattern admits",
            unknown_arn(REGION, ACCOUNT, "team.ml/algorithms/trainer.v2"),
        ),
        (
            "another account's algorithm",
            unknown_arn(REGION, "210987654321", "d1e2f3a4-5b6c-7d8e-9f01-23456789abcd"),
        ),
        (
            "the minted id under another account",
            unknown_arn(
                REGION,
                "210987654321",
                present.rsplit('/').next().unwrap_or_default(),
            ),
        ),
    ];

    for (case, arn) in &cases {
        let answer = get_algorithm(&router, REGION, arn).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404 for {arn:?}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException for {arn:?}, got {:?}",
            answer.body
        );
        let message = answer.at("message").as_str().unwrap_or_default().to_string();
        // The sentence belongs to the KIND of resource that was not found: the
        // configured-model-algorithm kind is reported `<kind>: <arn> not found.`, with the
        // ARN the request carried as its only variable part.
        let expected = format!("Configured Model Algorithm: {arn} not found.");
        assert_eq!(
            message, expected,
            "case {case:?}: expected the refusal to say {expected:?}, got {message:?}"
        );
    }

    // The misses changed nothing: the one persisted algorithm is still served.
    let still_there = get_algorithm(&router, REGION, &present).await;
    assert_eq!(
        still_there.status, 200,
        "the persisted algorithm must still be served after the misses, got {} with {:?}",
        still_there.status, still_there.body
    );
}
