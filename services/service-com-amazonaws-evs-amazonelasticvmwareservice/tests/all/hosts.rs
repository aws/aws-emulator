// SPDX-License-Identifier: CC0-1.0

//! `CreateEnvironmentHost` / `ListEnvironmentHosts` over awsJson1_0: the ESX hosts an
//! environment holds once it is deployed.
//!
//! The rules under test, each from the model's own documentation
//! (`smithy_query(query="operation",operation="CreateEnvironmentHost")` and the shape
//! docs it quotes):
//!
//! * `CreateEnvironmentHost` declares NO `ResourceNotFoundException` (its errors are
//!   `ThrottlingException` and `ValidationException` only), so an `environmentId` that
//!   names no environment is refused with the `ValidationException` that names it;
//! * "This action can only be used after the Amazon EVS environment is deployed";
//! * "You cannot use the `dedicatedHostId` and `placementGroupId` parameters together
//!   in the same `CreateEnvironmentHost` action";
//! * a `placementGroupId` or `dedicatedHostId` names an EC2 resource of the caller's own
//!   account, so one the account does not hold is refused with the member named (the
//!   accepted side of that rule lives in `host_placement_references`, which mints the
//!   resources through Amazon EC2 itself);
//! * "DNS hostnames for hosts must be unique across Amazon EVS environments";
//! * an ESX host's management interface lives on the `vmkManagement` VLAN, so its
//!   address comes out of that VLAN subnet and that subnet's usable range is what
//!   bounds how many hosts one environment can hold;
//! * what is created is PERSISTED: `ListEnvironmentHosts` reports it, pages over it
//!   with `maxResults` and resumes at the host the page stopped at.
//!
//! The harness (`emulator`, `call`, `create`) is the one `environment_lifecycle`
//! already owns, so every test drives the service the way a client does.

use std::time::Duration;

use serde_json::{json, Value};

use crate::environment_lifecycle::{
    call, create, emulator, error_type, poll_environment_created, vlans_with_prefix, Emulator,
    POLL_DEADLINE, POLL_INTERVAL,
};

/// An environment id that is well formed (`^(env-[a-zA-Z0-9]{10})$`) and names no
/// environment.
const ABSENT_ENVIRONMENT: &str = "env-0000000000";

/// Long enough for the transition this service owes a new environment or host to come
/// due (its span is under a second by construction).
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// A `CreateEnvironmentHost` request for one host name, with `extra` merged into the
/// `host` object.
fn host_request(environment_id: &str, host_name: &str, extra: Value) -> Value {
    let mut host = json!({
        "hostName": host_name,
        "keyName": "evs-key",
        "instanceType": "i4i.metal",
    });
    for (member, value) in extra.as_object().expect("the extra members are an object") {
        host[member] = value.clone();
    }
    json!({ "environmentId": environment_id, "host": host })
}

/// Create an environment and wait for the deployment the clock owes it, so a host can
/// be added to it.
async fn deployed_environment(router: &Emulator, name: &str) -> String {
    let environment = create(router, Some(name)).await;
    std::thread::sleep(PAST_THE_TRANSITION);
    environment
}

#[tokio::test]
async fn a_host_is_added_to_a_deployed_environment_and_read_back_by_list() {
    let router = emulator();
    let environment = deployed_environment(&router, "host-env").await;

    let (status, body) = call(
        &router,
        "CreateEnvironmentHost",
        &host_request(&environment, "esx-1", json!({})),
    )
    .await;
    assert_eq!(
        status, 200,
        "create host: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    let host = body["host"].clone();
    for (member, expected) in [
        ("hostName", json!("esx-1")),
        ("keyName", json!("evs-key")),
        ("instanceType", json!("i4i.metal")),
        // A host is CREATING until it has been built and joined the environment.
        ("hostState", json!("CREATING")),
        // The management address comes out of the environment's own vmkManagement VLAN
        // subnet (10.0.0.0/28 here), whose first usable address is its fifth: AWS
        // reserves the first four addresses of a subnet. The host's own address is the
        // one its ACTIVE interface holds; its standby interface holds the next one, both
        // being interfaces of that subnet (`NetworkInterface` doc).
        ("ipAddress", json!("10.0.0.4")),
    ] {
        assert_eq!(
            host[member], expected,
            "create host: {member}: expected {expected}, got {}",
            host[member]
        );
    }
    assert_eq!(
        body["environmentSummary"]["environmentId"],
        json!(environment),
        "create host: the summary must report the environment the host joined, got {}",
        body["environmentSummary"]["environmentId"]
    );

    // The read-back, and a second host taking the next free address of the same block.
    let (status, body) = call(
        &router,
        "CreateEnvironmentHost",
        &host_request(&environment, "esx-2", json!({})),
    )
    .await;
    assert_eq!(status, 200, "second host: got {status} {body}");
    assert_eq!(
        body["host"]["ipAddress"],
        json!("10.0.0.6"),
        "second host: expected the next address of the block no interface of the first host \
         holds, got {}",
        body["host"]["ipAddress"]
    );

    let (status, body) = call(
        &router,
        "ListEnvironmentHosts",
        &json!({ "environmentId": environment }),
    )
    .await;
    assert_eq!(status, 200, "list hosts: expected 200, got {status} {body}");
    let listed: Vec<&str> = body["environmentHosts"]
        .as_array()
        .unwrap_or_else(|| panic!("list hosts: no environmentHosts in {body}"))
        .iter()
        .filter_map(|host| host["hostName"].as_str())
        .collect();
    assert_eq!(
        listed,
        vec!["esx-1", "esx-2"],
        "list hosts: expected both hosts in hostName order, got {listed:?}"
    );

    // `maxResults` bounds the page and `nextToken` resumes at the host it stopped at.
    let (status, page) = call(
        &router,
        "ListEnvironmentHosts",
        &json!({ "environmentId": environment, "maxResults": 1 }),
    )
    .await;
    assert_eq!(status, 200, "first page: got {status} {page}");
    assert_eq!(
        page["environmentHosts"].as_array().map(Vec::len),
        Some(1),
        "first page: maxResults of 1 must bound the page, got {}",
        page["environmentHosts"]
    );
    let token = page["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("first page: expected a nextToken in {page}"))
        .to_string();
    let (status, page) = call(
        &router,
        "ListEnvironmentHosts",
        &json!({ "environmentId": environment, "maxResults": 1, "nextToken": token }),
    )
    .await;
    assert_eq!(status, 200, "second page: got {status} {page}");
    assert_eq!(
        page["environmentHosts"][0]["hostName"],
        json!("esx-2"),
        "second page: expected the host the first page stopped at, got {}",
        page["environmentHosts"][0]["hostName"]
    );

    // The build the clock owes each host comes due: looked for again and again until every
    // host IS `CREATED`, with generous room, rather than waiting a span a loaded host
    // measures differently.
    let deadline = std::time::Instant::now() + POLL_DEADLINE;
    let listed = loop {
        let (status, body) = call(
            &router,
            "ListEnvironmentHosts",
            &json!({ "environmentId": environment }),
        )
        .await;
        assert_eq!(status, 200, "list after the transition: got {status} {body}");
        let hosts = body["environmentHosts"]
            .as_array()
            .unwrap_or_else(|| panic!("list after the transition: no environmentHosts in {body}"))
            .clone();
        if !hosts.is_empty()
            && hosts
                .iter()
                .all(|host| host["hostState"] == json!("CREATED"))
        {
            break hosts;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "after the transition: every host must reach CREATED; last read {body}"
        );
        ::tokio::time::sleep(POLL_INTERVAL).await;
    };
    assert_eq!(
        listed.len(),
        2,
        "after the transition: expected the two hosts, got {listed:?}"
    );
}

#[tokio::test]
async fn an_absent_environment_is_a_validation_exception_for_the_host_surface() {
    let router = emulator();

    // `CreateEnvironmentHost` models no not-found error at all, so the refusal is the
    // ValidationException that names the id; `ListEnvironmentHosts` DOES model one, so
    // the same absent environment is a ResourceNotFoundException there. Every modelled
    // CLIENT error of this awsJson1_0 service is reported with HTTP 400 — see
    // `tests/all/error_wire_status.rs`, which owns that rule.
    for (operation, request, expected) in [
        (
            "CreateEnvironmentHost",
            host_request(ABSENT_ENVIRONMENT, "esx-absent", json!({})),
            "ValidationException",
        ),
        (
            "ListEnvironmentHosts",
            json!({ "environmentId": ABSENT_ENVIRONMENT }),
            "ResourceNotFoundException",
        ),
    ] {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {operation:?}: expected HTTP 400, got {status} {body}"
        );
        assert!(
            error_type(&body).contains(expected),
            "case {operation:?}: expected {expected}, got {} in {body}",
            error_type(&body)
        );
        assert!(
            body["message"]
                .as_str()
                .unwrap_or_default()
                .contains(ABSENT_ENVIRONMENT),
            "case {operation:?}: the refusal must name the id the request gave, got {}",
            body["message"]
        );
        assert_eq!(
            body["host"],
            Value::Null,
            "case {operation:?}: a refused request must report no host, got {}",
            body["host"]
        );
    }
}

#[tokio::test]
async fn every_host_precondition_is_refused_with_a_validation_exception() {
    let router = emulator();
    let deployed = deployed_environment(&router, "host-rules").await;
    // An environment that has not finished deploying: its create has only just run.
    let creating = create(&router, Some("still-creating")).await;
    // One host already added, so the unique-name rule has something to collide with.
    let (status, body) = call(
        &router,
        "CreateEnvironmentHost",
        &host_request(&deployed, "esx-taken", json!({})),
    )
    .await;
    assert_eq!(status, 200, "the first host: got {status} {body}");

    // Each row: the case, the request, and the input members the refusal must name —
    // none for a row that is accepted.
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "a host of a deployed environment with a fresh name",
            host_request(&deployed, "esx-fresh", json!({})),
            vec![],
        ),
        (
            "an environment this service holds no record of",
            host_request(ABSENT_ENVIRONMENT, "esx-absent", json!({})),
            vec!["environmentId"],
        ),
        (
            "an environment that has not finished deploying",
            host_request(&creating, "esx-too-early", json!({})),
            vec!["environmentId"],
        ),
        (
            "both placement mechanisms on one host",
            host_request(
                &deployed,
                "esx-placement",
                json!({
                    "dedicatedHostId": "h-0123456789abcdef0",
                    "placementGroupId": "pg-0123456789abcdef0",
                }),
            ),
            vec!["host"],
        ),
        (
            // The EC2 resources a host is placed on belong to the caller's account, and
            // this account holds neither of these two: the held case is covered by
            // `host_placement_references`, which mints them through Amazon EC2 itself.
            "a Dedicated Host the account does not hold",
            host_request(
                &deployed,
                "esx-dedicated",
                json!({ "dedicatedHostId": "h-0123456789abcdef0" }),
            ),
            vec!["host.dedicatedHostId"],
        ),
        (
            "a placement group the account does not hold",
            host_request(
                &deployed,
                "esx-placement-group",
                json!({ "placementGroupId": "pg-0123456789abcdef0" }),
            ),
            vec!["host.placementGroupId"],
        ),
        (
            "a host name another host already carries",
            host_request(&deployed, "esx-taken", json!({})),
            vec!["host.hostName"],
        ),
        (
            "a name that is taken AND both placement mechanisms",
            host_request(
                &deployed,
                "esx-taken",
                json!({
                    "dedicatedHostId": "h-0123456789abcdef0",
                    "placementGroupId": "pg-0123456789abcdef0",
                }),
            ),
            vec!["host", "host.hostName"],
        ),
    ];

    for (case, request, expected_members) in rows {
        let (status, body) = call(&router, "CreateEnvironmentHost", &request).await;
        if expected_members.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} ({}) {body}",
                error_type(&body)
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({}) {body}",
            error_type(&body)
        );
        assert!(
            error_type(&body).contains("ValidationException"),
            "case {case:?}: expected a ValidationException (this operation models no \
             ResourceNotFoundException at all), got {}",
            error_type(&body)
        );
        assert_eq!(
            body["host"],
            Value::Null,
            "case {case:?}: a refused request must report no host, got {}",
            body["host"]
        );
        let named: Vec<&str> = body["fieldList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no fieldList in {body}"))
            .iter()
            .filter_map(|entry| entry["name"].as_str())
            .collect();
        assert_eq!(
            named, expected_members,
            "case {case:?}: expected one entry per failed precondition \
             ({expected_members:?}), got {named:?} in {body}"
        );
    }
}

#[tokio::test]
async fn the_vmk_management_subnet_bounds_how_many_hosts_an_environment_holds() {
    // An ESX host's interfaces live on the `vmkManagement` VLAN, so that subnet's usable
    // range is what bounds the host count. AWS leaves the first four addresses and the
    // last address of a subnet unusable (Amazon VPC User Guide, subnet sizing), so a
    // /<prefix> block has 2^(32-prefix) - 5 usable addresses — and each host takes one per
    // elastic network interface Amazon EVS provisions for it: "Amazon EVS provisions two
    // identically configured ENIs in the VMkernel management subnet during host creation"
    // (`NetworkInterface` doc), and no two interfaces of a subnet can hold one address. So
    // the block holds (2^(32-prefix) - 5) / 2 hosts.
    //
    // Each row is a different block size, which is what the rule generalises over.
    let rows: [(u8, usize); 2] = [(28, 5), (27, 13)];
    for (prefix_len, expected_hosts) in rows {
        let router = emulator();
        let mut request = router.create_request(Some("host-bound"));
        request["initialVlans"] = vlans_with_prefix(prefix_len);
        let (status, body) = call(&router, "CreateEnvironment", &request).await;
        assert_eq!(
            status, 200,
            "case /{prefix_len}: create expected 200, got {status} {body}"
        );
        let environment = body["environment"]["environmentId"]
            .as_str()
            .unwrap_or_else(|| panic!("case /{prefix_len}: no environmentId in {body}"))
            .to_string();
        // The deployment the clock owes the environment, waited for through the crate's
        // own poll-until helper, so a host can be added to it.
        poll_environment_created(&router, &format!("/{prefix_len}"), &environment).await;

        let mut admitted = 0usize;
        let mut addresses: Vec<String> = Vec::new();
        for index in 0..=expected_hosts {
            let (status, body) = call(
                &router,
                "CreateEnvironmentHost",
                &host_request(&environment, &format!("esx-{index}"), json!({})),
            )
            .await;
            if status != 200 {
                assert_eq!(
                    status, 400,
                    "case /{prefix_len}: host {index}: expected HTTP 400, got {status} {body}"
                );
                assert!(
                    error_type(&body).contains("ValidationException"),
                    "case /{prefix_len}: host {index}: expected a ValidationException, got {}",
                    error_type(&body)
                );
                break;
            }
            admitted += 1;
            addresses.push(
                body["host"]["ipAddress"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case /{prefix_len}: no ipAddress in {body}"))
                    .to_string(),
            );
        }
        assert_eq!(
            admitted, expected_hosts,
            "case /{prefix_len}: expected the subnet's usable addresses to bound the \
             host count at {expected_hosts}, got {admitted}"
        );
        let distinct: std::collections::BTreeSet<&String> = addresses.iter().collect();
        assert_eq!(
            distinct.len(),
            addresses.len(),
            "case /{prefix_len}: no two hosts may take the same address, got {addresses:?}"
        );
    }
}
