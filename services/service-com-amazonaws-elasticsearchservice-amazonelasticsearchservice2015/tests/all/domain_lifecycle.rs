// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Elasticsearch **domain** lifecycle: create,
//! describe, describe-config, update-config, list and delete, driven as real
//! restJson1 requests through the router.
//!
//! The rules under test, each asserted over a SET of inputs rather than one
//! recorded call:
//!
//! 1. Every option a create request supplies is PERSISTED, and every later read
//!    answers from that stored record (ids, ARN and endpoint included).
//! 2. `EngineType` follows the `ElasticsearchVersion` prefix, for each of the
//!    three forms the model's pattern admits plus an absent version.
//! 3. An update MERGES: an option the request carries replaces the stored one
//!    and is stamped with the domain's new config version; an option it omits
//!    keeps both its value and its version.
//! 4. `DryRun` persists nothing.
//! 5. A create naming a name that exists answers from the stored record and
//!    changes nothing; a name that does not exist is not found by any of the
//!    per-domain operations.
//! 6. A deleted domain leaves every read.
//! 7. A created domain is `Processing` for a non-zero span and then settles,
//!    without any read changing what the next read sees.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call_in, emulator_authority, router, Answer};

/// The region the test signs for; every ARN and endpoint must carry it.
const REGION: &str = "us-west-2";

/// The canonical emulator account id, which `DomainId` and the ARN embed.
const ACCOUNT: &str = "000000000000";

/// Longer than the emulator's provisioning span, so a change is certainly due.
const PAST_PROVISIONING: Duration = Duration::from_millis(900);

/// One call against [`REGION`].
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Answer {
    call_in(router, REGION, method, path, body).await
}

async fn create(router: &MultiServiceRouter, body: &Value) -> Answer {
    call(router, "POST", "/2015-01-01/es/domain", Some(body)).await
}

async fn create_in(router: &MultiServiceRouter, region: &str, body: &Value) -> Answer {
    call_in(router, region, "POST", "/2015-01-01/es/domain", Some(body)).await
}

async fn describe_in(router: &MultiServiceRouter, region: &str, name: &str) -> Answer {
    call_in(
        router,
        region,
        "GET",
        &format!("/2015-01-01/es/domain/{name}"),
        None,
    )
    .await
}

async fn list_in(router: &MultiServiceRouter, region: &str) -> Answer {
    call_in(router, region, "GET", "/2015-01-01/domain", None).await
}

async fn describe(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/2015-01-01/es/domain/{name}"),
        None,
    )
    .await
}

async fn describe_config(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/2015-01-01/es/domain/{name}/config"),
        None,
    )
    .await
}

async fn update_config(router: &MultiServiceRouter, name: &str, body: &Value) -> Answer {
    call(
        router,
        "POST",
        &format!("/2015-01-01/es/domain/{name}/config"),
        Some(body),
    )
    .await
}

async fn delete(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/2015-01-01/es/domain/{name}"),
        None,
    )
    .await
}

async fn list(router: &MultiServiceRouter, engine_type: Option<&str>) -> Answer {
    let path = match engine_type {
        Some(engine_type) => format!("/2015-01-01/domain?engineType={engine_type}"),
        None => "/2015-01-01/domain".to_string(),
    };
    call(router, "GET", &path, None).await
}

/// One create request and what the store must then answer.
struct CreateCase {
    label: &'static str,
    name: &'static str,
    request: Value,
    engine_type: &'static str,
    /// `AutomatedSnapshotStartHour` the domain must report — the request's, or
    /// the member's documented default of 0 when it carries none.
    snapshot_hour: i64,
}

/// Rows differ along what rule 1 generalises over: which options are present,
/// the version form, cluster size and shape, map and list sizes.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            label: "two-node Elasticsearch domain carrying every common option",
            name: "lifecycle-domain",
            request: json!({
                "DomainName": "lifecycle-domain",
                "ElasticsearchVersion": "7.10",
                "ElasticsearchClusterConfig": {
                    "InstanceType": "t2.small.elasticsearch",
                    "InstanceCount": 2
                },
                "EBSOptions": {"EBSEnabled": true, "VolumeType": "gp2", "VolumeSize": 20},
                "SnapshotOptions": {"AutomatedSnapshotStartHour": 3},
                "AccessPolicies": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
                "AdvancedOptions": {"rest.action.multi.allow_explicit_index": "true"},
                "TagList": [{"Key": "team", "Value": "search"}]
            }),
            engine_type: "Elasticsearch",
            snapshot_hour: 3,
        },
        CreateCase {
            label: "bare minimum: only the required domain name",
            name: "minimal-domain",
            request: json!({"DomainName": "minimal-domain"}),
            engine_type: "Elasticsearch",
            snapshot_hour: 0,
        },
        CreateCase {
            label: "a large cluster with dedicated masters, warm nodes and zone awareness",
            name: "wide-domain",
            request: json!({
                "DomainName": "wide-domain",
                "ElasticsearchVersion": "6.8",
                "ElasticsearchClusterConfig": {
                    "InstanceType": "r5.large.elasticsearch",
                    "InstanceCount": 10,
                    "DedicatedMasterEnabled": true,
                    "DedicatedMasterType": "c5.large.elasticsearch",
                    "DedicatedMasterCount": 3,
                    "ZoneAwarenessEnabled": true,
                    "ZoneAwarenessConfig": {"AvailabilityZoneCount": 3},
                    "WarmEnabled": true,
                    "WarmType": "ultrawarm1.medium.elasticsearch",
                    "WarmCount": 2
                },
                "EBSOptions": {"EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 512, "Iops": 3000},
                "NodeToNodeEncryptionOptions": {"Enabled": true},
                "EncryptionAtRestOptions": {"Enabled": true, "KmsKeyId": "alias/aws/es"},
                "DomainEndpointOptions": {"EnforceHTTPS": true, "TLSSecurityPolicy": "Policy-Min-TLS-1-2-2019-07"},
                "AdvancedOptions": {"indices.query.bool.max_clause_count": "1024", "override_main_response_version": "false"},
                "LogPublishingOptions": {
                    "INDEX_SLOW_LOGS": {"CloudWatchLogsLogGroupArn": "arn:aws:logs:us-west-2:000000000000:log-group:/es/slow", "Enabled": true}
                },
                "TagList": [{"Key": "a", "Value": "1"}, {"Key": "b", "Value": "2"}]
            }),
            engine_type: "Elasticsearch",
            snapshot_hour: 0,
        },
        CreateCase {
            label: "an OpenSearch-versioned domain (OpenSearch_X.Y form)",
            name: "opensearch-domain",
            request: json!({
                "DomainName": "opensearch-domain",
                "ElasticsearchVersion": "OpenSearch_2.17",
                "ElasticsearchClusterConfig": {"InstanceType": "m5.large.elasticsearch", "InstanceCount": 1},
                "SnapshotOptions": {"AutomatedSnapshotStartHour": 23},
                "CognitoOptions": {"Enabled": false}
            }),
            engine_type: "OpenSearch",
            snapshot_hour: 23,
        },
        CreateCase {
            label: "an OpenSearch-versioned domain (short OS_X.Y form)",
            name: "os-short-domain",
            request: json!({
                "DomainName": "os-short-domain",
                "ElasticsearchVersion": "OS_1.3",
                "VPCOptions": {"SubnetIds": ["subnet-1", "subnet-2"], "SecurityGroupIds": ["sg-1"]},
                "AutoTuneOptions": {"DesiredState": "ENABLED"}
            }),
            engine_type: "OpenSearch",
            snapshot_hour: 0,
        },
        CreateCase {
            label: "the oldest version form the pattern admits",
            name: "legacy-domain",
            request: json!({
                "DomainName": "legacy-domain",
                "ElasticsearchVersion": "1.5",
                "ElasticsearchClusterConfig": {"InstanceType": "m3.medium.elasticsearch", "InstanceCount": 1}
            }),
            engine_type: "Elasticsearch",
            snapshot_hour: 0,
        },
    ]
}

/// RULE 1: a create persists the record, and a separate describe answers from
/// it — same ids, same ARN, same endpoint, same options.
#[tokio::test]
async fn create_persists_the_domain_and_describe_answers_from_the_store() {
    for case in create_cases() {
        let router = router();
        let label = case.label;
        let created = create(&router, &case.request).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: create must succeed, got {} {}",
            created.status, created.body
        );

        let status = "/DomainStatus";
        assert_eq!(
            created.at(label, &format!("{status}/DomainName")),
            &json!(case.name),
            "case {label:?}: DomainName must be the name given"
        );
        assert_eq!(
            created.at(label, &format!("{status}/DomainId")),
            &json!(format!("{ACCOUNT}/{}", case.name)),
            "case {label:?}: DomainId must be <account>/<name>"
        );
        assert_eq!(
            created.at(label, &format!("{status}/ARN")),
            &json!(format!("arn:aws:es:{REGION}:{ACCOUNT}:domain/{}", case.name)),
            "case {label:?}: ARN must name this region, account and domain"
        );
        assert_eq!(
            created.at(label, &format!("{status}/Created")),
            &json!(true),
            "case {label:?}: a freshly created domain is Created"
        );
        assert_eq!(
            created.at(label, &format!("{status}/Deleted")),
            &json!(false),
            "case {label:?}: a freshly created domain is not Deleted"
        );
        assert_eq!(
            created.at(label, &format!("{status}/Processing")),
            &json!(true),
            "case {label:?}: a freshly created domain is still Processing"
        );
        let endpoint = created
            .at(label, &format!("{status}/Endpoint"))
            .as_str()
            .unwrap_or_default()
            .to_string();
        // The address published for the domain's data plane is one a client of
        // this emulator can DIAL: this emulator's own endpoint, with the
        // reserved path that names this plane and the domain. The host AWS
        // names that endpoint by resolves nowhere here, so publishing it would
        // announce an address nothing answers at.
        assert_eq!(
            endpoint,
            format!(
                "{}/_aws/es-2015-01-01/{}",
                emulator_authority(),
                case.name
            ),
            "case {label:?}: the published endpoint must be this emulator's own endpoint \
             with the reserved path naming this plane and the domain, got {endpoint:?}"
        );
        assert_eq!(
            created.at(label, &format!("{status}/SnapshotOptions/AutomatedSnapshotStartHour")),
            &json!(case.snapshot_hour),
            "case {label:?}: the stored snapshot hour must be the one given, or the documented default"
        );

        // The separate read returns the SAME stored record.
        let read = describe(&router, case.name).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: describe must succeed, got {} {}",
            read.status, read.body
        );
        for member in [
            "DomainId",
            "DomainName",
            "ARN",
            "Created",
            "Deleted",
            "Endpoint",
            "ElasticsearchVersion",
            "ElasticsearchClusterConfig",
            "EBSOptions",
            "SnapshotOptions",
            "AccessPolicies",
            "AdvancedOptions",
            "LogPublishingOptions",
            "VPCOptions",
            "CognitoOptions",
            "EncryptionAtRestOptions",
            "NodeToNodeEncryptionOptions",
            "DomainEndpointOptions",
        ] {
            let pointer = format!("{status}/{member}");
            assert_eq!(
                read.body.pointer(&pointer),
                created.body.pointer(&pointer),
                "case {label:?}: describe and create disagree on {member}: {:?} vs {:?}",
                read.body.pointer(&pointer),
                created.body.pointer(&pointer)
            );
        }

        // What the request carried is what the store holds.
        if let Some(wanted) = case.request.get("ElasticsearchClusterConfig") {
            assert_eq!(
                read.at(label, &format!("{status}/ElasticsearchClusterConfig")),
                wanted,
                "case {label:?}: the stored cluster config must be the one created with"
            );
        }
        for member in ["EBSOptions", "AccessPolicies", "AdvancedOptions", "CognitoOptions"] {
            if let Some(wanted) = case.request.get(member) {
                assert_eq!(
                    read.at(label, &format!("{status}/{member}")),
                    wanted,
                    "case {label:?}: the stored {member} must be the one created with"
                );
            }
        }
        if let Some(vpc) = case.request.get("VPCOptions") {
            assert_eq!(
                read.at(label, &format!("{status}/VPCOptions/SubnetIds")),
                vpc.get("SubnetIds").expect("case row has SubnetIds"),
                "case {label:?}: the stored VPC subnets must be the ones created with"
            );
        }

        // The same stored record is what the listing enumerates.
        let listed = list(&router, None).await;
        let entries = listed
            .at(label, "/DomainNames")
            .as_array()
            .expect("DomainNames is a list")
            .clone();
        assert_eq!(
            entries,
            vec![json!({"DomainName": case.name, "EngineType": case.engine_type})],
            "case {label:?}: the created domain must be listed with its engine type, got {entries:?}"
        );
    }
}

/// RULE 2: `EngineType` is derived from the version prefix — for each form the
/// `ElasticsearchVersionString` pattern admits, and for an absent version — and
/// the `engineType` filter selects on exactly that.
#[tokio::test]
async fn engine_type_follows_the_version_prefix_and_filters_the_listing() {
    let router = router();
    // Every row is a version form from the model's pattern
    // (^[0-9]{1}\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\.[0-9]{1,2}$|^OS_[0-9]{1,2}\.[0-9]{1,2}$),
    // plus the no-version case.
    let cases: Vec<(&str, Option<&str>, &str)> = vec![
        ("es-one-five", Some("1.5"), "Elasticsearch"),
        ("es-seven-ten", Some("7.10"), "Elasticsearch"),
        ("os-long-form", Some("OpenSearch_2.17"), "OpenSearch"),
        ("os-short-form", Some("OS_2.5"), "OpenSearch"),
        ("no-version-given", None, "Elasticsearch"),
    ];

    for (name, version, _) in &cases {
        let mut body = json!({"DomainName": name});
        if let Some(version) = version {
            body["ElasticsearchVersion"] = json!(version);
        }
        let created = create(&router, &body).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: create must succeed, got {} {}",
            created.status, created.body
        );
    }

    let listed = list(&router, None).await;
    assert_eq!(
        listed.status, 200,
        "unfiltered list must succeed, got {} {}",
        listed.status, listed.body
    );
    let entries = listed
        .at("unfiltered list", "/DomainNames")
        .as_array()
        .expect("DomainNames is a list")
        .clone();
    assert_eq!(
        entries.len(),
        cases.len(),
        "unfiltered list must hold every live domain, got {entries:?}"
    );
    for (name, _, engine_type) in &cases {
        assert!(
            entries.contains(&json!({"DomainName": name, "EngineType": engine_type})),
            "case {name:?}: list must report EngineType {engine_type:?}, got {entries:?}"
        );
    }

    for engine_type in ["Elasticsearch", "OpenSearch"] {
        let filtered = list(&router, Some(engine_type)).await;
        let got = filtered
            .at(engine_type, "/DomainNames")
            .as_array()
            .expect("DomainNames is a list")
            .clone();
        let expected: Vec<Value> = cases
            .iter()
            .filter(|(_, _, each)| *each == engine_type)
            .map(|(name, _, each)| json!({"DomainName": name, "EngineType": each}))
            .collect();
        assert_eq!(
            got, expected,
            "case {engine_type:?}: the engineType filter must list exactly that engine's domains, got {got:?}"
        );
    }
}

/// The create body every merge case starts from: several options set, so an
/// update can be checked to leave the untouched ones alone.
fn merge_baseline(name: &str) -> Value {
    json!({
        "DomainName": name,
        "ElasticsearchVersion": "7.10",
        "ElasticsearchClusterConfig": {"InstanceType": "t2.small.elasticsearch", "InstanceCount": 2},
        "EBSOptions": {"EBSEnabled": true, "VolumeType": "gp2", "VolumeSize": 20},
        "SnapshotOptions": {"AutomatedSnapshotStartHour": 3},
        "AccessPolicies": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
        "AdvancedOptions": {"rest.action.multi.allow_explicit_index": "true"},
        "CognitoOptions": {"Enabled": false}
    })
}

/// Which config members a merge case may touch, and where each one's value and
/// status live in `DomainConfig`.
const MERGE_MEMBERS: [&str; 6] = [
    "ElasticsearchClusterConfig",
    "EBSOptions",
    "SnapshotOptions",
    "AccessPolicies",
    "AdvancedOptions",
    "CognitoOptions",
];

/// RULE 3: an update merges. One row per option, so the rule is asserted for
/// each member kind — a structure, a map, a plain string — not only the one the
/// report cited.
#[tokio::test]
async fn update_merges_the_supplied_options_and_leaves_the_rest_as_stored() {
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "ElasticsearchClusterConfig",
            "cluster-case",
            json!({"InstanceType": "t2.medium.elasticsearch", "InstanceCount": 3}),
        ),
        (
            "EBSOptions",
            "ebs-case",
            json!({"EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 100}),
        ),
        (
            "SnapshotOptions",
            "snapshot-case",
            json!({"AutomatedSnapshotStartHour": 11}),
        ),
        (
            "AccessPolicies",
            "policy-case",
            json!("{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Deny\"}]}"),
        ),
        (
            "AdvancedOptions",
            "advanced-case",
            json!({"rest.action.multi.allow_explicit_index": "false", "indices.fielddata.cache.size": "20"}),
        ),
        (
            "CognitoOptions",
            "cognito-case",
            json!({"Enabled": true, "UserPoolId": "us-west-2_pool", "IdentityPoolId": "us-west-2:11111111-1111-1111-1111-111111111111", "RoleArn": "arn:aws:iam::000000000000:role/cognito"}),
        ),
    ];

    for (member, name, new_value) in cases {
        let label = format!("update {member}");
        let router = router();
        let created = create(&router, &merge_baseline(name)).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: create must succeed, got {} {}",
            created.status, created.body
        );
        let before = describe_config(&router, name).await;
        assert_eq!(
            before.status, 200,
            "case {label:?}: describe-config must succeed, got {} {}",
            before.status, before.body
        );

        let updated = update_config(&router, name, &json!({member: new_value.clone()})).await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: update must succeed, got {} {}",
            updated.status, updated.body
        );

        // The changed option carries the new value and the bumped version...
        let value_at = format!("/DomainConfig/{member}/Options");
        let version_at = format!("/DomainConfig/{member}/Status/UpdateVersion");
        assert_eq!(
            updated.at(&label, &value_at),
            &new_value,
            "case {label:?}: the update response must report the new {member}"
        );
        assert_eq!(
            updated.at(&label, &version_at),
            &json!(2),
            "case {label:?}: the changed option's UpdateVersion must be bumped, got {}",
            updated.at(&label, &version_at)
        );

        // ...while every other option keeps its value AND its version.
        for other in MERGE_MEMBERS.iter().filter(|other| **other != member) {
            let other_value = format!("/DomainConfig/{other}/Options");
            let other_version = format!("/DomainConfig/{other}/Status/UpdateVersion");
            assert_eq!(
                updated.body.pointer(&other_value),
                before.body.pointer(&other_value),
                "case {label:?}: {other} must be untouched, got {:?}",
                updated.body.pointer(&other_value)
            );
            assert_eq!(
                updated.body.pointer(&other_version),
                Some(&json!(1)),
                "case {label:?}: {other} must keep its original UpdateVersion, got {:?}",
                updated.body.pointer(&other_version)
            );
        }

        // The merge is persisted: a later read agrees with the update's answer.
        let after = describe_config(&router, name).await;
        assert_eq!(
            after.at(&label, &value_at),
            &new_value,
            "case {label:?}: the merged {member} must be persisted"
        );
        assert_eq!(
            after.at(&label, &version_at),
            &json!(2),
            "case {label:?}: the persisted UpdateVersion must be the bumped one"
        );
        // And the domain status reflects it too, from the same record.
        let status = describe(&router, name).await;
        if let Some(expected) = new_value.as_object() {
            let _ = expected;
            assert_eq!(
                status.body.pointer(&format!("/DomainStatus/{member}")),
                Some(&new_value),
                "case {label:?}: DescribeElasticsearchDomain must report the merged {member}, got {:?}",
                status.body.pointer(&format!("/DomainStatus/{member}"))
            );
        }
    }
}

/// RULE 4: a dry run answers with what the change would produce and persists
/// nothing.
#[tokio::test]
async fn a_dry_run_update_persists_nothing() {
    let cases: Vec<(&str, Value)> = vec![
        (
            "dry-cluster",
            json!({"ElasticsearchClusterConfig": {"InstanceType": "t2.medium.elasticsearch", "InstanceCount": 7}, "DryRun": true}),
        ),
        (
            "dry-snapshot",
            json!({"SnapshotOptions": {"AutomatedSnapshotStartHour": 19}, "DryRun": true}),
        ),
        (
            "dry-advanced",
            json!({"AdvancedOptions": {"rest.action.multi.allow_explicit_index": "false"}, "DryRun": true}),
        ),
    ];

    for (name, body) in cases {
        let label = format!("dry run {name}");
        let router = router();
        create(&router, &merge_baseline(name)).await;
        let before = describe_config(&router, name).await;

        let dry = update_config(&router, name, &body).await;
        assert_eq!(
            dry.status, 200,
            "case {label:?}: a dry run must succeed, got {} {}",
            dry.status, dry.body
        );

        let after = describe_config(&router, name).await;
        for member in MERGE_MEMBERS {
            let value_at = format!("/DomainConfig/{member}/Options");
            let version_at = format!("/DomainConfig/{member}/Status/UpdateVersion");
            assert_eq!(
                after.body.pointer(&value_at),
                before.body.pointer(&value_at),
                "case {label:?}: a dry run must not persist {member}, got {:?}",
                after.body.pointer(&value_at)
            );
            assert_eq!(
                after.body.pointer(&version_at),
                before.body.pointer(&version_at),
                "case {label:?}: a dry run must not bump {member}'s UpdateVersion, got {:?}",
                after.body.pointer(&version_at)
            );
        }
    }
}

/// RULE 5a: a create naming a domain that already exists is the SAME create
/// again — it answers 200 from the stored record and changes nothing.
#[tokio::test]
async fn creating_an_existing_domain_name_echoes_the_stored_domain_and_changes_nothing() {
    let router = router();
    let name = "taken-domain";
    let first = create(&router, &merge_baseline(name)).await;
    assert_eq!(
        first.status, 200,
        "the first create must succeed, got {} {}",
        first.status, first.body
    );

    for (label, body) in [
        ("identical request", merge_baseline(name)),
        (
            "different options, same name",
            json!({
                "DomainName": name,
                "ElasticsearchVersion": "OS_2.5",
                "ElasticsearchClusterConfig": {"InstanceType": "r5.large.elasticsearch", "InstanceCount": 9}
            }),
        ),
        (
            "only the required name",
            json!({"DomainName": name}),
        ),
    ] {
        let again = create(&router, &body).await;
        assert_eq!(
            again.status, 200,
            "case {label:?}: a repeated create must answer 200, got {} {}",
            again.status, again.body
        );
        assert_eq!(
            again.body.pointer("/DomainStatus/ElasticsearchClusterConfig"),
            first
                .body
                .pointer("/DomainStatus/ElasticsearchClusterConfig"),
            "case {label:?}: a repeated create must report the FIRST call's cluster config, got {:?}",
            again.body.pointer("/DomainStatus/ElasticsearchClusterConfig")
        );
        assert_eq!(
            again.body.pointer("/DomainStatus/ElasticsearchVersion"),
            Some(&json!("7.10")),
            "case {label:?}: a repeated create must report the stored version, got {:?}",
            again.body.pointer("/DomainStatus/ElasticsearchVersion")
        );
        assert_eq!(
            again.body.pointer("/DomainStatus/DomainId"),
            first.body.pointer("/DomainStatus/DomainId"),
            "case {label:?}: a repeated create must report the stored domain's id, got {:?}",
            again.body.pointer("/DomainStatus/DomainId")
        );
    }

    let read = describe(&router, name).await;
    assert_eq!(
        read.body.pointer("/DomainStatus/ElasticsearchClusterConfig"),
        first
            .body
            .pointer("/DomainStatus/ElasticsearchClusterConfig"),
        "a repeated create must leave the stored domain untouched, got {:?}",
        read.body.pointer("/DomainStatus/ElasticsearchClusterConfig")
    );
    assert_eq!(
        read.body.pointer("/DomainStatus/ElasticsearchVersion"),
        Some(&json!("7.10")),
        "a repeated create must leave the stored version untouched, got {:?}",
        read.body.pointer("/DomainStatus/ElasticsearchVersion")
    );
    let names = list(&router, None).await;
    assert_eq!(
        names.at("repeat create", "/DomainNames"),
        &json!([{"DomainName": name, "EngineType": "Elasticsearch"}]),
        "a repeated create must not add a second domain, got {:?}",
        names.body.pointer("/DomainNames")
    );
}

/// RULE 5b: every per-domain operation reports an unknown name as
/// `ResourceNotFoundException` (HTTP 404) — not a 501, and not an empty 200.
/// The status is the one this API answers a missing resource with, which is not
/// the `@httpError(409)` its model annotates on the shape; the whole rule, over
/// every refusal of that class, lives in
/// `tests/all/missing_resource_not_found_status.rs`.
#[tokio::test]
async fn an_unknown_domain_is_not_found_by_every_per_domain_operation() {
    let router = router();
    // One live domain, so "not found" is about the name and not an empty store.
    create(&router, &merge_baseline("present-domain")).await;

    for name in ["never-created", "absent-domain", "zzz-domain"] {
        let answers = vec![
            ("DescribeElasticsearchDomain", describe(&router, name).await),
            (
                "DescribeElasticsearchDomainConfig",
                describe_config(&router, name).await,
            ),
            (
                "UpdateElasticsearchDomainConfig",
                update_config(
                    &router,
                    name,
                    &json!({"SnapshotOptions": {"AutomatedSnapshotStartHour": 4}}),
                )
                .await,
            ),
            ("DeleteElasticsearchDomain", delete(&router, name).await),
        ];
        for (operation, answer) in answers {
            let label = format!("{operation} on {name}");
            assert_eq!(
                answer.status, 404,
                "case {label:?}: an unknown domain must answer 404, got {} {}",
                answer.status, answer.body
            );
            assert!(
                answer.error_type.contains("ResourceNotFoundException"),
                "case {label:?}: the error must be ResourceNotFoundException, got {:?} / {}",
                answer.error_type,
                answer.body
            );
        }
    }

    // The live domain is untouched by all of that.
    let read = describe(&router, "present-domain").await;
    assert_eq!(
        read.status, 200,
        "the live domain must still be readable, got {} {}",
        read.status, read.body
    );
}

/// RULE 6: a delete removes the domain from every read, and only that domain.
#[tokio::test]
async fn delete_removes_only_that_domain_from_every_read() {
    let router = router();
    let names = ["alpha-domain", "beta-domain", "gamma-domain"];
    for name in names {
        create(&router, &merge_baseline(name)).await;
    }

    for (index, name) in names.iter().enumerate() {
        let label = format!("delete {name}");
        let deleted = delete(&router, name).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: delete must succeed, got {} {}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.at(&label, "/DomainStatus/Deleted"),
            &json!(true),
            "case {label:?}: the deleted domain must report Deleted"
        );
        assert_eq!(
            deleted.at(&label, "/DomainStatus/DomainName"),
            &json!(name),
            "case {label:?}: the response must name the removed domain"
        );
        assert_eq!(
            deleted.at(&label, "/DomainStatus/Processing"),
            &json!(false),
            "case {label:?}: the removal completes within the call, so nothing is left processing"
        );
        assert_eq!(
            deleted.at(&label, "/DomainStatus/DomainProcessingStatus"),
            &json!("Deleting"),
            "case {label:?}: the final status of a removed domain is Deleting"
        );

        let gone = describe(&router, name).await;
        assert_eq!(
            gone.status, 404,
            "case {label:?}: describing a deleted domain must answer 404, got {} {}",
            gone.status, gone.body
        );
        let again = delete(&router, name).await;
        assert_eq!(
            again.status, 404,
            "case {label:?}: deleting it twice must answer 404, got {} {}",
            again.status, again.body
        );

        let listed = list(&router, None).await;
        let entries = listed
            .at(&label, "/DomainNames")
            .as_array()
            .expect("DomainNames is a list")
            .clone();
        assert!(
            !entries
                .iter()
                .any(|entry| entry.pointer("/DomainName") == Some(&json!(name))),
            "case {label:?}: a deleted domain must not be listed, got {entries:?}"
        );
        assert_eq!(
            entries.len(),
            names.len() - index - 1,
            "case {label:?}: every surviving domain must still be listed, got {entries:?}"
        );
    }
}

/// RULE 7: the create → active transition is owed to the CLOCK: non-zero, so a
/// caller polling at once still sees `Processing`, and settled afterwards — and
/// no read changes what the next read sees.
#[tokio::test]
async fn a_created_domain_is_processing_for_a_non_zero_span_then_settles() {
    let router = router();
    let name = "settling-domain";
    create(&router, &merge_baseline(name)).await;

    let first = describe(&router, name).await;
    let second = describe(&router, name).await;
    for (label, answer) in [("first read", &first), ("second read", &second)] {
        assert_eq!(
            answer.at(label, "/DomainStatus/Processing"),
            &json!(true),
            "case {label:?}: a domain polled immediately after create is still Processing"
        );
        assert_eq!(
            answer.at(label, "/DomainStatus/DomainProcessingStatus"),
            &json!("Creating"),
            "case {label:?}: that domain's processing status is Creating"
        );
    }
    let config = describe_config(&router, name).await;
    assert_eq!(
        config.at(
            "config while creating",
            "/DomainConfig/ElasticsearchClusterConfig/Status/State"
        ),
        &json!("Processing"),
        "while creating, each option's State is Processing"
    );

    // The transition is due by the clock alone; nothing polls or sleeps for it
    // inside the emulator.
    tokio::time::sleep(PAST_PROVISIONING).await;

    let settled = describe(&router, name).await;
    assert_eq!(
        settled.at("settled", "/DomainStatus/Processing"),
        &json!(false),
        "once the span has passed the domain is no longer Processing"
    );
    assert_eq!(
        settled.at("settled", "/DomainStatus/DomainProcessingStatus"),
        &json!("Active"),
        "once the span has passed the domain is Active"
    );
    let settled_config = describe_config(&router, name).await;
    assert_eq!(
        settled_config.at(
            "settled config",
            "/DomainConfig/ElasticsearchClusterConfig/Status/State"
        ),
        &json!("Active"),
        "once the span has passed each option's State is Active"
    );
    assert_eq!(
        settled_config.at(
            "settled config",
            "/DomainConfig/ElasticsearchClusterConfig/Status/UpdateVersion"
        ),
        &json!(1),
        "settling is not a config change, so no version was bumped"
    );
    assert_eq!(
        settled.body.pointer("/DomainStatus/DomainId"),
        first.body.pointer("/DomainStatus/DomainId"),
        "the domain's id is stable across reads, got {:?}",
        settled.body.pointer("/DomainStatus/DomainId")
    );
    assert_eq!(
        settled.body.pointer("/DomainStatus/Endpoint"),
        first.body.pointer("/DomainStatus/Endpoint"),
        "the domain's endpoint is stable across reads, got {:?}",
        settled.body.pointer("/DomainStatus/Endpoint")
    );
}

/// An update re-opens the provisioning span and reports `Modifying` until the
/// clock makes the change active.
#[tokio::test]
async fn an_update_re_opens_the_provisioning_span() {
    let router = router();
    let name = "modifying-domain";
    create(&router, &merge_baseline(name)).await;
    tokio::time::sleep(PAST_PROVISIONING).await;

    let updated = update_config(
        &router,
        name,
        &json!({"ElasticsearchClusterConfig": {"InstanceType": "t2.medium.elasticsearch", "InstanceCount": 4}}),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "the update must succeed, got {} {}",
        updated.status, updated.body
    );
    let during = describe(&router, name).await;
    assert_eq!(
        during.at("during update", "/DomainStatus/Processing"),
        &json!(true),
        "a domain is Processing while an update provisions"
    );
    assert_eq!(
        during.at("during update", "/DomainStatus/DomainProcessingStatus"),
        &json!("Modifying"),
        "a domain already created reports Modifying, not Creating"
    );

    tokio::time::sleep(PAST_PROVISIONING).await;
    let after = describe(&router, name).await;
    assert_eq!(
        after.at("after update", "/DomainStatus/Processing"),
        &json!(false),
        "the update settles by the clock"
    );
    assert_eq!(
        after.at("after update", "/DomainStatus/ElasticsearchClusterConfig/InstanceCount"),
        &json!(4),
        "the settled domain reports the updated instance count"
    );
}

/// `AccessPolicies` is a policy document carried as a string: documents of very
/// different sizes round-trip unchanged, so nothing about the value's length is
/// special-cased.
#[tokio::test]
async fn access_policies_round_trip_whatever_the_document_is() {
    let cases: Vec<(&str, &str)> = vec![
        ("empty-policy-domain", "{}"),
        (
            "short-policy-domain",
            "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
        ),
        (
            "long-policy-domain",
            "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\"Principal\":{\"AWS\":\"*\"},\"Action\":\"es:*\",\"Resource\":\"arn:aws:es:us-west-2:000000000000:domain/long-policy-domain/*\"}]}",
        ),
    ];
    for (name, policy) in cases {
        let router = router();
        let created = create(
            &router,
            &json!({"DomainName": name, "AccessPolicies": policy}),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: create must succeed, got {} {}",
            created.status, created.body
        );
        let read = describe(&router, name).await;
        assert_eq!(
            read.at(name, "/DomainStatus/AccessPolicies"),
            &json!(policy),
            "case {name:?}: the stored access policy must round-trip unchanged"
        );
        let config = describe_config(&router, name).await;
        assert_eq!(
            config.at(name, "/DomainConfig/AccessPolicies/Options"),
            &json!(policy),
            "case {name:?}: the config must report the same access policy"
        );
    }
}

/// A domain name is unique per account AND region, so the same name in another
/// region is another domain — each with its own ARN, endpoint and listing.
#[tokio::test]
async fn the_same_domain_name_in_another_region_is_another_domain() {
    let router = router();
    let name = "shared-name-domain";
    let regions = ["us-west-2", "eu-central-1", "ap-southeast-1"];

    for (index, region) in regions.iter().enumerate() {
        let created = create_in(
            &router,
            region,
            &json!({
                "DomainName": name,
                "ElasticsearchVersion": "7.10",
                "ElasticsearchClusterConfig": {"InstanceCount": index as i64 + 1}
            }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {region:?}: the same name in a new region must be creatable, got {} {}",
            created.status, created.body
        );
        assert_eq!(
            created.at(region, "/DomainStatus/ARN"),
            &json!(format!("arn:aws:es:{region}:{ACCOUNT}:domain/{name}")),
            "case {region:?}: the ARN must name the region the create was signed for"
        );
    }

    // Each region reads back its own record, not another region's.
    for (index, region) in regions.iter().enumerate() {
        let read = describe_in(&router, region, name).await;
        assert_eq!(
            read.at(region, "/DomainStatus/ElasticsearchClusterConfig/InstanceCount"),
            &json!(index as i64 + 1),
            "case {region:?}: each region must answer from its own stored record"
        );
        let listed = list_in(&router, region).await;
        assert_eq!(
            listed.at(region, "/DomainNames"),
            &json!([{"DomainName": name, "EngineType": "Elasticsearch"}]),
            "case {region:?}: a region lists only its own domains"
        );
    }

    // And deleting one leaves the others.
    let deleted = call_in(
        &router,
        regions[0],
        "DELETE",
        &format!("/2015-01-01/es/domain/{name}"),
        None,
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "deleting the first region's domain must succeed, got {} {}",
        deleted.status, deleted.body
    );
    assert_eq!(
        describe_in(&router, regions[0], name).await.status,
        404,
        "the deleted region's domain must be gone"
    );
    for region in &regions[1..] {
        assert_eq!(
            describe_in(&router, region, name).await.status,
            200,
            "case {region:?}: another region's domain must survive the delete"
        );
    }
}

/// The account's per-region domain quota is enforced with the operation's own
/// `LimitExceededException`, and one region's quota does not consume another's.
#[tokio::test]
async fn creating_past_the_per_region_domain_quota_is_refused() {
    // Mirrors `crate::state::MAX_DOMAINS_PER_REGION`, the service's documented
    // default quota of domains per account per Region.
    const QUOTA: usize = 100;
    let router = router();

    for index in 0..QUOTA {
        let name = format!("quota-domain-{index:03}");
        let created = create(&router, &json!({"DomainName": name})).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: a create inside the quota must succeed, got {} {}",
            created.status, created.body
        );
    }

    let refused = create(&router, &json!({"DomainName": "quota-domain-over"})).await;
    assert_eq!(
        refused.status, 409,
        "the create past the quota must be refused with 409, got {} {}",
        refused.status, refused.body
    );
    assert!(
        refused.error_type.contains("LimitExceededException"),
        "the error must be LimitExceededException, got {:?} / {}",
        refused.error_type,
        refused.body
    );
    assert_eq!(
        describe(&router, "quota-domain-over").await.status,
        404,
        "the refused domain must not have been stored"
    );

    // The quota is per region: another region starts empty.
    let elsewhere = create_in(
        &router,
        "eu-west-1",
        &json!({"DomainName": "quota-domain-over"}),
    )
    .await;
    assert_eq!(
        elsewhere.status, 200,
        "another region's quota must be its own, got {} {}",
        elsewhere.status, elsewhere.body
    );

    // Deleting one frees a slot.
    assert_eq!(
        delete(&router, "quota-domain-000").await.status,
        200,
        "deleting a domain must succeed"
    );
    let after_delete = create(&router, &json!({"DomainName": "quota-domain-over"})).await;
    assert_eq!(
        after_delete.status, 200,
        "a freed slot must be usable again, got {} {}",
        after_delete.status, after_delete.body
    );
}

/// `AutomatedSnapshotPauseOptions` carries a documented maximum suspension of 3
/// days: a window inside it is accepted, a window that cannot hold is refused
/// with `ValidationException` (400) by both create and update.
#[tokio::test]
async fn a_snapshot_suspension_window_is_held_to_its_documented_maximum() {
    const DAY: i64 = 24 * 60 * 60;
    let start: i64 = 1_800_000_000;
    // Rows span both sides of the published bound (3 days) and the incoherent
    // window, with the duration as the thing that varies.
    let cases: Vec<(&str, i64, u16)> = vec![
        ("one hour", 60 * 60, 200),
        ("one day", DAY, 200),
        ("exactly three days, the published maximum", 3 * DAY, 200),
        ("one second past the maximum", 3 * DAY + 1, 400),
        ("four days", 4 * DAY, 400),
        ("an end before its start", -DAY, 400),
    ];

    for (index, (label, span, expected)) in cases.iter().enumerate() {
        let name = format!("pause-domain-{index}");
        let options = json!({
            "Enabled": true,
            "StartTime": start,
            "EndTime": start + span
        });

        let router = router();
        let created = create(
            &router,
            &json!({"DomainName": name, "AutomatedSnapshotPauseOptions": options.clone()}),
        )
        .await;
        assert_eq!(
            created.status, *expected,
            "case {label:?}: create must answer {expected}, got {} {}",
            created.status, created.body
        );

        if *expected == 200 {
            let read = describe(&router, &name).await;
            assert_eq!(
                read.at(label, "/DomainStatus/AutomatedSnapshotPauseOptions/EndTime"),
                &json!(start + span),
                "case {label:?}: an accepted window must be stored as given"
            );
        } else {
            assert!(
                created.error_type.contains("ValidationException"),
                "case {label:?}: the refusal must be a ValidationException, got {:?} / {}",
                created.error_type,
                created.body
            );
            assert_eq!(
                describe(&router, &name).await.status,
                404,
                "case {label:?}: a refused create must store nothing"
            );
        }

        // The same rule holds on update, against an existing domain.
        let update_router = router;
        if *expected != 200 {
            create(&update_router, &json!({"DomainName": "pause-target"})).await;
            let updated = update_config(
                &update_router,
                "pause-target",
                &json!({"AutomatedSnapshotPauseOptions": options}),
            )
            .await;
            assert_eq!(
                updated.status, 400,
                "case {label:?}: update must refuse the same window, got {} {}",
                updated.status, updated.body
            );
            let unchanged = describe_config(&update_router, "pause-target").await;
            assert!(
                unchanged.missing("/DomainConfig/AutomatedSnapshotPauseOptions"),
                "case {label:?}: a refused update must store nothing, got {}",
                unchanged.body
            );
        }
    }
}

/// A value outside the modelled `EngineType` enum is refused as a
/// `ValidationException` (400) by the boundary validator — not answered with an
/// empty 200.
#[tokio::test]
async fn an_engine_type_outside_the_enum_is_refused() {
    let router = router();
    create(&router, &json!({"DomainName": "filtered-domain"})).await;

    for value in ["Bogus", "elasticsearch", "OPENSEARCH", ""] {
        let answer = call(
            &router,
            "GET",
            &format!("/2015-01-01/domain?engineType={value}"),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {value:?}: an off-enum engineType must be refused with 400, got {} {}",
            answer.status, answer.body
        );
    }

    // A modelled value still works.
    let ok = list(&router, Some("Elasticsearch")).await;
    assert_eq!(
        ok.status, 200,
        "a modelled engineType must be accepted, got {} {}",
        ok.status, ok.body
    );
}

/// The store has a hard ceiling across EVERY region, so a caller that varies
/// the region it signs for cannot grow it without limit.
#[tokio::test]
async fn the_store_has_a_hard_ceiling_across_every_region() {
    // Mirrors `crate::state::MAX_DOMAINS_TOTAL` and `MAX_DOMAINS_PER_REGION`.
    const TOTAL: usize = 1_000;
    const PER_REGION: usize = 100;
    let router = router();

    let mut accepted = 0usize;
    let mut refused = 0usize;
    // More region partitions than the ceiling allows domains in, so the ceiling
    // — not the per-region quota — is what stops the growth.
    for region_index in 0..(TOTAL / PER_REGION) + 4 {
        let region = format!("ap-test-{region_index}");
        for name_index in 0..PER_REGION {
            let name = format!("ceiling-domain-{name_index:03}");
            let answer = create_in(&router, &region, &json!({"DomainName": name})).await;
            match answer.status {
                200 => accepted += 1,
                409 => {
                    refused += 1;
                    assert!(
                        answer.error_type.contains("LimitExceededException"),
                        "case {region:?}/{name:?}: past the ceiling the error must be \
                         LimitExceededException, got {:?} / {}",
                        answer.error_type,
                        answer.body
                    );
                }
                other => panic!(
                    "case {region:?}/{name:?}: unexpected status {other}: {}",
                    answer.body
                ),
            }
        }
    }

    assert_eq!(
        accepted, TOTAL,
        "the store must accept exactly its ceiling of domains across all regions, got {accepted}"
    );
    assert!(
        refused > 0,
        "every create past the ceiling must be refused, got {refused} refusals"
    );
    // A region that never got a slot holds nothing, and the ceiling still holds
    // after a delete frees one.
    let listed = list_in(&router, "ap-test-11").await;
    assert_eq!(
        listed.at("ap-test-11", "/DomainNames"),
        &json!([]),
        "a region past the ceiling must hold no domains"
    );
}

/// `AccessPolicies` is held to the documented maximum policy size, on create
/// and on update.
#[tokio::test]
async fn an_access_policy_larger_than_the_documented_maximum_is_refused() {
    // Mirrors `crate::state::MAX_ACCESS_POLICY_BYTES` — AWS's documented
    // maximum domain access policy size of 100 KB.
    const MAX: usize = 100 * 1024;
    // The row's policy is a valid JSON document padded to the stated length, so
    // only the SIZE varies across rows.
    let policy_of = |len: usize| {
        let prefix = "{\"Version\":\"2012-10-17\",\"Statement\":[],\"Pad\":\"";
        let suffix = "\"}";
        let pad = len.saturating_sub(prefix.len() + suffix.len());
        format!("{prefix}{}{suffix}", "p".repeat(pad))
    };
    let cases: Vec<(&str, usize, u16)> = vec![
        ("one kilobyte", 1024, 200),
        ("exactly the documented maximum", MAX, 200),
        ("one byte past the maximum", MAX + 1, 400),
        ("twice the maximum", 2 * MAX, 400),
    ];

    for (index, (label, len, expected)) in cases.iter().enumerate() {
        let name = format!("policy-size-domain-{index}");
        let policy = policy_of(*len);
        assert_eq!(
            policy.len(),
            *len,
            "case {label:?}: the row's policy must be exactly {len} bytes, got {}",
            policy.len()
        );

        let router = router();
        let created = create(
            &router,
            &json!({"DomainName": name, "AccessPolicies": policy.clone()}),
        )
        .await;
        assert_eq!(
            created.status, *expected,
            "case {label:?}: create must answer {expected}, got {} {}",
            created.status,
            created.body.to_string().chars().take(200).collect::<String>()
        );

        if *expected == 200 {
            let read = describe(&router, &name).await;
            assert_eq!(
                read.at(label, "/DomainStatus/AccessPolicies"),
                &json!(policy),
                "case {label:?}: an accepted policy must be stored as given"
            );
        } else {
            assert!(
                created.error_type.contains("ValidationException"),
                "case {label:?}: the refusal must be a ValidationException, got {:?}",
                created.error_type
            );
            // The same bound holds on update, and refuses without storing.
            create(&router, &json!({"DomainName": "policy-size-target"})).await;
            let updated = update_config(
                &router,
                "policy-size-target",
                &json!({"AccessPolicies": policy}),
            )
            .await;
            assert_eq!(
                updated.status, 400,
                "case {label:?}: update must refuse the same policy, got {}",
                updated.status
            );
            let unchanged = describe_config(&router, "policy-size-target").await;
            assert!(
                unchanged.missing("/DomainConfig/AccessPolicies"),
                "case {label:?}: a refused update must store nothing"
            );
        }
    }
}

/// The master user password a create may carry is never stored and never
/// reported back.
#[tokio::test]
async fn an_advanced_security_master_password_is_never_reported() {
    let router = router();
    let name = "secured-domain";
    let secret = "SuperSecret-123!";
    let created = create(
        &router,
        &json!({
            "DomainName": name,
            "AdvancedSecurityOptions": {
                "Enabled": true,
                "InternalUserDatabaseEnabled": true,
                "MasterUserOptions": {"MasterUserName": "admin", "MasterUserPassword": secret}
            }
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "create must succeed, got {} {}",
        created.status, created.body
    );

    for (label, answer) in [
        ("create", &created),
        ("describe", &describe(&router, name).await),
        ("describe-config", &describe_config(&router, name).await),
    ] {
        let rendered = answer.body.to_string();
        assert!(
            !rendered.contains(secret),
            "case {label:?}: the master user password must never be reported, got {rendered}"
        );
    }
    let read = describe(&router, name).await;
    assert_eq!(
        read.at("describe", "/DomainStatus/AdvancedSecurityOptions/Enabled"),
        &json!(true),
        "the non-secret advanced security options are still reported"
    );
    assert!(
        read.missing("/DomainStatus/AdvancedSecurityOptions/MasterUserOptions"),
        "the output shape has no MasterUserOptions member, got {}",
        read.body
    );
}
