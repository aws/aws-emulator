// SPDX-License-Identifier: CC0-1.0

//! Multi-Region user pool replicas over the wire: `CreateUserPoolReplica`
//! stores a secondary replica on the pool it names, `ListUserPoolReplicas`
//! renders the STORED replicas, and `UpdateUserPoolReplica` writes the status
//! its request names. Driven through the router exactly as an SDK drives it
//! (`POST /` with `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: a user pool is a set of replicas keyed by Region — the
//! pool's own Region holds the PRIMARY one, and each `CreateUserPoolReplica`
//! adds a SECONDARY one that comes up `CREATING` and reaches `INACTIVE` only once
//! the clock makes that transition due; `UpdateUserPoolReplica` stores the
//! `Status` its request names on the named secondary replica and answers with
//! the value it stored, so overlapping calls on one replica are serialized and
//! every later read reports the status the last of them wrote, never a
//! transient one; and a Region the pool holds no replica in, or a plan that does
//! not include replication, is refused with a modelled error that persists
//! nothing. Each table row names the case it asserts.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use cognito_idp::state::{
    REPLICA_ACTIVATION_SPAN, REPLICA_ROLE_PRIMARY, REPLICA_ROLE_SECONDARY, REPLICA_STATUS_ACTIVE,
    REPLICA_STATUS_CREATING, REPLICA_STATUS_INACTIVE,
};
use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service, and
/// its Region is the one a created pool — the PRIMARY replica — lives in. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// The Region the requests above are signed for, so the Region every pool this
/// suite creates is primary in.
const PRIMARY_REGION: &str = "us-east-1";

/// A `KeyConfiguration.KmsKeyArn`-shaped ARN (`EncryptionKeyArnType`, len
/// 20..2048) standing for the multi-Region customer-managed key a replicated
/// pool is encrypted under.
const MULTI_REGION_KEY_ARN: &str = "arn:aws:kms:us-east-1:123456789012:key/multi-region-example";

/// Long enough that the clock has made a created replica's activation due.
const PAST_THE_ACTIVATION: Duration =
    REPLICA_ACTIVATION_SPAN.saturating_add(Duration::from_millis(250));

/// The two `UpdateReplicaStatusType` values the model admits
/// (`smithy_query(query="shape",shape="UpdateReplicaStatusType")`), which is
/// what an update may ask for.
const UPDATE_STATUSES: [&str; 2] = [REPLICA_STATUS_ACTIVE, REPLICA_STATUS_INACTIVE];

/// How many overlapping updates of one replica the race test drives.
const RACER_COUNTS: [usize; 3] = [2, 3, 5];

/// One emulator with just this service mounted. Shared with the sibling replica
/// suites, which drive the same wire surface.
pub(crate) fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router.set_request_authentication(false);
    router
}

/// Call `operation` with `body` and return its status and JSON response body.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "cognito-idp.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, payload)
}

/// Call `operation`, requiring success, and return its response body.
pub(crate) async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// The `__type` of an error body, which is the wire error code.
pub(crate) fn error_code(payload: &Value) -> &str {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
}

/// The `message` member of an error body.
pub(crate) fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// Create a pool on `tier` (one of the modelled `UserPoolTierType` values, or
/// none to take the service default) and return its minted `UserPoolId` and
/// ARN.
///
/// The pool is encrypted under a customer-managed key, which is what a pool has
/// to hold to be replicated at all: "you must use a multi-region key to enable
/// multi-region replication for a user pool"
/// (`smithy_query(query="doc",shape="KeyConfigurationType",member="KeyType")`).
pub(crate) async fn pool_in_tier(
    router: &MultiServiceRouter,
    name: &str,
    tier: Option<&str>,
) -> (String, String) {
    let mut body = Map::new();
    body.insert("PoolName".to_string(), json!(name));
    body.insert(
        "KeyConfiguration".to_string(),
        json!({ "KeyType": "CUSTOMER_MANAGED_KEY", "KmsKeyArn": MULTI_REGION_KEY_ARN }),
    );
    if let Some(tier) = tier {
        body.insert("UserPoolTier".to_string(), json!(tier));
    }
    let created = ok(router, "CreateUserPool", &Value::Object(body)).await;
    let id = created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    let arn = created["UserPool"]["Arn"]
        .as_str()
        .expect("a minted pool ARN")
        .to_string();
    (id, arn)
}

/// `pool_arn` with its Region field replaced by `region`: the ARN a replica of
/// that pool carries, since a replicated user pool keeps ONE id across the
/// Regions it is replicated to.
pub(crate) fn arn_in_region(pool_arn: &str, region: &str) -> String {
    let mut fields: Vec<&str> = pool_arn.split(':').collect();
    assert!(
        fields.len() > 3,
        "a pool ARN has a Region field, got {pool_arn:?}"
    );
    fields[3] = region;
    fields.join(":")
}

/// The replicas `ListUserPoolReplicas` reports for `user_pool_id`, in the order
/// the listing served them.
pub(crate) async fn replicas(router: &MultiServiceRouter, user_pool_id: &str) -> Vec<Value> {
    let listed = ok(
        router,
        "ListUserPoolReplicas",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await;
    assert!(
        listed.get("NextToken").is_none(),
        "a listing that served every replica carries no NextToken, got {listed}"
    );
    listed["UserPoolReplicas"]
        .as_array()
        .unwrap_or_else(|| panic!("ListUserPoolReplicas answers a list, got {listed}"))
        .clone()
}

/// The entry `ListUserPoolReplicas` must carry for the replica in `region`.
pub(crate) fn expected_entry(pool_arn: &str, region: &str, status: &str, role: &str) -> Value {
    json!({
        "RegionName": region,
        "Status": status,
        "Role": role,
        "UserPoolArn": arn_in_region(pool_arn, region),
    })
}

/// One row of the replica table. The rows differ along what the rule
/// generalises over: how many Regions one pool is replicated to, the length and
/// form of each Region name, whether the create submits `UserPoolTags`, and the
/// pool's feature plan.
struct ReplicaCase {
    case: &'static str,
    tier: Option<&'static str>,
    regions: &'static [&'static str],
    tags: Option<Value>,
}

/// The table. `RegionName` spans Regions of several AWS partitions — drawn from
/// the Region catalog (`cognito_idp::state::AWS_REGIONS`) and including entries
/// the issue did not cite — all inside its modelled length 5..32
/// (`smithy_query(query="shape",shape="RegionNameType")`); `UserPoolTier` spans
/// the two plans that include replication; `UserPoolTags` spans absent, empty
/// and populated; and the replica count spans one and several per pool.
fn replica_cases() -> Vec<ReplicaCase> {
    vec![
        ReplicaCase {
            case: "one replica on a pool that takes the default feature plan",
            tier: None,
            regions: &["us-west-2"],
            tags: None,
        },
        ReplicaCase {
            case: "one replica on the Essentials plan, with tags of its own",
            tier: Some("ESSENTIALS"),
            regions: &["eu-west-1"],
            tags: Some(json!({ "team": "identity", "env": "test" })),
        },
        ReplicaCase {
            case: "three replicas of one pool, created out of Region order",
            tier: Some("PLUS"),
            regions: &["us-west-2", "ap-southeast-2", "eu-central-1"],
            tags: Some(json!({})),
        },
        ReplicaCase {
            case: "a Region of the GovCloud partition",
            tier: Some("PLUS"),
            regions: &["us-gov-west-1"],
            tags: None,
        },
        ReplicaCase {
            case: "the longest Region name the catalog carries",
            tier: Some("ESSENTIALS"),
            regions: &["us-isof-south-1"],
            tags: None,
        },
    ]
}

/// A created replica is stored on the pool it names, comes up `CREATING`, and
/// reaches `INACTIVE` only once the clock makes that transition due — beside the
/// pool's own `PRIMARY` replica, which `ListUserPoolReplicas` reports as
/// `ACTIVE` from the moment the pool exists.
#[tokio::test]
async fn a_created_replica_is_listed_and_settles_from_creating_to_inactive() {
    for ReplicaCase {
        case,
        tier,
        regions,
        tags,
    } in replica_cases()
    {
        let router = router();
        let (user_pool_id, pool_arn) = pool_in_tier(&router, "replicated", tier).await;

        // The pool is its own PRIMARY replica in the Region it was created in,
        // before anything is replicated.
        assert_eq!(
            replicas(&router, &user_pool_id).await,
            vec![expected_entry(
                &pool_arn,
                PRIMARY_REGION,
                REPLICA_STATUS_ACTIVE,
                REPLICA_ROLE_PRIMARY
            )],
            "case {case:?}: a pool nothing replicated lists its primary replica alone"
        );

        for region in regions {
            assert!(
                (5..=32).contains(&region.len()),
                "case {case:?}: the row's Region name {region:?} must satisfy the modelled \
                 length 5..32, got {} characters",
                region.len()
            );
            let mut body = Map::new();
            body.insert("UserPoolId".to_string(), json!(user_pool_id));
            body.insert("RegionName".to_string(), json!(region));
            if let Some(tags) = &tags {
                body.insert("UserPoolTags".to_string(), tags.clone());
            }
            let created = ok(&router, "CreateUserPoolReplica", &Value::Object(body)).await;
            assert_eq!(
                created["UserPoolReplica"],
                expected_entry(
                    &pool_arn,
                    region,
                    REPLICA_STATUS_CREATING,
                    REPLICA_ROLE_SECONDARY
                ),
                "case {case:?}: the create answers the replica it stored in {region:?}, \
                 CREATING because its activation is not due yet"
            );
        }

        // Every Region, ascending: the primary beside each secondary. The span
        // is never zero, so a caller listing straight after the creates still
        // sees the transient status.
        let mut expected_regions: Vec<&str> = regions.to_vec();
        expected_regions.push(PRIMARY_REGION);
        expected_regions.sort_unstable();
        let creating: Vec<Value> = expected_regions
            .iter()
            .map(|region| {
                if *region == PRIMARY_REGION {
                    expected_entry(
                        &pool_arn,
                        region,
                        REPLICA_STATUS_ACTIVE,
                        REPLICA_ROLE_PRIMARY,
                    )
                } else {
                    expected_entry(
                        &pool_arn,
                        region,
                        REPLICA_STATUS_CREATING,
                        REPLICA_ROLE_SECONDARY,
                    )
                }
            })
            .collect();
        let listed = replicas(&router, &user_pool_id).await;
        assert_eq!(
            listed, creating,
            "case {case:?}: every created replica is listed, ascending by Region, and is \
             CREATING before its activation is due"
        );

        // The pool's own description reports the Regions it is replicated in,
        // exactly the ones the listing serves.
        let described = ok(&router, "ListUserPools", &json!({ "MaxResults": 20 })).await;
        let entry = described["UserPools"]
            .as_array()
            .and_then(|pools| {
                pools
                    .iter()
                    .find(|pool| pool["Id"] == json!(user_pool_id))
                    .cloned()
            })
            .unwrap_or_else(|| panic!("case {case:?}: the created pool must be listed"));
        assert_eq!(
            entry["ReplicaRegions"],
            json!(expected_regions),
            "case {case:?}: the pool's description reports the Regions its replicas are in, \
             got {entry}"
        );

        tokio::time::sleep(PAST_THE_ACTIVATION).await;

        // A created secondary replica settles into INACTIVE, not ACTIVE: "The
        // replica has been created, but is not accepting requests for
        // end-users" (`UserPoolReplicaType.Status`), which is the status the
        // recorded multi-Region traces read back from a replica nothing has
        // activated yet. The pool's own Region serves every operation, so its
        // PRIMARY replica is ACTIVE from the moment the pool exists.
        let settled_replicas: Vec<Value> = expected_regions
            .iter()
            .map(|region| {
                let (status, role) = if *region == PRIMARY_REGION {
                    (REPLICA_STATUS_ACTIVE, REPLICA_ROLE_PRIMARY)
                } else {
                    (REPLICA_STATUS_INACTIVE, REPLICA_ROLE_SECONDARY)
                };
                expected_entry(&pool_arn, region, status, role)
            })
            .collect();
        let settled = replicas(&router, &user_pool_id).await;
        assert_eq!(
            settled, settled_replicas,
            "case {case:?}: each created replica settles into INACTIVE once the clock owes it \
             the transition, beside the pool's ACTIVE primary"
        );

        // A read changes nothing a later caller sees.
        assert_eq!(
            replicas(&router, &user_pool_id).await,
            settled,
            "case {case:?}: repeated listings report the same settled replicas"
        );
    }
}

/// One row of the update table: the statuses to apply to one replica, in order.
struct UpdateCase {
    case: &'static str,
    /// Whether the clock has made the replica's activation due before the first
    /// update, so a row can drive an update of a replica that is still coming
    /// up as well as one of a settled replica.
    settled_first: bool,
    statuses: &'static [&'static str],
}

/// The table. The rows span each `UpdateReplicaStatusType` value as the first
/// and as the last write, a repeat of one value, an alternating sequence, and
/// both a replica that is still `CREATING` and one the clock has already
/// activated.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "deactivating a settled replica",
            settled_first: true,
            statuses: &[REPLICA_STATUS_INACTIVE],
        },
        UpdateCase {
            case: "activating a settled replica",
            settled_first: true,
            statuses: &[REPLICA_STATUS_ACTIVE],
        },
        UpdateCase {
            case: "deactivating a replica that is still creating",
            settled_first: false,
            statuses: &[REPLICA_STATUS_INACTIVE],
        },
        UpdateCase {
            case: "activating a replica that is still creating",
            settled_first: false,
            statuses: &[REPLICA_STATUS_ACTIVE],
        },
        UpdateCase {
            case: "the same status twice, which is idempotent",
            settled_first: true,
            statuses: &[REPLICA_STATUS_INACTIVE, REPLICA_STATUS_INACTIVE],
        },
        UpdateCase {
            case: "an alternating sequence, where the last write stands",
            settled_first: false,
            statuses: &[
                REPLICA_STATUS_INACTIVE,
                REPLICA_STATUS_ACTIVE,
                REPLICA_STATUS_INACTIVE,
            ],
        },
    ]
}

/// An update stores the `Status` it names on the replica it names, answers with
/// the value it stored, and every later read reports that value — including
/// after the activation the clock owed a replica that was still coming up.
#[tokio::test]
async fn an_update_stores_the_status_it_names_and_every_later_read_reports_it() {
    const REGION: &str = "us-west-2";

    for UpdateCase {
        case,
        settled_first,
        statuses,
    } in update_cases()
    {
        let router = router();
        let (user_pool_id, pool_arn) = pool_in_tier(&router, "updated", Some("PLUS")).await;
        ok(
            &router,
            "CreateUserPoolReplica",
            &json!({ "UserPoolId": user_pool_id, "RegionName": REGION }),
        )
        .await;
        if settled_first {
            tokio::time::sleep(PAST_THE_ACTIVATION).await;
        }

        for status in statuses {
            assert!(
                UPDATE_STATUSES.contains(status),
                "case {case:?}: {status:?} must be a modelled UpdateReplicaStatusType value"
            );
            let updated = ok(
                &router,
                "UpdateUserPoolReplica",
                &json!({
                    "UserPoolId": user_pool_id,
                    "RegionName": REGION,
                    "Status": status,
                }),
            )
            .await;
            assert_eq!(
                updated["UserPoolReplica"],
                expected_entry(&pool_arn, REGION, status, REPLICA_ROLE_SECONDARY),
                "case {case:?}: the update answers the replica it wrote, carrying the status \
                 {status:?} it was asked for"
            );
            assert_eq!(
                replicas(&router, &user_pool_id).await,
                vec![
                    expected_entry(
                        &pool_arn,
                        PRIMARY_REGION,
                        REPLICA_STATUS_ACTIVE,
                        REPLICA_ROLE_PRIMARY
                    ),
                    expected_entry(&pool_arn, REGION, status, REPLICA_ROLE_SECONDARY),
                ],
                "case {case:?}: the listing reports the stored status {status:?}"
            );
        }

        // The status a call set supersedes the activation the clock owed a
        // replica that was still creating, so waiting past it changes nothing.
        tokio::time::sleep(PAST_THE_ACTIVATION).await;
        let last = statuses
            .last()
            .expect("every row applies at least one update");
        let settled = replicas(&router, &user_pool_id).await;
        assert_eq!(
            settled,
            vec![
                expected_entry(
                    &pool_arn,
                    PRIMARY_REGION,
                    REPLICA_STATUS_ACTIVE,
                    REPLICA_ROLE_PRIMARY
                ),
                expected_entry(&pool_arn, REGION, last, REPLICA_ROLE_SECONDARY),
            ],
            "case {case:?}: the status the last update wrote is what every later read reports"
        );
        assert_eq!(
            replicas(&router, &user_pool_id).await,
            settled,
            "case {case:?}: repeated listings report the same status"
        );
    }
}

/// One emulator shared by every racer, so the overlapping calls reach one store.
fn shared_router() -> Arc<MultiServiceRouter> {
    Arc::new(router())
}

/// Overlapping updates of ONE replica are serialized: each answers with the
/// status it asked for, the stored status settles on exactly one of the
/// requested values, and repeated reads all report that one value.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overlapping_updates_of_one_replica_settle_on_one_requested_status() {
    const REGION: &str = "eu-central-1";

    for count in RACER_COUNTS {
        let case = format!("{count} overlapping updates of one replica");
        let router = shared_router();
        let (user_pool_id, pool_arn) = pool_in_tier(&router, "raced", Some("PLUS")).await;
        ok(
            &router,
            "CreateUserPoolReplica",
            &json!({ "UserPoolId": user_pool_id, "RegionName": REGION }),
        )
        .await;
        tokio::time::sleep(PAST_THE_ACTIVATION).await;

        // Racer `n` asks for the status at `n % 2`, so every row drives both
        // modelled values at once.
        let mut tasks = Vec::new();
        for racer in 0..count {
            let router = Arc::clone(&router);
            let user_pool_id = user_pool_id.clone();
            let status = UPDATE_STATUSES[racer % UPDATE_STATUSES.len()];
            tasks.push(tokio::spawn(async move {
                let (http_status, payload) = call(
                    &router,
                    "UpdateUserPoolReplica",
                    &json!({
                        "UserPoolId": user_pool_id,
                        "RegionName": REGION,
                        "Status": status,
                    }),
                )
                .await;
                (racer, status, http_status, payload)
            }));
        }

        let mut requested: Vec<&str> = Vec::new();
        for task in tasks {
            let (racer, status, http_status, payload) =
                task.await.expect("a racing task must not panic");
            assert_eq!(
                http_status, 200,
                "case {case}: racer {racer} must be served — this operation models no \
                 conflict error — got {http_status} with {payload}"
            );
            assert_eq!(
                payload["UserPoolReplica"],
                expected_entry(&pool_arn, REGION, status, REPLICA_ROLE_SECONDARY),
                "case {case}: racer {racer} must be answered the status {status:?} it asked \
                 for, got {payload}"
            );
            requested.push(status);
        }

        // The settled status is one of the values the racers asked for, never a
        // transient one, and it does not flap across repeated reads.
        let settled = replicas(&router, &user_pool_id).await;
        let status = settled
            .iter()
            .find(|entry| entry["RegionName"] == json!(REGION))
            .map(|entry| entry["Status"].clone())
            .unwrap_or_else(|| panic!("case {case}: the replica must still be listed"));
        assert!(
            requested.iter().any(|asked| json!(asked) == status),
            "case {case}: the settled status must be one the racers asked for ({requested:?}), \
             got {status}"
        );
        for read in 0..3 {
            assert_eq!(
                replicas(&router, &user_pool_id).await,
                settled,
                "case {case}: read {read} after the race must report the same settled replicas"
            );
        }
    }
}

/// One row of the refusal table: a replica call this pool does not admit, and
/// the modelled error it must answer.
struct RefusalCase {
    case: &'static str,
    operation: &'static str,
    /// The request body, built from the pool the row is run against.
    body: fn(&str) -> Value,
    status: u16,
    code: &'static str,
    /// The message the refusal must carry, where the service words it as fixed
    /// text or as a template this row can fill in; `None` asserts only that a
    /// refusal says something.
    message: Option<&'static str>,
}

/// A replica call a pool does not admit is refused with a modelled error and
/// leaves the stored replicas untouched — never an unmodelled status.
#[tokio::test]
async fn a_replica_call_a_pool_does_not_admit_is_refused_and_persists_nothing() {
    const REGION: &str = "us-west-2";
    /// A pool id that satisfies the modelled `UserPoolIdType` pattern and names
    /// no pool this suite created.
    const ABSENT_POOL: &str = "us-east-1_absentpool";
    /// A Region the pool below is not replicated to.
    const UNREPLICATED_REGION: &str = "ap-south-1";

    let refusals: Vec<RefusalCase> = vec![
        RefusalCase {
            case: "a create naming a pool that does not exist",
            operation: "CreateUserPoolReplica",
            body: |_| json!({ "UserPoolId": ABSENT_POOL, "RegionName": REGION }),
            status: 400,
            code: "ResourceNotFoundException",
            // This operation words its own pool lookup, joining `UserPool with
            // ID: ` to `does not exist.` with no separator.
            message: Some("UserPool with ID: us-east-1_absentpooldoes not exist."),
        },
        RefusalCase {
            case: "a create in the Region that already holds the primary replica",
            operation: "CreateUserPoolReplica",
            body: |pool| json!({ "UserPoolId": pool, "RegionName": PRIMARY_REGION }),
            status: 400,
            code: "InvalidParameterException",
            message: None,
        },
        RefusalCase {
            case: "a create in a Region the pool is already replicated to",
            operation: "CreateUserPoolReplica",
            body: |pool| json!({ "UserPoolId": pool, "RegionName": REGION }),
            status: 400,
            code: "InvalidParameterException",
            message: None,
        },
        RefusalCase {
            case: "a listing of a pool that does not exist",
            operation: "ListUserPoolReplicas",
            body: |_| json!({ "UserPoolId": ABSENT_POOL }),
            status: 400,
            code: "ResourceNotFoundException",
            message: Some("User pool us-east-1_absentpool does not exist."),
        },
        RefusalCase {
            case: "a listing resumed from a token this listing never issued",
            operation: "ListUserPoolReplicas",
            body: |pool| json!({ "UserPoolId": pool, "NextToken": "not-a-token" }),
            status: 400,
            code: "InvalidParameterException",
            message: None,
        },
        RefusalCase {
            case: "an update naming a pool that does not exist",
            operation: "UpdateUserPoolReplica",
            body: |_| json!({
                "UserPoolId": ABSENT_POOL,
                "RegionName": REGION,
                "Status": REPLICA_STATUS_INACTIVE,
            }),
            status: 400,
            code: "ResourceNotFoundException",
            message: Some("User pool us-east-1_absentpool does not exist."),
        },
        RefusalCase {
            case: "an update naming a Region the pool holds no replica in",
            operation: "UpdateUserPoolReplica",
            body: |pool| json!({
                "UserPoolId": pool,
                "RegionName": UNREPLICATED_REGION,
                "Status": REPLICA_STATUS_ACTIVE,
            }),
            status: 400,
            code: "ResourceNotFoundException",
            // The service words this refusal as fixed text, interpolating no
            // value.
            message: Some("Replica not found in the requested region"),
        },
        RefusalCase {
            case: "an update of the Region that holds the primary replica",
            operation: "UpdateUserPoolReplica",
            body: |pool| json!({
                "UserPoolId": pool,
                "RegionName": PRIMARY_REGION,
                "Status": REPLICA_STATUS_INACTIVE,
            }),
            status: 400,
            code: "ResourceNotFoundException",
            message: Some("Replica not found in the requested region"),
        },
    ];

    let router = router();
    let (user_pool_id, pool_arn) = pool_in_tier(&router, "refusals", Some("ESSENTIALS")).await;
    ok(
        &router,
        "CreateUserPoolReplica",
        &json!({ "UserPoolId": user_pool_id, "RegionName": REGION }),
    )
    .await;
    tokio::time::sleep(PAST_THE_ACTIVATION).await;
    let before = replicas(&router, &user_pool_id).await;

    for RefusalCase {
        case,
        operation,
        body,
        status,
        code,
        message,
    } in refusals
    {
        let (answered, payload) = call(&router, operation, &body(&user_pool_id)).await;
        assert_eq!(
            (answered, error_code(&payload)),
            (status, code),
            "case {case:?}: expected HTTP {status} {code}, got {answered} with {payload}"
        );
        match message {
            Some(expected) => assert_eq!(
                error_message(&payload),
                expected,
                "case {case:?}: the refusal must be worded as the service words it, got {payload}"
            ),
            None => assert!(
                error_message(&payload) != "<no message>" && !error_message(&payload).is_empty(),
                "case {case:?}: a refusal carries a message, got {payload}"
            ),
        }
        assert_eq!(
            replicas(&router, &user_pool_id).await,
            before,
            "case {case:?}: a refused call leaves the stored replicas untouched"
        );
    }

    // A Region name outside the modelled length 5..32 is refused at the
    // boundary, before the handler runs, and stores nothing either.
    for region in ["us-1", &"a".repeat(33)] {
        let (answered, payload) = call(
            &router,
            "CreateUserPoolReplica",
            &json!({ "UserPoolId": user_pool_id, "RegionName": region }),
        )
        .await;
        assert_eq!(
            answered, 400,
            "case {region:?}: a Region name outside the modelled length 5..32 is refused, \
             got {answered} with {payload}"
        );
        assert_eq!(
            replicas(&router, &user_pool_id).await,
            before,
            "case {region:?}: the refused create stored nothing"
        );
    }

    // The pool's own entry is untouched by every refusal above.
    assert_eq!(
        before,
        vec![
            expected_entry(
                &pool_arn,
                PRIMARY_REGION,
                REPLICA_STATUS_ACTIVE,
                REPLICA_ROLE_PRIMARY
            ),
            expected_entry(
                &pool_arn,
                REGION,
                // A created secondary replica settles into INACTIVE until an
                // update activates it.
                REPLICA_STATUS_INACTIVE,
                REPLICA_ROLE_SECONDARY
            ),
        ],
        "the pool under test holds its primary replica and the one secondary it created"
    );
}

/// Every replica of `user_pool_id`, collected by following `NextToken` to the
/// end of the listing.
async fn all_replicas(router: &MultiServiceRouter, user_pool_id: &str) -> Vec<Value> {
    let mut collected: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    for page in 0..16 {
        let mut body = Map::new();
        body.insert("UserPoolId".to_string(), json!(user_pool_id));
        if let Some(token) = &next_token {
            body.insert("NextToken".to_string(), json!(token));
        }
        let listed = ok(router, "ListUserPoolReplicas", &Value::Object(body)).await;
        let served = listed["UserPoolReplicas"]
            .as_array()
            .unwrap_or_else(|| panic!("ListUserPoolReplicas answers a list, got {listed}"))
            .clone();
        assert!(
            !served.is_empty(),
            "page {page}: a page a token pointed at serves at least one replica, got {listed}"
        );
        collected.extend(served);
        next_token = listed
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            return collected;
        }
    }
    panic!("the listing must terminate, got {} replicas", collected.len());
}

/// A pool holds at most one replica per AWS Region other than its own — the
/// Region catalog is what bounds the store — and a listing of more replicas than
/// one page serves pages through them all, each Region exactly once, ascending.
#[tokio::test]
async fn the_listing_pages_through_every_replica_and_the_region_catalog_bounds_the_store() {
    let router = router();
    let (user_pool_id, _) = pool_in_tier(&router, "bounded", Some("PLUS")).await;

    // Every Region a replica of this pool can live in: the shared catalog minus
    // the pool's own Region, which holds the PRIMARY replica.
    let regions: Vec<String> = cognito_idp::state::AWS_REGIONS
        .iter()
        .filter(|region| **region != PRIMARY_REGION)
        .map(|region| (*region).to_string())
        .collect();
    for region in &regions {
        ok(
            &router,
            "CreateUserPoolReplica",
            &json!({ "UserPoolId": user_pool_id, "RegionName": region }),
        )
        .await;
    }

    let mut expected: Vec<String> = regions.clone();
    expected.push(PRIMARY_REGION.to_string());
    expected.sort();
    let served: Vec<String> = all_replicas(&router, &user_pool_id)
        .await
        .iter()
        .map(|entry| {
            entry["RegionName"]
                .as_str()
                .unwrap_or_else(|| panic!("a listed replica names its Region, got {entry}"))
                .to_string()
        })
        .collect();
    assert_eq!(
        served, expected,
        "paging through the listing serves every Region exactly once, ascending"
    );

    // Nothing is left to replicate to, so the store cannot grow: every Region of
    // the catalog already holds a replica of this pool or holds its primary, and
    // a value outside the catalog names no Region at all.
    for (case, region) in [
        (
            "a Region this pool already holds a replica in",
            "us-west-2",
        ),
        (
            "the pool's own Region, which holds the primary replica",
            PRIMARY_REGION,
        ),
        ("a value that names no AWS Region", "not-a-real-region"),
    ] {
        let (status, payload) = call(
            &router,
            "CreateUserPoolReplica",
            &json!({ "UserPoolId": user_pool_id, "RegionName": region }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {case:?}: a further create is refused, got {status} with {payload}"
        );
        assert_eq!(
            all_replicas(&router, &user_pool_id).await.len(),
            expected.len(),
            "case {case:?}: the refused create stored no replica"
        );
    }
}

/// Replication is a feature-plan feature: a pool on the plan that does not
/// include it is refused, and the same request on each plan that does include
/// it is served.
#[tokio::test]
async fn replication_is_refused_on_the_feature_plan_that_does_not_include_it() {
    const REGION: &str = "us-west-2";

    let router = router();
    let (lite_pool, _) = pool_in_tier(&router, "lite", Some("LITE")).await;
    let (answered, payload) = call(
        &router,
        "CreateUserPoolReplica",
        &json!({ "UserPoolId": lite_pool, "RegionName": REGION }),
    )
    .await;
    assert_eq!(
        (answered, error_code(&payload)),
        (400, "FeatureUnavailableInTierException"),
        "a pool on the LITE plan can't be replicated, got {answered} with {payload}"
    );
    assert!(
        error_message(&payload).contains("LITE"),
        "the refusal names the plan the pool is configured on, got {payload}"
    );
    assert_eq!(
        replicas(&router, &lite_pool).await.len(),
        1,
        "the refused create stored no replica, so the pool lists its primary alone"
    );

    for tier in ["ESSENTIALS", "PLUS"] {
        let (pool, pool_arn) = pool_in_tier(&router, tier, Some(tier)).await;
        let created = ok(
            &router,
            "CreateUserPoolReplica",
            &json!({ "UserPoolId": pool, "RegionName": REGION }),
        )
        .await;
        assert_eq!(
            created["UserPoolReplica"],
            expected_entry(
                &pool_arn,
                REGION,
                REPLICA_STATUS_CREATING,
                REPLICA_ROLE_SECONDARY
            ),
            "case {tier:?}: a plan that includes replication serves the create, got {created}"
        );
    }
}
