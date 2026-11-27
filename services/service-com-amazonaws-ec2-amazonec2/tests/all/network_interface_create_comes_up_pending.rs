// SPDX-License-Identifier: CC0-1.0

//! What the status `CreateNetworkInterface` answers with means, and what it does NOT mean.
//!
//! Driven over the wire this service speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=<Operation>`, answered
//! with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: the create's OWN answer carries the status of the instant of minting —
//! `pending`, which is what AWS is recorded answering there and which is not a member of the
//! modelled `NetworkInterfaceStatus` at all
//! (`smithy_query(query="shape",shape="NetworkInterfaceStatus")`: `available`, `associated`,
//! `attaching`, `in-use`, `detaching`) — while the interface it minted is STORED, and therefore
//! read, in the resting status of an interface nothing holds. So that status belongs to one
//! answer: it is not a state a caller has to wait out, nothing about the interface is withheld
//! while it is reported, and a delete taken straight after the create is served. An interface
//! already HELD when it reaches the store — one another Amazon Web Services service created to
//! serve a resource of the account — is reported in the status that holds it (`in-use`) from
//! the call that placed it.
//!
//! What every READ of a created interface reports, over a table of creates and both read
//! paths, is held by the sibling suite `tests/all/network_interface_created_status.rs`.
//!
//! Nothing here waits for a span or asserts on elapsed time, so no row depends on how fast the
//! host runs.

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, NetworkInterfaceSpec,
    NETWORK_INTERFACE_STATUS_AVAILABLE, NETWORK_INTERFACE_STATUS_IN_USE,
    NETWORK_INTERFACE_STATUS_PENDING,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, indexed};

/// The Region every call here is signed for; the account's default subnets are seeded for it.
const REGION: &str = "us-east-1";

/// One form field.
fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// The account's default subnet ids, in store order (one per zone of the Region).
fn subnet_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .subnets
        .keys()
        .cloned()
        .collect()
}

/// One call that must be served, as its body.
async fn ok(
    case: &str,
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> String {
    let (status, body) = call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// The status `DescribeNetworkInterfaces` reports for one interface.
async fn described_status(case: &str, router: &MultiServiceRouter, id: &str) -> String {
    let body = ok(
        case,
        router,
        "DescribeNetworkInterfaces",
        &indexed("NetworkInterfaceId", &[id]),
    )
    .await;
    element(&body, "status")
        .unwrap_or_else(|| panic!("case {case:?}: the reported interface carries no status: {body}"))
}

/// One row of the table below: what the row is about, which of the account's subnets the
/// interface sits in, and the fields the create carries beyond `SubnetId`.
#[derive(Debug, Clone)]
struct CreateRow {
    /// What the row is about.
    case: &'static str,
    /// Which of the account's subnets the interface sits in.
    subnet_index: usize,
    /// The fields the create carries beyond `SubnetId`.
    extra: Vec<(String, String)>,
}

/// Rule: the create answers the minting status, and the interface it minted is held — and so
/// read — at rest. The rows differ along what the create carries, because none of it bears on
/// either status.
#[tokio::test]
async fn the_create_answers_the_minting_status_and_stores_the_resting_one() {
    let (router, engine) = super::ec2_query::emulator();
    let subnets = subnet_ids(&engine);
    assert!(
        subnets.len() >= 2,
        "the account's default VPC must have a subnet per zone, got {subnets:?}"
    );

    let rows = [
        CreateRow {
            case: "the subnet alone",
            subnet_index: 0,
            extra: Vec::new(),
        },
        CreateRow {
            case: "a description and a tag",
            subnet_index: 1,
            extra: vec![
                param("Description", "a described interface"),
                param("TagSpecification.1.ResourceType", "network-interface"),
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", "web"),
            ],
        },
        CreateRow {
            case: "a client token, so the create can be replayed",
            subnet_index: 0,
            extra: vec![param("ClientToken", "created-status-token")],
        },
    ];

    for CreateRow {
        case,
        subnet_index,
        extra,
    } in rows
    {
        let mut params = vec![param("SubnetId", &subnets[subnet_index])];
        params.extend(extra.clone());
        let created = ok(case, &router, "CreateNetworkInterface", &params).await;
        let network_interface_id = element(&created, "networkInterfaceId")
            .unwrap_or_else(|| panic!("case {case:?}: the create must answer an id: {created}"));
        let created_status = element(&created, "status");
        assert_eq!(
            created_status.as_deref(),
            Some(NETWORK_INTERFACE_STATUS_PENDING),
            "case {case:?}: expected the create to answer {NETWORK_INTERFACE_STATUS_PENDING:?}, \
             got {created_status:?} in {created}"
        );

        // The record it wrote rests in the status every read of it reports, so the minting
        // status is nowhere in the store.
        {
            let store = engine.state::<Ec2State>();
            let guard = store.lock();
            let held = guard
                .network_interfaces
                .get(&network_interface_id)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the store must hold the created interface")
                });
            assert_eq!(
                held.status.as_deref(),
                Some(NETWORK_INTERFACE_STATUS_AVAILABLE),
                "case {case:?}: the stored interface rests in \
                 {NETWORK_INTERFACE_STATUS_AVAILABLE:?}, got {:?}",
                held.status
            );
        }
        let read_back = described_status(case, &router, &network_interface_id).await;
        assert_eq!(
            read_back, NETWORK_INTERFACE_STATUS_AVAILABLE,
            "case {case:?}: a read of the created interface reports \
             {NETWORK_INTERFACE_STATUS_AVAILABLE:?}, got {read_back:?}"
        );

        // A replay of the very same create — the idempotent repeat a client retrying makes —
        // is the same answer about the same interface, minting status and all.
        if extra
            .iter()
            .any(|(name, _)| name == "ClientToken")
        {
            let replayed = ok(case, &router, "CreateNetworkInterface", &params).await;
            assert_eq!(
                (
                    element(&replayed, "networkInterfaceId").as_deref(),
                    element(&replayed, "status").as_deref(),
                ),
                (
                    Some(network_interface_id.as_str()),
                    Some(NETWORK_INTERFACE_STATUS_PENDING),
                ),
                "case {case:?}: a replayed create answers the same interface and the same \
                 status, got {replayed}"
            );
        }
    }
}

/// Rule, the other half: a caller does not have to wait anything out before using the
/// interface it was handed — the interface the create answered is the one the account holds,
/// and the delete of an interface nothing holds is served straight away.
#[tokio::test]
async fn a_freshly_created_interface_is_the_one_the_account_holds() {
    let (router, engine) = super::ec2_query::emulator();
    let subnets = subnet_ids(&engine);
    let case = "a freshly created interface";

    let created = ok(
        case,
        &router,
        "CreateNetworkInterface",
        &[param("SubnetId", &subnets[0])],
    )
    .await;
    let network_interface_id = element(&created, "networkInterfaceId")
        .unwrap_or_else(|| panic!("case {case:?}: the create must answer an id: {created}"));
    {
        let store = engine.state::<Ec2State>();
        let guard = store.lock();
        let held = guard
            .network_interfaces
            .get(&network_interface_id)
            .unwrap_or_else(|| panic!("case {case:?}: the store must hold the created interface"));
        assert_eq!(
            held.subnet_id.as_deref(),
            Some(subnets[0].as_str()),
            "case {case:?}: the stored interface sits in the subnet the create named, got {:?}",
            held.subnet_id
        );
    }

    // A delete taken straight after the create is served: nothing HOLDS the interface, and
    // that — not any status it might be reported in — is what a delete refuses on.
    let deleted = ok(
        case,
        &router,
        "DeleteNetworkInterface",
        &[param("NetworkInterfaceId", &network_interface_id)],
    )
    .await;
    assert!(
        deleted.contains("DeleteNetworkInterfaceResponse"),
        "case {case:?}: the delete must answer its own envelope, got {deleted}"
    );
    let gone = ok(case, &router, "DescribeNetworkInterfaces", &[]).await;
    assert!(
        !gone.contains(&network_interface_id),
        "case {case:?}: a deleted interface is reported by nothing, got {gone}"
    );
}

/// The boundary of the rule: an interface that is already HELD when it reaches the store —
/// one another Amazon Web Services service created to serve a resource of the account — is
/// reported in the status that holds it rather than at rest.
#[tokio::test]
async fn an_interface_a_service_holds_is_in_use() {
    let (router, engine) = super::ec2_query::emulator();
    let subnets = subnet_ids(&engine);
    let case = "an interface a service holds";

    let network_interface_id = {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let subnet = guard
            .subnets
            .get(&subnets[0])
            .expect("the account's default subnet")
            .clone();
        let cidr_block = subnet.cidr_block.clone().unwrap_or_default();
        let address = guard
            .mint_private_ip_in_cidr(&cidr_block)
            .expect("the default subnet has a free address");
        guard
            .insert_network_interface(NetworkInterfaceSpec {
                subnet_id: subnets[0].clone(),
                vpc_id: subnet.vpc_id.clone().unwrap_or_default(),
                availability_zone: subnet.availability_zone.clone().unwrap_or_default(),
                availability_zone_id: subnet.availability_zone_id.clone().unwrap_or_default(),
                region: REGION.to_string(),
                interface_type: "interface".to_string(),
                private_ip_addresses: vec![address],
                requester: Some("amazon-elb".to_string()),
                ..NetworkInterfaceSpec::default()
            })
            .expect("the store holds the seeded interface")
    };

    let status = described_status(case, &router, &network_interface_id).await;
    assert_eq!(
        status, NETWORK_INTERFACE_STATUS_IN_USE,
        "case {case:?}: expected {NETWORK_INTERFACE_STATUS_IN_USE:?} for an interface a \
         service holds, got {status:?}"
    );

    // And the two lifecycles do not drift into each other: an interface created beside the
    // held one rests `available`, and the held one keeps the status that holds it throughout.
    let created = ok(
        case,
        &router,
        "CreateNetworkInterface",
        &[param("SubnetId", &subnets[1])],
    )
    .await;
    let unheld = element(&created, "networkInterfaceId")
        .unwrap_or_else(|| panic!("case {case:?}: the create must answer an id: {created}"));
    let unheld_status = described_status(case, &router, &unheld).await;
    assert_eq!(
        unheld_status, NETWORK_INTERFACE_STATUS_AVAILABLE,
        "case {case:?}: the interface created beside it is held by nothing, so it rests \
         {NETWORK_INTERFACE_STATUS_AVAILABLE:?}, got {unheld_status:?}"
    );
    let later = described_status(case, &router, &network_interface_id).await;
    assert_eq!(
        later, NETWORK_INTERFACE_STATUS_IN_USE,
        "case {case:?}: an interface a service holds keeps the status that holds it, got \
         {later:?}"
    );
}
