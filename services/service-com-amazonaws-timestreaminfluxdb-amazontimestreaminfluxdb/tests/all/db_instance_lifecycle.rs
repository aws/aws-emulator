// SPDX-License-Identifier: CC0-1.0

//! The DB instance lifecycle, end to end over the wire.
//!
//! Every test here drives real `awsJson1_0` requests (`POST /` with
//! `X-Amz-Target: AmazonTimestreamInfluxDB.<Op>`) through the same
//! [`MultiServiceRouter`] the production router dispatches with, against one shared
//! engine — so what the tests assert is what an SDK client sees.
//!
//! The assertions are written as RULES over a table of inputs, not as a replay of one
//! recorded call: the row set varies the value, the size, the enum member, the list
//! length, the page size and the number of instances. Every hand-written catalog or
//! range names the model member it came from.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_timestreaminfluxdb_amazontimestreaminfluxdb as influxdb;
use smithy_server_runtime::MultiServiceRouter;

/// The region the tests sign with by default; the ARNs must be scoped to it.
pub(crate) const REGION: &str = "us-west-2";

/// The account segment every emulated ARN carries.
pub(crate) const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// Members that `CreateDbInstanceInput` and `CreateDbInstanceOutput` BOTH model, from
/// `smithy_query(query="operation", operation="CreateDbInstance")`. A value the request
/// supplies for one of these must come back from the stored record unchanged.
const ECHOED_CREATE_MEMBERS: &[&str] = &[
    "allocatedStorage",
    "dbBackupConfigurations",
    "dbInstanceType",
    "dbParameterGroupIdentifier",
    "dbStorageType",
    "deploymentType",
    "kmsKeyId",
    "logDeliveryConfiguration",
    "maintenanceSchedule",
    "name",
    "networkType",
    "port",
    "publiclyAccessible",
    "vpcSecurityGroupIds",
    "vpcSubnetIds",
];

/// Members of `DbInstanceSummary`, from `smithy_query(query="shape",
/// shape="DbInstanceSummary")`. A live instance's summary must agree with its record.
const SUMMARY_MEMBERS: &[&str] = &[
    "allocatedStorage",
    "arn",
    "dbInstanceType",
    "dbStorageType",
    "deploymentType",
    "endpoint",
    "id",
    "name",
    "networkType",
    "port",
    "status",
];

/// The `DbInstanceSummary` members every live instance reports: the three the model
/// marks `@required` plus the ones this emulator always stores a value for.
const ALWAYS_REPORTED_SUMMARY_MEMBERS: &[&str] = &[
    "allocatedStorage",
    "arn",
    "dbInstanceType",
    "endpoint",
    "id",
    "name",
    "port",
    "status",
];

/// A router mounting only this service over a fresh shared engine, handed back beside
/// the engine itself — what a test needs when it has to write the store the way the
/// world outside this API does (an AWS Marketplace subscription, say).
pub(crate) fn router_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These tests exercise handler behaviour, not SigV4: the `Authorization` header
    // below is there to carry the credential scope's region, not to be verified.
    router.set_request_authentication(false);
    router.register_service(influxdb::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// A router mounting only this service over a fresh shared engine.
pub(crate) fn router() -> MultiServiceRouter {
    router_and_engine().0
}

/// One `awsJson1_0` call, signed into `region`'s credential scope.
pub(crate) async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("AmazonTimestreamInfluxDB.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTKEY/20260101/{region}\
                 /timestream-influxdb/aws4_request, SignedHeaders=host, Signature=00"
            ),
        )
        .body(::bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    (status, parsed)
}

/// One `awsJson1_0` call in [`REGION`].
pub(crate) async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    call_in(router, REGION, operation, body).await
}

/// Unwrap a 200 answer, reporting the case and the actual answer on failure.
pub(crate) fn ok(case: &str, answer: (u16, Value)) -> Value {
    let (status, body) = answer;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {body}"
    );
    body
}

/// The `id` of a DB instance record in a response body.
fn id_of(case: &str, body: &Value) -> String {
    body["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string `id`, got {body}"))
        .to_string()
}

/// `^[a-zA-Z0-9]+$`, length 3..64 — the modelled `DbInstanceId` constraint.
fn is_modelled_db_instance_id(id: &str) -> bool {
    (3..=64).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric())
}

/// The minimum a `CreateDbInstance` request must carry, per the model's `@required`
/// members (`name`, `password`, `dbInstanceType`, `vpcSubnetIds`,
/// `vpcSecurityGroupIds`, `allocatedStorage`).
fn minimal_create(name: &str) -> Value {
    json!({
        "name": name,
        "password": "supersecret1",
        "dbInstanceType": "db.influx.medium",
        "allocatedStorage": 20,
        "vpcSubnetIds": ["subnet-aaa"],
        "vpcSecurityGroupIds": ["sg-aaa"],
    })
}

/// A case for the create/read round trip: a request, and what the record must report
/// for the members the request did NOT set.
struct CreateCase {
    label: &'static str,
    body: Value,
    /// `port` carries `@default: 8086`, so an omitted port must come back as 8086.
    expected_port: i64,
    /// A standby's Availability Zone is reported only for `WITH_MULTIAZ_STANDBY`.
    expects_secondary_zone: bool,
}

/// Rows spanning what `CreateDbInstance` generalises over: which optional members are
/// present, the enum member chosen, the list length, and the numeric bound taken.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            label: "only the required members, port omitted",
            body: minimal_create("minimal-instance"),
            expected_port: 8086,
            expects_secondary_zone: false,
        },
        CreateCase {
            label: "every optional member set",
            body: json!({
                "name": "fully-specified-1",
                "password": "supersecret1",
                "username": "myuser",
                "organization": "myorg",
                "bucket": "mybucket",
                "dbInstanceType": "db.influx.8xlarge",
                "allocatedStorage": 400,
                "vpcSubnetIds": ["subnet-aaa", "subnet-bbb"],
                "vpcSecurityGroupIds": ["sg-aaa", "sg-bbb"],
                "publiclyAccessible": true,
                "dbStorageType": "InfluxIOIncludedT3",
                "dbParameterGroupIdentifier": "pg1",
                "deploymentType": "SINGLE_AZ",
                "networkType": "DUAL",
                "port": 9999,
                "kmsKeyId": "alias/my-key",
                "logDeliveryConfiguration": {
                    "s3Configuration": { "bucketName": "log-bucket", "enabled": true }
                },
                "maintenanceSchedule": {
                    "timezone": "UTC",
                    "preferredMaintenanceWindow": "Sun:23:00-Sun:23:30"
                },
                "dbBackupConfigurations": [
                    { "type": "DAILY", "retentionDays": 7, "enabled": true }
                ],
                "tags": { "team": "data", "env": "test" },
            }),
            expected_port: 9999,
            expects_secondary_zone: false,
        },
        CreateCase {
            label: "multi-AZ standby deployment",
            body: json!({
                "name": "standby-pair",
                "password": "supersecret1",
                "dbInstanceType": "db.influx.large",
                "allocatedStorage": 100,
                "vpcSubnetIds": ["subnet-aaa", "subnet-bbb"],
                "vpcSecurityGroupIds": ["sg-aaa"],
                "deploymentType": "WITH_MULTIAZ_STANDBY",
            }),
            expected_port: 8086,
            expects_secondary_zone: true,
        },
        CreateCase {
            label: "the published lower bounds of port and allocatedStorage",
            body: json!({
                "name": "low-bounds",
                "password": "supersecret1",
                "dbInstanceType": "db.influx.medium",
                "allocatedStorage": 20,
                "port": 1024,
                "vpcSubnetIds": ["subnet-aaa"],
                "vpcSecurityGroupIds": ["sg-aaa"],
            }),
            expected_port: 1024,
            expects_secondary_zone: false,
        },
        CreateCase {
            label: "the published upper bounds of port, allocatedStorage and both lists",
            body: json!({
                "name": "high-bounds",
                "password": "supersecret1",
                "dbInstanceType": "db.influx.24xlarge",
                "allocatedStorage": 15360,
                "port": 65535,
                "vpcSubnetIds": [
                    "subnet-a1", "subnet-a2", "subnet-a3",
                    "subnet-a4", "subnet-a5", "subnet-a6"
                ],
                "vpcSecurityGroupIds": [
                    "sg-b1", "sg-b2", "sg-b3", "sg-b4", "sg-b5"
                ],
            }),
            expected_port: 65535,
            expects_secondary_zone: false,
        },
        CreateCase {
            label: "the shortest name the model allows",
            body: minimal_create("abc"),
            expected_port: 8086,
            expects_secondary_zone: false,
        },
        CreateCase {
            label: "the longest name the model allows",
            // DbInstanceName length 3..40.
            body: minimal_create("a23456789-123456789-123456789-123456789"),
            expected_port: 8086,
            expects_secondary_zone: false,
        },
    ]
}

#[tokio::test]
async fn a_created_db_instance_reads_back_from_state_member_for_member() {
    let router = router();
    for case in create_cases() {
        let created = ok(
            case.label,
            call(&router, "CreateDbInstance", case.body.clone()).await,
        );
        let id = id_of(case.label, &created);

        // Every input member the output shape also models is reported from the record.
        for member in ECHOED_CREATE_MEMBERS {
            if let Some(sent) = case.body.get(member) {
                assert_eq!(
                    created.get(member),
                    Some(sent),
                    "case {:?}: CreateDbInstance must report the stored {member}, got {:?}",
                    case.label,
                    created.get(member)
                );
            }
        }
        assert_eq!(
            created["port"].as_i64(),
            Some(case.expected_port),
            "case {:?}: expected port {}, got {:?}",
            case.label,
            case.expected_port,
            created["port"]
        );
        assert_eq!(
            created.get("secondaryAvailabilityZone").is_some(),
            case.expects_secondary_zone,
            "case {:?}: a standby zone is reported only for WITH_MULTIAZ_STANDBY, got {:?}",
            case.label,
            created.get("secondaryAvailabilityZone")
        );
        assert!(
            created.get("password").is_none() && created.get("username").is_none(),
            "case {:?}: no output shape models the admin credentials, got {created}",
            case.label
        );

        // GetDbInstance returns the same record. `status` is the one member the clock
        // may have advanced between the two calls.
        let fetched = ok(
            case.label,
            call(&router, "GetDbInstance", json!({ "identifier": id })).await,
        );
        let created_members: BTreeSet<&String> = created
            .as_object()
            .expect("an object response")
            .keys()
            .collect();
        let fetched_members: BTreeSet<&String> = fetched
            .as_object()
            .expect("an object response")
            .keys()
            .collect();
        assert_eq!(
            created_members, fetched_members,
            "case {:?}: GetDbInstance must report the same members as the create",
            case.label
        );
        for member in created_members {
            if member == "status" {
                continue;
            }
            assert_eq!(
                fetched.get(member),
                created.get(member),
                "case {:?}: GetDbInstance member {member} drifted from the created record",
                case.label
            );
        }
    }
}

#[tokio::test]
async fn the_minted_id_arn_and_endpoint_follow_the_modelled_formats() {
    // The region comes from the request's SigV4 credential scope, so the ARN follows it.
    for region in ["us-west-2", "eu-central-1", "ap-southeast-4"] {
        let router = router();
        let mut minted: BTreeSet<String> = BTreeSet::new();
        for case in create_cases() {
            let case_label = format!("{} in {region}", case.label);
            let created = ok(
                &case_label,
                call_in(&router, region, "CreateDbInstance", case.body.clone()).await,
            );
            let id = id_of(&case_label, &created);
            let name = case.body["name"].as_str().expect("a name");

            assert!(
                is_modelled_db_instance_id(&id),
                "case {case_label:?}: id {id:?} must match ^[a-zA-Z0-9]+$ with length 3..64"
            );
            assert!(
                minted.insert(id.clone()),
                "case {case_label:?}: id {id:?} was minted twice"
            );
            assert_eq!(
                created["arn"].as_str(),
                Some(
                    format!("arn:aws:timestream-influxdb:{region}:{ACCOUNT}:db-instance/{id}")
                        .as_str()
                ),
                "case {case_label:?}: unexpected arn {:?}",
                created["arn"]
            );
            let endpoint = created["endpoint"].as_str().unwrap_or_default();
            assert!(
                endpoint.starts_with(&format!("{name}-")) && endpoint.contains(region),
                "case {case_label:?}: endpoint {endpoint:?} must be prefixed by the name \
                 and scoped to {region}"
            );
            assert_eq!(
                created["availabilityZone"].as_str(),
                Some(format!("{region}a").as_str()),
                "case {case_label:?}: unexpected availabilityZone {:?}",
                created["availabilityZone"]
            );
            let secret = created["influxAuthParametersSecretArn"]
                .as_str()
                .unwrap_or_default();
            assert!(
                secret.starts_with(&format!("arn:aws:secretsmanager:{region}:{ACCOUNT}:secret:")),
                "case {case_label:?}: unexpected influxAuthParametersSecretArn {secret:?}"
            );
        }
    }
}

#[tokio::test]
async fn every_modelled_instance_and_storage_type_is_stored_and_reported() {
    // Catalogs from the model's own enums, as rendered into this crate's generated
    // `crate::ops::types` constants.
    use influxdb::ops::types::shared_types::{
        DB_INSTANCE_TYPE_VALUES, DB_STORAGE_TYPE_VALUES, DEPLOYMENT_TYPE_VALUES,
        NETWORK_TYPE_VALUES,
    };

    let router = router();
    for (index, instance_type) in DB_INSTANCE_TYPE_VALUES.iter().enumerate() {
        let storage_type = DB_STORAGE_TYPE_VALUES[index % DB_STORAGE_TYPE_VALUES.len()];
        let network_type = NETWORK_TYPE_VALUES[index % NETWORK_TYPE_VALUES.len()];
        let deployment_type = DEPLOYMENT_TYPE_VALUES[index % DEPLOYMENT_TYPE_VALUES.len()];
        let case = format!("{instance_type} / {storage_type} / {network_type} / {deployment_type}");
        let request = json!({
            "name": format!("catalog-{index}"),
            "password": "supersecret1",
            "dbInstanceType": instance_type,
            "dbStorageType": storage_type,
            "networkType": network_type,
            "deploymentType": deployment_type,
            "allocatedStorage": 20 + index as i64,
            "vpcSubnetIds": ["subnet-aaa", "subnet-bbb"],
            "vpcSecurityGroupIds": ["sg-aaa"],
        });
        let created = ok(&case, call(&router, "CreateDbInstance", request.clone()).await);
        let id = id_of(&case, &created);
        let fetched = ok(
            &case,
            call(&router, "GetDbInstance", json!({ "identifier": id })).await,
        );
        for member in [
            "dbInstanceType",
            "dbStorageType",
            "networkType",
            "deploymentType",
            "allocatedStorage",
        ] {
            assert_eq!(
                fetched.get(member),
                request.get(member),
                "case {case:?}: GetDbInstance must report the stored {member}, got {:?}",
                fetched.get(member)
            );
        }
    }
}

#[tokio::test]
async fn paging_yields_every_live_instance_exactly_once() {
    // (instances created, maxResults): `MaxResults` is modelled range 1..100.
    let cases: [(usize, Option<i64>); 6] = [
        (1, None),
        (2, Some(1)),
        (5, Some(2)),
        (3, Some(100)),
        (7, Some(3)),
        (4, Some(4)),
    ];
    for (count, max_results) in cases {
        let case = format!("{count} instances at maxResults {max_results:?}");
        let router = router();
        let mut expected: Vec<String> = Vec::new();
        for index in 0..count {
            let created = ok(
                &case,
                call(
                    &router,
                    "CreateDbInstance",
                    minimal_create(&format!("page-{index}")),
                )
                .await,
            );
            expected.push(id_of(&case, &created));
        }
        expected.sort();

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            pages += 1;
            assert!(
                pages <= count + 1,
                "case {case:?}: paging did not terminate after {pages} pages, saw {seen:?}"
            );
            let mut request = json!({});
            if let Some(limit) = max_results {
                request["maxResults"] = json!(limit);
            }
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let page = ok(&case, call(&router, "ListDbInstances", request).await);
            let items = page["items"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: expected an `items` list, got {page}"));
            if let Some(limit) = max_results {
                assert!(
                    items.len() as i64 <= limit,
                    "case {case:?}: a page of {} exceeds maxResults {limit}",
                    items.len()
                );
            }
            for item in items {
                for member in ALWAYS_REPORTED_SUMMARY_MEMBERS {
                    assert!(
                        item.get(member).is_some(),
                        "case {case:?}: the summary omits {member}: {item}"
                    );
                }
                seen.push(id_of(&case, item));
            }
            match page.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }

        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            seen.len(),
            sorted.len(),
            "case {case:?}: paging repeated an instance: {seen:?}"
        );
        assert_eq!(
            sorted, expected,
            "case {case:?}: paging must yield exactly the live instances"
        );
    }
}

#[tokio::test]
async fn every_list_summary_agrees_with_the_full_record() {
    let router = router();
    for case in create_cases() {
        ok(
            case.label,
            call(&router, "CreateDbInstance", case.body.clone()).await,
        );
    }
    let listed = ok(
        "summaries",
        call(&router, "ListDbInstances", json!({ "maxResults": 100 })).await,
    );
    let items = listed["items"].as_array().expect("an `items` list");
    assert_eq!(
        items.len(),
        create_cases().len(),
        "case \"summaries\": every created instance must be listed, got {listed}"
    );
    for item in items {
        let id = id_of("summaries", item);
        let record = ok(
            &id,
            call(&router, "GetDbInstance", json!({ "identifier": &id })).await,
        );
        for member in SUMMARY_MEMBERS {
            // `status` is the one member the clock may advance between the two calls.
            if *member == "status" {
                continue;
            }
            assert_eq!(
                item.get(member),
                record.get(member),
                "case {id:?}: the summary's {member} disagrees with the record"
            );
        }
    }
}

#[tokio::test]
async fn one_name_can_be_held_once_per_region() {
    // Names are unique per customer PER REGION, so the same name in two regions is two
    // instances — asserted through ONE router, i.e. one shared store.
    let router = router();
    let regions = ["us-west-2", "eu-central-1", "ap-southeast-4"];
    let mut ids = BTreeSet::new();
    for name in ["shared", "Shared-Name-1"] {
        for region in regions {
            let case = format!("{name:?} in {region}");
            let created = ok(
                &case,
                call_in(&router, region, "CreateDbInstance", minimal_create(name)).await,
            );
            assert!(
                ids.insert(id_of(&case, &created)),
                "case {case:?}: each region's instance must be its own resource"
            );
            assert_eq!(
                created["name"].as_str(),
                Some(name),
                "case {case:?}: got {created}"
            );

            // A SECOND create of that name in the SAME region still conflicts.
            let (status, body) =
                call_in(&router, region, "CreateDbInstance", minimal_create(name)).await;
            assert_eq!(
                status, 409,
                "case {case:?}: a duplicate name in the same region must conflict, \
                 got {status} with {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_duplicate_name_conflicts_and_the_name_is_free_again_after_delete() {
    // Names spanning the modelled `DbInstanceName` pattern.
    for name in ["dup", "Dup-Instance", "a-b-c-9", "abcdefghij"] {
        let router = router();
        let first = ok(name, call(&router, "CreateDbInstance", minimal_create(name)).await);
        let first_id = id_of(name, &first);

        let (status, body) = call(&router, "CreateDbInstance", minimal_create(name)).await;
        assert_eq!(
            status, 409,
            "case {name:?}: a duplicate name must conflict, got {status} with {body}"
        );
        assert!(
            body["__type"]
                .as_str()
                .unwrap_or_default()
                .ends_with("ConflictException"),
            "case {name:?}: expected a ConflictException, got {body}"
        );
        assert_eq!(
            body["resourceId"].as_str(),
            Some(first_id.as_str()),
            "case {name:?}: the conflict must name the existing instance, got {body}"
        );
        assert_eq!(
            body["resourceType"].as_str(),
            Some("DB_INSTANCE"),
            "case {name:?}: unexpected resourceType in {body}"
        );

        // Deleting the holder frees the name, and the replacement is a new resource.
        ok(
            name,
            call(
                &router,
                "DeleteDbInstance",
                json!({ "identifier": first_id }),
            )
            .await,
        );
        let second = ok(name, call(&router, "CreateDbInstance", minimal_create(name)).await);
        assert_ne!(
            id_of(name, &second),
            first_id,
            "case {name:?}: the replacement instance must be a newly minted id"
        );
    }
}

#[tokio::test]
async fn an_unknown_identifier_is_not_found_on_every_operation_that_resolves_one() {
    let router = router();
    // A live instance exists, so a 404 is about resolution and not about empty state.
    let live = ok(
        "setup",
        call(&router, "CreateDbInstance", minimal_create("live-one")).await,
    );
    let live_id = id_of("setup", &live);

    // Identifiers that satisfy the modelled `DbInstanceId` constraint but name nothing.
    for identifier in ["abc", "doesnotexist", "0000000000", &"z".repeat(64)] {
        for operation in ["GetDbInstance", "UpdateDbInstance", "DeleteDbInstance"] {
            let case = format!("{operation} on {identifier:?}");
            let (status, body) = call(
                &router,
                operation,
                json!({ "identifier": identifier }),
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status} with {body}"
            );
            assert!(
                body["__type"]
                    .as_str()
                    .unwrap_or_default()
                    .ends_with("ResourceNotFoundException"),
                "case {case:?}: expected a ResourceNotFoundException, got {body}"
            );
            assert_eq!(
                body["resourceId"].as_str(),
                Some(identifier),
                "case {case:?}: the error must name the identifier asked for, got {body}"
            );
        }
    }

    // The live instance is untouched by all of that.
    let still_there = ok(
        "the live instance survives",
        call(&router, "GetDbInstance", json!({ "identifier": &live_id })).await,
    );
    assert_eq!(
        id_of("the live instance survives", &still_there),
        live_id,
        "a 404 on another identifier must not disturb a live record"
    );
}

/// A row of the update table: the patch, and the transient status it must report.
struct UpdateCase {
    label: &'static str,
    patch: Value,
    expected_status: &'static str,
}

#[tokio::test]
async fn update_mutates_the_stored_record_and_reports_the_matching_transient_status() {
    let cases = [
        UpdateCase {
            label: "port only",
            patch: json!({ "port": 1234 }),
            expected_status: "UPDATING",
        },
        UpdateCase {
            label: "allocatedStorage only",
            patch: json!({ "allocatedStorage": 40 }),
            expected_status: "UPDATING",
        },
        UpdateCase {
            label: "a different dbInstanceType",
            patch: json!({ "dbInstanceType": "db.influx.large" }),
            expected_status: "UPDATING_INSTANCE_TYPE",
        },
        UpdateCase {
            label: "the dbInstanceType it already has",
            patch: json!({ "dbInstanceType": "db.influx.medium" }),
            expected_status: "UPDATING",
        },
        UpdateCase {
            label: "a different deploymentType",
            patch: json!({ "deploymentType": "WITH_MULTIAZ_STANDBY" }),
            expected_status: "UPDATING_DEPLOYMENT_TYPE",
        },
        UpdateCase {
            label: "both the deployment type and the instance type",
            patch: json!({
                "deploymentType": "WITH_MULTIAZ_STANDBY",
                "dbInstanceType": "db.influx.2xlarge",
            }),
            expected_status: "UPDATING_DEPLOYMENT_TYPE",
        },
        UpdateCase {
            label: "storage type and parameter group",
            patch: json!({
                "dbStorageType": "InfluxIOIncludedT2",
                "dbParameterGroupIdentifier": "pg2",
            }),
            expected_status: "UPDATING",
        },
        UpdateCase {
            label: "the maintenance schedule and log delivery",
            patch: json!({
                "maintenanceSchedule": {
                    "timezone": "UTC",
                    "preferredMaintenanceWindow": "Mon:01:00-Mon:01:30"
                },
                "logDeliveryConfiguration": {
                    "s3Configuration": { "bucketName": "other-bucket", "enabled": false }
                },
            }),
            expected_status: "UPDATING",
        },
        UpdateCase {
            label: "every mutable member at once",
            patch: json!({
                "port": 2048,
                "allocatedStorage": 15360,
                "dbInstanceType": "db.influx.16xlarge",
                "dbStorageType": "InfluxIOIncludedT3",
                "dbParameterGroupIdentifier": "pg3",
            }),
            expected_status: "UPDATING_INSTANCE_TYPE",
        },
    ];

    let router = router();
    for (index, case) in cases.iter().enumerate() {
        let base = json!({
            "name": format!("update-{index}"),
            "password": "supersecret1",
            "dbInstanceType": "db.influx.medium",
            "dbStorageType": "InfluxIOIncludedT1",
            "deploymentType": "SINGLE_AZ",
            "allocatedStorage": 20,
            "port": 8086,
            "vpcSubnetIds": ["subnet-aaa", "subnet-bbb"],
            "vpcSecurityGroupIds": ["sg-aaa"],
        });
        let created = ok(case.label, call(&router, "CreateDbInstance", base).await);
        let id = id_of(case.label, &created);

        let mut request = case.patch.clone();
        request["identifier"] = json!(&id);
        let updated = ok(case.label, call(&router, "UpdateDbInstance", request).await);
        assert_eq!(
            updated["status"].as_str(),
            Some(case.expected_status),
            "case {:?}: expected status {:?}, got {:?}",
            case.label,
            case.expected_status,
            updated["status"]
        );

        // The mutation is persisted: a later read reports the new values, and the
        // members the request never mentioned keep the values they were created with.
        let fetched = ok(
            case.label,
            call(&router, "GetDbInstance", json!({ "identifier": &id })).await,
        );
        for (member, value) in case.patch.as_object().expect("a patch object") {
            assert_eq!(
                fetched.get(member),
                Some(value),
                "case {:?}: GetDbInstance must report the updated {member}, got {:?}",
                case.label,
                fetched.get(member)
            );
        }
        for member in ["id", "name", "arn", "endpoint", "vpcSubnetIds", "vpcSecurityGroupIds"] {
            assert_eq!(
                fetched.get(member),
                created.get(member),
                "case {:?}: {member} is not mutable by UpdateDbInstance",
                case.label
            );
        }
        for member in case.patch.as_object().expect("a patch object").keys() {
            if member == "deploymentType" {
                continue;
            }
            assert_eq!(
                fetched.get("availabilityZone"),
                created.get("availabilityZone"),
                "case {:?}: updating {member} must not move the instance's zone",
                case.label
            );
        }
    }
}

#[tokio::test]
async fn delete_removes_the_instance_from_every_read_path() {
    let router = router();
    let mut ids = Vec::new();
    for index in 0..4 {
        let created = ok(
            "setup",
            call(
                &router,
                "CreateDbInstance",
                minimal_create(&format!("doomed-{index}")),
            )
            .await,
        );
        ids.push(id_of("setup", &created));
    }

    for (index, id) in ids.iter().enumerate() {
        let case = format!("deleting instance {index}");
        let deleted = ok(
            &case,
            call(&router, "DeleteDbInstance", json!({ "identifier": id })).await,
        );
        assert_eq!(
            deleted["status"].as_str(),
            Some("DELETING"),
            "case {case:?}: expected the final record to report DELETING, got {:?}",
            deleted["status"]
        );
        assert_eq!(
            deleted["id"].as_str(),
            Some(id.as_str()),
            "case {case:?}: the final record must be the one asked for, got {deleted}"
        );

        let (status, body) = call(&router, "GetDbInstance", json!({ "identifier": id })).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a deleted instance must not resolve, got {status} with {body}"
        );

        let listed = ok(&case, call(&router, "ListDbInstances", json!({})).await);
        let remaining: BTreeSet<String> = listed["items"]
            .as_array()
            .expect("an `items` list")
            .iter()
            .map(|item| id_of(&case, item))
            .collect();
        assert!(
            !remaining.contains(id),
            "case {case:?}: ListDbInstances still reports the deleted {id}"
        );
        assert_eq!(
            remaining,
            ids[index + 1..].iter().cloned().collect::<BTreeSet<String>>(),
            "case {case:?}: only the instances not yet deleted must remain"
        );
    }
}

#[tokio::test]
async fn a_new_instance_is_creating_until_the_clock_settles_it_to_available() {
    let router = router();
    let created = ok(
        "clock",
        call(&router, "CreateDbInstance", minimal_create("settling-one")).await,
    );
    let id = id_of("clock", &created);
    assert_eq!(
        created["status"].as_str(),
        Some("CREATING"),
        "case \"clock\": a freshly created instance is provisioning, got {:?}",
        created["status"]
    );

    // A caller reading straight back still observes the transient status.
    let immediate = ok(
        "clock: immediate read",
        call(&router, "GetDbInstance", json!({ "identifier": &id })).await,
    );
    assert_eq!(
        immediate["status"].as_str(),
        Some("CREATING"),
        "case \"clock: immediate read\": the transition span must not be zero, got {:?}",
        immediate["status"]
    );

    tokio::time::sleep(Duration::from_millis(900)).await;
    let settled = ok(
        "clock: after the span",
        call(&router, "GetDbInstance", json!({ "identifier": &id })).await,
    );
    assert_eq!(
        settled["status"].as_str(),
        Some("AVAILABLE"),
        "case \"clock: after the span\": expected AVAILABLE, got {:?}",
        settled["status"]
    );
    let listed = ok(
        "clock: list reports the settled status",
        call(&router, "ListDbInstances", json!({})).await,
    );
    assert_eq!(
        listed["items"][0]["status"].as_str(),
        Some("AVAILABLE"),
        "case \"clock: list reports the settled status\": got {}",
        listed["items"][0]
    );

    // An update starts another transient span that the clock settles the same way.
    let updated = ok(
        "clock: update",
        call(
            &router,
            "UpdateDbInstance",
            json!({ "identifier": &id, "port": 1024 }),
        )
        .await,
    );
    assert_eq!(
        updated["status"].as_str(),
        Some("UPDATING"),
        "case \"clock: update\": got {:?}",
        updated["status"]
    );
    tokio::time::sleep(Duration::from_millis(900)).await;
    let resettled = ok(
        "clock: update settles",
        call(&router, "GetDbInstance", json!({ "identifier": &id })).await,
    );
    assert_eq!(
        resettled["status"].as_str(),
        Some("AVAILABLE"),
        "case \"clock: update settles\": expected AVAILABLE, got {:?}",
        resettled["status"]
    );
}

#[tokio::test]
async fn a_value_outside_a_modelled_constraint_is_refused_and_stores_nothing() {
    let router = router();
    let mut long_name = String::from("a");
    long_name.push_str(&"b".repeat(40)); // DbInstanceName length 3..40 — one past the max.

    let cases: Vec<(&str, Value)> = vec![
        ("port one below the modelled minimum 1024", {
            let mut body = minimal_create("bad-port-low");
            body["port"] = json!(1023);
            body
        }),
        ("port one above the modelled maximum 65535", {
            let mut body = minimal_create("bad-port-high");
            body["port"] = json!(65536);
            body
        }),
        ("allocatedStorage one below the modelled minimum 20", {
            let mut body = minimal_create("bad-storage-low");
            body["allocatedStorage"] = json!(19);
            body
        }),
        ("allocatedStorage one above the modelled maximum 15360", {
            let mut body = minimal_create("bad-storage-high");
            body["allocatedStorage"] = json!(15361);
            body
        }),
        ("a name shorter than the modelled minimum 3", minimal_create("ab")),
        ("a name longer than the modelled maximum 40", minimal_create(&long_name)),
        ("one more vpcSubnetId than the modelled maximum 6", {
            let mut body = minimal_create("too-many-subnets");
            body["vpcSubnetIds"] = json!([
                "subnet-a1", "subnet-a2", "subnet-a3",
                "subnet-a4", "subnet-a5", "subnet-a6", "subnet-a7"
            ]);
            body
        }),
        ("one more vpcSecurityGroupId than the modelled maximum 5", {
            let mut body = minimal_create("too-many-groups");
            body["vpcSecurityGroupIds"] =
                json!(["sg-b1", "sg-b2", "sg-b3", "sg-b4", "sg-b5", "sg-b6"]);
            body
        }),
        ("an empty vpcSubnetIds, below the modelled minimum 1", {
            let mut body = minimal_create("no-subnets");
            body["vpcSubnetIds"] = json!([]);
            body
        }),
        ("a request missing the required dbInstanceType", {
            let mut body = minimal_create("no-instance-type");
            body.as_object_mut().expect("an object").remove("dbInstanceType");
            body
        }),
    ];

    for (case, body) in cases {
        let (status, answer) = call(&router, "CreateDbInstance", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answer}"
        );
    }

    let listed = ok(
        "nothing was stored",
        call(&router, "ListDbInstances", json!({})).await,
    );
    assert_eq!(
        listed["items"].as_array().map(Vec::len),
        Some(0),
        "case \"nothing was stored\": a refused create must persist nothing, got {listed}"
    );
}

#[tokio::test]
async fn the_store_is_bounded_and_delete_releases_capacity() {
    let router = router();
    let mut first_id = String::new();
    for index in 0..influxdb::state::DB_INSTANCE_QUOTA {
        let created = ok(
            "filling the store to its bound",
            call(
                &router,
                "CreateDbInstance",
                minimal_create(&format!("quota-{index}")),
            )
            .await,
        );
        if index == 0 {
            first_id = id_of("filling the store to its bound", &created);
        }
    }

    // One past the bound is refused with the modelled quota error, not stored.
    let (status, body) = call(
        &router,
        "CreateDbInstance",
        minimal_create("quota-overflow"),
    )
    .await;
    assert_eq!(
        status, 402,
        "case \"one past the bound\": expected the modelled quota refusal, \
         got {status} with {body}"
    );
    assert!(
        body["__type"]
            .as_str()
            .unwrap_or_default()
            .ends_with("ServiceQuotaExceededException"),
        "case \"one past the bound\": expected a ServiceQuotaExceededException, got {body}"
    );

    // Deleting a record releases capacity: the bound is a live count, not a high-water
    // mark.
    ok(
        "delete releases capacity",
        call(
            &router,
            "DeleteDbInstance",
            json!({ "identifier": &first_id }),
        )
        .await,
    );
    ok(
        "delete releases capacity",
        call(
            &router,
            "CreateDbInstance",
            minimal_create("quota-overflow"),
        )
        .await,
    );
}

#[tokio::test]
async fn the_operations_outside_this_slice_still_answer_501() {
    let router = router();
    for (operation, body) in [
        // `GetDbCluster` and `ListDbClusters` are implemented beside `CreateDbCluster`;
        // the cluster operations still scaffolded are these.
        ("ListDbInstancesForCluster", json!({ "dbClusterId": "abc" })),
        ("ListDbBackups", json!({})),
    ] {
        let (status, answer) = call(&router, operation, body).await;
        assert_eq!(
            status, 501,
            "case {operation:?}: an unimplemented operation must answer 501, \
             got {status} with {answer}"
        );
    }
}

#[test]
fn every_status_and_resource_type_this_service_reports_is_a_modelled_value() {
    use influxdb::ops::types::shared_types::{RESOURCE_TYPE_VALUES, STATUS_VALUES};
    use influxdb::state as st;

    for status in [
        st::STATUS_CREATING,
        st::STATUS_AVAILABLE,
        st::STATUS_DELETING,
        st::STATUS_UPDATING,
        st::STATUS_UPDATING_INSTANCE_TYPE,
        st::STATUS_UPDATING_DEPLOYMENT_TYPE,
        st::STATUS_REBOOTING,
    ] {
        assert!(
            STATUS_VALUES.contains(&status),
            "case {status:?}: not a member of the modelled Status enum {STATUS_VALUES:?}"
        );
    }
    assert!(
        RESOURCE_TYPE_VALUES.contains(&st::RESOURCE_TYPE_DB_INSTANCE),
        "case {:?}: not a member of the modelled ResourceType enum {RESOURCE_TYPE_VALUES:?}",
        st::RESOURCE_TYPE_DB_INSTANCE
    );
}

#[test]
fn the_clock_owed_transition_span_settles_before_a_clients_next_read() {
    let span = influxdb::state::TRANSITION_SPAN;
    // Non-zero: the transient status a lifecycle call announces is the record's real
    // status at that instant, so a caller that acts on the answer it just got still
    // observes it and the state errors keyed to it.
    assert!(
        !span.is_zero(),
        "case \"transition span\": a transient status must be observable, got {span:?}"
    );
    // Shorter than the round trip a client needs to issue its next read: the settle is
    // already due when a polling client's first probe arrives, so no workflow spends one
    // of the client's probe intervals per transition it waits through.
    assert!(
        span <= Duration::from_millis(50),
        "case \"transition span\": a transient status must be settled before a client's \
         follow-up read, got {span:?}"
    );
}
