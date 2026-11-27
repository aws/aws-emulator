// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how this service PROJECTS its persisted records, driven over
//! the wire through the harness in [`super::accelerator_lifecycle`].
//!
//! Five rules, each stated once and asserted over a SET of inputs (a recorded request
//! is one row):
//!
//! 1. Every `IpSet` an accelerator is projected into reports one `IpAddressDetails`
//!    entry per address of that set, in the same order as `IpAddresses`, each naming
//!    that address and the distinct network zone it sits in.
//! 2. A listener reports the `PortRanges`, `Protocol` and `ClientAffinity` last
//!    persisted for it, through every operation that projects a listener.
//! 3. A minted ARN's final segment is a lowercase-hex id of the width that resource's
//!    ARNs carry: 8 for a listener, 12 for an endpoint group.
//! 4. Every DNS name an accelerator is projected with announces the host THIS emulator
//!    answers on.
//! 5. For an enumerated member a resource carries, EVERY value the MODEL admits for it
//!    is accepted and projected back unchanged by the read of that resource — the value
//!    sets come from the model's own rendered constants, never from a list written here,
//!    so a value set the upstream model gains or loses is exercised as it stands.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::accelerator_lifecycle::{
    arn_of, call, call_on_host, create_accelerator, emulator, emulator_served_at, error_code, ok,
    AWS_ENDPOINT_HOST, UNSERVED_HOST,
};
// Rule 5 reads every value set off the constants this crate RENDERS from its own model, so
// no value is written out here: the rule follows the model rather than a copy of it.
use service_com_amazonaws_globalaccelerator_globalaccelerator_v20180706::ops::types::{
    CLIENT_AFFINITY_VALUES, CUSTOM_ROUTING_PROTOCOL_VALUES, HEALTH_CHECK_PROTOCOL_VALUES,
    IP_ADDRESS_FAMILY_VALUES, IP_ADDRESS_TYPE_VALUES, PROTOCOL_VALUES,
};

/// `NetworkZone`'s modelled constraint: `@length(min: 0, max: 24)` on
/// `com.amazonaws.globalaccelerator#NetworkZone` (smithy_query query="shape",
/// shape="com.amazonaws.globalaccelerator#IpAddressDetail").
const NETWORK_ZONE_MAX_LEN: usize = 24;

/// Rule 1, asserted over one projected `Accelerator`.
fn assert_ip_address_details(case: &str, accelerator: &Value) {
    let ip_sets = accelerator["IpSets"].as_array().cloned().unwrap_or_default();
    assert!(
        !ip_sets.is_empty(),
        "case {case}: expected at least one IpSet, got {accelerator}"
    );
    for (set_index, set) in ip_sets.iter().enumerate() {
        let addresses = set["IpAddresses"].as_array().cloned().unwrap_or_default();
        let details = set["IpAddressDetails"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case}: IpSets[{set_index}] reports no IpAddressDetails, got {set}")
            });
        assert_eq!(
            details.len(),
            addresses.len(),
            "case {case}: IpSets[{set_index}] has {} addresses, so expected as many \
             IpAddressDetails entries, got {details:?}",
            addresses.len()
        );
        let mut zones: Vec<String> = Vec::with_capacity(details.len());
        for (index, detail) in details.iter().enumerate() {
            assert_eq!(
                detail["IpAddress"], addresses[index],
                "case {case}: IpSets[{set_index}].IpAddressDetails[{index}] should carry \
                 address {}, got {detail}",
                addresses[index]
            );
            let zone = detail["NetworkZone"].as_str().unwrap_or_else(|| {
                panic!(
                    "case {case}: IpSets[{set_index}].IpAddressDetails[{index}] carries no \
                     NetworkZone, got {detail}"
                )
            });
            assert!(
                !zone.is_empty() && zone.chars().count() <= NETWORK_ZONE_MAX_LEN,
                "case {case}: a NetworkZone is a non-empty string of at most \
                 {NETWORK_ZONE_MAX_LEN} characters, got {zone:?}"
            );
            assert!(
                !zones.iter().any(|seen| seen == zone),
                "case {case}: IpSets[{set_index}] places two addresses in zone {zone:?}; the \
                 zones of one address set are distinct, got {details:?}"
            );
            zones.push(zone.to_string());
        }
    }
}

#[tokio::test]
async fn every_ip_set_reports_one_address_detail_per_static_address() {
    let router = emulator();

    // The rows differ along what the rule generalises over: how many addresses the
    // caller supplied (none, one, both) and how many address sets the accelerator has
    // (an IPV4 accelerator has one; a DUAL_STACK one also has an IPv6 set).
    let rows: [(&str, Value, usize); 4] = [
        ("ipv4-allocated", json!({}), 1),
        (
            "ipv4-one-supplied",
            json!({ "IpAddresses": ["198.51.100.10"] }),
            1,
        ),
        (
            "ipv4-two-supplied",
            json!({ "IpAddresses": ["198.51.100.11", "198.51.100.12"] }),
            1,
        ),
        ("dual-stack", json!({ "IpAddressType": "DUAL_STACK" }), 2),
    ];

    for (case, extra, expected_sets) in rows {
        let mut body = json!({
            "Name": format!("detail-{case}"),
            "IdempotencyToken": format!("tok-{case}"),
        });
        if let (Some(target), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                target.insert(key.clone(), value.clone());
            }
        }
        let created = ok(&router, "CreateAccelerator", body).await["Accelerator"].clone();
        let arn = arn_of(&created);
        assert_eq!(
            created["IpSets"].as_array().map(Vec::len),
            Some(expected_sets),
            "case {case}: expected {expected_sets} IP address set(s), got {created}"
        );
        assert_ip_address_details(&format!("{case}/create"), &created);

        // The same projection out of the reporting call...
        let described = ok(
            &router,
            "DescribeAccelerator",
            json!({ "AcceleratorArn": arn }),
        )
        .await["Accelerator"]
            .clone();
        assert_ip_address_details(&format!("{case}/describe"), &described);
        assert_eq!(
            described["IpSets"], created["IpSets"],
            "case {case}: DescribeAccelerator should report the persisted address sets {}, \
             got {}",
            created["IpSets"], described["IpSets"]
        );

        // ... out of the list ...
        let listed = ok(&router, "ListAccelerators", json!({})).await["Accelerators"].clone();
        let from_list = listed
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|accelerator| arn_of(accelerator) == arn)
            .unwrap_or_else(|| panic!("case {case}: {arn} is missing from {listed}"));
        assert_ip_address_details(&format!("{case}/list"), &from_list);

        // ... and out of the mutating call's own response.
        let updated = ok(
            &router,
            "UpdateAccelerator",
            json!({ "AcceleratorArn": arn, "Name": format!("detail-{case}-renamed") }),
        )
        .await["Accelerator"]
            .clone();
        assert_ip_address_details(&format!("{case}/update"), &updated);
        assert_eq!(
            updated["IpSets"], created["IpSets"],
            "case {case}: an update that renames the accelerator keeps its address sets {}, \
             got {}",
            created["IpSets"], updated["IpSets"]
        );
    }
}

#[tokio::test]
async fn address_details_follow_the_addresses_an_update_changes() {
    let router = emulator();
    let accelerator = create_accelerator(&router, "detail-update", "tok-detail-update").await;
    let arn = arn_of(&accelerator);

    // Each row updates the address layout and states what the resulting IPv4 set must
    // hold; the rule (one detail per address, in order, distinct zones) is re-asserted
    // after each one.
    let rows: [(&str, Value, Option<&str>); 3] = [
        (
            "replace-both-ipv4-addresses",
            json!({ "IpAddresses": ["198.51.100.20", "198.51.100.21"] }),
            Some("198.51.100.20"),
        ),
        ("add-an-ipv6-set", json!({ "IpAddressType": "DUAL_STACK" }), None),
        ("drop-the-ipv6-set", json!({ "IpAddressType": "IPV4" }), None),
    ];

    for (case, extra, expected_leading) in rows {
        let mut body = json!({ "AcceleratorArn": arn });
        if let (Some(target), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                target.insert(key.clone(), value.clone());
            }
        }
        let updated = ok(&router, "UpdateAccelerator", body).await["Accelerator"].clone();
        assert_ip_address_details(&format!("{case}/update"), &updated);
        if let Some(expected) = expected_leading {
            assert_eq!(
                updated["IpSets"][0]["IpAddressDetails"][0]["IpAddress"], expected,
                "case {case}: expected the supplied address to lead the IPv4 set's details, \
                 got {}",
                updated["IpSets"][0]
            );
        }

        let described = ok(
            &router,
            "DescribeAccelerator",
            json!({ "AcceleratorArn": arn }),
        )
        .await["Accelerator"]
            .clone();
        assert_eq!(
            described["IpSets"], updated["IpSets"],
            "case {case}: the describe should report what the update persisted {}, got {}",
            updated["IpSets"], described["IpSets"]
        );
    }
}

/// `CreateListener` with the members a row names, returning the created `Listener`.
async fn create_listener(
    router: &MultiServiceRouter,
    accelerator_arn: &str,
    token: &str,
    extra: &Value,
) -> Value {
    let mut body = json!({
        "AcceleratorArn": accelerator_arn,
        "IdempotencyToken": token,
        "PortRanges": [{ "FromPort": 80, "ToPort": 80 }],
        "Protocol": "TCP",
    });
    if let (Some(target), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            target.insert(key.clone(), value.clone());
        }
    }
    ok(router, "CreateListener", body).await["Listener"].clone()
}

#[tokio::test]
async fn a_listener_reports_the_configuration_last_persisted_for_it() {
    let router = emulator();
    let accelerator = create_accelerator(&router, "listener-cfg", "tok-listener-cfg").await;
    let accelerator_arn = arn_of(&accelerator);

    // The rows differ along every member an update can change and along the SHAPE of
    // the port-range list (one range, two, the modelled maximum of ten — `PortRanges`
    // is @length(1..10)), plus a row whose update names nothing but the protocol, so an
    // omitted member must keep the persisted value rather than fall back to a default.
    let ten_ranges: Vec<Value> = (0..10)
        .map(|index| json!({ "FromPort": 1000 + index * 10, "ToPort": 1000 + index * 10 + 5 }))
        .collect();
    let rows: [(&str, Value, Value, Value); 5] = [
        (
            "source-ip-and-two-ranges",
            json!({}),
            json!({
                "ClientAffinity": "SOURCE_IP",
                "PortRanges": [
                    { "FromPort": 443, "ToPort": 443 },
                    { "FromPort": 8080, "ToPort": 8081 },
                ],
            }),
            json!({
                "ClientAffinity": "SOURCE_IP",
                "Protocol": "TCP",
                "PortRanges": [
                    { "FromPort": 443, "ToPort": 443 },
                    { "FromPort": 8080, "ToPort": 8081 },
                ],
            }),
        ),
        (
            "protocol-only-keeps-the-rest",
            json!({ "ClientAffinity": "SOURCE_IP" }),
            json!({ "Protocol": "UDP" }),
            json!({
                "ClientAffinity": "SOURCE_IP",
                "Protocol": "UDP",
                "PortRanges": [{ "FromPort": 80, "ToPort": 80 }],
            }),
        ),
        (
            "ten-ranges",
            json!({}),
            json!({ "PortRanges": ten_ranges.clone() }),
            json!({
                "ClientAffinity": "NONE",
                "Protocol": "TCP",
                "PortRanges": ten_ranges,
            }),
        ),
        (
            "affinity-back-to-none",
            json!({ "ClientAffinity": "SOURCE_IP" }),
            json!({ "ClientAffinity": "NONE" }),
            json!({
                "ClientAffinity": "NONE",
                "Protocol": "TCP",
                "PortRanges": [{ "FromPort": 80, "ToPort": 80 }],
            }),
        ),
        (
            "every-member-at-once",
            json!({}),
            json!({
                "ClientAffinity": "SOURCE_IP",
                "Protocol": "UDP",
                "PortRanges": [{ "FromPort": 65535, "ToPort": 65535 }],
            }),
            json!({
                "ClientAffinity": "SOURCE_IP",
                "Protocol": "UDP",
                "PortRanges": [{ "FromPort": 65535, "ToPort": 65535 }],
            }),
        ),
    ];

    // ARN -> the configuration that row expects, re-checked through ListListeners once
    // every row has run, so one listener's update cannot be reported for another.
    let mut expected_by_arn: Vec<(String, &str, Value)> = Vec::new();

    for (case, create_extra, update_extra, expected) in &rows {
        let created =
            create_listener(&router, &accelerator_arn, &format!("lsn-{case}"), create_extra).await;
        let listener_arn = created["ListenerArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: ListenerArn missing from {created}"))
            .to_string();

        let mut body = json!({ "ListenerArn": listener_arn });
        if let (Some(target), Some(extra)) = (body.as_object_mut(), update_extra.as_object()) {
            for (key, value) in extra {
                target.insert(key.clone(), value.clone());
            }
        }
        let updated = ok(&router, "UpdateListener", body).await["Listener"].clone();

        for (member, want) in expected.as_object().expect("an object of expectations") {
            assert_eq!(
                updated.get(member).unwrap_or(&Value::Null),
                want,
                "case {case}/update: expected {member} {want}, got {updated}"
            );
        }

        // The mutating call's own response, the describe and the list all project the
        // same stored record.
        let described = ok(
            &router,
            "DescribeListener",
            json!({ "ListenerArn": listener_arn }),
        )
        .await["Listener"]
            .clone();
        assert_eq!(
            described, updated,
            "case {case}/describe: expected the persisted listener {updated}, got {described}"
        );

        expected_by_arn.push((listener_arn, case, expected.clone()));
    }

    let listeners = ok(
        &router,
        "ListListeners",
        json!({ "AcceleratorArn": accelerator_arn }),
    )
    .await["Listeners"]
        .clone();
    let listed = listeners.as_array().cloned().unwrap_or_default();
    assert_eq!(
        listed.len(),
        rows.len(),
        "case list: expected {} listeners, got {listeners}",
        rows.len()
    );
    for (listener_arn, case, expected) in &expected_by_arn {
        let found = listed
            .iter()
            .find(|listener| listener["ListenerArn"] == Value::String(listener_arn.clone()))
            .unwrap_or_else(|| panic!("case {case}/list: {listener_arn} is missing from {listeners}"));
        for (member, want) in expected.as_object().expect("an object of expectations") {
            assert_eq!(
                found.get(member).unwrap_or(&Value::Null),
                want,
                "case {case}/list: expected {member} {want}, got {found}"
            );
        }
    }
}

#[tokio::test]
async fn a_refused_listener_update_changes_nothing() {
    let router = emulator();
    let accelerator = create_accelerator(&router, "listener-refuse", "tok-listener-refuse").await;
    let accelerator_arn = arn_of(&accelerator);
    let created = create_listener(
        &router,
        &accelerator_arn,
        "lsn-refuse",
        &json!({ "ClientAffinity": "SOURCE_IP" }),
    )
    .await;
    let listener_arn = created["ListenerArn"].as_str().unwrap_or_default().to_string();

    // A port range names the inclusive bounds of a range of ports, so a range that runs
    // backwards names none: the update path enforces the same rule as the create path.
    // An ARN the store does not hold is the modelled not-found refusal.
    let rows: [(&str, Value, &str); 4] = [
        (
            "reversed-range",
            json!({
                "ListenerArn": listener_arn,
                "PortRanges": [{ "FromPort": 443, "ToPort": 80 }],
            }),
            "InvalidPortRangeException",
        ),
        (
            "second-range-reversed",
            json!({
                "ListenerArn": listener_arn,
                "PortRanges": [
                    { "FromPort": 80, "ToPort": 81 },
                    { "FromPort": 9000, "ToPort": 8999 },
                ],
            }),
            "InvalidPortRangeException",
        ),
        (
            "range-missing-an-end",
            json!({
                "ListenerArn": listener_arn,
                "PortRanges": [{ "FromPort": 80 }],
            }),
            "InvalidPortRangeException",
        ),
        (
            "unknown-listener",
            json!({
                "ListenerArn": format!("{accelerator_arn}/listener/deadbeef"),
                "ClientAffinity": "NONE",
            }),
            "ListenerNotFoundException",
        ),
    ];

    for (case, body, expected_code) in rows {
        let (status, response) = call(&router, "UpdateListener", body).await;
        assert_eq!(
            error_code(&response),
            expected_code,
            "case {case}: expected {expected_code}, got {status} {response}"
        );
        let described = ok(
            &router,
            "DescribeListener",
            json!({ "ListenerArn": listener_arn }),
        )
        .await["Listener"]
            .clone();
        assert_eq!(
            described, created,
            "case {case}: a refused update must leave the listener at {created}, got {described}"
        );
    }
}

/// The ids in a `.../accelerator/<id>/listener/<id>/endpoint-group/<id>` ARN, outermost
/// first.
fn arn_segment_ids(arn: &str) -> Vec<&str> {
    arn.split('/').skip(1).step_by(2).collect()
}

#[tokio::test]
async fn a_minted_arn_segment_has_the_width_that_resources_arns_carry() {
    let router = emulator();
    let accelerator = create_accelerator(&router, "arn-widths", "tok-arn-widths").await;
    let accelerator_arn = arn_of(&accelerator);

    // The rows differ along the parent the group hangs off and the Region it covers (a
    // listener holds one endpoint group per Region), so the widths are asserted over
    // several minted ids rather than one.
    let rows: [(&str, &str); 4] = [
        ("first-listener", "us-west-2"),
        ("first-listener", "eu-west-1"),
        ("second-listener", "us-west-2"),
        ("second-listener", "ap-south-1"),
    ];

    let mut listener_arns: Vec<(String, String)> = Vec::new();
    let mut seen_group_ids: Vec<String> = Vec::new();

    for (listener_case, region) in rows {
        let listener_arn = match listener_arns
            .iter()
            .find(|(case, _)| case == listener_case)
            .map(|(_, arn)| arn.clone())
        {
            Some(arn) => arn,
            None => {
                let created = create_listener(
                    &router,
                    &accelerator_arn,
                    &format!("lsn-{listener_case}"),
                    &json!({}),
                )
                .await;
                let arn = created["ListenerArn"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                listener_arns.push((listener_case.to_string(), arn.clone()));
                arn
            }
        };

        let case = format!("{listener_case}/{region}");
        let group = ok(
            &router,
            "CreateEndpointGroup",
            json!({
                "ListenerArn": listener_arn,
                "EndpointGroupRegion": region,
                "IdempotencyToken": format!("eg-{case}"),
            }),
        )
        .await["EndpointGroup"]
            .clone();
        let group_arn = group["EndpointGroupArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: EndpointGroupArn missing from {group}"))
            .to_string();
        assert!(
            group_arn.starts_with(&format!("{listener_arn}/endpoint-group/")),
            "case {case}: an endpoint group ARN hangs off its listener {listener_arn:?}, got \
             {group_arn:?}"
        );

        // The id of each segment, with the width that segment carries: an accelerator
        // id is a 36-character UUID, a listener id 8 lowercase hex characters and an
        // endpoint-group id 12.
        let ids = arn_segment_ids(&group_arn);
        assert_eq!(
            ids.len(),
            3,
            "case {case}: expected accelerator/listener/endpoint-group ids, got {group_arn:?}"
        );
        for (segment, id, width) in [
            ("accelerator", ids[0], 36),
            ("listener", ids[1], 8),
            ("endpoint-group", ids[2], 12),
        ] {
            assert_eq!(
                id.chars().count(),
                width,
                "case {case}: a {segment} id is {width} characters wide, got {id:?} in \
                 {group_arn:?}"
            );
            assert!(
                id.chars()
                    .all(|c| (c.is_ascii_hexdigit() || c == '-') && !c.is_ascii_uppercase()),
                "case {case}: a {segment} id is lowercase hex, got {id:?}"
            );
        }
        assert!(
            !seen_group_ids.iter().any(|seen| seen == ids[2]),
            "case {case}: endpoint-group id {:?} was minted twice, seen {seen_group_ids:?}",
            ids[2]
        );
        seen_group_ids.push(ids[2].to_string());

        // The minted ARN is the one the store answers for.
        let described = ok(
            &router,
            "DescribeEndpointGroup",
            json!({ "EndpointGroupArn": group_arn }),
        )
        .await["EndpointGroup"]
            .clone();
        assert_eq!(
            described, group,
            "case {case}: expected the created endpoint group {group}, got {described}"
        );
    }
}

/// The zone AWS's own Global Accelerator names live in, from the naming convention
/// `Accelerator$DnsName` documents ("followed by .awsglobalaccelerator.com"). This emulator
/// serves no host under it, so no response may announce one.
const AWS_ACCELERATOR_ZONE: &str = "awsglobalaccelerator.com";

/// Call one operation addressed to `host` and require a 200.
async fn ok_on_host(
    router: &MultiServiceRouter,
    host: &str,
    operation: &str,
    body: Value,
) -> Value {
    let (status, parsed) = call_on_host(router, host, operation, body).await;
    assert_eq!(
        status, 200,
        "case {host}: expected HTTP 200 from {operation}, got {status} with {parsed}"
    );
    parsed
}

/// Rule 4, for one announced name: it is exactly the host this emulator answers on, carries
/// no port (a client resolves it and connects on the accelerator's listener port) and is
/// never a host under the zone AWS serves its own accelerators in.
fn assert_announced(case: &str, operation: &str, announced: &str, expected_host: &str) {
    assert!(
        !announced.contains(AWS_ACCELERATOR_ZONE),
        "case {case}: {operation} must announce no host under {AWS_ACCELERATOR_ZONE}, \
         got {announced:?}"
    );
    assert_eq!(
        announced, expected_host,
        "case {case}: {operation} must announce the host this emulator answers on, \
         got {announced:?}"
    );
}

/// Rule 4, over both accelerator kinds and every operation that projects one.
///
/// The rows differ along what the rule generalises over: the form of the address a client
/// reaches this emulator at (an IPv4 address with a port, the loopback name with a port, a
/// bracketed IPv6 literal, a routable address) and the cases where the request names no
/// address of this emulator at all — a client resolving AWS's own hostnames, or a `Host`
/// that is no address — where the base this process is served at names the host instead
/// (here the unserved one, since a router built in-process holds no port).
#[tokio::test]
async fn an_announced_dns_name_is_the_host_the_emulator_answers_on() {
    let router = emulator();

    // (case, Host header, expected announced host)
    let rows: Vec<(&str, &str, &str)> = vec![
        ("an address with a port", "127.0.0.1:41835", "127.0.0.1"),
        ("the loopback name with a port", "localhost:9297", "localhost"),
        ("a bracketed IPv6 literal", "[::1]:9297", "[::1]"),
        ("a routable address", "10.0.0.7:9297", "10.0.0.7"),
        // A `Host` carrying userinfo is not an address this emulator can be reached at,
        // so the base it is served at answers for it, exactly as for an AWS host.
        (
            "an address carrying userinfo",
            "user@localhost:9297",
            UNSERVED_HOST,
        ),
        (
            "the service's own AWS endpoint",
            AWS_ENDPOINT_HOST,
            UNSERVED_HOST,
        ),
    ];

    for (index, (case, host, expected_host)) in rows.iter().enumerate() {
        let created = ok_on_host(
            &router,
            host,
            "CreateAccelerator",
            json!({
                "Name": format!("announced-{index}"),
                "IdempotencyToken": format!("announced-{index}"),
                "IpAddressType": "DUAL_STACK",
            }),
        )
        .await["Accelerator"]
            .clone();
        let arn = arn_of(&created);
        assert_announced(
            case,
            "CreateAccelerator",
            created["DnsName"].as_str().unwrap_or_default(),
            expected_host,
        );
        // One process answers for both address families, so the dual-stack name is the
        // same host; it is present because this accelerator is dual-stack.
        assert_announced(
            case,
            "CreateAccelerator (DualStackDnsName)",
            created["DualStackDnsName"].as_str().unwrap_or_default(),
            expected_host,
        );

        // Every other operation that projects this accelerator announces the same host:
        // the read, the update and the listing.
        let described = ok_on_host(
            &router,
            host,
            "DescribeAccelerator",
            json!({ "AcceleratorArn": arn }),
        )
        .await["Accelerator"]
            .clone();
        let updated = ok_on_host(
            &router,
            host,
            "UpdateAccelerator",
            json!({ "AcceleratorArn": arn, "Name": format!("announced-{index}-renamed") }),
        )
        .await["Accelerator"]
            .clone();
        let listed = ok_on_host(&router, host, "ListAccelerators", json!({})).await
            ["Accelerators"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|entry| entry["AcceleratorArn"] == created["AcceleratorArn"])
            .unwrap_or_else(|| panic!("case {case}: {arn} is missing from ListAccelerators"));
        for (operation, projection) in [
            ("DescribeAccelerator", &described),
            ("UpdateAccelerator", &updated),
            ("ListAccelerators", &listed),
        ] {
            assert_announced(
                case,
                operation,
                projection["DnsName"].as_str().unwrap_or_default(),
                expected_host,
            );
            assert_announced(
                case,
                operation,
                projection["DualStackDnsName"].as_str().unwrap_or_default(),
                expected_host,
            );
        }

        // A custom routing accelerator is announced the same host, and models no
        // dual-stack name at all.
        let custom = ok_on_host(
            &router,
            host,
            "CreateCustomRoutingAccelerator",
            json!({
                "Name": format!("announced-cr-{index}"),
                "IdempotencyToken": format!("announced-cr-{index}"),
            }),
        )
        .await["Accelerator"]
            .clone();
        let custom_arn = arn_of(&custom);
        assert_announced(
            case,
            "CreateCustomRoutingAccelerator",
            custom["DnsName"].as_str().unwrap_or_default(),
            expected_host,
        );
        assert!(
            custom.get("DualStackDnsName").is_none(),
            "case {case}: `CustomRoutingAccelerator` models no DualStackDnsName, got {custom}"
        );
        let custom_described = ok_on_host(
            &router,
            host,
            "DescribeCustomRoutingAccelerator",
            json!({ "AcceleratorArn": custom_arn }),
        )
        .await["Accelerator"]
            .clone();
        assert_announced(
            case,
            "DescribeCustomRoutingAccelerator",
            custom_described["DnsName"].as_str().unwrap_or_default(),
            expected_host,
        );
    }

    // The announced host is a property of where this emulator is REACHED, not of the
    // record: ONE accelerator read from each address above is announced that address.
    let subject = create_accelerator(&router, "announced-across-hosts", "across-hosts").await;
    let subject_arn = arn_of(&subject);
    for (case, host, expected_host) in &rows {
        let described = ok_on_host(
            &router,
            host,
            "DescribeAccelerator",
            json!({ "AcceleratorArn": subject_arn }),
        )
        .await["Accelerator"]
            .clone();
        assert_announced(
            case,
            "DescribeAccelerator (one accelerator, many addresses)",
            described["DnsName"].as_str().unwrap_or_default(),
            expected_host,
        );
    }

    // A client that resolves AWS's own hostnames reaches none of the addresses above, so
    // it is announced the host THIS emulator is SERVED at — the one address such a caller
    // can reach it at.
    let served = emulator_served_at("https://127.0.0.1:9297");
    let on_served = ok_on_host(
        &served,
        AWS_ENDPOINT_HOST,
        "CreateAccelerator",
        json!({ "Name": "announced-served", "IdempotencyToken": "announced-served" }),
    )
    .await["Accelerator"]
        .clone();
    assert_announced(
        "a served emulator addressed by an AWS host",
        "CreateAccelerator",
        on_served["DnsName"].as_str().unwrap_or_default(),
        "127.0.0.1",
    );
}

// ---------------------------------------------------------------------------
// Rule 5: the value sets a resource is projected with are the MODEL's.
// ---------------------------------------------------------------------------

/// Amazon Web Services Region codes, used one per endpoint group because a listener holds
/// at most one endpoint group per Region (`EndpointGroup` doc). Real Region codes rather
/// than made-up strings, so a row reads like a call a client makes; `EndpointGroupRegion`
/// is a `GenericString` (len 0..255), so the service constrains none of them. Six of them,
/// which leaves room under the documented per-accelerator quota of ten endpoint groups for
/// a value set the model grows.
const PROJECTION_REGIONS: [&str; 6] = [
    "us-east-1",
    "us-west-2",
    "eu-west-1",
    "eu-central-1",
    "ap-southeast-2",
    "ap-northeast-1",
];

/// The ARN member `member` of a projected resource.
fn arn_member(resource: &Value, member: &str) -> String {
    resource[member]
        .as_str()
        .unwrap_or_else(|| panic!("{member} missing from {resource}"))
        .to_string()
}

/// Rule 5, over the two enumerated members a listener carries: `Protocol` and
/// `ClientAffinity`.
///
/// The rows are the CROSS PRODUCT of the two value sets the model declares for them, so
/// each value of each member is exercised beside every value of the other; a value set the
/// upstream model grows adds its rows here without this file naming a value.
#[tokio::test]
async fn every_modelled_listener_value_set_is_projected_back() {
    let router = emulator();
    let accelerator = arn_of(&create_accelerator(&router, "listener-value-sets", "lvs").await);

    assert!(
        !PROTOCOL_VALUES.is_empty() && !CLIENT_AFFINITY_VALUES.is_empty(),
        "expected the model's own value sets, got {PROTOCOL_VALUES:?} and \
         {CLIENT_AFFINITY_VALUES:?}"
    );

    for (protocol_index, protocol) in PROTOCOL_VALUES.iter().enumerate() {
        for (affinity_index, affinity) in CLIENT_AFFINITY_VALUES.iter().enumerate() {
            let case = format!("{protocol}/{affinity}");
            // A port of its own per row, so no row depends on another's listener.
            let port = 5000 + (protocol_index * CLIENT_AFFINITY_VALUES.len() + affinity_index);
            let created = ok(
                &router,
                "CreateListener",
                json!({
                    "AcceleratorArn": accelerator,
                    "PortRanges": [{ "FromPort": port, "ToPort": port }],
                    "Protocol": protocol,
                    "ClientAffinity": affinity,
                    "IdempotencyToken": format!("lvs-{port}"),
                }),
            )
            .await["Listener"]
                .clone();
            assert_eq!(
                created["Protocol"],
                json!(protocol),
                "case {case}: expected the Protocol the request named, got {created}"
            );
            assert_eq!(
                created["ClientAffinity"],
                json!(affinity),
                "case {case}: expected the ClientAffinity the request named, got {created}"
            );

            let listener_arn = arn_member(&created, "ListenerArn");
            let described = ok(
                &router,
                "DescribeListener",
                json!({ "ListenerArn": listener_arn }),
            )
            .await["Listener"]
                .clone();
            assert_eq!(
                described, created,
                "case {case}: expected the read to project what the create persisted, got \
                 {described}"
            );
        }
    }
}

/// Rule 5, over the enumerated member an endpoint group carries: `HealthCheckProtocol`.
///
/// One row per value the model declares for it, each in a Region of its own because a
/// listener holds at most one endpoint group per Region.
#[tokio::test]
async fn every_modelled_health_check_protocol_is_projected_back() {
    let router = emulator();
    let accelerator = arn_of(&create_accelerator(&router, "health-value-sets", "hvs").await);
    let listener_arn = arn_member(
        &ok(
            &router,
            "CreateListener",
            json!({
                "AcceleratorArn": accelerator,
                "PortRanges": [{ "FromPort": 80, "ToPort": 80 }],
                "Protocol": "TCP",
                "IdempotencyToken": "hvs-listener",
            }),
        )
        .await["Listener"],
        "ListenerArn",
    );

    assert!(
        !HEALTH_CHECK_PROTOCOL_VALUES.is_empty()
            && HEALTH_CHECK_PROTOCOL_VALUES.len() <= PROJECTION_REGIONS.len(),
        "expected one Region per value the model admits, got {HEALTH_CHECK_PROTOCOL_VALUES:?} \
         over {PROJECTION_REGIONS:?}"
    );

    for (index, protocol) in HEALTH_CHECK_PROTOCOL_VALUES.iter().enumerate() {
        let region = PROJECTION_REGIONS[index];
        let created = ok(
            &router,
            "CreateEndpointGroup",
            json!({
                "ListenerArn": listener_arn,
                "EndpointGroupRegion": region,
                "HealthCheckProtocol": protocol,
                "IdempotencyToken": format!("hvs-{index}"),
            }),
        )
        .await["EndpointGroup"]
            .clone();
        assert_eq!(
            created["HealthCheckProtocol"],
            json!(protocol),
            "case {protocol}: expected the HealthCheckProtocol the request named, got {created}"
        );

        let group_arn = arn_member(&created, "EndpointGroupArn");
        let described = ok(
            &router,
            "DescribeEndpointGroup",
            json!({ "EndpointGroupArn": group_arn }),
        )
        .await["EndpointGroup"]
            .clone();
        assert_eq!(
            described, created,
            "case {protocol}: expected the read to project what the create persisted, got \
             {described}"
        );
    }
}

/// Rule 5, over the enumerated member an accelerator carries: `IpAddressType`, and the
/// `IpAddressFamily` every address set it is projected with reports.
///
/// One row per value the model admits for the type; the families are not enumerated here
/// either — each projected set must report one of the families the model declares.
#[tokio::test]
async fn every_modelled_ip_address_type_is_projected_back() {
    let router = emulator();

    assert!(
        !IP_ADDRESS_TYPE_VALUES.is_empty() && !IP_ADDRESS_FAMILY_VALUES.is_empty(),
        "expected the model's own value sets, got {IP_ADDRESS_TYPE_VALUES:?} and \
         {IP_ADDRESS_FAMILY_VALUES:?}"
    );

    for (index, ip_address_type) in IP_ADDRESS_TYPE_VALUES.iter().enumerate() {
        let created = ok(
            &router,
            "CreateAccelerator",
            json!({
                "Name": format!("ip-type-{index}"),
                "IpAddressType": ip_address_type,
                "IdempotencyToken": format!("ip-type-{index}"),
            }),
        )
        .await["Accelerator"]
            .clone();
        assert_eq!(
            created["IpAddressType"],
            json!(ip_address_type),
            "case {ip_address_type}: expected the IpAddressType the request named, got {created}"
        );

        let sets = created["IpSets"].as_array().cloned().unwrap_or_default();
        assert!(
            !sets.is_empty(),
            "case {ip_address_type}: expected at least one IpSet, got {created}"
        );
        for set in &sets {
            let family = set["IpAddressFamily"].as_str().unwrap_or_default();
            assert!(
                IP_ADDRESS_FAMILY_VALUES.contains(&family),
                "case {ip_address_type}: expected an IpAddressFamily the model admits \
                 ({IP_ADDRESS_FAMILY_VALUES:?}), got {family:?} in {set}"
            );
        }

        // The read projects the same type and the same address sets; `Status` is owed by
        // the clock rather than by this rule, so it is not part of the comparison.
        let described = ok(
            &router,
            "DescribeAccelerator",
            json!({ "AcceleratorArn": arn_of(&created) }),
        )
        .await["Accelerator"]
            .clone();
        assert_eq!(
            described["IpAddressType"], created["IpAddressType"],
            "case {ip_address_type}: expected the read to project the persisted type, got \
             {described}"
        );
        assert_eq!(
            described["IpSets"], created["IpSets"],
            "case {ip_address_type}: expected the read to project the persisted address sets, \
             got {described}"
        );
    }
}

/// Rule 5, over the enumerated member a custom routing endpoint group's destinations carry:
/// the `Protocols` a destination accepts client traffic on.
///
/// One row per value the model declares, plus one naming the whole set at once, since a
/// destination accepts a LIST of protocols.
#[tokio::test]
async fn every_modelled_custom_routing_protocol_is_projected_back() {
    let router = emulator();
    let accelerator = arn_of(
        &ok(
            &router,
            "CreateCustomRoutingAccelerator",
            json!({ "Name": "cr-value-sets", "IdempotencyToken": "crvs" }),
        )
        .await["Accelerator"],
    );
    let listener_arn = arn_member(
        &ok(
            &router,
            "CreateCustomRoutingListener",
            json!({
                "AcceleratorArn": accelerator,
                "PortRanges": [{ "FromPort": 7000, "ToPort": 7010 }],
                "IdempotencyToken": "crvs-listener",
            }),
        )
        .await["Listener"],
        "ListenerArn",
    );

    let mut rows: Vec<(String, Vec<&str>)> = CUSTOM_ROUTING_PROTOCOL_VALUES
        .iter()
        .map(|protocol| ((*protocol).to_string(), vec![*protocol]))
        .collect();
    rows.push((
        "every modelled protocol at once".to_string(),
        CUSTOM_ROUTING_PROTOCOL_VALUES.to_vec(),
    ));
    assert!(
        rows.len() > 1 && rows.len() <= PROJECTION_REGIONS.len(),
        "expected one Region per row, got {rows:?} over {PROJECTION_REGIONS:?}"
    );

    for (index, (case, protocols)) in rows.iter().enumerate() {
        // A destination port of its own per row: the port a destination accepts traffic on
        // is independent of the listener's own range.
        let port = 8000 + index;
        let created = ok(
            &router,
            "CreateCustomRoutingEndpointGroup",
            json!({
                "ListenerArn": listener_arn,
                "EndpointGroupRegion": PROJECTION_REGIONS[index],
                "DestinationConfigurations": [
                    { "FromPort": port, "ToPort": port, "Protocols": protocols },
                ],
                "IdempotencyToken": format!("crvs-{index}"),
            }),
        )
        .await["EndpointGroup"]
            .clone();
        let destinations = created["DestinationDescriptions"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            destinations.len(),
            1,
            "case {case}: expected the one destination the request configured, got {created}"
        );
        assert_eq!(
            destinations[0]["Protocols"],
            json!(protocols),
            "case {case}: expected the Protocols the request named, got {created}"
        );

        let group_arn = arn_member(&created, "EndpointGroupArn");
        let described = ok(
            &router,
            "DescribeCustomRoutingEndpointGroup",
            json!({ "EndpointGroupArn": group_arn }),
        )
        .await["EndpointGroup"]
            .clone();
        assert_eq!(
            described, created,
            "case {case}: expected the read to project what the create persisted, got {described}"
        );
    }
}
