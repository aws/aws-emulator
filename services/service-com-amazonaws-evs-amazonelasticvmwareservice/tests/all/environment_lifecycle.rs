// SPDX-License-Identifier: CC0-1.0

//! The Amazon EVS environment lifecycle, end to end over awsJson1_0.
//!
//! Every test drives the service the way a client does — `POST /` with
//! `X-Amz-Target: AmazonElasticVMwareService.<Operation>` through the same
//! [`MultiServiceRouter`] the production router mounts — so what it asserts is the
//! wire behaviour, not an internal call.
//!
//! The rules under test, and where each comes from:
//!
//! * `CreateEnvironment` mints and PERSISTS an environment, and every other
//!   operation answers from that stored record.
//! * A VLAN subnet CIDR block is between /24 and /28 and may not overlap another of
//!   the same environment (`InitialVlans` doc).
//! * The `vpcId` and `serviceAccessSubnetId` name a VPC and a subnet of it that the
//!   account holds — read from Amazon EC2's own store, which is why every test here
//!   registers the network it names ([`Emulator::fresh_network`]).
//! * `dedicatedHostId` and `placementGroupId` cannot be used together in one
//!   `HostInfoForCreate` (its own doc).
//! * `hcxNetworkAclId` is required when `isHcxPublic` is true (its own doc), and names a
//!   network ACL the account holds in that VPC — read from Amazon EC2's own store, which
//!   is why a test whose create carries it registers that list
//!   ([`Emulator::register_hcx_network_acl`]).
//! * An environment name is unique within the account and Region (`environmentName` doc).
//! * `maxResults` is a real page bound and `nextToken` resumes exactly where the page
//!   stopped; a token this service did not issue, or one issued more than 24 hours
//!   ago, is refused (`nextToken` doc).
//! * Every modeled CLIENT error of this awsJson1_0 service is reported with HTTP 400,
//!   whatever exact code its `@httpError` trait names — see
//!   `tests/all/error_wire_status.rs`, which owns that rule.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use service_com_amazonaws_evs_amazonelasticvmwareservice as evs;
use service_com_amazonaws_secretsmanager_secretsmanager as secretsmanager;
use smithy_server_runtime::MultiServiceRouter;

/// The Region the test client signs for, so the ARNs under test carry it.
pub(crate) const REGION: &str = "us-west-2";

/// The account every emulated ARN carries.
pub(crate) const ACCOUNT: &str = "000000000000";

/// Long enough for the lifecycle transition this service owes a new or deleted
/// environment to come due (its span is under a second by construction).
///
/// A suite that builds its own router, and so holds no engine to [`settle`] through,
/// sleeps for this before reading an environment's resources back: a list addressed to
/// an environment that is still initializing is refused
/// (`tests/all/list_while_initializing.rs`).
pub(crate) const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// The emulator a test drives: one shared engine, this service mounted on it.
///
/// The engine is kept beside the router because the account's own Amazon EC2 store
/// lives on it, and that store is where `CreateEnvironment` reads the VPC and subnet a
/// request names from — so a request builder has to put them there first
/// ([`Emulator::fresh_network`]).
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

/// So a test that only dispatches requests passes `&emulator` where a
/// `&MultiServiceRouter` is expected.
impl std::ops::Deref for Emulator {
    type Target = MultiServiceRouter;

    fn deref(&self) -> &MultiServiceRouter {
        &self.router
    }
}

impl Emulator {
    /// A VPC and a subnet inside it, written into the account's own Amazon EC2 store —
    /// the store `CreateEnvironment` checks the network a request names against.
    ///
    /// A fresh pair per call, because Amazon EVS allows one deployed environment per
    /// VPC ("The VPC cannot be used with any other deployed Amazon EVS environment",
    /// `vpcId` doc), so a test that creates several environments gives each its own.
    /// The ids are minted by EC2's own id minter, so they are the ids that service
    /// would hand out.
    pub(crate) fn fresh_network(&self) -> (String, String) {
        let ec2_state = ec2::state::service_state(&*self.engine)
            .expect("the in-memory engine carries the account's EC2 store");
        let (vpc_id, subnet_id) = {
            let mut store = ec2_state.lock();
            (store.next_id("vpc"), store.next_id("subnet"))
        };
        register_network(&self.engine, &vpc_id, &subnet_id);
        (vpc_id, subnet_id)
    }

    /// A `CreateEnvironment` request that satisfies every rule, optionally named, in a
    /// VPC and subnet this account holds.
    pub(crate) fn create_request(&self, name: Option<&str>) -> Value {
        let (vpc_id, subnet_id) = self.fresh_network();
        let mut body = json!({
            "vpcId": vpc_id,
            "serviceAccessSubnetId": subnet_id,
            "vcfVersion": "VCF-5.2.1",
            "termsAccepted": true,
            "initialVlans": vlans(),
        });
        if let Some(name) = name {
            body["environmentName"] = json!(name);
        }
        body
    }

    /// Write the network access control list `request`'s `initialVlans.hcxNetworkAclId`
    /// names into the account's own Amazon EC2 store, inside the VPC that request deploys
    /// into — and nothing at all for a request that names none.
    ///
    /// `CreateEnvironment` resolves that id against THAT store, in that VPC: a network ACL
    /// is an Amazon EC2 resource of the caller's account that Amazon EVS does not create,
    /// and a network ACL belongs to one VPC. So every suite whose create carries the member
    /// and expects to be served registers the list it names here first, exactly as
    /// [`register_network`] registers the VPC and the service access subnet.
    /// `tests/all/hcx_network_acl_references.rs` owns that rule and mints its lists through
    /// Amazon EC2's own writers.
    pub(crate) fn register_hcx_network_acl(&self, request: &Value) {
        let Some(network_acl_id) = request["initialVlans"]["hcxNetworkAclId"].as_str() else {
            return;
        };
        let vpc_id = request["vpcId"]
            .as_str()
            .expect("a create request names the VPC it is deployed into");
        register_network_acl(&self.engine, vpc_id, network_acl_id);
    }
}

/// Write one VPC and one subnet inside it into the account's own Amazon EC2 store, at
/// the ids given.
///
/// `CreateEnvironment` checks the network a request names against THAT store (an
/// environment is deployed into the caller's existing VPC), so every test that expects
/// a create to be accepted registers the VPC and subnet it names here first. The other
/// suites of this target use it for the fixed ids their requests carry.
///
/// The service access subnet is placed high in the VPC's range, clear of the blocks
/// [`vlans_with_prefix`] gives the VLAN subnets: an environment's VLAN subnet blocks
/// "must not overlap with other subnets in the VPC" (`InitialVlans` doc), and the
/// service access subnet is one of those other subnets. It carries an Availability
/// Zone because every subnet EC2 creates has one, and it is the zone the environment's
/// VLAN subnets are provisioned in ("all VPC subnets exist in a single Availability
/// Zone", `vpcId` doc).
pub(crate) fn register_network(
    engine: &engine::InMemoryEngine,
    vpc_id: &str,
    subnet_id: &str,
) {
    let ec2_state = ec2::state::service_state(engine)
        .expect("the in-memory engine carries the account's EC2 store");
    let mut store = ec2_state.lock();
    store.vpcs.insert(
        vpc_id.to_string(),
        ec2::ops::types::Vpc {
            vpc_id: Some(vpc_id.to_string()),
            state: Some(ec2::state::VPC_STATE_AVAILABLE.to_string()),
            cidr_block: Some(VPC_CIDR.to_string()),
            ..Default::default()
        },
    );
    store.subnets.insert(
        subnet_id.to_string(),
        ec2::ops::types::Subnet {
            subnet_id: Some(subnet_id.to_string()),
            vpc_id: Some(vpc_id.to_string()),
            state: Some(ec2::state::SUBNET_STATE_AVAILABLE.to_string()),
            cidr_block: Some(SERVICE_ACCESS_SUBNET_CIDR.to_string()),
            availability_zone: Some(ZONE.to_string()),
            availability_zone_id: ec2::state::zone_id_for_zone_name(ZONE),
            ..Default::default()
        },
    );
}

/// Write one route server peer per id into the account's own Amazon EC2 store.
///
/// `CreateEnvironment` resolves every `connectivityInfo.privateRouteServerPeerings` entry
/// against THAT store — a route server peer is an Amazon EC2 resource of the caller's
/// account and Amazon EVS creates none — so a test whose create carries the member and
/// expects to be served registers the peers it names here first, exactly as
/// [`register_network`] registers the VPC and the service access subnet.
/// `tests/all/route_server_peer_references.rs` owns that rule and mints its peers through
/// Amazon EC2's own `CreateRouteServerPeer`.
pub(crate) fn register_route_server_peers(engine: &engine::InMemoryEngine, peer_ids: &[&str]) {
    let ec2_state = ec2::state::service_state(engine)
        .expect("the in-memory engine carries the account's EC2 store");
    let mut store = ec2_state.lock();
    for peer_id in peer_ids {
        store.route_server_peers.insert(
            (*peer_id).to_string(),
            ec2::ops::types::RouteServerPeer {
                route_server_peer_id: Some((*peer_id).to_string()),
                state: Some(ec2::state::ROUTE_SERVER_PEER_STATE_AVAILABLE.to_string()),
                ..Default::default()
            },
        );
    }
}

/// Write one network access control list of `vpc_id` into the account's own Amazon EC2
/// store, at the id given.
///
/// `CreateEnvironment` resolves the `initialVlans.hcxNetworkAclId` a request carries
/// against THAT store, inside the VPC the environment is deployed into — a network ACL is
/// an Amazon EC2 resource of the caller's account that Amazon EVS does not create, and it
/// belongs to one VPC — so a test whose create carries the member and expects to be served
/// registers the list it names here first, exactly as [`register_network`] registers the
/// VPC. [`Emulator::register_hcx_network_acl`] does it for a request already built, and
/// `tests/all/hcx_network_acl_references.rs` owns the rule and mints its lists through
/// Amazon EC2's own writers.
pub(crate) fn register_network_acl(
    engine: &engine::InMemoryEngine,
    vpc_id: &str,
    network_acl_id: &str,
) {
    let ec2_state = ec2::state::service_state(engine)
        .expect("the in-memory engine carries the account's EC2 store");
    let mut store = ec2_state.lock();
    store.network_acls.insert(
        network_acl_id.to_string(),
        ec2::ops::types::NetworkAcl {
            network_acl_id: Some(network_acl_id.to_string()),
            vpc_id: Some(vpc_id.to_string()),
            is_default: Some(false),
            associations: Some(Vec::new()),
            entries: Some(ec2::state::default_network_acl_entries()),
            ..Default::default()
        },
    );
}

/// The IPv4 CIDR block of the VPC [`register_network`] writes: every VLAN subnet block
/// a test uses is a block OF it, because Amazon EVS provisions the VLAN subnets inside
/// that VPC.
pub(crate) const VPC_CIDR: &str = "10.0.0.0/16";

/// The block of the service access subnet [`register_network`] writes — clear of every
/// block [`vlans_with_prefix`] hands the VLAN subnets.
pub(crate) const SERVICE_ACCESS_SUBNET_CIDR: &str = "10.0.240.0/24";

/// The Availability Zone the registered network is in, and so the zone the
/// environment's VLAN subnets are provisioned in. A zone of [`REGION`], which is what
/// EC2's own zone list reports for it.
pub(crate) const ZONE: &str = "us-west-2a";

/// The ARN form of the modelled `SecretIdentifier`, whose pattern's other alternative —
/// a bare name over `[a-zA-Z0-9/_+=.@!-]` — admits no colon.
pub(crate) const SECRET_ARN_PREFIX: &str = "arn:aws:secretsmanager:";

/// The ARN a connector reports for the credential secret a request named by
/// `secret_identifier`: the identifier itself when it already is an ARN, and otherwise
/// the ARN of that name in the Region the test client addresses and this account.
///
/// `Connector$secretArn` is "The Amazon Resource Name (ARN) of" the secret while
/// `secretIdentifier` is "The ARN or name of" it, so the ARN is what every suite that
/// reads a connector expects; `tests/all/connector_secret_arn.rs` owns that rule and
/// covers the secrets the account really holds, whose ARN Secrets Manager minted.
pub(crate) fn secret_arn_of(secret_identifier: &str) -> String {
    if secret_identifier.starts_with("arn:") {
        return secret_identifier.to_string();
    }
    format!("{SECRET_ARN_PREFIX}{REGION}:{ACCOUNT}:secret:{secret_identifier}")
}

/// The fields a connector's credential secret stores, as a JSON object: every field the
/// three `ConnectorType`s require between them — "SDDC_MANAGER requires an apiKey field;
/// OPERATIONS_MANAGER and VCENTER require username and password fields"
/// (`smithy_query(query="doc", shape="CreateEnvironmentConnectorRequest",
/// member="secretIdentifier")`) — so one secret serves a connector of any type.
///
/// The VALUES are fixtures and say nothing: what the rule is about is which fields the
/// secret states.
pub(crate) fn credential_secret_value() -> Value {
    json!({
        "apiKey": "fixture",
        "username": "fixture",
        "password": "fixture",
    })
}

/// Write the credential secret a connector request names into the account's own Amazon
/// Web Services Secrets Manager store, storing every field a `ConnectorType` requires.
///
/// `CreateEnvironmentConnector` and `UpdateEnvironmentConnector` resolve the
/// `secretIdentifier` they are given in THAT store — "A connector establishes a connection
/// to the given appliance type using the credentials stored in Amazon Web Services Secrets
/// Manager" (`Connector` doc), and Amazon EVS creates no secret — so every suite that
/// expects a connector create or update to be SERVED registers the secret it names here
/// first, exactly as [`register_network`] registers the VPC an environment is deployed
/// into. `tests/all/connector_credential_secret.rs` owns that rule and covers the
/// identifiers that resolve to nothing and the secrets that store the wrong fields.
pub(crate) fn register_credential_secret(
    engine: &engine::InMemoryEngine,
    secret_identifier: &str,
) {
    register_secret_storing(engine, secret_identifier, &credential_secret_value());
}

/// [`register_credential_secret`] with the secret's stored value GIVEN, for a suite that
/// varies what the secret states rather than whether it exists.
pub(crate) fn register_secret_storing(
    engine: &engine::InMemoryEngine,
    secret_identifier: &str,
    value: &Value,
) {
    let state = secretsmanager::state::service_state(engine)
        .expect("the in-memory engine carries the account's Secrets Manager store");
    let now = std::time::SystemTime::now();
    let mut secret = secretsmanager::state::Secret::new(
        secretsmanager::state::secret_name_in(secret_identifier).to_string(),
        secret_arn_of(secret_identifier),
        secret_region_of(secret_identifier).to_string(),
        now,
    );
    secret.put_version(
        secretsmanager::state::SecretVersion {
            version_id: FIXTURE_SECRET_VERSION_ID.to_string(),
            value: secretsmanager::state::envelope::StoredValue::as_sent(
                &secretsmanager::state::envelope::SecretValue::from_request(
                    Some(&value.to_string()),
                    None,
                )
                .expect("the fixture carries a secret string"),
            ),
            version_stages: vec![secretsmanager::state::STAGE_CURRENT.to_string()],
            created_date: now,
            last_accessed_date: None,
        },
        now,
    );
    state.lock().insert(secret);
}

/// The one version id every registered fixture secret's current value carries: a secret
/// namespace of its own, so no two fixtures collide inside one secret.
const FIXTURE_SECRET_VERSION_ID: &str = "00000000-0000-4000-8000-000000000000";

/// The Region a secret identifier names the secret in: the Region segment of an ARN, and
/// the Region the test client signs for otherwise — the namespace a secret is held in.
pub(crate) fn secret_region_of(secret_identifier: &str) -> &str {
    if !secret_identifier.starts_with("arn:") {
        return REGION;
    }
    match secret_identifier.split(':').nth(3) {
        Some(named) if !named.is_empty() => named,
        _ => REGION,
    }
}

/// The credential secrets the ACCOUNT holds in every emulator this target builds: the
/// identifiers the connector requests of these suites name, in both forms
/// `secretIdentifier` accepts.
///
/// A connector is created against a secret the account really holds, so they are
/// registered once with the emulator rather than per suite, exactly as the Premium
/// Support subscription every create needs is granted there. A suite naming a secret
/// outside this list registers it itself ([`register_credential_secret`]), and
/// `tests/all/connector_credential_secret.rs` owns the rule for the identifiers that
/// resolve to nothing.
pub(crate) const CREDENTIAL_SECRETS: &[&str] = &[
    "arn:aws:secretsmanager:us-west-2:000000000000:secret:evs/vcf-AbCd12",
    "evs/vcf-appliance-credentials",
    "evs/appliance",
];

/// The emulator a test drives: one shared engine, this service mounted on it.
pub(crate) fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The tests exercise behaviour, not signing: the credentials below are unsigned.
    router.set_request_authentication(false);
    // An environment is created only for an account holding a Premium Support
    // subscription, which the Support store owns and an operator grants out of band — so
    // a suite whose creates are expected to be SERVED grants one here.
    // `tests/all/premium_support_entitlement.rs` owns that rule and covers the account
    // that holds none.
    crate::premium_support_entitlement::grant_premium_support(&engine);
    // The credential secrets a connector create or update is served against, in the
    // account's own Secrets Manager store.
    for secret_identifier in CREDENTIAL_SECRETS {
        register_credential_secret(&engine, secret_identifier);
    }
    router.register_service(
        service_com_amazonaws_evs_amazonelasticvmwareservice::EmulatedService::new(engine.clone()),
    );
    Emulator { router, engine }
}

/// The access key id the test client signs with by default: one no session of this
/// emulator was minted for, so it speaks for the account's own published credential.
pub(crate) const DEFAULT_ACCESS_KEY_ID: &str = "AKIDTESTONLY";

/// Call one operation and return its HTTP status and JSON body.
pub(crate) async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, REGION, operation, body).await
}

/// Call one operation as a client signing for `region`.
pub(crate) async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    call_as(router, DEFAULT_ACCESS_KEY_ID, region, operation, body).await
}

/// Call one operation as a client signing for `region` with `access_key_id` — which is
/// what names the principal the request speaks for, and so what the service's
/// authorization gate decides against.
pub(crate) async fn call_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("https://evs.us-west-2.amazonaws.com/")
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("AmazonElasticVMwareService.{operation}"),
        )
        // The SigV4 credential scope is what routes the request to `evs` and what
        // names the Region the service builds ARNs in; the access key id in it is what
        // names the principal the request speaks for.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{region}/evs/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0"
            ),
        )
        .body(::bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("the request builds");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, payload)
}

/// Ten non-overlapping VLAN subnet CIDR blocks of the given prefix length, one per
/// `InitialVlans` member, each in its own `10.0.<n>.0` block.
pub(crate) fn vlans_with_prefix(prefix_len: u8) -> Value {
    let cidr = |index: u8| json!({ "cidr": format!("10.0.{}.0/{prefix_len}", index * 2) });
    json!({
        "vmkManagement": cidr(0),
        "vmManagement": cidr(1),
        "vMotion": cidr(2),
        "vSan": cidr(3),
        "vTep": cidr(4),
        "edgeVTep": cidr(5),
        "nsxUplink": cidr(6),
        "hcx": cidr(7),
        "expansionVlan1": cidr(8),
        "expansionVlan2": cidr(9),
    })
}

/// The ten VLAN subnets a valid create uses, at the smallest allowed size.
fn vlans() -> Value {
    vlans_with_prefix(28)
}

/// Four hosts, as `hosts` requires (len 4..4), with `extra` merged into the first.
pub(crate) fn four_hosts(extra: Value) -> Value {
    let mut first = json!({
        "hostName": "esx-1",
        "keyName": "evs-key",
        "instanceType": "i4i.metal",
    });
    for (key, value) in extra.as_object().expect("the extra members are an object") {
        first[key] = value.clone();
    }
    json!([
        first,
        { "hostName": "esx-2", "keyName": "evs-key", "instanceType": "i4i.metal" },
        { "hostName": "esx-3", "keyName": "evs-key", "instanceType": "i4i.metal" },
        { "hostName": "esx-4", "keyName": "evs-key", "instanceType": "i4i.metal" },
    ])
}

/// Whether `id` satisfies the modelled `^(env-[a-zA-Z0-9]{10})$`.
fn is_environment_id(id: &str) -> bool {
    match id.strip_prefix("env-") {
        Some(rest) => rest.len() == 10 && rest.bytes().all(|b| b.is_ascii_alphanumeric()),
        None => false,
    }
}

/// Whether `id` satisfies the modelled `^subnet-[a-f0-9]{8}([a-f0-9]{9})?$`.
fn is_subnet_id(id: &str) -> bool {
    match id.strip_prefix("subnet-") {
        Some(rest) => {
            (rest.len() == 8 || rest.len() == 17)
                && rest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }
        None => false,
    }
}

/// The `__type` an error body carries, for an assertion message.
pub(crate) fn error_type(body: &Value) -> String {
    body["__type"].as_str().unwrap_or("<none>").to_string()
}

/// Create an environment and return its id, failing the test if it is refused.
pub(crate) async fn create(router: &Emulator, name: Option<&str>) -> String {
    let (status, body) = call(router, "CreateEnvironment", &router.create_request(name)).await;
    assert_eq!(
        status, 200,
        "create {name:?}: expected 200, got {status} {body}"
    );
    body["environment"]["environmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("create {name:?}: no environmentId in {body}"))
        .to_string()
}

/// Settle every lifecycle transition this service owes, WITHOUT sleeping for it.
///
/// A new environment is `CREATING` — still initializing — until the transition the
/// clock owes it comes due, and a LIST addressed to an environment resource in that
/// state is refused with a `ValidationException` (`ValidationException` doc; see
/// `tests/all/list_while_initializing.rs`, which owns that rule). So a test that lists
/// an environment's resources has to get past the create transition first.
///
/// The stored due instants are compared against the instant handed in, so settling
/// against an instant [`PAST_THE_TRANSITION`] from now applies exactly the transitions
/// a call made after that span would — and costs no wall-clock time, which matters for
/// a test that creates many environments. A test that asserts the TIMING itself sleeps
/// instead.
pub(crate) fn settle(router: &Emulator) {
    let state = evs::state::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store");
    let past_the_transition = std::time::Instant::now() + PAST_THE_TRANSITION;
    let mut store = state.lock();
    for environment in store.environments.values_mut() {
        environment.materialise_due_transition(past_the_transition);
    }
    // The health checks are derived from the states just settled, exactly as the
    // handlers' own `materialise_due_transitions` recomputes them after a transition.
    store.recompute_environment_checks();
}

/// Create an environment, settle the transition the clock owes it, and return its id —
/// what a test that goes on to LIST that environment's VLANs, hosts, connectors or
/// entitlements needs ([`settle`]).
pub(crate) async fn create_settled(router: &Emulator, name: Option<&str>) -> String {
    let id = create(router, name).await;
    settle(router);
    id
}

/// How long a poll-until helper keeps looking for a state change the clock owes before it
/// fails the test: far longer than the span itself, so a loaded or slow host cannot fail a
/// test that only waits for a CONDITION to hold.
pub(crate) const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// How long it waits between two looks.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Look at the connectors of one environment again and again, through
/// `ListEnvironmentConnectors`, until `holds` is true of the connector `connector_id`, and
/// answer that record.
///
/// This is how a test waits for the state change the clock owes a connector: it waits for
/// the state to BE what it waits for, with [`POLL_DEADLINE`] of room, rather than for a
/// span of wall clock that a faster or busier host measures differently. `expected` names
/// what it is waiting for, so a timeout says which condition never held.
pub(crate) async fn poll_connector_until<F>(
    router: &MultiServiceRouter,
    case: &str,
    environment_id: &str,
    connector_id: &str,
    expected: &str,
    holds: F,
) -> Value
where
    F: Fn(&Value) -> bool,
{
    let deadline = std::time::Instant::now() + POLL_DEADLINE;
    let mut last = Value::Null;
    loop {
        let (status, body) = call(
            router,
            "ListEnvironmentConnectors",
            &json!({ "environmentId": environment_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: list expected 200, got {status} {body}"
        );
        last = body["connectors"]
            .as_array()
            .and_then(|listed| {
                listed
                    .iter()
                    .find(|connector| connector["connectorId"] == json!(connector_id))
                    .cloned()
            })
            .unwrap_or(last);
        if holds(&last) {
            return last;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the connector {connector_id} never reached {expected}; last read \
             {last}"
        );
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// [`poll_connector_until`] for the state change a create or an update owes a connector:
/// the move to `ACTIVE` once the connection to the appliance is established.
pub(crate) async fn poll_connector_active(
    router: &MultiServiceRouter,
    case: &str,
    environment_id: &str,
    connector_id: &str,
) -> Value {
    poll_connector_until(
        router,
        case,
        environment_id,
        connector_id,
        "ACTIVE",
        |connector| connector["state"] == json!("ACTIVE"),
    )
    .await
}

#[tokio::test]
async fn an_environment_round_trips_through_create_get_list_vlans_and_delete() {
    let router = emulator();

    let request = router.create_request(Some("demo-env"));
    let (status, created) = call(&router, "CreateEnvironment", &request).await;
    assert_eq!(status, 200, "create: expected 200, got {status} {created}");
    let environment = created["environment"].clone();
    let id = environment["environmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("create: no environmentId in {created}"))
        .to_string();

    assert!(
        is_environment_id(&id),
        "create: environmentId {id:?} must match ^(env-[a-zA-Z0-9]{{10}})$"
    );
    assert_eq!(
        environment["environmentArn"],
        json!(format!("arn:aws:evs:{REGION}:{ACCOUNT}:environment/{id}")),
        "create: expected the environment ARN of {id}, got {}",
        environment["environmentArn"]
    );
    assert_eq!(
        environment["environmentState"],
        json!("CREATING"),
        "create: a just-created environment is still CREATING, got {}",
        environment["environmentState"]
    );
    for (member, expected) in [
        ("environmentName", json!("demo-env")),
        ("vpcId", request["vpcId"].clone()),
        ("serviceAccessSubnetId", request["serviceAccessSubnetId"].clone()),
        ("vcfVersion", json!("VCF-5.2.1")),
        ("termsAccepted", json!(true)),
        ("environmentStatus", json!("UNKNOWN")),
    ] {
        assert_eq!(
            environment[member], expected,
            "create: {member}: expected {expected}, got {}",
            environment[member]
        );
    }

    // The read-back: GetEnvironment must report the record create stored.
    let (status, got) = call(&router, "GetEnvironment", &json!({ "environmentId": id })).await;
    assert_eq!(status, 200, "get: expected 200, got {status} {got}");
    for member in [
        "environmentId",
        "environmentArn",
        "environmentName",
        "vpcId",
        "serviceAccessSubnetId",
        "vcfVersion",
        "termsAccepted",
        "createdAt",
    ] {
        assert_eq!(
            got["environment"][member], environment[member],
            "get: {member}: expected the stored {}, got {}",
            environment[member], got["environment"][member]
        );
    }

    // The transition the clock owes a new environment: it is CREATED once due, and
    // its VLAN subnets with it.
    std::thread::sleep(PAST_THE_TRANSITION);
    let (status, settled) = call(&router, "GetEnvironment", &json!({ "environmentId": id })).await;
    assert_eq!(status, 200, "get settled: expected 200, got {status} {settled}");
    assert_eq!(
        settled["environment"]["environmentState"],
        json!("CREATED"),
        "get settled: the owed transition must have been applied, got {}",
        settled["environment"]["environmentState"]
    );
    assert_eq!(
        settled["environment"]["environmentStatus"],
        json!("FAILED"),
        "get settled: this environment holds no host, so its HOST_COUNT check fails and \
         the roll-up is FAILED, got {} from {}",
        settled["environment"]["environmentStatus"],
        settled["environment"]["checks"]
    );
    assert_eq!(
        settled["environment"]["checks"]
            .as_array()
            .map(|checks| checks
                .iter()
                .filter(|check| check["result"] == json!("FAILED"))
                .filter_map(|check| check["type"].as_str())
                .collect::<Vec<&str>>()),
        Some(vec!["HOST_COUNT"]),
        "get settled: the failing check is the minimum host count, got {}",
        settled["environment"]["checks"]
    );
    let created_at = settled["environment"]["createdAt"]
        .as_f64()
        .expect("createdAt is a timestamp");
    let modified_at = settled["environment"]["modifiedAt"]
        .as_f64()
        .expect("modifiedAt is a timestamp");
    assert!(
        modified_at > created_at,
        "get settled: modifiedAt {modified_at} must advance past createdAt {created_at}"
    );

    // The VLAN subnets derived from the initialVlans supplied at create time.
    let (status, listed) = call(
        &router,
        "ListEnvironmentVlans",
        &json!({ "environmentId": id }),
    )
    .await;
    assert_eq!(status, 200, "list vlans: expected 200, got {status} {listed}");
    let vlans = listed["environmentVlans"]
        .as_array()
        .unwrap_or_else(|| panic!("list vlans: no environmentVlans in {listed}"));
    assert_eq!(
        vlans.len(),
        10,
        "list vlans: InitialVlans declares ten VLAN subnets, got {}",
        vlans.len()
    );
    let expected_cidrs = vlans_with_prefix(28);
    let mut seen_subnets = BTreeSet::new();
    let mut seen_tags = BTreeSet::new();
    for vlan in vlans {
        let function = vlan["functionName"]
            .as_str()
            .unwrap_or_else(|| panic!("list vlans: no functionName in {vlan}"));
        assert_eq!(
            vlan["cidr"], expected_cidrs[function]["cidr"],
            "list vlans: {function}: expected the CIDR block the request supplied ({}), got {}",
            expected_cidrs[function]["cidr"], vlan["cidr"]
        );
        let subnet = vlan["subnetId"]
            .as_str()
            .unwrap_or_else(|| panic!("list vlans: {function}: no subnetId in {vlan}"));
        assert!(
            is_subnet_id(subnet),
            "list vlans: {function}: subnetId {subnet:?} must match \
             ^subnet-[a-f0-9]{{8}}([a-f0-9]{{9}})?$"
        );
        assert!(
            seen_subnets.insert(subnet.to_string()),
            "list vlans: {function}: subnetId {subnet} is used by another VLAN"
        );
        let tag = vlan["vlanId"]
            .as_i64()
            .unwrap_or_else(|| panic!("list vlans: {function}: no vlanId in {vlan}"));
        assert!(
            seen_tags.insert(tag),
            "list vlans: {function}: vlanId {tag} is used by another VLAN"
        );
        assert_eq!(
            vlan["vlanState"],
            json!("CREATED"),
            "list vlans: {function}: the owed VLAN transition must have been applied, got {}",
            vlan["vlanState"]
        );
        assert_eq!(
            vlan["isPublic"],
            json!(false),
            "list vlans: {function}: only the HCX VLAN can be public, got {}",
            vlan["isPublic"]
        );
    }
    let functions: BTreeSet<&str> = vlans
        .iter()
        .filter_map(|vlan| vlan["functionName"].as_str())
        .collect();
    let expected_functions: BTreeSet<&str> = [
        "vmkManagement",
        "vmManagement",
        "vMotion",
        "vSan",
        "vTep",
        "edgeVTep",
        "nsxUplink",
        "hcx",
        "expansionVlan1",
        "expansionVlan2",
    ]
    .into_iter()
    .collect();
    assert_eq!(
        functions, expected_functions,
        "list vlans: expected one VLAN per InitialVlans member, got {functions:?}"
    );

    // The environment is enumerable.
    let (status, all) = call(&router, "ListEnvironments", &json!({})).await;
    assert_eq!(status, 200, "list: expected 200, got {status} {all}");
    let ids: Vec<&str> = all["environmentSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("list: no environmentSummaries in {all}"))
        .iter()
        .filter_map(|summary| summary["environmentId"].as_str())
        .collect();
    assert_eq!(
        ids,
        vec![id.as_str()],
        "list: expected exactly the created environment, got {ids:?}"
    );

    // Deleting it moves it to DELETING and stamps modifiedAt.
    let (status, deleted) = call(
        &router,
        "DeleteEnvironment",
        &json!({ "environmentId": id, "clientToken": "delete-1" }),
    )
    .await;
    assert_eq!(status, 200, "delete: expected 200, got {status} {deleted}");
    assert_eq!(
        deleted["environment"]["environmentState"],
        json!("DELETING"),
        "delete: expected DELETING, got {}",
        deleted["environment"]["environmentState"]
    );
    let deleting_modified_at = deleted["environment"]["modifiedAt"]
        .as_f64()
        .expect("modifiedAt is a timestamp");
    assert!(
        deleting_modified_at >= modified_at,
        "delete: modifiedAt {deleting_modified_at} must not go backwards from {modified_at}"
    );

    // @idempotent: deleting it again reports the same record and restarts nothing.
    let (status, again) = call(
        &router,
        "DeleteEnvironment",
        &json!({ "environmentId": id, "clientToken": "delete-2" }),
    )
    .await;
    assert_eq!(status, 200, "delete again: expected 200, got {status} {again}");
    assert_eq!(
        again["environment"]["modifiedAt"].as_f64(),
        Some(deleting_modified_at),
        "delete again: a repeated delete must not move modifiedAt, got {}",
        again["environment"]["modifiedAt"]
    );

    // Once the deletion is due, the environment reports DELETED and drops out of a
    // list filtered to the active states.
    std::thread::sleep(PAST_THE_TRANSITION);
    let (status, purged) = call(&router, "GetEnvironment", &json!({ "environmentId": id })).await;
    assert_eq!(status, 200, "get deleted: expected 200, got {status} {purged}");
    assert_eq!(
        purged["environment"]["environmentState"],
        json!("DELETED"),
        "get deleted: expected DELETED, got {}",
        purged["environment"]["environmentState"]
    );

    for (filter, expected) in [
        (json!(["CREATED", "CREATING"]), Vec::<&str>::new()),
        (json!(["DELETED"]), vec![id.as_str()]),
    ] {
        let (status, filtered) =
            call(&router, "ListEnvironments", &json!({ "state": filter })).await;
        assert_eq!(
            status, 200,
            "list {filter}: expected 200, got {status} {filtered}"
        );
        let got: Vec<&str> = filtered["environmentSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("list {filter}: no environmentSummaries in {filtered}"))
            .iter()
            .filter_map(|summary| summary["environmentId"].as_str())
            .collect();
        assert_eq!(
            got, expected,
            "list {filter}: expected {expected:?}, got {got:?}"
        );
    }
}

#[tokio::test]
async fn create_environment_enforces_the_documented_request_rules() {
    // Each row: the case, the request, and the HTTP status the rule gives it. Rows
    // that pass create a nameless environment, so no row collides with another.
    let license = json!([{
        "solutionKey": "AAAAA-BBBBB-CCCCC-DDDDD-EEEEE",
        "vsanKey": "FFFFF-GGGGG-HHHHH-IIIII-JJJJJ",
    }]);
    let hostnames = json!({
        "vCenter": "vcenter", "nsx": "nsx", "nsxManager1": "nsxm1", "nsxManager2": "nsxm2",
        "nsxManager3": "nsxm3", "nsxEdge1": "nsxe1", "nsxEdge2": "nsxe2",
        "sddcManager": "sddc", "cloudBuilder": "builder",
    });
    // The two route server peers the `connectivityInfo` rows name. Every row registers
    // them in the account's own EC2 store below, because an installed version resolves
    // them there (`tests/all/route_server_peer_references.rs` owns that rule).
    const PEERINGS: [&str; 2] = ["rsp-0123456789abcdef0", "rsp-00fedcba98765432"];
    let connectivity = json!({ "privateRouteServerPeerings": PEERINGS });

    let mut overlapping = vlans();
    // vmManagement's /24 swallows vMotion's /28 — the same addresses twice.
    overlapping["vmManagement"] = json!({ "cidr": "10.0.4.0/24" });

    let mut public_hcx = vlans();
    public_hcx["isHcxPublic"] = json!(true);
    let mut public_hcx_with_acl = public_hcx.clone();
    public_hcx_with_acl["hcxNetworkAclId"] = json!("acl-0123456789abcdef0");

    let rows: Vec<(&str, Value, u16)> = vec![
        // The five parameters `SELF_DEPLOYED` "does not support" are ACCEPTED: the mode
        // does nothing with them, and the recorded wire shows AWS refusing nothing for
        // carrying them (see this operation's handler docs).
        (
            "SELF_DEPLOYED accepts the install parameters it does not use: siteId",
            json!({ "vcfVersion": "SELF_DEPLOYED", "siteId": "site-1" }),
            200,
        ),
        (
            "SELF_DEPLOYED accepts the install parameters it does not use: licenseInfo",
            json!({ "vcfVersion": "SELF_DEPLOYED", "licenseInfo": license }),
            200,
        ),
        (
            "SELF_DEPLOYED accepts the install parameters it does not use: hosts",
            json!({ "vcfVersion": "SELF_DEPLOYED", "hosts": four_hosts(json!({})) }),
            200,
        ),
        (
            "SELF_DEPLOYED accepts the install parameters it does not use: vcfHostnames",
            json!({ "vcfVersion": "SELF_DEPLOYED", "vcfHostnames": hostnames }),
            200,
        ),
        (
            "SELF_DEPLOYED accepts the install parameters it does not use: connectivityInfo",
            json!({ "vcfVersion": "SELF_DEPLOYED", "connectivityInfo": connectivity }),
            200,
        ),
        (
            "SELF_DEPLOYED with none of them is accepted",
            json!({ "vcfVersion": "SELF_DEPLOYED" }),
            200,
        ),
        (
            "an installed version accepts those parameters too",
            json!({
                "vcfVersion": "VCF-5.2.2",
                "siteId": "site-1",
                "licenseInfo": license,
                "vcfHostnames": hostnames,
                "connectivityInfo": connectivity,
            }),
            200,
        ),
        (
            "a /28 VLAN subnet is the smallest allowed",
            json!({ "initialVlans": vlans_with_prefix(28) }),
            200,
        ),
        (
            "a /25 VLAN subnet is inside the allowed range",
            json!({ "initialVlans": vlans_with_prefix(25) }),
            200,
        ),
        (
            "a /24 VLAN subnet is the largest allowed",
            json!({ "initialVlans": vlans_with_prefix(24) }),
            200,
        ),
        (
            "a /29 VLAN subnet is smaller than the minimum",
            json!({ "initialVlans": vlans_with_prefix(29) }),
            400,
        ),
        (
            "a /23 VLAN subnet is larger than the maximum",
            json!({ "initialVlans": vlans_with_prefix(23) }),
            400,
        ),
        (
            "two VLAN subnets may not overlap",
            json!({ "initialVlans": overlapping }),
            400,
        ),
        (
            "a public HCX VLAN requires a network ACL id",
            json!({ "initialVlans": public_hcx }),
            400,
        ),
        (
            "a public HCX VLAN with a network ACL id is accepted",
            json!({ "initialVlans": public_hcx_with_acl }),
            200,
        ),
        (
            "a host cannot name both a dedicated host and a placement group",
            json!({
                "hosts": four_hosts(json!({
                    "dedicatedHostId": "h-0123456789abcdef0",
                    "placementGroupId": "pg-0123456789abcdef0",
                })),
            }),
            400,
        ),
        (
            // A host is PLACED on an EC2 resource of the caller's own account, and this
            // account holds neither of these two: the accepted side of that rule lives in
            // `host_placement_references`, which mints them through Amazon EC2 itself.
            "a host may not name a placement group the account does not hold",
            json!({ "hosts": four_hosts(json!({ "placementGroupId": "pg-0123456789abcdef0" })) }),
            400,
        ),
        (
            "a host may not name a dedicated host the account does not hold",
            json!({ "hosts": four_hosts(json!({ "dedicatedHostId": "h-0123456789abcdef0" })) }),
            400,
        ),
    ];

    for (case, overrides, expected_status) in rows {
        let router = emulator();
        register_route_server_peers(&router.engine, &PEERINGS);
        let mut request = router.create_request(None);
        for (member, value) in overrides.as_object().expect("the overrides are an object") {
            request[member] = value.clone();
        }
        // A row whose `initialVlans` names the HCX network ACL names a list of the
        // account: the create resolves it in Amazon EC2's own store, so it is registered
        // there first (`tests/all/hcx_network_acl_references.rs` owns that rule).
        router.register_hcx_network_acl(&request);

        let (status, body) = call(&router, "CreateEnvironment", &request).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} ({}) {body}",
            error_type(&body)
        );
        if expected_status == 400 {
            assert!(
                error_type(&body).contains("ValidationException"),
                "case {case:?}: expected a ValidationException, got {}",
                error_type(&body)
            );
        } else {
            let id = body["environment"]["environmentId"].as_str().unwrap_or("");
            assert!(
                is_environment_id(id),
                "case {case:?}: expected a minted environmentId, got {id:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_environment_name_is_unique_within_the_account_and_region() {
    let router = emulator();
    let first = create(&router, Some("shared-name")).await;

    let (status, body) = call(
        &router,
        "CreateEnvironment",
        &router.create_request(Some("shared-name")),
    )
    .await;
    assert_eq!(
        status, 400,
        "a second environment named shared-name must be refused, got {status} {body}"
    );
    assert!(
        error_type(&body).contains("ValidationException"),
        "duplicate name: expected a ValidationException, got {}",
        error_type(&body)
    );

    // A different name is accepted, and is a different environment.
    let second = create(&router, Some("another-name")).await;
    assert_ne!(
        first, second,
        "two creates must mint two environments, both came back as {first}"
    );

    // The scope of the rule is one Region: the same name in another Region is a
    // different environment, and each ARN names the Region its caller addressed.
    // Each row is a Region token shape — three segments, and the four-segment form
    // the gov/iso partitions use.
    for other_region in ["eu-central-1", "us-gov-west-1"] {
        let (status, body) = call_in_region(
            &router,
            other_region,
            "CreateEnvironment",
            &router.create_request(Some("shared-name")),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {other_region}: the name is free in another Region, got {status} {body}"
        );
        let id = body["environment"]["environmentId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {other_region}: no environmentId in {body}"));
        assert_ne!(
            id, first,
            "case {other_region}: expected a new environment, got the {first} of {REGION}"
        );
        assert_eq!(
            body["environment"]["environmentArn"],
            json!(format!(
                "arn:aws:evs:{other_region}:{ACCOUNT}:environment/{id}"
            )),
            "case {other_region}: the ARN must name the Region the caller addressed, got {}",
            body["environment"]["environmentArn"]
        );

        // ...and the rule still bites inside that Region.
        let (status, body) = call_in_region(
            &router,
            other_region,
            "CreateEnvironment",
            &router.create_request(Some("shared-name")),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {other_region}: a second environment of that name must be refused, \
             got {status} {body}"
        );
    }
}

#[tokio::test]
async fn an_environment_is_only_visible_at_the_region_it_was_created_in() {
    // An Amazon EVS environment is a regional resource: it lives at the endpoint of
    // the Region it was created in, which is the Region its own ARN names.
    let router = emulator();
    let home = create(&router, Some("home-region-env")).await;

    // Each row: an operation that resolves an environment by id, and the request for
    // it. From any other Region every one of them is a miss.
    let rows: Vec<(&str, Value)> = vec![
        ("GetEnvironment", json!({ "environmentId": home })),
        ("ListEnvironmentVlans", json!({ "environmentId": home })),
        ("DeleteEnvironment", json!({ "environmentId": home })),
    ];
    for (operation, request) in rows {
        let (status, body) = call_in_region(&router, "eu-central-1", operation, &request).await;
        assert_eq!(
            status, 400,
            "case {operation:?} from another Region: expected HTTP 400, got {status} {body}"
        );
        assert!(
            error_type(&body).contains("ResourceNotFoundException"),
            "case {operation:?} from another Region: expected a ResourceNotFoundException, got {}",
            error_type(&body)
        );
    }

    // ListEnvironments reports "the environments in your account in the specified
    // Region" — so the other Region's list is empty and the home Region's is not.
    for (region, expected) in [("eu-central-1", Vec::new()), (REGION, vec![home.as_str()])] {
        let (status, body) = call_in_region(&router, region, "ListEnvironments", &json!({})).await;
        assert_eq!(
            status, 200,
            "case list in {region}: expected 200, got {status} {body}"
        );
        let got: Vec<&str> = body["environmentSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case list in {region}: no summaries in {body}"))
            .iter()
            .filter_map(|summary| summary["environmentId"].as_str())
            .collect();
        assert_eq!(
            got, expected,
            "case list in {region}: expected {expected:?}, got {got:?}"
        );
    }

    // And it is still fully usable at its own Region.
    let (status, body) = call(&router, "GetEnvironment", &json!({ "environmentId": home })).await;
    assert_eq!(
        status, 200,
        "the environment at its own Region: expected 200, got {status} {body}"
    );
}

#[tokio::test]
async fn the_client_token_has_no_effect_on_which_environment_is_created() {
    // "This parameter is not used in Amazon EVS currently. If you supply input for
    // this parameter, it will have no effect." (`clientToken` doc.) Two creates that
    // repeat one token are therefore two environments.
    let router = emulator();
    let mut request = router.create_request(None);
    request["clientToken"] = json!("tok-1");

    let (first_status, first) = call(&router, "CreateEnvironment", &request).await;
    // A second VPC, because one VPC carries one deployed environment: the repeated
    // token is what this test varies, nothing else.
    let (vpc_id, subnet_id) = router.fresh_network();
    request["vpcId"] = json!(vpc_id);
    request["serviceAccessSubnetId"] = json!(subnet_id);
    let (second_status, second) = call(&router, "CreateEnvironment", &request).await;
    assert_eq!(
        (first_status, second_status),
        (200, 200),
        "both creates must be accepted, got {first_status} {first} and {second_status} {second}"
    );
    assert_ne!(
        first["environment"]["environmentId"], second["environment"]["environmentId"],
        "a repeated clientToken must not be deduplicated, both creates returned {}",
        first["environment"]["environmentId"]
    );
}

#[tokio::test]
async fn unknown_resources_are_reported_as_not_found() {
    let router = emulator();
    // One real environment, so a miss is a miss on the lookup and not an empty store.
    let id = create(&router, Some("present")).await;
    let absent = "env-0000000000";
    let absent_arn = format!("arn:aws:evs:{REGION}:{ACCOUNT}:environment/{absent}");

    // Each row: an operation, its request, and the status it answers an absent
    // resource with. Every modeled client error of this awsJson1_0 service goes on the
    // wire as a 400, whatever exact code its `@httpError` trait names, so an operation
    // addressing an environment BY ID and the ARN-addressed tagging surface answer the
    // same status and differ only in the body — see `tests/all/error_wire_status.rs`
    // for the status rule and `tests/all/tagging_not_found.rs` for the tagging body.
    let rows: Vec<(&str, Value, u16)> = vec![
        ("GetEnvironment", json!({ "environmentId": absent }), 400),
        ("DeleteEnvironment", json!({ "environmentId": absent }), 400),
        (
            "ListEnvironmentVlans",
            json!({ "environmentId": absent }),
            400,
        ),
        (
            "ListTagsForResource",
            json!({ "resourceArn": absent_arn.clone() }),
            400,
        ),
        (
            "TagResource",
            json!({ "resourceArn": absent_arn.clone(), "tags": { "k": "v" } }),
            400,
        ),
        (
            "UntagResource",
            json!({ "resourceArn": absent_arn.clone(), "tagKeys": ["k"] }),
            400,
        ),
    ];

    for (operation, request, expected_status) in rows {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, expected_status,
            "case {operation:?} on an absent resource: expected HTTP {expected_status}, \
             got {status} {body}"
        );
        assert!(
            error_type(&body).contains("ResourceNotFoundException"),
            "case {operation:?}: expected a ResourceNotFoundException, got {}",
            error_type(&body)
        );
    }

    // An id that is not an environment id at all is refused before any lookup.
    let (status, body) = call(
        &router,
        "GetEnvironment",
        &json!({ "environmentId": "not-an-env-id" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a malformed environmentId must be refused, got {status} {body}"
    );

    // And the environment that does exist is still reachable.
    let (status, body) = call(&router, "GetEnvironment", &json!({ "environmentId": id })).await;
    assert_eq!(status, 200, "the present environment: got {status} {body}");
}

#[tokio::test]
async fn list_environments_pages_with_max_results_and_a_resumable_token() {
    let router = emulator();
    let mut created = BTreeSet::new();
    for index in 0..5 {
        created.insert(create(&router, Some(&format!("env-number-{index}"))).await);
    }

    // Every page size spans the collection exactly once, with no duplicates and no
    // gaps, however many pages it takes.
    for page_size in [1, 2, 3, 5, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "maxResults": page_size });
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let (status, body) = call(&router, "ListEnvironments", &request).await;
            assert_eq!(
                status, 200,
                "case page size {page_size}: expected 200, got {status} {body}"
            );
            let page = body["environmentSummaries"]
                .as_array()
                .unwrap_or_else(|| panic!("case page size {page_size}: no summaries in {body}"));
            assert!(
                page.len() <= page_size as usize,
                "case page size {page_size}: a page may not exceed the bound, got {}",
                page.len()
            );
            seen.extend(
                page.iter()
                    .filter_map(|summary| summary["environmentId"].as_str())
                    .map(str::to_string),
            );
            pages += 1;
            assert!(
                pages <= 10,
                "case page size {page_size}: pagination did not terminate after {pages} pages"
            );
            match body["nextToken"].as_str() {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }
        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case page size {page_size}: pages must not repeat an environment, got {seen:?}"
        );
        assert_eq!(
            unique, created,
            "case page size {page_size}: expected every environment exactly once, got {unique:?}"
        );
    }

    // A token this service did not issue, and one it issued more than 24 hours ago,
    // are both refused rather than silently restarting the scan.
    let stale = ::hex::encode("0:env-0000000000");
    for (case, token) in [("malformed", "not-a-token".to_string()), ("expired", stale)] {
        let (status, body) = call(
            &router,
            "ListEnvironments",
            &json!({ "nextToken": token.clone() }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case} token {token:?}: expected HTTP 400, got {status} {body}"
        );
        assert!(
            error_type(&body).contains("ValidationException"),
            "case {case} token: expected a ValidationException, got {}",
            error_type(&body)
        );
    }
}

#[tokio::test]
async fn list_environment_vlans_pages_with_max_results_and_a_resumable_token() {
    let router = emulator();
    // Settled, because a list against an environment that is still initializing is
    // refused — what `tests/all/list_while_initializing.rs` asserts.
    let id = create_settled(&router, Some("paged-vlans")).await;

    for page_size in [1, 3, 4, 10, 100] {
        let mut seen: Vec<i64> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "environmentId": id, "maxResults": page_size });
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let (status, body) = call(&router, "ListEnvironmentVlans", &request).await;
            assert_eq!(
                status, 200,
                "case page size {page_size}: expected 200, got {status} {body}"
            );
            let page = body["environmentVlans"]
                .as_array()
                .unwrap_or_else(|| panic!("case page size {page_size}: no VLANs in {body}"));
            assert!(
                page.len() <= page_size as usize,
                "case page size {page_size}: a page may not exceed the bound, got {}",
                page.len()
            );
            seen.extend(page.iter().filter_map(|vlan| vlan["vlanId"].as_i64()));
            pages += 1;
            assert!(
                pages <= 12,
                "case page size {page_size}: pagination did not terminate after {pages} pages"
            );
            match body["nextToken"].as_str() {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }
        let unique: BTreeSet<i64> = seen.iter().copied().collect();
        assert_eq!(
            (unique.len(), seen.len()),
            (10, 10),
            "case page size {page_size}: expected the ten VLAN subnets exactly once, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn tags_are_stored_on_the_environment_and_read_back_by_arn() {
    let router = emulator();
    let (status, created) = call(
        &router,
        "CreateEnvironment",
        &{
            let mut request = router.create_request(Some("tagged-env"));
            request["tags"] = json!({ "team": "platform", "stage": "dev" });
            request
        },
    )
    .await;
    assert_eq!(status, 200, "create: expected 200, got {status} {created}");
    let arn = created["environment"]["environmentArn"]
        .as_str()
        .expect("the created environment has an ARN")
        .to_string();

    // Each step: what the tag set must be after it.
    let steps: Vec<(&str, &str, Value, Value)> = vec![
        (
            "the tags CreateEnvironment was given",
            "ListTagsForResource",
            json!({ "resourceArn": arn }),
            json!({ "team": "platform", "stage": "dev" }),
        ),
        (
            "TagResource adds a key and overwrites one",
            "TagResource",
            json!({ "resourceArn": arn, "tags": { "stage": "prod", "owner": "evs" } }),
            json!({ "team": "platform", "stage": "prod", "owner": "evs" }),
        ),
        (
            "UntagResource removes only the named keys",
            "UntagResource",
            json!({ "resourceArn": arn, "tagKeys": ["team", "absent"] }),
            json!({ "stage": "prod", "owner": "evs" }),
        ),
        (
            "UntagResource is idempotent",
            "UntagResource",
            json!({ "resourceArn": arn, "tagKeys": ["team"] }),
            json!({ "stage": "prod", "owner": "evs" }),
        ),
    ];

    for (case, operation, request, expected) in steps {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from {operation}, got {status} {body}"
        );
        let (status, listed) = call(
            &router,
            "ListTagsForResource",
            &json!({ "resourceArn": arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from ListTagsForResource, got {status} {listed}"
        );
        assert_eq!(
            listed["tags"], expected,
            "case {case:?}: expected the tags {expected}, got {}",
            listed["tags"]
        );
    }
}

#[tokio::test]
async fn the_store_is_bounded_and_reclaims_only_finished_environments() {
    use service_com_amazonaws_evs_amazonelasticvmwareservice::state::MAX_ENVIRONMENTS;

    let router = emulator();
    let first = create(&router, None).await;
    for index in 1..MAX_ENVIRONMENTS {
        let (status, body) = call(&router, "CreateEnvironment", &router.create_request(None)).await;
        assert_eq!(
            status, 200,
            "create {index} of {MAX_ENVIRONMENTS}: expected 200, got {status} {body}"
        );
    }

    // At the cap, a create is refused with a quota error rather than growing the store.
    let (status, body) = call(&router, "CreateEnvironment", &router.create_request(None)).await;
    assert_eq!(
        status, 400,
        "create past the cap of {MAX_ENVIRONMENTS}: expected HTTP 400, got {status} {body}"
    );
    assert!(
        error_type(&body).contains("ServiceQuotaExceededException"),
        "create past the cap: expected a ServiceQuotaExceededException, got {}",
        error_type(&body)
    );

    // A live record is never evicted to make room — not even one that is on its way
    // out, until its deletion has actually come due.
    let (status, body) = call(
        &router,
        "DeleteEnvironment",
        &json!({ "environmentId": first }),
    )
    .await;
    assert_eq!(status, 200, "delete: expected 200, got {status} {body}");
    let (status, body) = call(&router, "CreateEnvironment", &router.create_request(None)).await;
    assert_eq!(
        status, 400,
        "create while the only deletion is still DELETING: expected HTTP 400, got {status} {body}"
    );

    // Once it is DELETED, its room is reclaimed and the next create is admitted; the
    // purged environment is then reported as absent.
    std::thread::sleep(PAST_THE_TRANSITION);
    let (status, body) = call(&router, "CreateEnvironment", &router.create_request(None)).await;
    assert_eq!(
        status, 200,
        "create after a deletion came due: expected 200, got {status} {body}"
    );
    let (status, body) = call(
        &router,
        "GetEnvironment",
        &json!({ "environmentId": first }),
    )
    .await;
    assert_eq!(
        status, 400,
        "get a purged environment: expected HTTP 400, got {status} {body}"
    );
}

#[tokio::test]
async fn every_create_mints_a_distinct_environment_id_and_subnet_ids() {
    let router = emulator();
    let mut environment_ids = BTreeSet::new();
    let mut subnet_ids = BTreeSet::new();

    for index in 0..25 {
        // Settled, because the list below is refused against an environment that is
        // still initializing (`tests/all/list_while_initializing.rs`).
        let id = create_settled(&router, Some(&format!("unique-{index}"))).await;
        assert!(
            is_environment_id(&id),
            "create {index}: environmentId {id:?} must match ^(env-[a-zA-Z0-9]{{10}})$"
        );
        assert!(
            environment_ids.insert(id.clone()),
            "create {index}: environmentId {id} was already minted"
        );

        let (status, body) = call(
            &router,
            "ListEnvironmentVlans",
            &json!({ "environmentId": id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "create {index}: expected 200 from ListEnvironmentVlans, got {status} {body}"
        );
        for vlan in body["environmentVlans"]
            .as_array()
            .unwrap_or_else(|| panic!("create {index}: no VLANs in {body}"))
        {
            let subnet = vlan["subnetId"]
                .as_str()
                .unwrap_or_else(|| panic!("create {index}: no subnetId in {vlan}"))
                .to_string();
            assert!(
                subnet_ids.insert(subnet.clone()),
                "create {index}: subnetId {subnet} was already minted"
            );
        }
    }
    assert_eq!(
        subnet_ids.len(),
        250,
        "25 environments of ten VLAN subnets each need 250 distinct subnet ids, got {}",
        subnet_ids.len()
    );
}
