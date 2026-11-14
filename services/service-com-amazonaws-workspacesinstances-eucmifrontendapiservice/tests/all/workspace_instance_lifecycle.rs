// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the WorkSpace Instance lifecycle plus tagging, driven over
//! the wire this service really speaks: awsJson1_0, `POST /`, with
//! `X-Amz-Target: EUCMIFrontendAPIService.<OperationName>` and the SigV4 signing
//! name `workspaces-instances` in the credential scope.
//!
//! Every assertion goes through the router, so nothing here can pass against a
//! handler that returns a canned value: an id must come back out of the store
//! through `GetWorkspaceInstance` / `ListWorkspaceInstances` / `ListTagsForResource`
//! before the case counts.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use service_com_amazonaws_workspacesinstances_eucmifrontendapiservice as wsinst;
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "EUCMIFrontendAPIService";

/// The VPC of the account's own Amazon EC2 that the launch fixtures of this suite — and
/// of its siblings — are placed in, and the zone its subnets sit in: the first zone of
/// [`DEFAULT_TEST_REGION`], which is the zone a launch that names none lands in.
const ACCOUNT_VPC: &str = "vpc-0123456789abcdef0";
const ACCOUNT_VPC_CIDR: &str = "10.0.0.0/16";
pub(crate) const ACCOUNT_ZONE: &str = "us-east-1a";

/// The subnets the launch fixtures of this crate's suites name, held in the account's own
/// Amazon EC2 so the launch a create makes is placed in a subnet the account really has.
pub(crate) const ACCOUNT_SUBNETS: &[&str] =
    &["subnet-abc123", "subnet-0a1b2c3d", "subnet-0e1f2a3b"];

/// The security groups those fixtures name, of [`ACCOUNT_VPC`], for the same reason.
pub(crate) const ACCOUNT_SECURITY_GROUPS: &[&str] = &[
    "sg-111111",
    "sg-222222",
    "sg-0000000a",
    "sg-0000000b",
    "sg-00000001",
    "sg-00000002",
];

/// The AMIs the launch fixtures of this crate's suites boot from, held in the account's
/// own Amazon EC2 for the same reason the subnets are: `CreateWorkspaceInstance` resolves
/// `ManagedInstance.ImageId` against the images the account can see and refuses an id that
/// names none, so a fixture that launches names an image the account really has.
///
/// Each id is `ami-` followed by 17 lowercase hexadecimal digits — EC2's long id form,
/// which is what `ec2:DescribeImages` reads an AMI id as.
pub(crate) const ACCOUNT_IMAGES: &[&str] = &[
    TEST_IMAGE_ID,
    "ami-00112233445566778",
    "ami-0a1b2c3d4e5f60718",
    "ami-0deadbeefdeadbeef",
    "ami-0fedcba9876543210",
    "ami-01111111111111111",
    "ami-02222222222222222",
];

/// Write [`ACCOUNT_VPC`], [`ACCOUNT_SUBNETS`], [`ACCOUNT_SECURITY_GROUPS`] and
/// [`ACCOUNT_IMAGES`] into the account's own Amazon EC2 store, at those ids.
///
/// `CreateWorkspaceInstance` launches the managed EC2 instance in the caller's own
/// network from an image of the caller's own account, so a fixture that names a subnet, a
/// security group or an image names one the account has. Written through EC2's own store
/// at CHOSEN ids rather than through EC2's API, because `ec2:CreateSubnet` and
/// `ec2:RegisterImage` mint the id they choose and these fixtures name specific ones.
///
/// `pub(crate)` because the suites that build a router of their own (`region_catalog`,
/// `instance_type_catalog`, `validation_messages`, `wire_error_responses`) seed the same
/// account through it rather than restating any of it.
pub(crate) fn seed_account_network(engine: &Arc<engine::InMemoryEngine>) {
    let store = ec2::state::service_state(&**engine)
        .expect("the in-memory engine carries the account's EC2 store");
    let mut store = store.lock();
    store.vpcs.insert(
        ACCOUNT_VPC.to_string(),
        ec2::ops::types::Vpc {
            vpc_id: Some(ACCOUNT_VPC.to_string()),
            state: Some(ec2::state::VPC_STATE_AVAILABLE.to_string()),
            cidr_block: Some(ACCOUNT_VPC_CIDR.to_string()),
            ..Default::default()
        },
    );
    // Amazon EC2 is regional, so a VPC written without a Region is in no Region and its
    // own describes report neither it nor the subnets and groups of it.
    store
        .vpc_regions
        .insert(ACCOUNT_VPC.to_string(), DEFAULT_TEST_REGION.to_string());
    for (index, subnet_id) in ACCOUNT_SUBNETS.iter().enumerate() {
        store.subnets.insert(
            (*subnet_id).to_string(),
            ec2::ops::types::Subnet {
                subnet_id: Some((*subnet_id).to_string()),
                vpc_id: Some(ACCOUNT_VPC.to_string()),
                state: Some(ec2::state::SUBNET_STATE_AVAILABLE.to_string()),
                // A distinct /24 of the VPC's block per subnet: two subnets of one VPC
                // cannot overlap, and an instance launched into one draws its private
                // address from that block.
                cidr_block: Some(format!("10.0.{index}.0/24")),
                availability_zone: Some(ACCOUNT_ZONE.to_string()),
                availability_zone_id: ec2::state::zone_id_for_zone_name(ACCOUNT_ZONE),
                ..Default::default()
            },
        );
    }
    for group_id in ACCOUNT_SECURITY_GROUPS {
        store.security_groups.insert(
            (*group_id).to_string(),
            ec2::ops::types::SecurityGroup {
                group_id: Some((*group_id).to_string()),
                vpc_id: Some(ACCOUNT_VPC.to_string()),
                ..Default::default()
            },
        );
    }
    // The images those launches boot from, owned by this account so the account's own
    // `ec2:DescribeImages` reports them. Written at CHOSEN ids for the same reason the
    // subnets are: `ec2:RegisterImage` mints the id it chooses, and these fixtures name
    // specific ones.
    for image_id in ACCOUNT_IMAGES {
        store.images.insert(
            (*image_id).to_string(),
            ec2::ops::types::Image {
                image_id: Some((*image_id).to_string()),
                owner_id: Some(engine::emulator_account_id().to_string()),
                state: Some(ec2::state::IMAGE_STATE_AVAILABLE.to_string()),
                ..Default::default()
            },
        );
    }
}

/// A router carrying this service and the service that OWNS the EC2 instance a WorkSpace
/// Instance is, over one shared engine — the same `EmulatedService::new(engine)` seam
/// `core/router` mounts both through. The engine is returned alongside so a test can read
/// the stored record directly where the wire cannot show what it needs (the clock deadline
/// on a record, or that a refused request stored nothing).
///
/// Amazon EC2 is mounted because `CreateWorkspaceInstance` launches a real instance there
/// and `DeleteWorkspaceInstance` terminates it: without the owner of instances beside it,
/// no launch could be made at all.
pub(crate) fn harness() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are the emulator's concern, not this service's; the tests drive
    // unsigned requests whose credential scope still names the service.
    router.set_request_authentication(false);
    router.register_service(wsinst::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(ec2::EmulatedService::new(Arc::clone(&engine)));
    seed_account_network(&engine);
    (router, engine)
}

/// [`harness`] when the test needs only the router.
///
/// `pub(crate)` with the wire helpers below: the sibling `volume_lifecycle` suite
/// drives the same router over the same wire rather than rebuilding a harness.
pub(crate) fn router() -> MultiServiceRouter {
    harness().0
}

/// The Region the wire helpers sign for unless a test asks for another one.
pub(crate) const DEFAULT_TEST_REGION: &str = "us-east-1";

/// One awsJson1_0 call. Returns the HTTP status and the parsed JSON body.
pub(crate) async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    call_in_region(router, DEFAULT_TEST_REGION, operation, body).await
}

/// [`call`], signed for `region`: the Region a request belongs to travels in its
/// SigV4 credential scope, so this is how a test drives the same operation in two
/// Regions.
pub(crate) async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("workspaces-instances.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/workspaces-instances/aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let payload = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or_else(|e| {
            panic!(
                "{operation} answered a body that is not JSON ({e}): {}",
                String::from_utf8_lossy(&raw)
            )
        })
    };
    (status, payload)
}

/// `call`, asserting a 200.
pub(crate) async fn ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation}: expected HTTP 200, got {status} with {payload}"
    );
    payload
}

/// The awsJson error code: the `__type` discriminator in the error envelope.
pub(crate) fn error_code(payload: &Value) -> String {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// `WorkspaceInstanceId`'s modelled shape: `len 15..70`, `^wsinst-[0-9a-zA-Z]{8,63}$`
/// (`smithy_query(query="shape",shape="WorkspaceInstanceId")`).
fn is_modelled_instance_id(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix("wsinst-") else {
        return false;
    };
    (15..=70).contains(&id.len())
        && (8..=63).contains(&suffix.len())
        && !suffix.is_empty()
        && suffix.chars().all(|c| c.is_ascii_alphanumeric())
}

/// `CreateWorkspaceInstance`, returning the minted id.
pub(crate) async fn create(router: &MultiServiceRouter, body: Value) -> String {
    let payload = ok(router, "CreateWorkspaceInstance", body).await;
    payload
        .get("WorkspaceInstanceId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("CreateWorkspaceInstance returned no WorkspaceInstanceId: {payload}"))
        .to_string()
}

/// The `ImageId` every fixture here boots from: inside the modelled
/// `^ami-[0-9a-zA-Z]{1,63}$` (`smithy_query(query="shape",shape="ImageId")`).
pub(crate) const TEST_IMAGE_ID: &str = "ami-0123456789abcdef0";

/// The instance type every fixture here launches: one row of the catalog this
/// service publishes through `ListInstanceTypes`.
pub(crate) const TEST_INSTANCE_TYPE: &str = "t3.medium";

/// The smallest `ManagedInstance` this service can launch: the image the instance
/// boots and the published instance type it runs on, which a launch cannot be made
/// without.
pub(crate) fn launchable_managed_instance() -> Value {
    json!({ "ImageId": TEST_IMAGE_ID, "InstanceType": TEST_INSTANCE_TYPE })
}

/// A minimal valid create body (`ManagedInstance` is the only required member, and
/// it has to describe a launch this service can carry out).
pub(crate) fn minimal_create() -> Value {
    json!({ "ManagedInstance": launchable_managed_instance() })
}

/// The tags `ListTagsForResource` reports, as a map.
pub(crate) async fn stored_tags(
    router: &MultiServiceRouter,
    id: &str,
) -> BTreeMap<String, String> {
    let payload = ok(
        router,
        "ListTagsForResource",
        json!({ "WorkspaceInstanceId": id }),
    )
    .await;
    payload
        .get("Tags")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ListTagsForResource returned no Tags: {payload}"))
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Every `WorkspaceInstanceId` `ListWorkspaceInstances` reports, paging to the end.
pub(crate) async fn list_all_ids(router: &MultiServiceRouter, mut request: Value) -> Vec<String> {
    let mut ids = Vec::new();
    loop {
        let page = ok(router, "ListWorkspaceInstances", request.clone()).await;
        for instance in page["WorkspaceInstances"]
            .as_array()
            .unwrap_or_else(|| panic!("ListWorkspaceInstances returned no WorkspaceInstances: {page}"))
        {
            ids.push(instance["WorkspaceInstanceId"].as_str().unwrap_or_default().to_string());
        }
        match page.get("NextToken").and_then(Value::as_str) {
            Some(token) => request["NextToken"] = json!(token),
            None => return ids,
        }
    }
}

/// A `ManagedInstanceRequest` per row, spanning absent / flat / nested / list-valued
/// configuration, each with its own tag set and billing choice. The stored record —
/// not the request — is what every assertion reads back.
struct CreateRow {
    case: &'static str,
    managed_instance: Value,
    tags: Vec<(String, String)>,
    /// The `BillingConfiguration.BillingMode` the create call sends, if any.
    billing_mode: Option<&'static str>,
    /// The ACTIVE billing mode the instance must report afterwards.
    expected_billing_mode: &'static str,
}

fn create_rows() -> Vec<CreateRow> {
    let fifty_tags: Vec<(String, String)> = (0..50)
        .map(|i| (format!("key-{i:02}"), format!("value-{i:02}")))
        .collect();
    vec![
        CreateRow {
            case: "only the members a launch cannot be made without",
            managed_instance: launchable_managed_instance(),
            tags: vec![],
            billing_mode: None,
            // The model documents hourly billing as the default when no
            // BillingConfiguration is supplied.
            expected_billing_mode: "HOURLY",
        },
        CreateRow {
            case: "flat members",
            managed_instance: json!({ "ImageId": "ami-00112233445566778", "InstanceType": "t3.medium", "SubnetId": ACCOUNT_SUBNETS[0] }),
            tags: vec![("env".to_string(), "dev".to_string())],
            billing_mode: Some("HOURLY"),
            expected_billing_mode: "HOURLY",
        },
        CreateRow {
            case: "nested structures",
            managed_instance: json!({
                "ImageId": "ami-0a1b2c3d4e5f60718",
                "InstanceType": "m5.large",
                "Placement": { "AvailabilityZone": "us-east-1a", "Tenancy": "default" },
                "CpuOptions": { "CoreCount": 2, "ThreadsPerCore": 2 },
                "MetadataOptions": { "HttpTokens": "required", "HttpPutResponseHopLimit": 2 }
            }),
            tags: vec![
                ("env".to_string(), "prod".to_string()),
                ("owner".to_string(), "alice".to_string()),
            ],
            billing_mode: Some("MONTHLY"),
            expected_billing_mode: "MONTHLY",
        },
        CreateRow {
            case: "list-valued members",
            // A launch that names its network interfaces names its subnet and its
            // security groups ON them — "if you specify a network interface, you must
            // specify any subnets as part of the network interface instead of using this
            // parameter" (Amazon EC2 `RunInstances$SubnetId`, `$SecurityGroupIds`) — so
            // the lists here are the interface's, and both name resources the account has.
            managed_instance: json!({
                "ImageId": "ami-0deadbeefdeadbeef",
                "InstanceType": "c5.large",
                "BlockDeviceMappings": [
                    { "DeviceName": "/dev/sda1", "Ebs": { "VolumeType": "gp3", "VolumeSize": 100 } }
                ],
                "NetworkInterfaces": [
                    {
                        "DeviceIndex": 0,
                        "SubnetId": ACCOUNT_SUBNETS[0],
                        "Groups": [ACCOUNT_SECURITY_GROUPS[0], ACCOUNT_SECURITY_GROUPS[1]],
                    }
                ]
            }),
            tags: vec![],
            billing_mode: Some("MONTHLY"),
            expected_billing_mode: "MONTHLY",
        },
        CreateRow {
            case: "the modelled maximum of 50 tags",
            managed_instance: json!({ "ImageId": TEST_IMAGE_ID, "InstanceType": "t3.small" }),
            tags: fifty_tags,
            billing_mode: None,
            expected_billing_mode: "HOURLY",
        },
    ]
}

fn tag_list(tags: &[(String, String)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

#[tokio::test]
async fn create_mints_a_persisted_id_every_later_read_resolves() {
    let router = router();
    let mut minted: Vec<String> = Vec::new();

    for row in create_rows() {
        let CreateRow {
            case,
            managed_instance,
            tags,
            billing_mode,
            expected_billing_mode,
        } = row;
        let mut body = json!({ "ManagedInstance": managed_instance, "Tags": tag_list(&tags) });
        if let Some(mode) = billing_mode {
            body["BillingConfiguration"] = json!({ "BillingMode": mode });
        }

        let id = create(&router, body).await;
        assert!(
            is_modelled_instance_id(&id),
            "case {case:?}: expected an id matching ^wsinst-[0-9a-zA-Z]{{8,63}}$ of length 15..70, got {id:?}"
        );
        assert!(
            !minted.contains(&id),
            "case {case:?}: expected a fresh id, got the already-minted {id:?}"
        );

        // The round trip: the create must have STORED the record, not just minted an id.
        let got = ok(
            &router,
            "GetWorkspaceInstance",
            json!({ "WorkspaceInstanceId": id }),
        )
        .await;
        assert_eq!(
            got["WorkspaceInstanceId"].as_str(),
            Some(id.as_str()),
            "case {case:?}: expected the stored id {id:?}, got {got}"
        );
        let ec2 = got["EC2ManagedInstance"]["InstanceId"].as_str().unwrap_or_default();
        assert!(
            ec2.starts_with("i-") && ec2.len() > 2,
            "case {case:?}: expected a minted EC2 instance id, got {ec2:?}"
        );
        assert_eq!(
            got["BillingConfiguration"]["BillingMode"].as_str(),
            Some(expected_billing_mode),
            "case {case:?}: expected BillingMode {expected_billing_mode:?}, got {got}"
        );

        let expected_tags: BTreeMap<String, String> = tags.iter().cloned().collect();
        let actual_tags = stored_tags(&router, &id).await;
        assert_eq!(
            actual_tags, expected_tags,
            "case {case:?}: expected the persisted tags {expected_tags:?}, got {actual_tags:?}"
        );

        minted.push(id);
    }

    let listed = list_all_ids(&router, json!({})).await;
    let mut expected = minted.clone();
    expected.sort();
    let mut actual = listed.clone();
    actual.sort();
    assert_eq!(
        actual, expected,
        "case list-after-create: expected every created instance, got {listed:?}"
    );
}

#[tokio::test]
async fn a_spent_idempotency_token_creates_nothing_more() {
    let router = router();
    // `ClientToken`: len 1..64, pattern ^[\x20-\x7E]+$ (printable ASCII).
    let long = "t".repeat(64);
    let tokens: Vec<&str> = vec!["tok-1", "a", &long, "token with spaces ~!@#$%^&*()"];

    for token in &tokens {
        let body = json!({
            "ClientToken": token,
            "ManagedInstance": launchable_managed_instance(),
            "Tags": [{ "Key": "env", "Value": "dev" }]
        });
        let first = create(&router, body.clone()).await;
        // The create SPENT that token, so re-submitting the identical request under it
        // is refused rather than answered with the first call's id — however many times
        // it arrives — and no second instance exists for it.
        for attempt in 1..=2 {
            let (status, payload) = call(&router, "CreateWorkspaceInstance", body.clone()).await;
            assert_eq!(
                status, 409,
                "case token {token:?} (attempt {attempt}): expected HTTP 409 for a spent \
                 token, got {status} with {payload}"
            );
            assert_eq!(
                error_code(&payload),
                "ConflictException",
                "case token {token:?} (attempt {attempt}): expected ConflictException, got {payload}"
            );
            assert_eq!(
                payload["ResourceId"].as_str(),
                Some(first.as_str()),
                "case token {token:?} (attempt {attempt}): expected the conflict to name \
                 {first:?}, got {payload}"
            );
        }
    }

    let ids = list_all_ids(&router, json!({})).await;
    assert_eq!(
        ids.len(),
        tokens.len(),
        "case spent-token-count: expected exactly one instance per distinct token ({}), got {ids:?}",
        tokens.len()
    );

    // A token this service has not seen creates a NEW instance.
    let fresh = create(
        &router,
        json!({ "ClientToken": "tok-fresh", "ManagedInstance": launchable_managed_instance() }),
    )
    .await;
    assert!(
        !ids.contains(&fresh),
        "case unseen-token: expected a new instance, got the existing {fresh:?}"
    );
}

#[tokio::test]
async fn pagination_round_trips_the_whole_set_at_every_page_size() {
    let router = router();
    let mut created = Vec::new();
    for _ in 0..7 {
        created.push(create(&router, minimal_create()).await);
    }
    created.sort();

    // `MaxResults` is modelled `range 1..25`: both published bounds, sizes below,
    // at and above the set size, and the absent case (which uses the modelled max).
    for page_size in [None, Some(1), Some(2), Some(3), Some(6), Some(7), Some(25)] {
        let mut request = json!({});
        if let Some(size) = page_size {
            request["MaxResults"] = json!(size);
        }

        // Walk the pages by hand so the per-page size and the token can be asserted.
        let mut seen: Vec<String> = Vec::new();
        let mut pages = 0;
        loop {
            let page = ok(&router, "ListWorkspaceInstances", request.clone()).await;
            let items = page["WorkspaceInstances"].as_array().cloned().unwrap_or_default();
            pages += 1;
            if let Some(size) = page_size {
                assert!(
                    items.len() <= size as usize,
                    "case MaxResults={page_size:?}: page {pages} carried {} items, more than the requested {size}",
                    items.len()
                );
            }
            for item in &items {
                let id = item["WorkspaceInstanceId"].as_str().unwrap_or_default().to_string();
                assert!(
                    !seen.contains(&id),
                    "case MaxResults={page_size:?}: {id:?} appeared on two pages"
                );
                seen.push(id);
            }
            match page.get("NextToken").and_then(Value::as_str) {
                Some(token) => {
                    assert!(
                        !token.is_empty() && token.len() <= 2048,
                        "case MaxResults={page_size:?}: NextToken {token:?} is outside the modelled len 1..2048"
                    );
                    request["NextToken"] = json!(token);
                }
                None => break,
            }
            assert!(
                pages <= 20,
                "case MaxResults={page_size:?}: pagination did not terminate"
            );
        }

        seen.sort();
        assert_eq!(
            seen, created,
            "case MaxResults={page_size:?}: expected every instance exactly once, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn max_results_outside_the_modelled_range_is_refused() {
    let router = router();
    create(&router, minimal_create()).await;

    // `range 1..25`: each published bound is accepted, one past each is refused.
    for (case, max_results, expected_status) in [
        ("at the minimum", 1, 200),
        ("at the maximum", 25, 200),
        ("below the minimum", 0, 400),
        ("above the maximum", 26, 400),
    ] {
        let (status, payload) = call(
            &router,
            "ListWorkspaceInstances",
            json!({ "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?} (MaxResults={max_results}): expected HTTP {expected_status}, got {status} with {payload}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&payload),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {payload}"
            );
        }
    }
}

/// The `ProvisionState` `GetWorkspaceInstance` reports for one id.
pub(crate) async fn provision_state(router: &MultiServiceRouter, id: &str) -> String {
    let got = ok(
        router,
        "GetWorkspaceInstance",
        json!({ "WorkspaceInstanceId": id }),
    )
    .await;
    got["ProvisionState"]
        .as_str()
        .unwrap_or_else(|| panic!("GetWorkspaceInstance returned no ProvisionState: {got}"))
        .to_string()
}

#[tokio::test]
async fn provision_state_is_the_transient_allocating_until_the_span_is_due() {
    // Two assertions, neither of which races the wall clock:
    //  1. the deadline the create stored is at least one whole ALLOCATION_SPAN after
    //     the call, so the transition is never instantaneous; and
    //  2. whenever the clock says the deadline has NOT passed — checked against an
    //     instant taken AFTER the Get returned, so it bounds the instant the handler
    //     itself read — the wire reported the transient ALLOCATING.
    let (router, engine) = harness();
    let before_create = std::time::Instant::now();
    let id = create(&router, minimal_create()).await;

    let store = wsinst::state::service_state(&*engine).expect("the service's own store");
    let allocation_due = store
        .lock()
        .instances
        .get(&id)
        .expect("the create must have stored the record")
        .allocation_due;
    assert!(
        allocation_due >= before_create + wsinst::state::ALLOCATION_SPAN,
        "case the stored deadline: expected the ALLOCATED transition to fall due no sooner than one span after the call, got a deadline {:?} earlier",
        (before_create + wsinst::state::ALLOCATION_SPAN).saturating_duration_since(allocation_due)
    );

    let before_get = std::time::Instant::now();
    let state = provision_state(&router, &id).await;
    let after_get = std::time::Instant::now();
    // The handler read the record at some instant in [before_get, after_get], so only
    // these two implications are sound; inside the window either answer is correct.
    if after_get < allocation_due {
        assert_eq!(
            state, "ALLOCATING",
            "case read wholly before the deadline: expected the transient ALLOCATING, got {state:?}"
        );
    } else if before_get >= allocation_due {
        assert_eq!(
            state, "ALLOCATED",
            "case read wholly at or after the deadline: expected ALLOCATED, got {state:?}"
        );
    } else {
        assert!(
            state == "ALLOCATING" || state == "ALLOCATED",
            "case read across the deadline: expected ALLOCATING or ALLOCATED, got {state:?}"
        );
    }
}

#[tokio::test]
async fn provision_state_reaches_allocated_once_the_span_is_due_and_stays_there() {
    let router = router();
    let id = create(&router, minimal_create()).await;

    // Past the due instant the transition has been made: the clock owed it, so it
    // shows up without anybody calling anything in between.
    tokio::time::sleep(wsinst::state::ALLOCATION_SPAN + std::time::Duration::from_millis(250))
        .await;

    // And ALLOCATED is terminal: repeated reads do not move it on again.
    for read in 1..=3 {
        let state = provision_state(&router, &id).await;
        assert_eq!(
            state, "ALLOCATED",
            "case read {read} after the span elapsed: expected ALLOCATED, got {state:?}"
        );
    }
}

#[tokio::test]
async fn provision_states_filters_on_the_stored_state() {
    let router = router();
    let mut created = Vec::new();
    for _ in 0..3 {
        created.push(create(&router, minimal_create()).await);
    }
    created.sort();

    // Let the ALLOCATING -> ALLOCATED transition fall due first, so the sweep below
    // reads a TERMINAL state and cannot race the clock between its own requests.
    tokio::time::sleep(wsinst::state::ALLOCATION_SPAN + std::time::Duration::from_millis(250))
        .await;

    // Every `ProvisionStateEnum` value (ALLOCATING | ALLOCATED | DEALLOCATING |
    // DEALLOCATED | ERROR_ALLOCATING | ERROR_DEALLOCATING), plus a superset, a
    // disjoint set, the empty list and the absent member.
    for (case, request, expect_all) in [
        ("no filter at all", json!({}), true),
        (
            "the stored state",
            json!({ "ProvisionStates": ["ALLOCATED"] }),
            true,
        ),
        (
            "a superset of it",
            json!({ "ProvisionStates": ["ALLOCATING", "ALLOCATED"] }),
            true,
        ),
        (
            "the state it came from",
            json!({ "ProvisionStates": ["ALLOCATING"] }),
            false,
        ),
        (
            "deallocation states",
            json!({ "ProvisionStates": ["DEALLOCATING", "DEALLOCATED"] }),
            false,
        ),
        (
            "error states",
            json!({ "ProvisionStates": ["ERROR_ALLOCATING", "ERROR_DEALLOCATING"] }),
            false,
        ),
        ("an empty filter list", json!({ "ProvisionStates": [] }), true),
    ] {
        let mut ids = list_all_ids(&router, request).await;
        ids.sort();
        let expected: Vec<String> = if expect_all { created.clone() } else { vec![] };
        assert_eq!(
            ids, expected,
            "case filter {case:?}: expected {expected:?}, got {ids:?}"
        );
    }
}

#[tokio::test]
async fn the_instance_quota_is_bounded_and_released_by_delete() {
    let router = router();
    let quota = wsinst::state::MAX_INSTANCES;

    let mut created = Vec::new();
    for n in 1..=quota {
        let (status, payload) = call(&router, "CreateWorkspaceInstance", minimal_create()).await;
        assert_eq!(
            status, 200,
            "case instance {n} of the quota {quota}: expected HTTP 200, got {status} with {payload}"
        );
        created.push(payload["WorkspaceInstanceId"].as_str().unwrap_or_default().to_string());
    }

    // One past the quota is the modelled ServiceQuotaExceededException, not a 500 and
    // not an unbounded store.
    let (status, payload) = call(&router, "CreateWorkspaceInstance", minimal_create()).await;
    assert_eq!(
        status, 402,
        "case one past the quota: expected HTTP 402, got {status} with {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ServiceQuotaExceededException",
        "case one past the quota: expected ServiceQuotaExceededException, got {payload}"
    );
    let quota_code = payload["QuotaCode"].as_str().unwrap_or_default();
    assert!(
        quota_code.starts_with("L-") && quota_code.len() > 2,
        "case one past the quota: expected an L-… Service Quotas code, got {quota_code:?} in {payload}"
    );
    assert_eq!(
        payload["ServiceCode"].as_str(),
        Some("workspaces-instances"),
        "case one past the quota: expected the service's own code, got {payload}"
    );

    // Deleting releases capacity: the quota is on RUNNING instances, and the delete
    // moves this one into teardown, so the slot is free even though the record is
    // still reported for a while.
    ok(
        &router,
        "DeleteWorkspaceInstance",
        json!({ "WorkspaceInstanceId": created[0] }),
    )
    .await;
    let (status, payload) = call(&router, "CreateWorkspaceInstance", minimal_create()).await;
    assert_eq!(
        status, 200,
        "case create after a delete freed capacity: expected HTTP 200, got {status} with {payload}"
    );
}

#[tokio::test]
async fn tag_resource_merges_onto_the_stored_tags() {
    let router = router();
    let id = create(
        &router,
        json!({
            "ManagedInstance": launchable_managed_instance(),
            "Tags": [{ "Key": "env", "Value": "dev" }],
        }),
    )
    .await;

    let mut expected: BTreeMap<String, String> =
        [("env".to_string(), "dev".to_string())].into_iter().collect();

    // `Tag.Key` len 1..128 / `Tag.Value` len 0..256, both over
    // ^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$ — so letters of any script, digits, and that
    // punctuation set are all legal keys and values.
    for (case, tags) in [
        ("a new key", vec![("owner", "alice")]),
        ("the same key again", vec![("env", "prod")]),
        ("an empty value", vec![("cost-centre", "")]),
        ("non-ASCII letters", vec![("Ünïcode.Key", "valeur/accentuée")]),
        ("the full punctuation set", vec![("a_b.c:d/e=f+g-h@i", "j_k.l:m/n=o+p-q@r")]),
        ("several keys at once", vec![("team", "euc"), ("tier", "gold")]),
        ("an empty list", vec![]),
    ] {
        let body = json!({
            "WorkspaceInstanceId": id,
            "Tags": tags.iter().map(|(k, v)| json!({ "Key": k, "Value": v })).collect::<Vec<_>>()
        });
        ok(&router, "TagResource", body).await;
        for (key, value) in tags {
            expected.insert(key.to_string(), value.to_string());
        }

        let actual = stored_tags(&router, &id).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the merged tags {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn the_stored_tag_set_cannot_grow_past_what_the_model_can_report() {
    let router = router();
    let limit = wsinst::state::MAX_TAGS_PER_INSTANCE;

    // Fill the resource to the modelled `TagList len 0..50` at create time.
    let full: Vec<(String, String)> = (0..limit)
        .map(|i| (format!("key-{i:02}"), format!("value-{i:02}")))
        .collect();
    let id = create(
        &router,
        json!({ "ManagedInstance": launchable_managed_instance(), "Tags": tag_list(&full) }),
    )
    .await;
    let expected: BTreeMap<String, String> = full.iter().cloned().collect();
    assert_eq!(
        stored_tags(&router, &id).await,
        expected,
        "case filled to the limit at create: expected {limit} tags to persist"
    );

    for (case, tags, expected_status) in [
        // Re-tagging an existing key replaces a value; the set does not grow.
        ("overwrite one existing key", vec![("key-00", "replaced")], 200),
        ("overwrite every existing key", full
            .iter()
            .map(|(k, _)| (k.as_str(), "rewritten"))
            .collect::<Vec<_>>(), 200),
        ("an empty list", vec![], 200),
        // One fresh key is one past what the resource can hold.
        ("one key past the limit", vec![("one-too-many", "x")], 400),
        ("several keys past the limit", vec![("a", "1"), ("b", "2")], 400),
        // A mix of an overwrite and a fresh key still grows the set by one.
        (
            "an overwrite plus a fresh key",
            vec![("key-00", "again"), ("fresh", "y")],
            400,
        ),
    ] {
        let body = json!({
            "WorkspaceInstanceId": id,
            "Tags": tags.iter().map(|(k, v)| json!({ "Key": k, "Value": v })).collect::<Vec<_>>()
        });
        let (status, payload) = call(&router, "TagResource", body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {payload}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&payload),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {payload}"
            );
        }

        // Either way the stored set is still at most the modelled maximum, and a
        // refused request stored nothing at all.
        let actual = stored_tags(&router, &id).await;
        assert!(
            actual.len() <= limit,
            "case {case:?}: the stored tag set grew to {} past the limit {limit}",
            actual.len()
        );
        if expected_status == 400 {
            assert!(
                actual.keys().eq(expected.keys()),
                "case {case:?}: a refused request changed the stored keys, got {:?}",
                actual.keys().collect::<Vec<_>>()
            );
        }
    }
}

#[tokio::test]
async fn delete_removes_the_instance_from_every_view_once_the_teardown_completes() {
    let router = router();
    let doomed = create(
        &router,
        json!({
            "ClientToken": "tok-doomed",
            "ManagedInstance": launchable_managed_instance(),
        }),
    )
    .await;
    let survivor = create(&router, minimal_create()).await;

    ok(
        &router,
        "DeleteWorkspaceInstance",
        json!({ "WorkspaceInstanceId": doomed }),
    )
    .await;

    // Deletion is ASYNCHRONOUS teardown, so the record is still reported right after
    // the call (the `instance_deletion_lifecycle` suite spans that window). It stops
    // being reported once the teardown and its retention have both fallen due.
    tokio::time::sleep(
        wsinst::state::DEALLOCATION_SPAN
            + wsinst::state::DEALLOCATED_RETENTION
            + std::time::Duration::from_millis(250),
    )
    .await;

    for operation in [
        "GetWorkspaceInstance",
        "ListTagsForResource",
        "DeleteWorkspaceInstance",
    ] {
        let (status, payload) = call(
            &router,
            operation,
            json!({ "WorkspaceInstanceId": doomed }),
        )
        .await;
        // This service answers `ResourceNotFoundException` with HTTP 400, not the 404
        // its shape declares — see `crate::state::RESOURCE_NOT_FOUND_STATUS`, and the
        // table in `tests/all/wire_error_responses.rs` that spans this rule.
        assert_eq!(
            status, 400,
            "case {operation} after delete: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ResourceNotFoundException",
            "case {operation} after delete: expected ResourceNotFoundException, got {payload}"
        );
    }

    let listed = list_all_ids(&router, json!({})).await;
    assert_eq!(
        listed,
        vec![survivor.clone()],
        "case list-after-delete: expected only {survivor:?}, got {listed:?}"
    );

    // The torn-down record's `ClientToken` was retired with it, so replaying that
    // create does not hand back an id that no longer exists.
    let replayed = create(
        &router,
        json!({
            "ClientToken": "tok-doomed",
            "ManagedInstance": launchable_managed_instance(),
        }),
    )
    .await;
    assert_ne!(
        replayed, doomed,
        "case replay-after-delete: expected a new instance, got the deleted {doomed:?}"
    );
    let got = ok(
        &router,
        "GetWorkspaceInstance",
        json!({ "WorkspaceInstanceId": replayed }),
    )
    .await;
    assert_eq!(
        got["WorkspaceInstanceId"].as_str(),
        Some(replayed.as_str()),
        "case replay-after-delete: expected the new instance to be readable, got {got}"
    );
}

#[tokio::test]
async fn a_well_formed_but_unknown_id_is_not_found() {
    let router = router();
    create(&router, minimal_create()).await;

    // Well-formed per the model, stored by nobody: the shortest legal id, the
    // longest legal id, and a mixed-case one.
    let unknown_ids = [
        "wsinst-00000000".to_string(),
        format!("wsinst-{}", "z".repeat(63)),
        "wsinst-AbCdEf01234".to_string(),
    ];

    for id in &unknown_ids {
        assert!(
            is_modelled_instance_id(id),
            "case {id:?}: the test's own id must satisfy the model"
        );
        for (operation, body) in [
            ("GetWorkspaceInstance", json!({ "WorkspaceInstanceId": id })),
            ("ListTagsForResource", json!({ "WorkspaceInstanceId": id })),
            ("DeleteWorkspaceInstance", json!({ "WorkspaceInstanceId": id })),
            (
                "TagResource",
                json!({ "WorkspaceInstanceId": id, "Tags": [{ "Key": "env", "Value": "dev" }] }),
            ),
        ] {
            let (status, payload) = call(&router, operation, body).await;
            assert_eq!(
                status, 400,
                "case {operation} on {id:?}: expected HTTP 400, got {status} with {payload}"
            );
            assert_eq!(
                error_code(&payload),
                "ResourceNotFoundException",
                "case {operation} on {id:?}: expected ResourceNotFoundException, got {payload}"
            );
            assert_eq!(
                payload["ResourceId"].as_str(),
                Some(id.as_str()),
                "case {operation} on {id:?}: expected the error to name the id, got {payload}"
            );
        }
    }
}

#[tokio::test]
async fn an_id_outside_the_modelled_shape_is_refused() {
    let router = router();

    // Each row breaks exactly one part of `len 15..70` /
    // `^wsinst-[0-9a-zA-Z]{8,63}$`.
    let too_long_suffix = format!("wsinst-{}", "a".repeat(64));
    let malformed: Vec<(&str, &str)> = vec![
        ("no prefix", "bogus"),
        ("prefix only", "wsinst-"),
        ("suffix one short of the minimum", "wsinst-1234567"),
        ("suffix one past the maximum", &too_long_suffix),
        ("a character outside the suffix class", "wsinst-abc_defg"),
        ("the wrong prefix case", "WSINST-12345678"),
        ("a leading space", " wsinst-12345678"),
    ];

    for (case, id) in malformed {
        for operation in [
            "GetWorkspaceInstance",
            "ListTagsForResource",
            "DeleteWorkspaceInstance",
        ] {
            let (status, payload) =
                call(&router, operation, json!({ "WorkspaceInstanceId": id })).await;
            assert_eq!(
                status, 400,
                "case {case:?} ({id:?}) on {operation}: expected HTTP 400, got {status} with {payload}"
            );
            assert_eq!(
                error_code(&payload),
                "ValidationException",
                "case {case:?} ({id:?}) on {operation}: expected ValidationException, got {payload}"
            );
        }
    }
}

#[tokio::test]
async fn an_absent_required_member_is_refused() {
    let router = router();

    for (case, operation, body) in [
        (
            "CreateWorkspaceInstance without ManagedInstance",
            "CreateWorkspaceInstance",
            json!({ "ClientToken": "tok-1" }),
        ),
        (
            "GetWorkspaceInstance without WorkspaceInstanceId",
            "GetWorkspaceInstance",
            json!({}),
        ),
        (
            "DeleteWorkspaceInstance without WorkspaceInstanceId",
            "DeleteWorkspaceInstance",
            json!({}),
        ),
        (
            "ListTagsForResource without WorkspaceInstanceId",
            "ListTagsForResource",
            json!({}),
        ),
        (
            "TagResource without Tags",
            "TagResource",
            json!({ "WorkspaceInstanceId": "wsinst-00000000" }),
        ),
    ] {
        let (status, payload) = call(&router, operation, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {payload}"
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let router = router();
    create(&router, minimal_create()).await;
    create(&router, minimal_create()).await;

    for (case, token) in [
        ("not an encoding at all", "not-a-token".to_string()),
        ("odd-length hex", "abc".to_string()),
        ("hex that is not UTF-8", "ff".to_string()),
        ("a resource id passed through raw", "wsinst-00000000".to_string()),
        // Valid hex of valid UTF-8 that is NOT an instance id: the decoded resume
        // point must itself satisfy the WorkspaceInstanceId shape.
        ("hex of a non-id string", hex::encode("zzzzzzzzzzzzzz")),
        ("hex of an id with the wrong prefix", hex::encode("vol-0123456789")),
        ("hex of an id with too short a suffix", hex::encode("wsinst-1234567")),
    ] {
        let (status, payload) = call(
            &router,
            "ListWorkspaceInstances",
            json!({ "MaxResults": 1, "NextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?} ({token:?}): expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case:?} ({token:?}): expected ValidationException, got {payload}"
        );
    }
}

#[tokio::test]
async fn state_is_shared_across_requests_not_per_call() {
    // Two routers over two engines must not see each other's instances: the state a
    // request writes lives in the engine the service was constructed from.
    let first = router();
    let second = router();

    let id = create(&first, minimal_create()).await;
    let (status, payload) = call(
        &second,
        "GetWorkspaceInstance",
        json!({ "WorkspaceInstanceId": id }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case cross-engine read: expected HTTP 400 ResourceNotFoundException, got \
         {status} with {payload}"
    );
    assert_eq!(
        list_all_ids(&first, json!({})).await,
        vec![id.clone()],
        "case same-engine read: expected {id:?} to survive between requests"
    );
}
