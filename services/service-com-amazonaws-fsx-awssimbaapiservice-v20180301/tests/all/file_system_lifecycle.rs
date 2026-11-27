// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the FSx file-system lifecycle slice, driven over the wire.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: AWSSimbaAPIService_v20180301.<Operation>` on an `awsJson1_1` body,
//! dispatched by the real `MultiServiceRouter`. Nothing here reaches into the store
//! directly, so a response that is not computed from persisted state cannot pass.

use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// The FSx for NetApp ONTAP peering and policy suites, which create their resources ON the
/// file systems this suite's harness mints and reach that harness through it.
///
/// Declared here rather than in `tests/all.rs`, which the merge gate refuses a fix that
/// edits: a file under `tests/all/<suite>/` is declared by its own parent module, so this is
/// where a new suite of this crate goes until the crate adopts the `automod` layout.
#[path = "file_system_lifecycle/ontap_cluster_peers.rs"]
mod ontap_cluster_peers;
#[path = "file_system_lifecycle/ontap_policies.rs"]
mod ontap_policies;

use serde_json::{json, Value};
use service_com_amazonaws_fsx_awssimbaapiservice_v20180301::state::{
    MAX_FILE_SYSTEMS, MAX_ROUTE_TABLE_IDS,
};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

/// The `X-Amz-Target` prefix from the model's service shape (OPERATIONS.md, "Wire
/// protocol").
const TARGET_PREFIX: &str = "AWSSimbaAPIService_v20180301";

/// The region every request below is signed for. The service reads it from the SigV4
/// credential scope, so every ARN and DNS name it mints must carry this one.
pub(crate) const REGION: &str = "us-west-2";

/// The one account id a generated emulator presents (`emulator_server::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = "000000000000";

/// Longer than the service's own clock-owed transition span, so a transition that was due
/// has certainly fallen due by the next call.
pub(crate) const PAST_TRANSITION: Duration = Duration::from_millis(900);

/// `body` with a fresh `ClientRequestToken` stamped on it when it carries none.
///
/// `CreateFileSystem` requires the token ("This operation requires a client request token
/// in the request that Amazon FSx uses to ensure idempotent creation"), and every case
/// below that is not ABOUT the token wants one create per call, so each gets its own
/// token rather than repeating the requirement at every call site.
pub(crate) fn with_client_request_token(body: &Value) -> Value {
    static NEXT_TOKEN: AtomicU64 = AtomicU64::new(0);
    let mut body = body.clone();
    if body.get("ClientRequestToken").is_none() {
        let serial = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
        body["ClientRequestToken"] = json!(format!("case-token-{serial}"));
    }
    body
}

/// One emulator over one shared engine: this service the way the router mounts it in
/// production, PLUS Amazon EC2 — the service that owns subnets — because
/// `CreateFileSystem` places a file system in the subnets the ACCOUNT has and resolves
/// every identifier it names against EC2. An EC2 client over the router's own in-process
/// callback comes with it: the very seam that resolution reaches EC2 through, and what the
/// suites that build their own network mint subnets with.
///
/// Derefs to the router, so every helper below still takes `&MultiServiceRouter`.
pub(crate) struct Emulator {
    pub(crate) router: MultiServiceRouter,
    pub(crate) ec2: ::aws_sdk_ec2::Client,
    subnets: Vec<String>,
}

impl Deref for Emulator {
    type Target = MultiServiceRouter;

    fn deref(&self) -> &Self::Target {
        &self.router
    }
}

impl Emulator {
    /// The subnets the account has in [`REGION`], as EC2 reported them, ordered by
    /// Availability Zone. These are the default VPC's default subnets, which EC2
    /// materialises for every Region of the account and mints the ids of — nothing here
    /// derives an identifier of its own.
    pub(crate) fn account_subnets(&self) -> &[String] {
        &self.subnets
    }

    /// The account's subnet in the first Availability Zone of [`REGION`].
    pub(crate) fn subnet_a(&self) -> &str {
        &self.subnets[0]
    }

    /// The account's subnet in the second Availability Zone of [`REGION`], so a case that
    /// needs two subnets names two zones.
    pub(crate) fn subnet_b(&self) -> &str {
        &self.subnets[1]
    }

    /// The VPC `subnet_id` belongs to, as the service that OWNS subnets reports it. What a
    /// created file system's `VpcId` is compared against, so nothing here builds that value
    /// itself.
    pub(crate) async fn subnet_vpc(&self, subnet_id: &str) -> String {
        self.ec2
            .describe_subnets()
            .subnet_ids(subnet_id)
            .send()
            .await
            .unwrap_or_else(|error| panic!("EC2 must report the subnet {subnet_id}: {error:?}"))
            .subnets()
            .iter()
            .find(|subnet| subnet.subnet_id() == Some(subnet_id))
            .and_then(::aws_sdk_ec2::types::Subnet::vpc_id)
            .unwrap_or_else(|| panic!("EC2 must report a VPC for the subnet {subnet_id}"))
            .to_string()
    }
}

/// An EC2 client for `region` over `callback`, the in-process seam this emulator routes on:
/// no socket is opened and the endpoint URL is only the address that transport routes by.
pub(crate) fn ec2_client(callback: &EmbeddedEmulator, region: &str) -> ::aws_sdk_ec2::Client {
    let config = ::aws_sdk_ec2::Config::builder()
        .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
        .region(::aws_sdk_ec2::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://ec2.{region}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_ec2::Client::from_conf(config)
}

/// One emulator with both services mounted, and the account's subnets read back from EC2.
pub(crate) async fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not the subject of these tests; the requests still carry the SigV4
    // credential scope, which is what routes them and names the region.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_fsx_awssimbaapiservice_v20180301::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    let ec2 = ec2_client(&EmbeddedEmulator::from_router(router.clone()), REGION);

    // ASKED, never assumed: the ids below are the ones EC2 answers with for this account,
    // which is also what the service under test resolves `SubnetIds` against. Only the
    // subnets in a zone OF THIS REGION are kept, so a case names a subnet of the Region its
    // requests are signed for.
    let described = ec2
        .describe_subnets()
        .send()
        .await
        .expect("EC2 must report the account's subnets");
    let mut reported: Vec<(String, String)> = described
        .subnets()
        .iter()
        .filter_map(|subnet| {
            Some((
                subnet.availability_zone()?.to_string(),
                subnet.subnet_id()?.to_string(),
            ))
        })
        .filter(|(zone, _)| zone.starts_with(REGION))
        .collect();
    reported.sort();
    let subnets: Vec<String> = reported.into_iter().map(|(_, id)| id).collect();
    assert!(
        subnets.len() >= 2,
        "the account must have at least two subnets in {REGION} for these cases, got \
         {subnets:?}"
    );

    Emulator {
        router,
        ec2,
        subnets,
    }
}

/// Dispatch one operation and return `(status, body)`.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("https://fsx.us-west-2.amazonaws.com/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240102/{REGION}/fsx/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Dispatch one operation, requiring a 200, and return the body.
pub(crate) async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got HTTP {status} with body {parsed}"
    );
    parsed
}

/// Dispatch one operation, requiring a failure, and return `(status, error code)`.
pub(crate) async fn failure(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, String) {
    let (status, parsed) = call(router, operation, body).await;
    assert_ne!(
        status, 200,
        "{operation} must fail, got HTTP 200 with body {parsed}"
    );
    let code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        // awsJson error codes may be namespaced (`com.amazonaws.fsx#X`); clients match on
        // the shape name.
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    (status, code)
}

/// The modelled `FileSystemId` pattern `^(fs-[0-9a-f]{8,})$`, with `length 11..21`.
fn is_well_formed_file_system_id(id: &str) -> bool {
    let Some(hex) = id.strip_prefix("fs-") else {
        return false;
    };
    (11..=21).contains(&id.len())
        && hex.len() >= 8
        && hex.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
}

/// The modelled `NextToken` pattern
/// `^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$`: base64 with padding.
fn is_base64_shaped(token: &str) -> bool {
    if !token.len().is_multiple_of(4) || token.is_empty() {
        return false;
    }
    let body = token.trim_end_matches('=');
    let padding = token.len() - body.len();
    padding <= 2
        && body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/')
}

/// The tags of a `FileSystem` (or of a `ListTagsForResource` page) as sorted pairs.
pub(crate) fn tag_pairs(tags: Option<&Value>) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = tags
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|tag| {
                    (
                        tag["Key"].as_str().unwrap_or_default().to_string(),
                        tag["Value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    pairs.sort();
    pairs
}

/// A route-table id matching the modelled `RouteTableId` pattern `^(rtb-[0-9a-f]{8,})$`.
fn route_table_id(index: usize) -> String {
    format!("rtb-{index:017x}")
}

/// The stored ONTAP route-table ids of a `FileSystem`.
fn route_table_ids(file_system: &Value) -> Vec<String> {
    file_system["OntapConfiguration"]["RouteTableIds"]
        .as_array()
        .map(|ids| {
            ids.iter()
                .map(|id| id.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Describe exactly one file system, requiring it to exist.
async fn describe_one(router: &MultiServiceRouter, id: &str) -> Value {
    let body = ok(
        router,
        "DescribeFileSystems",
        &json!({ "FileSystemIds": [id] }),
    )
    .await;
    let systems = body["FileSystems"]
        .as_array()
        .expect("FileSystems must be a list")
        .clone();
    assert_eq!(
        systems.len(),
        1,
        "describing {id} must return exactly one file system, got {body}"
    );
    systems[0].clone()
}

/// Wait out the clock-owed transition and return the file system's lifecycle.
async fn settled_lifecycle(router: &MultiServiceRouter, id: &str) -> String {
    tokio::time::sleep(PAST_TRANSITION).await;
    describe_one(router, id).await["Lifecycle"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A create request for one row of the type table, carrying the required
/// `ClientRequestToken`.
fn create_request(file_system_type: &str, storage_type: &str, subnets: &[&str]) -> Value {
    with_client_request_token(&json!({
        "FileSystemType": file_system_type,
        "StorageCapacity": 1200,
        "StorageType": storage_type,
        "SubnetIds": subnets,
    }))
}

/// RULE: a created file system is persisted, and the response is that stored record, so a
/// following `DescribeFileSystems` reports the very same values.
///
/// The table spans the modelled `FileSystemType` and `StorageType` enumerations and the
/// subnet count, which is what the rule generalises over.
#[tokio::test]
async fn create_persists_every_file_system_type_and_describe_reports_it_back() {
    let router = emulator().await;
    // (FileSystemType, StorageType) from the modelled `FileSystemType` /
    // `StorageType` enums, plus one and two subnets.
    let cases: &[(&str, &str, &[&str])] = &[
        ("WINDOWS", "SSD", &[router.subnet_a()]),
        ("WINDOWS", "HDD", &[router.subnet_a(), router.subnet_b()]),
        ("LUSTRE", "SSD", &[router.subnet_a()]),
        ("LUSTRE", "INTELLIGENT_TIERING", &[router.subnet_a()]),
        ("ONTAP", "SSD", &[router.subnet_a(), router.subnet_b()]),
        ("OPENZFS", "INTELLIGENT_TIERING", &[router.subnet_a()]),
    ];

    let mut minted: Vec<String> = Vec::new();
    for case in cases {
        let (file_system_type, storage_type, subnets) = *case;
        let created = ok(
            &router,
            "CreateFileSystem",
            &create_request(file_system_type, storage_type, subnets),
        )
        .await;
        let file_system = created["FileSystem"].clone();
        let id = file_system["FileSystemId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        assert!(
            is_well_formed_file_system_id(&id),
            "case {case:?}: expected a FileSystemId matching ^fs-[0-9a-f]{{8,}}$ of length \
             11..21, got {id:?}"
        );
        assert!(
            !minted.contains(&id),
            "case {case:?}: expected a fresh FileSystemId, got the already-minted {id:?}"
        );
        assert_eq!(
            file_system["Lifecycle"], "CREATING",
            "case {case:?}: expected Lifecycle CREATING on create, got {:?}",
            file_system["Lifecycle"]
        );
        assert_eq!(
            file_system["OwnerId"], ACCOUNT,
            "case {case:?}: expected OwnerId {ACCOUNT}, got {:?}",
            file_system["OwnerId"]
        );
        assert_eq!(
            file_system["ResourceARN"],
            json!(format!("arn:aws:fsx:{REGION}:{ACCOUNT}:file-system/{id}")),
            "case {case:?}: expected the file-system ARN for the signed region, got {:?}",
            file_system["ResourceARN"]
        );
        assert_eq!(
            file_system["DNSName"],
            json!(format!("{id}.fsx.{REGION}.amazonaws.com")),
            "case {case:?}: expected the FSx DNS name, got {:?}",
            file_system["DNSName"]
        );
        assert!(
            file_system["CreationTime"].as_f64().unwrap_or_default() > 0.0,
            "case {case:?}: expected a CreationTime, got {:?}",
            file_system["CreationTime"]
        );

        // The persisted record, read back through the implemented reader.
        let described = describe_one(&router, &id).await;
        for member in [
            "FileSystemId",
            "FileSystemType",
            "StorageCapacity",
            "StorageType",
            "SubnetIds",
            "OwnerId",
            "ResourceARN",
            "DNSName",
            "CreationTime",
        ] {
            assert_eq!(
                described[member], file_system[member],
                "case {case:?}: DescribeFileSystems must report the stored {member}; create \
                 said {:?} and describe said {:?}",
                file_system[member], described[member]
            );
        }
        assert_eq!(
            described["FileSystemType"], json!(file_system_type),
            "case {case:?}: expected the stored FileSystemType, got {:?}",
            described["FileSystemType"]
        );
        assert_eq!(
            described["StorageType"], json!(storage_type),
            "case {case:?}: expected the stored StorageType, got {:?}",
            described["StorageType"]
        );
        assert_eq!(
            described["StorageCapacity"], json!(1200),
            "case {case:?}: expected the stored StorageCapacity, got {:?}",
            described["StorageCapacity"]
        );
        assert_eq!(
            described["SubnetIds"], json!(subnets),
            "case {case:?}: expected the stored SubnetIds, got {:?}",
            described["SubnetIds"]
        );
        minted.push(id);
    }

    // Every created file system is in the store, and `DescribeFileSystems` with no filter
    // reports exactly them.
    let all = ok(&router, "DescribeFileSystems", &json!({})).await;
    let mut listed: Vec<String> = all["FileSystems"]
        .as_array()
        .expect("FileSystems must be a list")
        .iter()
        .map(|fs| fs["FileSystemId"].as_str().unwrap_or_default().to_string())
        .collect();
    listed.sort();
    let mut expected = minted.clone();
    expected.sort();
    assert_eq!(
        listed, expected,
        "an unfiltered DescribeFileSystems must report exactly the stored file systems"
    );
}

/// RULE: a member the create request omits takes the default AWS documents for it, and a
/// member it carries is stored as given.
#[tokio::test]
async fn documented_defaults_are_applied_only_where_the_request_is_silent() {
    let router = emulator().await;
    // (StorageType in the request, NetworkType in the request, expected stored pair).
    // Defaults: "Default value is SSD" (CreateFileSystem$StorageType) and "The default is
    // IPV4" (CreateFileSystem$NetworkType).
    let cases: &[(Option<&str>, Option<&str>, &str, &str)] = &[
        (None, None, "SSD", "IPV4"),
        (Some("HDD"), None, "HDD", "IPV4"),
        (None, Some("DUAL"), "SSD", "DUAL"),
        (Some("INTELLIGENT_TIERING"), Some("IPV4"), "INTELLIGENT_TIERING", "IPV4"),
        (Some("SSD"), Some("DUAL"), "SSD", "DUAL"),
    ];

    for case in cases {
        let (storage_type, network_type, expected_storage, expected_network) = *case;
        let mut request = json!({
            "FileSystemType": "WINDOWS",
            "SubnetIds": [router.subnet_a()],
            "StorageCapacity": 32,
        });
        if let Some(storage_type) = storage_type {
            request["StorageType"] = json!(storage_type);
        }
        if let Some(network_type) = network_type {
            request["NetworkType"] = json!(network_type);
        }

        let created = ok(&router, "CreateFileSystem", &with_client_request_token(&request)).await;
        let id = created["FileSystem"]["FileSystemId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let described = describe_one(&router, &id).await;
        assert_eq!(
            described["StorageType"], json!(expected_storage),
            "case {case:?}: expected StorageType {expected_storage}, got {:?}",
            described["StorageType"]
        );
        assert_eq!(
            described["NetworkType"], json!(expected_network),
            "case {case:?}: expected NetworkType {expected_network}, got {:?}",
            described["NetworkType"]
        );
    }
}

/// RULE: `FileSystemTypeVersion` defaults as AWS documents it on
/// `CreateFileSystem$FileSystemTypeVersion` — 2.10, except 2.12 for a `PERSISTENT_2`
/// Lustre deployment without a metadata configuration and 2.15 for one with it — applies
/// to FSx for Lustre only, and an explicit value always wins.
#[tokio::test]
async fn lustre_file_system_type_version_takes_its_documented_default() {
    let router = emulator().await;
    // (FileSystemType, Lustre DeploymentType, metadata configuration mode, requested
    // version, expected stored version).
    type VersionCase<'a> = (
        &'a str,
        Option<&'a str>,
        Option<&'a str>,
        Option<&'a str>,
        Option<&'a str>,
    );
    let cases: &[VersionCase] = &[
        ("LUSTRE", None, None, None, Some("2.10")),
        ("LUSTRE", Some("SCRATCH_2"), None, None, Some("2.10")),
        ("LUSTRE", Some("PERSISTENT_1"), None, None, Some("2.10")),
        ("LUSTRE", Some("PERSISTENT_2"), None, None, Some("2.12")),
        (
            "LUSTRE",
            Some("PERSISTENT_2"),
            Some("AUTOMATIC"),
            None,
            Some("2.15"),
        ),
        (
            "LUSTRE",
            Some("PERSISTENT_2"),
            Some("USER_PROVISIONED"),
            None,
            Some("2.15"),
        ),
        // An explicit version is stored as given, never defaulted over.
        ("LUSTRE", Some("PERSISTENT_2"), None, Some("2.15"), Some("2.15")),
        ("LUSTRE", None, None, Some("2.12"), Some("2.12")),
        // The member is documented as the Lustre version, so no other type gets one.
        ("WINDOWS", None, None, None, None),
        ("ONTAP", None, None, None, None),
        ("OPENZFS", None, None, None, None),
        ("WINDOWS", None, None, Some("2.10"), Some("2.10")),
    ];

    for case in cases {
        let (file_system_type, deployment_type, metadata_mode, requested, expected) = *case;
        let mut request = json!({
            "FileSystemType": file_system_type,
            "SubnetIds": [router.subnet_a()],
            "StorageCapacity": 1200,
        });
        if let Some(requested) = requested {
            request["FileSystemTypeVersion"] = json!(requested);
        }
        if deployment_type.is_some() || metadata_mode.is_some() {
            let mut lustre = json!({});
            if let Some(deployment_type) = deployment_type {
                lustre["DeploymentType"] = json!(deployment_type);
            }
            if let Some(mode) = metadata_mode {
                lustre["MetadataConfiguration"] = json!({ "Mode": mode });
            }
            request["LustreConfiguration"] = lustre;
        }

        let created = ok(&router, "CreateFileSystem", &with_client_request_token(&request)).await;
        let id = created["FileSystem"]["FileSystemId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let described = describe_one(&router, &id).await;
        let actual = described["FileSystemTypeVersion"].as_str();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected FileSystemTypeVersion {expected:?}, got {actual:?}"
        );
    }
}

/// RULE: a per-type configuration the create request carries is persisted and reported
/// back; a secret in it never is.
#[tokio::test]
async fn per_type_configuration_round_trips_without_echoing_secrets() {
    let router = emulator().await;
    let created = ok(
        &router,
        "CreateFileSystem",
        &json!({
            "FileSystemType": "WINDOWS",
            "ClientRequestToken": "per-type-configuration",
            "SubnetIds": [router.subnet_a(), router.subnet_b()],
            "StorageCapacity": 64,
            "WindowsConfiguration": {
                "ThroughputCapacity": 32,
                "DeploymentType": "MULTI_AZ_1",
                "PreferredSubnetId": router.subnet_b(),
                "WeeklyMaintenanceStartTime": "1:05:00",
                "DailyAutomaticBackupStartTime": "03:00",
                "AutomaticBackupRetentionDays": 7,
                "CopyTagsToBackups": true,
                "SelfManagedActiveDirectoryConfiguration": {
                    "DomainName": "corp.example.com",
                    "UserName": "fsx-admin",
                    "Password": "super-secret-password",
                    "DnsIps": ["10.0.0.1"],
                },
            },
        }),
    )
    .await;
    let id = created["FileSystem"]["FileSystemId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let described = describe_one(&router, &id).await;
    let windows = &described["WindowsConfiguration"];
    for (member, expected) in [
        ("ThroughputCapacity", json!(32)),
        ("DeploymentType", json!("MULTI_AZ_1")),
        ("PreferredSubnetId", json!(router.subnet_b())),
        ("WeeklyMaintenanceStartTime", json!("1:05:00")),
        ("DailyAutomaticBackupStartTime", json!("03:00")),
        ("AutomaticBackupRetentionDays", json!(7)),
        ("CopyTagsToBackups", json!(true)),
    ] {
        assert_eq!(
            windows[member], expected,
            "case WindowsConfiguration.{member}: expected {expected:?}, got {:?}",
            windows[member]
        );
    }
    let directory = &windows["SelfManagedActiveDirectoryConfiguration"];
    assert_eq!(
        directory["DomainName"], json!("corp.example.com"),
        "case self-managed AD: expected the stored DomainName, got {:?}",
        directory["DomainName"]
    );
    assert_eq!(
        directory["DnsIps"], json!(["10.0.0.1"]),
        "case self-managed AD: expected the stored DnsIps, got {:?}",
        directory["DnsIps"]
    );
    let rendered = described.to_string();
    assert!(
        !rendered.contains("super-secret-password") && !rendered.contains("Password"),
        "case self-managed AD: the directory password must never be reported back, got \
         {rendered}"
    );

    // An update carries only the members it changes, and leaves the rest of the stored
    // configuration alone.
    assert_eq!(
        settled_lifecycle(&router, &id).await,
        "AVAILABLE",
        "case update: the file system must settle to AVAILABLE before it can be updated"
    );
    ok(
        &router,
        "UpdateFileSystem",
        &json!({
            "FileSystemId": id,
            "WindowsConfiguration": { "ThroughputCapacity": 64 },
        }),
    )
    .await;
    let updated = describe_one(&router, &id).await;
    assert_eq!(
        updated["WindowsConfiguration"]["ThroughputCapacity"], json!(64),
        "case update: expected the new ThroughputCapacity 64, got {:?}",
        updated["WindowsConfiguration"]["ThroughputCapacity"]
    );
    assert_eq!(
        updated["WindowsConfiguration"]["WeeklyMaintenanceStartTime"], json!("1:05:00"),
        "case update: an untouched member must keep its stored value, got {:?}",
        updated["WindowsConfiguration"]["WeeklyMaintenanceStartTime"]
    );
}

/// RULE: every member an update carries replaces the stored one and is what later reads
/// report; an absent member leaves the stored value alone.
#[tokio::test]
async fn update_mutates_the_stored_record_member_by_member() {
    let router = emulator().await;
    let created = ok(
        &router,
        "CreateFileSystem",
        &json!({
            "FileSystemType": "LUSTRE",
            "ClientRequestToken": "update-member-by-member",
            "SubnetIds": [router.subnet_a()],
            "StorageCapacity": 1200,
            "StorageType": "SSD",
            "FileSystemTypeVersion": "2.12",
            "NetworkType": "IPV4",
            // A deployment type whose storage capacity CAN be increased: AWS documents that
            // a `SCRATCH_1` file system's capacity cannot be, and a Lustre record that names
            // no deployment type is read as that documented default.
            "LustreConfiguration": { "DeploymentType": "SCRATCH_2" },
        }),
    )
    .await;
    let id = created["FileSystem"]["FileSystemId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // (member, value) rows, applied one at a time: each must be observable afterwards and
    // must not disturb the others.
    let cases: &[(&str, Value)] = &[
        ("StorageCapacity", json!(2400)),
        ("StorageType", json!("HDD")),
        ("FileSystemTypeVersion", json!("2.15")),
        ("NetworkType", json!("DUAL")),
        ("StorageCapacity", json!(4800)),
    ];

    let mut expected = serde_json::Map::new();
    expected.insert("StorageCapacity".to_string(), json!(1200));
    expected.insert("StorageType".to_string(), json!("SSD"));
    expected.insert("FileSystemTypeVersion".to_string(), json!("2.12"));
    expected.insert("NetworkType".to_string(), json!("IPV4"));

    for case in cases {
        let (member, value) = case;
        assert_eq!(
            settled_lifecycle(&router, &id).await,
            "AVAILABLE",
            "case {case:?}: the file system must be AVAILABLE before the next update"
        );
        let response = ok(
            &router,
            "UpdateFileSystem",
            &json!({ "FileSystemId": id, *member: value }),
        )
        .await;
        // A storage-capacity target is carried out by a `FILE_SYSTEM_UPDATE` administrative
        // action and Amazon FSx performs it in place, so the file system stays AVAILABLE
        // while it runs; every other member is a customer-initiated update, which moves the
        // lifecycle to UPDATING.
        let capacity_target = *member == "StorageCapacity";
        let in_flight_lifecycle = if capacity_target { "AVAILABLE" } else { "UPDATING" };
        assert_eq!(
            response["FileSystem"]["Lifecycle"], json!(in_flight_lifecycle),
            "case {case:?}: expected Lifecycle {in_flight_lifecycle} while the update is in \
             flight, got {:?}",
            response["FileSystem"]["Lifecycle"]
        );
        expected.insert((*member).to_string(), value.clone());

        // A capacity target lands when the administrative action the clock owes completes,
        // not on the call that asked for it.
        if capacity_target {
            tokio::time::sleep(PAST_TRANSITION).await;
        }
        let described = describe_one(&router, &id).await;
        for (expected_member, expected_value) in &expected {
            assert_eq!(
                described[expected_member], *expected_value,
                "case {case:?}: expected {expected_member} to be {expected_value:?} after the \
                 update, got {:?}",
                described[expected_member]
            );
        }
    }

    let (status, code) = failure(
        &router,
        "UpdateFileSystem",
        &json!({ "FileSystemId": "fs-0000000000000dead" }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "FileSystemNotFound"),
        "case unknown id: expected 400 FileSystemNotFound, got {status} {code}"
    );
}

/// RULE: a lifecycle transition the clock owes is due a short span after the call that
/// starts it, so a caller that acts immediately still sees the transient state and the
/// state errors that go with it.
#[tokio::test]
async fn lifecycle_transitions_fall_due_after_the_call_that_starts_them() {
    let router = emulator().await;
    let created = ok(
        &router,
        "CreateFileSystem",
        &json!({
            "FileSystemType": "OPENZFS",
            "ClientRequestToken": "lifecycle-transitions",
            "SubnetIds": [router.subnet_a()],
            "StorageCapacity": 64,
        }),
    )
    .await;
    let id = created["FileSystem"]["FileSystemId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // CREATING is observable, and a file system that is still coming up is not updatable.
    assert_eq!(
        describe_one(&router, &id).await["Lifecycle"],
        json!("CREATING"),
        "case straight after create: expected Lifecycle CREATING"
    );
    let (status, code) = failure(
        &router,
        "UpdateFileSystem",
        &json!({ "FileSystemId": id, "StorageCapacity": 128 }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "BadRequest"),
        "case update while CREATING: expected 400 BadRequest, got {status} {code}"
    );

    assert_eq!(
        settled_lifecycle(&router, &id).await,
        "AVAILABLE",
        "case after the transition span: expected Lifecycle AVAILABLE"
    );

    // UPDATING, then back to AVAILABLE. A property the update WRITES onto the record is what
    // moves the lifecycle; a storage-capacity target is performed in place and leaves it
    // AVAILABLE (see `tests/all/storage_capacity_update.rs`).
    ok(
        &router,
        "UpdateFileSystem",
        &json!({ "FileSystemId": id, "NetworkType": "DUAL" }),
    )
    .await;
    assert_eq!(
        describe_one(&router, &id).await["Lifecycle"],
        json!("UPDATING"),
        "case straight after update: expected Lifecycle UPDATING"
    );
    assert_eq!(
        settled_lifecycle(&router, &id).await,
        "AVAILABLE",
        "case after the update transition span: expected Lifecycle AVAILABLE"
    );

    // DELETING, then gone.
    let deleted = ok(
        &router,
        "DeleteFileSystem",
        &json!({ "FileSystemId": id }),
    )
    .await;
    assert_eq!(
        deleted["FileSystemId"], json!(id),
        "case delete: expected the deleted FileSystemId, got {:?}",
        deleted["FileSystemId"]
    );
    assert_eq!(
        deleted["Lifecycle"], json!("DELETING"),
        "case delete: expected Lifecycle DELETING, got {:?}",
        deleted["Lifecycle"]
    );
    assert_eq!(
        describe_one(&router, &id).await["Lifecycle"],
        json!("DELETING"),
        "case straight after delete: expected the record to still report DELETING"
    );
    // @idempotent: deleting again reports the same lifecycle rather than failing.
    let again = ok(
        &router,
        "DeleteFileSystem",
        &json!({ "FileSystemId": id }),
    )
    .await;
    assert_eq!(
        again["Lifecycle"], json!("DELETING"),
        "case second delete: expected Lifecycle DELETING again, got {:?}",
        again["Lifecycle"]
    );

    tokio::time::sleep(PAST_TRANSITION).await;
    let (status, code) = failure(
        &router,
        "DescribeFileSystems",
        &json!({ "FileSystemIds": [id] }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "FileSystemNotFound"),
        "case after the removal span: expected 400 FileSystemNotFound, got {status} {code}"
    );
    let all = ok(&router, "DescribeFileSystems", &json!({})).await;
    assert_eq!(
        all["FileSystems"], json!([]),
        "case after the removal span: the removed file system must be gone from the list, \
         got {:?}",
        all["FileSystems"]
    );
    let (status, code) = failure(
        &router,
        "DeleteFileSystem",
        &json!({ "FileSystemId": id }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "FileSystemNotFound"),
        "case delete after removal: expected 400 FileSystemNotFound, got {status} {code}"
    );
}

/// RULE: `DescribeFileSystems` reports the stored file systems, selects by `FileSystemIds`,
/// and pages at `MaxResults` with an opaque `NextToken` that walks every record exactly
/// once.
#[tokio::test]
async fn describe_selects_by_id_and_pages_at_max_results() {
    let router = emulator().await;
    let mut ids: Vec<String> = Vec::new();
    for index in 0..5 {
        let created = ok(
            &router,
            "CreateFileSystem",
            &json!({
                "FileSystemType": "WINDOWS",
                "ClientRequestToken": format!("paging-{index}"),
                "SubnetIds": [router.subnet_a()],
                "StorageCapacity": 32 + index,
            }),
        )
        .await;
        ids.push(
            created["FileSystem"]["FileSystemId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }
    let mut all_ids = ids.clone();
    all_ids.sort();

    // Selecting by id returns exactly the requested set, whatever its size or order.
    let selections: &[&[usize]] = &[&[0], &[4], &[1, 3], &[3, 1], &[0, 1, 2, 3, 4]];
    for case in selections {
        let requested: Vec<String> = case.iter().map(|index| ids[*index].clone()).collect();
        let body = ok(
            &router,
            "DescribeFileSystems",
            &json!({ "FileSystemIds": requested }),
        )
        .await;
        let mut returned: Vec<String> = body["FileSystems"]
            .as_array()
            .expect("FileSystems must be a list")
            .iter()
            .map(|fs| fs["FileSystemId"].as_str().unwrap_or_default().to_string())
            .collect();
        returned.sort();
        let mut expected = requested;
        expected.sort();
        assert_eq!(
            returned, expected,
            "case {case:?}: expected exactly the requested file systems, got {returned:?}"
        );
    }

    // One unknown id in the request fails the whole call.
    for missing in ["fs-00000000deadbeef", "fs-0000000000000ffff"] {
        let (status, code) = failure(
            &router,
            "DescribeFileSystems",
            &json!({ "FileSystemIds": [missing] }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "FileSystemNotFound"),
            "case missing {missing}: expected 400 FileSystemNotFound, got {status} {code}"
        );
    }
    let (status, code) = failure(
        &router,
        "DescribeFileSystems",
        &json!({ "FileSystemIds": [ids[0].clone(), "fs-00000000deadbeef".to_string()] }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "FileSystemNotFound"),
        "case one known and one missing id: expected 400 FileSystemNotFound, got {status} \
         {code}"
    );

    // Paging: every page size walks all five records exactly once.
    for page_size in [1_usize, 2, 3, 5, 7] {
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "MaxResults": page_size });
            if let Some(token) = next_token.as_ref() {
                request["NextToken"] = json!(token);
            }
            let body = ok(&router, "DescribeFileSystems", &request).await;
            let page = body["FileSystems"]
                .as_array()
                .expect("FileSystems must be a list")
                .clone();
            assert!(
                page.len() <= page_size,
                "case MaxResults {page_size}: a page must hold at most {page_size} file \
                 systems, got {}",
                page.len()
            );
            walked.extend(
                page.iter()
                    .map(|fs| fs["FileSystemId"].as_str().unwrap_or_default().to_string()),
            );
            next_token = body["NextToken"].as_str().map(ToString::to_string);
            if let Some(token) = next_token.as_ref() {
                assert!(
                    is_base64_shaped(token),
                    "case MaxResults {page_size}: NextToken must match the modelled base64 \
                     pattern, got {token:?}"
                );
            }
            pages += 1;
            assert!(
                pages <= 10,
                "case MaxResults {page_size}: paging did not terminate after {pages} pages"
            );
            if next_token.is_none() {
                break;
            }
        }
        let mut sorted = walked.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted, all_ids,
            "case MaxResults {page_size}: paging must walk every file system exactly once, \
             got {walked:?}"
        );
    }

    // A token this service did not mint is refused rather than silently answered with an
    // empty page. The second and third decode as perfectly good base64 — being decodable
    // is not what makes a token this service's.
    for bogus in ["bm90LWEtdG9rZW4h", "////", "ZnMtMDAwMDAwMDAwMDAwMDAwMDE="] {
        let (status, code) = failure(
            &router,
            "DescribeFileSystems",
            &json!({ "NextToken": bogus }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "BadRequest"),
            "case bogus NextToken {bogus:?}: expected 400 BadRequest, got {status} {code}"
        );
    }

    // A token minted for one listing is not another listing's token.
    let first_arn = describe_one(&router, &ids[0]).await["ResourceARN"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let partial_page = ok(&router, "DescribeFileSystems", &json!({ "MaxResults": 1 })).await;
    let describe_token = partial_page["NextToken"]
        .as_str()
        .expect("a partial page must carry a NextToken")
        .to_string();
    let (status, code) = failure(
        &router,
        "ListTagsForResource",
        &json!({ "ResourceARN": first_arn, "NextToken": describe_token.clone() }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "BadRequest"),
        "case a DescribeFileSystems token on ListTagsForResource: expected 400 BadRequest, \
         got {status} {code}"
    );
    // Nor is an unfiltered listing's token a filtered listing's token: a cursor only means
    // something over the same selection.
    let (status, code) = failure(
        &router,
        "DescribeFileSystems",
        &json!({
            "FileSystemIds": [ids[0].clone(), ids[1].clone()],
            "MaxResults": 1,
            "NextToken": describe_token,
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "BadRequest"),
        "case an unfiltered token on a FileSystemIds-filtered listing: expected 400 \
         BadRequest, got {status} {code}"
    );
    // A filtered listing pages with its own token, whatever order the ids arrive in.
    let filtered_first = ok(
        &router,
        "DescribeFileSystems",
        &json!({ "FileSystemIds": [ids[1].clone(), ids[0].clone()], "MaxResults": 1 }),
    )
    .await;
    let filtered_token = filtered_first["NextToken"]
        .as_str()
        .expect("a partial filtered page must carry a NextToken")
        .to_string();
    let filtered_rest = ok(
        &router,
        "DescribeFileSystems",
        &json!({
            "FileSystemIds": [ids[0].clone(), ids[1].clone()],
            "MaxResults": 1,
            "NextToken": filtered_token,
        }),
    )
    .await;
    let mut walked_filtered: Vec<String> = filtered_first["FileSystems"]
        .as_array()
        .into_iter()
        .chain(filtered_rest["FileSystems"].as_array())
        .flatten()
        .map(|fs| fs["FileSystemId"].as_str().unwrap_or_default().to_string())
        .collect();
    walked_filtered.sort();
    let mut expected_filtered = vec![ids[0].clone(), ids[1].clone()];
    expected_filtered.sort();
    assert_eq!(
        walked_filtered, expected_filtered,
        "case a filtered listing paged with its own token: expected both selected file \
         systems exactly once, got {walked_filtered:?}"
    );
}

/// RULE: tagging operations work against the same stored tag map the create call wrote, so
/// `TagResource` merges, `UntagResource` removes, and both `ListTagsForResource` and
/// `DescribeFileSystems` report the result.
#[tokio::test]
async fn tags_are_stored_once_and_read_back_everywhere() {
    let router = emulator().await;
    let created = ok(
        &router,
        "CreateFileSystem",
        &json!({
            "FileSystemType": "WINDOWS",
            "ClientRequestToken": "tagging",
            "SubnetIds": [router.subnet_a()],
            "StorageCapacity": 32,
            "Tags": [{ "Key": "env", "Value": "dev" }],
        }),
    )
    .await;
    let id = created["FileSystem"]["FileSystemId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let arn = created["FileSystem"]["ResourceARN"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        tag_pairs(created["FileSystem"].get("Tags")),
        vec![("env".to_string(), "dev".to_string())],
        "case create with tags: expected the submitted tags on the response"
    );

    // (tags to add, keys to remove, expected tag set afterwards): a new key is added, a
    // present key is overwritten, and a removal takes only what it names.
    type Case<'a> = (&'a [(&'a str, &'a str)], &'a [&'a str], &'a [(&'a str, &'a str)]);
    let cases: &[Case] = &[
        (
            &[("team", "storage")],
            &[],
            &[("env", "dev"), ("team", "storage")],
        ),
        (
            &[("env", "prod")],
            &[],
            &[("env", "prod"), ("team", "storage")],
        ),
        (
            &[("a", "1"), ("b", "2")],
            &[],
            &[("a", "1"), ("b", "2"), ("env", "prod"), ("team", "storage")],
        ),
        (
            &[],
            &["a", "b"],
            &[("env", "prod"), ("team", "storage")],
        ),
        // Removing a key that is not there leaves the set alone (the operation is
        // @idempotent).
        (
            &[],
            &["not-a-key"],
            &[("env", "prod"), ("team", "storage")],
        ),
        (&[], &["env", "team"], &[]),
    ];

    for case in cases {
        let (add, remove, expected) = *case;
        if !add.is_empty() {
            let tags: Vec<Value> = add
                .iter()
                .map(|(key, value)| json!({ "Key": key, "Value": value }))
                .collect();
            ok(
                &router,
                "TagResource",
                &json!({ "ResourceARN": arn, "Tags": tags }),
            )
            .await;
        }
        if !remove.is_empty() {
            ok(
                &router,
                "UntagResource",
                &json!({ "ResourceARN": arn, "TagKeys": remove }),
            )
            .await;
        }

        let expected_pairs: Vec<(String, String)> = expected
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        let listed = ok(
            &router,
            "ListTagsForResource",
            &json!({ "ResourceARN": arn }),
        )
        .await;
        assert_eq!(
            tag_pairs(listed.get("Tags")),
            expected_pairs,
            "case {case:?}: expected ListTagsForResource to report {expected_pairs:?}, got {:?}",
            listed.get("Tags")
        );
        let described = describe_one(&router, &id).await;
        assert_eq!(
            tag_pairs(described.get("Tags")),
            expected_pairs,
            "case {case:?}: DescribeFileSystems must report the same stored tags, got {:?}",
            described.get("Tags")
        );
    }

    // Tag paging, over the same stored map.
    let tags: Vec<Value> = (0..5)
        .map(|index| json!({ "Key": format!("key{index}"), "Value": format!("value{index}") }))
        .collect();
    ok(
        &router,
        "TagResource",
        &json!({ "ResourceARN": arn, "Tags": tags }),
    )
    .await;
    for page_size in [1_usize, 2, 5] {
        let mut walked: Vec<(String, String)> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut request = json!({ "ResourceARN": arn, "MaxResults": page_size });
            if let Some(token) = next_token.as_ref() {
                request["NextToken"] = json!(token);
            }
            let body = ok(&router, "ListTagsForResource", &request).await;
            let page = tag_pairs(body.get("Tags"));
            assert!(
                page.len() <= page_size,
                "case MaxResults {page_size}: a tag page must hold at most {page_size} tags, \
                 got {}",
                page.len()
            );
            walked.extend(page);
            next_token = body["NextToken"].as_str().map(ToString::to_string);
            if next_token.is_none() {
                break;
            }
        }
        walked.sort();
        let expected: Vec<(String, String)> = (0..5)
            .map(|index| (format!("key{index}"), format!("value{index}")))
            .collect();
        assert_eq!(
            walked, expected,
            "case MaxResults {page_size}: tag paging must walk every stored tag exactly once"
        );
    }

    // A merge that would take the resource past the modelled maximum of 50 tags is
    // refused, and leaves the stored set alone.
    let stored_now = tag_pairs(
        ok(
            &router,
            "ListTagsForResource",
            &json!({ "ResourceARN": arn }),
        )
        .await
        .get("Tags"),
    );
    assert_eq!(
        stored_now.len(),
        5,
        "expected the five paging tags to be the stored set, got {stored_now:?}"
    );
    let overflowing: Vec<Value> = (0..46)
        .map(|index| json!({ "Key": format!("extra{index}"), "Value": "v" }))
        .collect();
    let (status, code) = failure(
        &router,
        "TagResource",
        &json!({ "ResourceARN": arn, "Tags": overflowing }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "BadRequest"),
        "case 5 stored + 46 new tags: expected 400 BadRequest for exceeding the modelled \
         maximum of 50, got {status} {code}"
    );
    assert_eq!(
        tag_pairs(
            ok(
                &router,
                "ListTagsForResource",
                &json!({ "ResourceARN": arn }),
            )
            .await
            .get("Tags")
        ),
        stored_now,
        "case 5 stored + 46 new tags: a refused TagResource must store nothing"
    );
    // Exactly at the maximum is accepted.
    let to_the_limit: Vec<Value> = (0..45)
        .map(|index| json!({ "Key": format!("extra{index}"), "Value": "v" }))
        .collect();
    ok(
        &router,
        "TagResource",
        &json!({ "ResourceARN": arn, "Tags": to_the_limit }),
    )
    .await;
    let mut walked_all: Vec<(String, String)> = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({ "ResourceARN": arn, "MaxResults": 20 });
        if let Some(token) = next_token.as_ref() {
            request["NextToken"] = json!(token);
        }
        let body = ok(&router, "ListTagsForResource", &request).await;
        walked_all.extend(tag_pairs(body.get("Tags")));
        next_token = body["NextToken"].as_str().map(ToString::to_string);
        if next_token.is_none() {
            break;
        }
    }
    assert_eq!(
        walked_all.len(),
        50,
        "case 5 stored + 45 new tags: expected the resource to hold exactly 50 tags, got {}",
        walked_all.len()
    );

    // An ARN that resolves to nothing, and one that names no FSx resource type at all.
    // Resource types from the model's own id shapes: a backup ARN is an FSx type that
    // holds no record here; `widget` is not an FSx type.
    let unresolvable: &[(&str, &str)] = &[
        (
            "arn:aws:fsx:us-west-2:000000000000:file-system/fs-00000000deadbeef",
            "ResourceNotFound",
        ),
        (
            "arn:aws:fsx:us-west-2:000000000000:backup/backup-0123456789abcdef0",
            "ResourceNotFound",
        ),
        (
            "arn:aws:fsx:us-west-2:000000000000:volume/fsvol-0123456789abcdef0",
            "ResourceNotFound",
        ),
        (
            "arn:aws:fsx:us-west-2:000000000000:widget/w-0123456789abcdef0",
            "NotServiceResourceError",
        ),
    ];
    for case in unresolvable {
        let (resource_arn, expected_code) = *case;
        for operation in ["TagResource", "UntagResource", "ListTagsForResource"] {
            let request = match operation {
                "TagResource" => json!({
                    "ResourceARN": resource_arn,
                    "Tags": [{ "Key": "k", "Value": "v" }],
                }),
                "UntagResource" => json!({ "ResourceARN": resource_arn, "TagKeys": ["k"] }),
                _ => json!({ "ResourceARN": resource_arn }),
            };
            let (status, code) = failure(&router, operation, &request).await;
            assert_eq!(
                (status, code.as_str()),
                (400, expected_code),
                "case {case:?} on {operation}: expected 400 {expected_code}, got {status} {code}"
            );
        }
    }
}

/// RULE: one `ClientRequestToken` stands for one create. Replaying it with the same
/// parameters returns the file system it already created; replaying it with different
/// parameters is `IncompatibleParameterError`.
#[tokio::test]
async fn create_replays_one_client_request_token_to_one_file_system() {
    let router = emulator().await;
    let base = json!({
        "FileSystemType": "WINDOWS",
        "SubnetIds": [router.subnet_a()],
        "StorageCapacity": 32,
        "StorageType": "SSD",
        "Tags": [
            { "Key": "env", "Value": "dev" },
            { "Key": "team", "Value": "storage" },
        ],
        "ClientRequestToken": "token-one",
    });

    let first = ok(&router, "CreateFileSystem", &base).await;
    let id = first["FileSystem"]["FileSystemId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The same token, replayed, is the same file system — and the SAME tag set in a
    // different order is the same parameters, not a conflict.
    let mut reordered_tags = base.clone();
    reordered_tags["Tags"] = json!([
        { "Key": "team", "Value": "storage" },
        { "Key": "env", "Value": "dev" },
    ]);
    for case in [base.clone(), reordered_tags] {
        let replay = ok(&router, "CreateFileSystem", &case).await;
        assert_eq!(
            replay["FileSystem"]["FileSystemId"], json!(id),
            "case {case}: a replayed token must return the same file system, got {:?}",
            replay["FileSystem"]["FileSystemId"]
        );
    }

    // One token, different parameters, one member at a time.
    let conflicts: &[(&str, Value)] = &[
        ("StorageCapacity", json!(64)),
        ("StorageType", json!("HDD")),
        ("SubnetIds", json!([router.subnet_b()])),
        ("FileSystemType", json!("LUSTRE")),
        ("Tags", json!([{ "Key": "env", "Value": "prod" }])),
        ("NetworkType", json!("DUAL")),
    ];
    for case in conflicts {
        let (member, value) = case;
        let mut request = base.clone();
        request[*member] = value.clone();
        let (status, code) = failure(
            &router,
            "CreateFileSystem",
            &with_client_request_token(&request),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "IncompatibleParameterError"),
            "case {case:?}: expected 400 IncompatibleParameterError, got {status} {code}"
        );
    }

    // A different token is a different file system.
    let mut other = base.clone();
    other["ClientRequestToken"] = json!("token-two");
    let second = ok(&router, "CreateFileSystem", &other).await;
    let second_id = second["FileSystem"]["FileSystemId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_ne!(
        second_id, id,
        "case a second token: expected a new file system, got {second_id}"
    );

    // Exactly two file systems exist, however many calls were made.
    let all = ok(&router, "DescribeFileSystems", &json!({})).await;
    assert_eq!(
        all["FileSystems"].as_array().map(Vec::len),
        Some(2),
        "expected exactly two file systems after the replays, got {:?}",
        all["FileSystems"]
    );

    // A token whose file system is gone replays nothing: the service releases the token
    // with the record rather than remembering it for the life of the process.
    assert_eq!(
        settled_lifecycle(&router, &id).await,
        "AVAILABLE",
        "case release: the file system must settle to AVAILABLE before it can be deleted"
    );
    ok(
        &router,
        "DeleteFileSystem",
        &json!({ "FileSystemId": id }),
    )
    .await;
    tokio::time::sleep(PAST_TRANSITION).await;
    let after_release = ok(&router, "CreateFileSystem", &base).await;
    let released_id = after_release["FileSystem"]["FileSystemId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_ne!(
        released_id, id,
        "case release: the token must no longer replay the deleted file system, got \
         {released_id}"
    );
}

/// RULE: the record store is bounded by the service's own published limit on file systems
/// per account per Region, so a create past the maximum is refused with the modelled
/// `ServiceLimitExceeded`, and freeing a file system makes room for exactly one more.
#[tokio::test]
async fn create_is_refused_once_the_file_system_count_limit_is_reached() {
    let router = emulator().await;
    let request = json!({
        "FileSystemType": "WINDOWS",
        "SubnetIds": [router.subnet_a()],
        "StorageCapacity": 32,
    });

    let mut ids: Vec<String> = Vec::new();
    for index in 0..MAX_FILE_SYSTEMS {
        let created = ok(&router, "CreateFileSystem", &with_client_request_token(&request)).await;
        let id = created["FileSystem"]["FileSystemId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_well_formed_file_system_id(&id),
            "case create {index}: expected a well-formed FileSystemId, got {id:?}"
        );
        ids.push(id);
    }

    let (status, code) = failure(
        &router,
        "CreateFileSystem",
        &with_client_request_token(&request),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ServiceLimitExceeded"),
        "case one past the limit of {MAX_FILE_SYSTEMS}: expected 400 ServiceLimitExceeded, \
         got {status} {code}"
    );

    // Freeing one makes room for exactly one more, so the bound is a limit and not a wall.
    assert_eq!(
        settled_lifecycle(&router, &ids[0]).await,
        "AVAILABLE",
        "case make room: the file system must settle to AVAILABLE before it can be deleted"
    );
    ok(
        &router,
        "DeleteFileSystem",
        &json!({ "FileSystemId": ids[0].clone() }),
    )
    .await;
    tokio::time::sleep(PAST_TRANSITION).await;
    let replacement = ok(&router, "CreateFileSystem", &with_client_request_token(&request)).await;
    assert!(
        replacement["FileSystem"]["FileSystemId"].is_string(),
        "case make room: expected a create to succeed once a file system was removed, got \
         {replacement}"
    );
    let (status, code) = failure(
        &router,
        "CreateFileSystem",
        &with_client_request_token(&request),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ServiceLimitExceeded"),
        "case full again: expected 400 ServiceLimitExceeded, got {status} {code}"
    );
}

/// RULE: a route-table merge is bounded by the modelled `RouteTableIds` maximum, the
/// merge is a set (adding the same id twice stores it once), and a refused update leaves
/// the record exactly as it was.
#[tokio::test]
async fn route_table_ids_merge_as_a_bounded_set() {
    let router = emulator().await;
    let created = ok(
        &router,
        "CreateFileSystem",
        &json!({
            "FileSystemType": "ONTAP",
            "ClientRequestToken": "route-table-merge",
            "SubnetIds": [router.subnet_a()],
            "StorageCapacity": 1024,
            "OntapConfiguration": {
                "DeploymentType": "SINGLE_AZ_1",
                "ThroughputCapacity": 128,
                "RouteTableIds": [route_table_id(0)],
            },
        }),
    )
    .await;
    let id = created["FileSystem"]["FileSystemId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        settled_lifecycle(&router, &id).await,
        "AVAILABLE",
        "the file system must settle to AVAILABLE before it can be updated"
    );

    // Adding ids merges them as a set: the already-stored id is not duplicated.
    ok(
        &router,
        "UpdateFileSystem",
        &json!({
            "FileSystemId": id,
            "OntapConfiguration": {
                "AddRouteTableIds": [route_table_id(0), route_table_id(1), route_table_id(1)],
            },
        }),
    )
    .await;
    let stored = route_table_ids(&describe_one(&router, &id).await);
    assert_eq!(
        stored,
        vec![route_table_id(0), route_table_id(1)],
        "case add with a duplicate: expected the ids stored once each, got {stored:?}"
    );

    // A merge that would exceed the modelled maximum is refused, and stores nothing.
    assert_eq!(
        settled_lifecycle(&router, &id).await,
        "AVAILABLE",
        "the file system must settle to AVAILABLE before the next update"
    );
    let too_many: Vec<String> = (2..=MAX_ROUTE_TABLE_IDS).map(route_table_id).collect();
    let (status, code) = failure(
        &router,
        "UpdateFileSystem",
        &json!({
            "FileSystemId": id,
            "OntapConfiguration": { "AddRouteTableIds": too_many },
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "BadRequest"),
        "case 2 stored + {} new route tables: expected 400 BadRequest for exceeding the \
         modelled maximum of {MAX_ROUTE_TABLE_IDS}, got {status} {code}",
        MAX_ROUTE_TABLE_IDS - 1
    );
    let after_refusal = describe_one(&router, &id).await;
    assert_eq!(
        route_table_ids(&after_refusal),
        vec![route_table_id(0), route_table_id(1)],
        "case refused merge: the stored route tables must be unchanged"
    );
    assert_eq!(
        after_refusal["Lifecycle"], json!("AVAILABLE"),
        "case refused merge: a refused update must not move the lifecycle, got {:?}",
        after_refusal["Lifecycle"]
    );

    // Exactly at the maximum is accepted, and removal takes only what it names.
    let to_the_limit: Vec<String> = (2..MAX_ROUTE_TABLE_IDS).map(route_table_id).collect();
    ok(
        &router,
        "UpdateFileSystem",
        &json!({
            "FileSystemId": id,
            "OntapConfiguration": { "AddRouteTableIds": to_the_limit },
        }),
    )
    .await;
    assert_eq!(
        route_table_ids(&describe_one(&router, &id).await).len(),
        MAX_ROUTE_TABLE_IDS,
        "case at the maximum: expected exactly {MAX_ROUTE_TABLE_IDS} stored route tables"
    );
    assert_eq!(
        settled_lifecycle(&router, &id).await,
        "AVAILABLE",
        "the file system must settle to AVAILABLE before the next update"
    );
    ok(
        &router,
        "UpdateFileSystem",
        &json!({
            "FileSystemId": id,
            "OntapConfiguration": { "RemoveRouteTableIds": [route_table_id(0)] },
        }),
    )
    .await;
    let after_removal = route_table_ids(&describe_one(&router, &id).await);
    assert_eq!(
        after_removal.len(),
        MAX_ROUTE_TABLE_IDS - 1,
        "case remove one: expected one fewer stored route table, got {}",
        after_removal.len()
    );
    assert!(
        !after_removal.contains(&route_table_id(0)),
        "case remove one: the removed id must be gone, got {after_removal:?}"
    );
}

/// RULE: a create request that satisfies every modelled constraint but names no subnet is
/// refused with the modelled network error, not accepted and not answered with a 501.
#[tokio::test]
async fn create_without_a_subnet_is_refused() {
    let router = emulator().await;
    let cases: &[Value] = &[
        json!({ "FileSystemType": "WINDOWS", "SubnetIds": [], "StorageCapacity": 32 }),
        json!({ "FileSystemType": "LUSTRE", "SubnetIds": [] }),
        json!({ "FileSystemType": "ONTAP", "SubnetIds": [], "StorageType": "SSD" }),
    ];
    for case in cases {
        let (status, code) =
            failure(&router, "CreateFileSystem", &with_client_request_token(case)).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidNetworkSettings"),
            "case {case}: expected 400 InvalidNetworkSettings, got {status} {code}"
        );
    }

    // And nothing was stored by a refused call.
    let all = ok(&router, "DescribeFileSystems", &json!({})).await;
    assert_eq!(
        all["FileSystems"], json!([]),
        "a refused create must store nothing, got {:?}",
        all["FileSystems"]
    );
}
