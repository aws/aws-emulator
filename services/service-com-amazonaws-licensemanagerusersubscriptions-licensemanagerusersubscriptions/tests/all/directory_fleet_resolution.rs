// SPDX-License-Identifier: CC0-1.0

//! The directory an identity-provider registration names is resolved against the
//! directories the ACCOUNT HAS, as the plane that creates directories recorded them.
//!
//! The rule under test: `ActiveDirectoryIdentityProvider.DirectoryId` is bounded by nothing
//! but its `@pattern` `^(d|sd)-[0-9a-f]{10}$`
//! (`smithy_query(query="shape",shape="ActiveDirectoryIdentityProvider")`) and is documented
//! as the id of the directory to register — there is no closed set of values — so
//! `RegisterIdentityProvider` mints a registration for ANY directory of this account that is
//! in service, and refuses one the account does not have, has not finished building, or has
//! deleted with the modelled `ValidationException` (HTTP 400) naming the requested id.
//!
//! A directory is AWS Directory Service's resource, so that service is mounted beside this
//! one on ONE engine and every directory a row needs is created and deleted through ITS OWN
//! API (`ds:CreateDirectory`, `ds:CreateMicrosoftAD`, `ds:DeleteDirectory`), at the ids it
//! minted. Nothing here writes a directory into either store: a directory is believed to
//! exist only because the plane that owns it created it, and a registration only because a
//! later `ListIdentityProviders` reports it.
//!
//! The rows differ along what the rule generalises over: which create made the directory (a
//! Simple AD of either published size, a Managed Microsoft AD), whether it came from this
//! service's own fleet seam instead, whether it exists at all, whether it has been deleted
//! again, and the form of an id no directory of the account has.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_directoryservice_directoryservice_20150416 as ds;
use service_com_amazonaws_ec2_amazonec2 as ec2;

// ONE wire harness per service and ONE reply reader: both come from the sibling suite rather
// than a second copy here.
use crate::identity_provider_subscriptions::{
    arn_prefix, as_str, harness, list_providers, provider_union, register, Harness, Reply,
    ACCOUNT_SECURITY_GROUPS, ACCOUNT_SUBNETS, PRODUCTS, REGION,
};

/// The `X-Amz-Target` prefix the Directory Service front door routes on: that service's
/// `awsJson1_1` protocol names the operation in this header.
const DS_TARGET_PREFIX: &str = "DirectoryService_20150416";

/// A password satisfying the `Password` pattern every Directory Service create carries (8-64
/// characters out of three of the four character classes) and that service's own
/// administrator password policy.
const DIRECTORY_PASSWORD: &str = "Passw0rd!2345";

/// The two published `DirectorySize` values a Simple AD create takes
/// (`smithy_query(query="shape",shape="DirectorySize")`).
const SIZE_SMALL: &str = "Small";
const SIZE_LARGE: &str = "Large";

/// The status a fresh registration carries, spelled out here rather than read from the
/// production constant so a change to it fails a test instead of silently changing what this
/// suite accepts.
const REGISTERING: &str = "REGISTERING";

/// A directory this service's OWN fleet seam holds, put there by the shared harness: the
/// other half of the resolution, which is what answers in a process that mounts this service
/// with no Directory Service beside it.
const SEAM_DIRECTORY: &str = "d-0123456789";

/// Well-formed ids NO directory of this account was ever created for, spanning the `d-` form
/// of the member's `@pattern`: hex letters only, digits only, and the lowest value the
/// pattern admits. None of them is a directory this service's seed or its harness holds.
const ABSENT_DIRECTORY_IDS: &[&str] = &["d-abcdefabcd", "d-7654321098", "d-0000000000"];

/// The VPC and the two subnets a Managed Microsoft AD create names, each id of the form that
/// create's `VpcSettings` admits (`^(vpc-[0-9a-f]{8}|vpc-[0-9a-f]{17})$` and the matching
/// `SubnetId` expression).
const MANAGED_AD_VPC: &str = "vpc-0ad00001";
const MANAGED_AD_SUBNETS: &[&str] = &["subnet-0ad00011", "subnet-0ad00012"];

/// The two Availability Zones those subnets are in — zones of [`REGION`], one each, because
/// "the two subnets must be in different Availability Zones" for a directory's servers
/// (`DirectoryVpcSettings$SubnetIds`).
const MANAGED_AD_ZONES: &[&str] = &["us-west-2a", "us-west-2b"];

/// How long a row gives the directory its create started to come into service.
///
/// A create answers while the directory is still being built, so a registration for it is
/// refused until that build is due. DELIBERATELY generous: nothing here asserts how long a
/// build takes — only that the registration is refused while it is outstanding and accepted
/// once it is done.
const BUILD_BUDGET: Duration = Duration::from_secs(60);

/// How long the poll below waits between attempts. It waits for the CONDITION — the
/// directory is in service — and never for a fixed time.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// This service, Amazon EC2 and the owner of directories over ONE shared engine — the same
/// engine the production router builds.
///
/// The first two and the account's network come from the shared harness; Directory Service
/// is mounted on the same engine beside them, which is what gives the `DirectoryId` of a
/// registration an owner to be resolved against.
fn fleet_with_directory_service() -> Harness {
    let mut h = harness();
    h.router
        .register_service(ds::EmulatedService::new(Arc::clone(&h.engine)));
    // The network a Managed Microsoft AD create places its directory servers in: two
    // subnets in two Availability Zones, which that create resolves against the service
    // that owns subnets. Written into Amazon EC2's own store at chosen ids, for the reason
    // the shared harness writes the rest of the account's network there — a create mints
    // the id it chooses, and this create names specific ones.
    let store = ec2::state::service_state(&*h.engine)
        .expect("the in-memory engine carries the account's EC2 store");
    let mut store = store.lock();
    store.vpcs.insert(
        MANAGED_AD_VPC.to_string(),
        ec2::ops::types::Vpc {
            vpc_id: Some(MANAGED_AD_VPC.to_string()),
            state: Some(ec2::state::VPC_STATE_AVAILABLE.to_string()),
            cidr_block: Some("10.7.0.0/16".to_string()),
            ..Default::default()
        },
    );
    store
        .vpc_regions
        .insert(MANAGED_AD_VPC.to_string(), REGION.to_string());
    for (index, subnet_id) in MANAGED_AD_SUBNETS.iter().enumerate() {
        let zone = MANAGED_AD_ZONES[index];
        store.subnets.insert(
            (*subnet_id).to_string(),
            ec2::ops::types::Subnet {
                subnet_id: Some((*subnet_id).to_string()),
                vpc_id: Some(MANAGED_AD_VPC.to_string()),
                state: Some(ec2::state::SUBNET_STATE_AVAILABLE.to_string()),
                cidr_block: Some(format!("10.7.{index}.0/24")),
                availability_zone: Some(zone.to_string()),
                availability_zone_id: ec2::state::zone_id_for_zone_name(zone),
                ..Default::default()
            },
        );
    }
    drop(store);
    h
}

/// One `awsJson1_1` call to the Directory Service front door, as a client makes it: the HTTP
/// status and the parsed body.
async fn ds_call(h: &Harness, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://ds.{REGION}.amazonaws.com/"))
        .header("host", format!("ds.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{DS_TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTEXAMPLE/20260101/{REGION}/ds/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = h.router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The `DirectoryId` a Directory Service create answered with.
fn created_id(case: &str, status: u16, parsed: &Value) -> String {
    assert_eq!(
        status, 200,
        "setup {case:?}: Directory Service must create the directory, got {status} / {parsed}"
    );
    parsed["DirectoryId"]
        .as_str()
        .unwrap_or_else(|| panic!("setup {case:?}: the create answered no DirectoryId: {parsed}"))
        .to_string()
}

/// Create a Simple AD directory of `size` through `ds:CreateDirectory` and answer the id
/// that service minted.
async fn create_simple_ad(h: &Harness, name: &str, size: &str) -> String {
    let body = json!({ "Name": name, "Password": DIRECTORY_PASSWORD, "Size": size });
    let (status, parsed) = ds_call(h, "CreateDirectory", &body).await;
    created_id(name, status, &parsed)
}

/// Create a Managed Microsoft AD directory through `ds:CreateMicrosoftAD` and answer the id
/// that service minted — the documented setup flow a user-based subscription follows.
async fn create_microsoft_ad(h: &Harness, name: &str) -> String {
    let body = json!({
        "Name": name,
        "Password": DIRECTORY_PASSWORD,
        "VpcSettings": { "VpcId": MANAGED_AD_VPC, "SubnetIds": MANAGED_AD_SUBNETS },
    });
    let (status, parsed) = ds_call(h, "CreateMicrosoftAD", &body).await;
    created_id(name, status, &parsed)
}

/// Delete a directory through `ds:DeleteDirectory`.
async fn delete_directory(h: &Harness, directory_id: &str) {
    let (status, parsed) =
        ds_call(h, "DeleteDirectory", &json!({ "DirectoryId": directory_id })).await;
    assert_eq!(
        status, 200,
        "setup {directory_id}: Directory Service must delete the directory, \
         got {status} / {parsed}"
    );
}

/// The `RegisterIdentityProvider` body a row sends: the directory it names, a published
/// product, and the network of the account the activation-server endpoints go in.
fn registration(directory_id: &str, product: &str) -> Value {
    json!({
        "IdentityProvider": provider_union(directory_id),
        "Product": product,
        "Settings": {
            "Subnets": [ACCOUNT_SUBNETS[0]],
            "SecurityGroupId": ACCOUNT_SECURITY_GROUPS[0],
        },
    })
}

/// The sentence this service refuses an unresolvable directory with, built from the id the
/// REQUEST sent: the fixed text of the error class with this row's own id, so a refusal
/// naming some other directory would not match.
fn not_usable(directory_id: &str) -> String {
    format!("{directory_id} directory either cannot be found or is not in active state")
}

/// Assert `reply` is the modelled `ValidationException` (HTTP 400) refusing `directory_id`.
fn assert_refused(reply: &Reply, case: &str, directory_id: &str) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected HTTP 400, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("ValidationException"),
        "case {case:?}: expected ValidationException, got {:?} with body {:?}",
        reply.error_type,
        reply.body
    );
    assert_eq!(
        reply.message(),
        Some(not_usable(directory_id).as_str()),
        "case {case:?}: expected the refusal to say {:?}, got {:?}",
        not_usable(directory_id),
        reply.message()
    );
}

/// Register an identity provider for `directory_id`, waiting for the directory the create
/// started to come into service, and answer the accepted `IdentityProviderSummary`.
///
/// Every refusal on the way is asserted to be THE directory refusal naming this id, so a
/// registration refused for any other reason fails the case at once: the wait pins the
/// refusal as well as the acceptance. A refused register persists nothing, so an attempt
/// costs the next one nothing.
async fn registered_once_in_service(
    h: &Harness,
    case: &str,
    directory_id: &str,
    product: &str,
) -> Value {
    let deadline = Instant::now() + BUILD_BUDGET;
    loop {
        let reply = register(h, &registration(directory_id, product)).await;
        if reply.status == 200 {
            return reply.body["IdentityProviderSummary"].clone();
        }
        assert_refused(&reply, case, directory_id);
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the directory must come into service and its registration be \
             accepted, still refused with {:?}",
            reply.message()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Every `DirectoryId` `ListIdentityProviders` reports a registration for, in listing order.
async fn registered_directory_ids(h: &Harness, case: &str) -> Vec<String> {
    let listed = list_providers(h, &json!({})).await;
    let body = listed.ok(case);
    body["IdentityProviderSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no summaries in {body:?}"))
        .iter()
        .map(|summary| {
            summary["IdentityProvider"]["ActiveDirectoryIdentityProvider"]["DirectoryId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a summary carries no DirectoryId: {summary:?}")
                })
                .to_string()
        })
        .collect()
}

/// RULE: a registration is minted for every directory the ACCOUNT has that is in service —
/// whichever create made it — and refused, naming the requested id, for one the account does
/// not have or has deleted.
#[tokio::test]
async fn a_directory_the_account_has_is_registrable_and_one_it_has_not_is_refused() {
    let h = fleet_with_directory_service();

    // Each of these ids was MINTED by the plane that owns directories, through the create
    // named; the seam row is the directory this service's own fleet seam holds.
    let simple_small = create_simple_ad(&h, "corp.example.com", SIZE_SMALL).await;
    let simple_large = create_simple_ad(&h, "ad-1.example.org", SIZE_LARGE).await;
    let managed = create_microsoft_ad(&h, "team.example.test").await;
    let deleted = create_simple_ad(&h, "gone.example.net", SIZE_SMALL).await;
    delete_directory(&h, &deleted).await;

    // (case, the directory id, the product the registration is for)
    let registrable: [(String, String, &str); 4] = [
        (
            format!("a Simple AD created Small ({simple_small})"),
            simple_small.clone(),
            PRODUCTS[0],
        ),
        (
            format!("a Simple AD created Large ({simple_large})"),
            simple_large.clone(),
            PRODUCTS[1],
        ),
        (
            format!("a Managed Microsoft AD ({managed})"),
            managed.clone(),
            PRODUCTS[2],
        ),
        (
            format!("this service's own fleet seam ({SEAM_DIRECTORY})"),
            SEAM_DIRECTORY.to_string(),
            PRODUCTS[3],
        ),
    ];

    for (case, directory_id, product) in &registrable {
        let summary = registered_once_in_service(&h, case, directory_id, product).await;
        assert_eq!(
            summary["IdentityProvider"],
            provider_union(directory_id),
            "case {case:?}: the summary must echo the directory that was registered, got \
             {summary:?}"
        );
        assert_eq!(
            summary.get("Status").and_then(Value::as_str),
            Some(REGISTERING),
            "case {case:?}: a fresh registration is reported in {REGISTERING}, got {summary:?}"
        );
        let arn = as_str(&summary, "IdentityProviderArn", case);
        assert!(
            arn.starts_with(&arn_prefix("identity-provider")),
            "case {case:?}: expected an ARN under {}, got {arn:?}",
            arn_prefix("identity-provider")
        );
    }

    // The round trip: every accepted registration is one a later read reports, under the
    // directory it was made for.
    let reported = registered_directory_ids(&h, "after the accepted registrations").await;
    for (case, directory_id, _) in &registrable {
        assert!(
            reported.contains(directory_id),
            "case {case:?}: ListIdentityProviders must report the registration of \
             {directory_id}, got {reported:?}"
        );
    }
    assert_eq!(
        reported.len(),
        registrable.len(),
        "the accepted registrations must be the only ones reported, got {reported:?}"
    );

    // And an id the account has no directory for is refused naming the id: one the plane
    // that owns directories deleted, and ids it never minted at all.
    let mut unresolvable: Vec<(String, String)> = vec![(
        format!("a directory deleted again ({deleted})"),
        deleted.clone(),
    )];
    for absent in ABSENT_DIRECTORY_IDS {
        // No create can have minted one of these: the rows above hold every id this
        // emulator's creates answered with.
        assert!(
            ![&simple_small, &simple_large, &managed, &deleted]
                .iter()
                .any(|minted| minted.as_str() == *absent),
            "setup: {absent} must be an id no create minted, got one of the created ones"
        );
        unresolvable.push((
            format!("an id no directory was created for ({absent})"),
            (*absent).to_string(),
        ));
    }

    for (case, directory_id) in &unresolvable {
        let reply = register(&h, &registration(directory_id, PRODUCTS[4])).await;
        assert_refused(&reply, case, directory_id);
    }

    // A refused registration persisted nothing: the listing still reports exactly the
    // accepted ones.
    let after_refusals = registered_directory_ids(&h, "after the refused registrations").await;
    assert_eq!(
        after_refusals, reported,
        "a refused registration must persist nothing, got {after_refusals:?}"
    );
}

/// RULE: the answer is read from the account's directories at EVERY call rather than copied
/// at registration time — so a directory deleted through the plane that owns it stops being
/// registrable, while the registrations the account already holds for it stay reported.
#[tokio::test]
async fn a_deleted_directory_stops_being_registrable() {
    let h = fleet_with_directory_service();
    let directory_id = create_simple_ad(&h, "later.example.com", SIZE_SMALL).await;
    let case = format!("a directory registered and then deleted ({directory_id})");

    registered_once_in_service(&h, &case, &directory_id, PRODUCTS[0]).await;

    // A second product of the SAME directory is registrable while the account holds it.
    register(&h, &registration(&directory_id, PRODUCTS[1]))
        .await
        .ok(&case);

    delete_directory(&h, &directory_id).await;

    // The directory is gone, so a further registration of it is refused naming the id —
    // for a THIRD product, so nothing but the directory can be what refuses it.
    let reply = register(&h, &registration(&directory_id, PRODUCTS[2])).await;
    assert_refused(&reply, &case, &directory_id);

    // What was registered before the delete is still the account's: this operation resolves
    // the directory of a NEW registration and touches no stored one.
    let reported = registered_directory_ids(&h, &case).await;
    assert_eq!(
        reported,
        vec![directory_id.clone(), directory_id.clone()],
        "case {case:?}: the two registrations made while the account held the directory must \
         still be reported, got {reported:?}"
    );
}
