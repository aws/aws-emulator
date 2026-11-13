// SPDX-License-Identifier: CC0-1.0

//! What a revision that names an S3 OBJECT makes a deployment do, driven over the
//! wire.
//!
//! RULE under test: a revision whose `s3Location` the object store can resolve is
//! the document in that object — the traffic shift its AppSpec declares is the one
//! the deployment performs — and a revision the store cannot resolve under the
//! identity the request gave it (the bucket, the key, and the `eTag` when one is
//! named) is accepted by `CreateDeployment` and then FAILS the deployment with the
//! `INVALID_REVISION` error code, leaving the alias where it was.
//!
//! The suite mounts Amazon S3 and AWS Lambda beside CodeDeploy on one engine and
//! drives all three through the same front door a client uses: the bucket and the
//! object through S3's own `restXml` API, the function, its versions and its alias
//! through Lambda's `restJson1` API, and the deployment through CodeDeploy's
//! awsJson envelope. Nothing reaches into any store.

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{terminal_deployment, Emulator};

/// The service role every deployment group here is created with; CodeDeploy only
/// stores it.
const SERVICE_ROLE: &str = "arn:aws:iam::123456789012:role/CodeDeployRole";

/// The collection a function's aliases bind, which is also where a function is
/// created: `POST /2015-03-31/functions`.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The alias every row deploys to, and the version it invokes to begin with.
const ALIAS: &str = "live";
/// See [`ALIAS`].
const ORIGINAL_VERSION: &str = "1";

/// The fixed sentences CodeDeploy answers a revision it cannot locate in the
/// named bucket with. The message interpolates nothing: CodeDeploy answers the
/// same text whichever part of the object's identity did not resolve.
const UNLOCATABLE_MESSAGE: &str = "The AppSpec file cannot be located in the specified S3 bucket. \
     Verify your AppSpec file is present and that the name and key value pair specified for your \
     S3 bucket are correct. The S3 bucket must be in your current region";

/// Which entity tag a row's revision names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ETag {
    /// The revision names none, so the object's own tag is not part of what it
    /// addresses.
    Unnamed,
    /// The tag the write reported, which is the stored object's.
    Stored,
    /// That tag with one character changed, which names no object in the bucket.
    Altered,
}

/// One row of the resolution table.
struct Row {
    /// What the row is about.
    case: &'static str,
    /// Whether the bucket the revision names exists at all.
    bucket_held: bool,
    /// The key the object is written at, when the row writes one.
    stored_key: Option<&'static str>,
    /// The key the revision addresses.
    addressed_key: &'static str,
    /// See [`ETag`].
    e_tag: ETag,
    /// The `TargetVersion` the stored object's AppSpec names.
    object_targets: &'static str,
    /// The terminal status the deployment must reach.
    status: &'static str,
    /// The `errorInformation.code` it must carry, when it carries one.
    error_code: Option<&'static str>,
    /// The version the alias must invoke once the deployment is terminal.
    alias_at: &'static str,
}

/// Create a function and publish `versions` versions of it, so every version a
/// row's AppSpec may target exists.
///
/// Each publish carries its own `Description`: "Lambda doesn't publish a version
/// if the function's configuration and code haven't changed since the last
/// version", so a distinct description is what makes each publish its own
/// snapshot.
async fn given_function(emulator: &Emulator, case: &str, name: &str, versions: usize) {
    emulator
        .lambda_ok(
            case,
            "POST",
            FUNCTIONS_URI,
            json!({
                "FunctionName": name,
                "Role": format!("arn:aws:iam::{}:role/handler", engine::emulator_account_id()),
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": {"ZipFile": ""},
            }),
            201,
        )
        .await;
    for version in 1..=versions {
        emulator
            .lambda_ok(
                case,
                "POST",
                &format!("{FUNCTIONS_URI}/{name}/versions"),
                json!({"Description": format!("snapshot {version}")}),
                201,
            )
            .await;
    }
    emulator
        .lambda_ok(
            case,
            "POST",
            &format!("{FUNCTIONS_URI}/{name}/aliases"),
            json!({"Name": ALIAS, "FunctionVersion": ORIGINAL_VERSION}),
            201,
        )
        .await;
}

/// The alias as a client reads it.
async fn alias_of(emulator: &Emulator, case: &str, function: &str) -> Value {
    emulator
        .lambda_ok(
            case,
            "GET",
            &format!("{FUNCTIONS_URI}/{function}/aliases/{ALIAS}"),
            Value::Null,
            200,
        )
        .await
}

/// An Lambda application with one deployment group on it, answering the group
/// name.
async fn given_lambda_group(emulator: &Emulator, application: &str) -> String {
    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": application, "computePlatform": "Lambda"}),
        )
        .await;
    let group = format!("{application}-dg");
    emulator
        .ok(
            "CreateDeploymentGroup",
            json!({
                "applicationName": application,
                "deploymentGroupName": group,
                "deploymentConfigName": "CodeDeployDefault.LambdaAllAtOnce",
                "serviceRoleArn": SERVICE_ROLE,
            }),
        )
        .await;
    group
}

/// The AppSpec of an Lambda deployment, YAML-formatted as the AWS CodeDeploy User
/// Guide writes one: one `Resources` entry naming the function, the alias and the
/// two versions traffic moves between.
fn app_spec_yaml(function: &str, target: &str) -> String {
    format!(
        "version: 0.0\n\
         Resources:\n\
         \x20 - {function}:\n\
         \x20     Type: AWS::Lambda::Function\n\
         \x20     Properties:\n\
         \x20       Name: {function}\n\
         \x20       Alias: {ALIAS}\n\
         \x20       CurrentVersion: \"{ORIGINAL_VERSION}\"\n\
         \x20       TargetVersion: \"{target}\"\n"
    )
}

/// One character of `e_tag` changed, so the tag names no object the bucket holds
/// while staying a tag of the same shape.
fn altered(e_tag: &str) -> String {
    let flipped: String = e_tag
        .chars()
        .map(|character| match character {
            '0' => '1',
            '1' => '0',
            'a' => 'b',
            other => other,
        })
        .collect();
    assert_ne!(
        flipped, e_tag,
        "the altered eTag has to differ from the stored one"
    );
    flipped
}

/// Create a deployment of `revision` and answer its minted id, asserting that the
/// call itself was ACCEPTED: nothing about an object's identity is a question
/// about the shape of the request.
async fn deploy(emulator: &Emulator, case: &str, application: &str, group: &str, revision: Value) -> String {
    let (status, body) = emulator
        .call(
            "CreateDeployment",
            json!({
                "applicationName": application,
                "deploymentGroupName": group,
                "revision": revision,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateDeployment must accept the revision and answer its id, got \
         {status} with {body:?}"
    );
    body["deploymentId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// RULE: a deployment whose revision names an S3 object does what the OBJECT says
/// — its AppSpec decides which version the alias ends up invoking — and a revision
/// the named bucket holds no such object for (a key with nothing at it, an `eTag`
/// that is not the stored object's) fails the deployment with `INVALID_REVISION`
/// and leaves the alias untouched.
#[tokio::test]
async fn an_s3_revisions_object_decides_what_the_deployment_does() {
    // Rows differ along what the rule generalises over: whether the object is
    // there, whether the revision makes the entity tag part of what it addresses
    // and whether that tag is the stored one, which version the stored document
    // targets, and whether the bucket is one this emulator holds at all.
    let rows: &[Row] = &[
        Row {
            case: "an object the revision addresses by bucket and key alone",
            bucket_held: true,
            stored_key: Some("appspec.yaml"),
            addressed_key: "appspec.yaml",
            e_tag: ETag::Unnamed,
            object_targets: "2",
            status: "Succeeded",
            error_code: None,
            alias_at: "2",
        },
        Row {
            case: "the same object addressed with its own eTag",
            bucket_held: true,
            stored_key: Some("appspec.yaml"),
            addressed_key: "appspec.yaml",
            e_tag: ETag::Stored,
            object_targets: "2",
            status: "Succeeded",
            error_code: None,
            alias_at: "2",
        },
        Row {
            // The object's CONTENTS decide the alias target, not the request.
            case: "an object whose AppSpec targets a further version",
            bucket_held: true,
            stored_key: Some("releases/3/appspec.yml"),
            addressed_key: "releases/3/appspec.yml",
            e_tag: ETag::Stored,
            object_targets: "3",
            status: "Succeeded",
            error_code: None,
            alias_at: "3",
        },
        Row {
            case: "an eTag altered by one character",
            bucket_held: true,
            stored_key: Some("appspec.yaml"),
            addressed_key: "appspec.yaml",
            e_tag: ETag::Altered,
            object_targets: "2",
            status: "Failed",
            error_code: Some("INVALID_REVISION"),
            alias_at: ORIGINAL_VERSION,
        },
        Row {
            case: "a key the bucket holds nothing at",
            bucket_held: true,
            stored_key: Some("appspec.yaml"),
            addressed_key: "somewhere/else.yaml",
            e_tag: ETag::Unnamed,
            object_targets: "2",
            status: "Failed",
            error_code: Some("INVALID_REVISION"),
            alias_at: ORIGINAL_VERSION,
        },
        Row {
            // A bucket this emulator holds no record of is no evidence about the
            // object: it may belong to another account, so the deployment goes
            // ahead with nothing declared and moves no alias.
            case: "a bucket this emulator holds no record of",
            bucket_held: false,
            stored_key: None,
            addressed_key: "appspec.yaml",
            e_tag: ETag::Unnamed,
            object_targets: "2",
            status: "Succeeded",
            error_code: None,
            alias_at: ORIGINAL_VERSION,
        },
    ];

    for row in rows {
        let case = row.case;
        let emulator = Emulator::with_lambda_and_s3();
        let function = "orders-fn";
        let bucket = "revision-bucket";
        given_function(&emulator, case, function, 3).await;
        let group = given_lambda_group(&emulator, "s3-revision-app").await;

        let mut stored_e_tag = String::new();
        if row.bucket_held {
            emulator.s3_bucket(case, bucket).await;
            if let Some(key) = row.stored_key {
                stored_e_tag = emulator
                    .s3_put_object(
                        case,
                        bucket,
                        key,
                        app_spec_yaml(function, row.object_targets).as_bytes(),
                    )
                    .await;
            }
        }

        let mut location = json!({
            "bucket": bucket,
            "key": row.addressed_key,
            "bundleType": "YAML",
        });
        match row.e_tag {
            ETag::Unnamed => {}
            ETag::Stored => location["eTag"] = json!(stored_e_tag),
            ETag::Altered => location["eTag"] = json!(altered(&stored_e_tag)),
        }
        let deployment_id = deploy(
            &emulator,
            case,
            "s3-revision-app",
            &group,
            json!({"revisionType": "S3", "s3Location": location}),
        )
        .await;

        let info = terminal_deployment(&emulator, &deployment_id, case).await;
        assert_eq!(
            info["status"].as_str(),
            Some(row.status),
            "case {case:?}: expected the deployment to end {}, got {info:?}",
            row.status
        );
        assert_eq!(
            info["errorInformation"]["code"].as_str(),
            row.error_code,
            "case {case:?}: expected the error code {:?}, got {info:?}",
            row.error_code
        );
        if row.error_code.is_some() {
            assert_eq!(
                info["errorInformation"]["message"].as_str(),
                Some(UNLOCATABLE_MESSAGE),
                "case {case:?}: expected the unlocatable-revision sentences, got {info:?}"
            );
        }
        // The revision is still the caller's own: nothing fetched is written into
        // it.
        assert_eq!(
            info["revision"]["s3Location"]["key"].as_str(),
            Some(row.addressed_key),
            "case {case:?}: expected the stored revision to survive the fetch, got {info:?}"
        );

        let alias = alias_of(&emulator, case, function).await;
        assert_eq!(
            alias["FunctionVersion"].as_str(),
            Some(row.alias_at),
            "case {case:?}: expected the alias to invoke version {}, got {alias:?}",
            row.alias_at
        );
        assert!(
            alias["RoutingConfig"]["AdditionalVersionWeights"]
                .as_object()
                .is_none_or(serde_json::Map::is_empty),
            "case {case:?}: expected no residual weighted routing, got {alias:?}"
        );
    }
}

/// RULE: a Lambda deployment reports the lifecycle event that MOVES the traffic —
/// `AllowTraffic` — on its one target, with the outcome of the alias move this
/// service performed: the move's own result, whichever revision form described it.
#[tokio::test]
async fn a_lambda_targets_lifecycle_events_report_the_traffic_move() {
    // Rows differ along the revision form the shift was declared in and along what
    // the move came to: one that moved the alias, and one whose AppSpec named a
    // function AWS Lambda does not hold.
    let rows: &[(&str, bool, &str, &str)] = &[
        (
            "an S3 revision whose object moved the alias",
            true,
            "orders-fn",
            "Succeeded",
        ),
        (
            "an inline AppSpec naming a function AWS Lambda does not hold",
            false,
            "absent-fn",
            "Failed",
        ),
    ];

    for (case, from_s3, function, expected) in rows {
        let emulator = Emulator::with_lambda_and_s3();
        let bucket = "target-bucket";
        let key = "appspec.yaml";
        given_function(&emulator, case, "orders-fn", 2).await;
        let group = given_lambda_group(&emulator, "s3-target-app").await;
        let document = app_spec_yaml(function, "2");
        let revision = if *from_s3 {
            emulator.s3_bucket(case, bucket).await;
            emulator
                .s3_put_object(case, bucket, key, document.as_bytes())
                .await;
            json!({
                "revisionType": "S3",
                "s3Location": {"bucket": bucket, "key": key, "bundleType": "YAML"},
            })
        } else {
            json!({
                "revisionType": "AppSpecContent",
                "appSpecContent": {"content": document},
            })
        };
        let deployment_id = deploy(&emulator, case, "s3-target-app", &group, revision).await;
        terminal_deployment(&emulator, &deployment_id, case).await;

        // The deployment exposes EXACTLY ONE target, the function its AppSpec
        // names.
        let listed = emulator
            .ok(
                "ListDeploymentTargets",
                json!({"deploymentId": deployment_id}),
            )
            .await;
        let target_ids: Vec<String> = listed["targetIds"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|id| id.as_str().map(str::to_string))
            .collect();
        assert_eq!(
            target_ids,
            vec![(*function).to_string()],
            "case {case:?}: expected exactly the function as the deployment's target, got \
             {listed:?}"
        );

        let batch = emulator
            .ok(
                "BatchGetDeploymentTargets",
                json!({"deploymentId": deployment_id, "targetIds": target_ids}),
            )
            .await;
        let target = batch["deploymentTargets"][0]["lambdaTarget"].clone();
        let events: Vec<(String, String)> = target["lifecycleEvents"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|event| {
                (
                    event["lifecycleEventName"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    event["status"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        assert!(
            events.contains(&("AllowTraffic".to_string(), (*expected).to_string())),
            "case {case:?}: expected the AllowTraffic lifecycle event to report {expected}, \
             got {events:?}"
        );
        assert_eq!(
            target["lambdaFunctionInfo"]["functionAlias"].as_str(),
            Some(ALIAS),
            "case {case:?}: expected the target to name the alias the AppSpec moves, got \
             {target:?}"
        );
    }
}
