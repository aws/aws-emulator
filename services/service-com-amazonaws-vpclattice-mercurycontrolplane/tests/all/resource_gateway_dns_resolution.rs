// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DNS-resolution choice a resource gateway carries.
//!
//! The rule: the `resourceConfigDnsResolution` a create is given is persisted on the
//! gateway and reported, unchanged, by every read of it — the create's own answer,
//! `GetResourceGateway` under either modelled identifier form, and the entry
//! `ListResourceGateways` carries — while a value the member's enum does not admit is
//! refused HTTP 400 `ValidationException` and no gateway is persisted for it.
//!
//! The rows span EVERY value the model admits for the member rather than one of them, so
//! no row can pass on a single remembered value.

use crate::vpclattice_service_slice::{as_label, Answer, Lattice};

/// The VPC a gateway of these cases is opened in, and the subnet and security group it is
/// given, each in the one form its member is modelled with (`VpcId`, `SubnetList`,
/// `SecurityGroupList`).
const VPC_ID: &str = "vpc-0a1b2c3d";
const SUBNET_ID: &str = "subnet-0a1b2c3d4e5f6a7b8";
const SECURITY_GROUP_ID: &str = "sg-0a1b2c3d4e5f6a7b8";

/// Every value `ResourceConfigDnsResolution` admits, written out here rather than read
/// from the service so no row can pass by agreeing with the code under test. SOURCE: the
/// member's own enum (`smithy_query(query="shape",shape="ResourceConfigDnsResolution")`).
const DNS_RESOLUTIONS: [&str; 2] = ["IN_VPC", "PUBLIC"];

/// The member the cases of this suite follow through a create and its reads.
const DNS_RESOLUTION_MEMBER: &str = "resourceConfigDnsResolution";

async fn create_gateway(lattice: &Lattice, body: serde_json::Value) -> Answer {
    lattice.call("POST", "/resourcegateways", Some(body)).await
}

async fn get_gateway(lattice: &Lattice, identifier: &str) -> Answer {
    lattice
        .call("GET", &format!("/resourcegateways/{identifier}"), None)
        .await
}

async fn list_gateways(lattice: &Lattice) -> Answer {
    lattice.call("GET", "/resourcegateways", None).await
}

/// The create body of a gateway named `name`, carrying `dns_resolution` when one is given.
fn gateway_body(name: &str, dns_resolution: Option<&str>) -> serde_json::Value {
    let mut body = serde_json::json!({
        "name": name,
        "vpcIdentifier": VPC_ID,
        "subnetIds": [SUBNET_ID],
        "securityGroupIds": [SECURITY_GROUP_ID],
    });
    if let Some(resolution) = dns_resolution {
        body[DNS_RESOLUTION_MEMBER] = serde_json::Value::String(resolution.to_string());
    }
    body
}

/// What `answer` reports for the member, as a borrowed JSON value.
fn reported_resolution(answer: &Answer) -> &serde_json::Value {
    &answer.body[DNS_RESOLUTION_MEMBER]
}

/// The create's choice is what the create answers and what every later read of the gateway
/// reports, for each value the member's enum admits.
#[tokio::test]
async fn the_dns_resolution_a_gateway_is_created_with_is_what_every_read_reports() {
    let lattice = Lattice::new();

    for (index, resolution) in DNS_RESOLUTIONS.into_iter().enumerate() {
        let case = format!("created with {resolution}");
        let name = format!("gw-dns-{index}");
        let created = create_gateway(&lattice, gateway_body(&name, Some(resolution))).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected 201, got {} / {}",
            created.status, created.body
        );
        assert_eq!(
            reported_resolution(&created).as_str(),
            Some(resolution),
            "case {case:?}: the create must answer the choice it was given, got {}",
            created.body
        );
        let id = created.str_field("id");
        let arn = created.str_field("arn");

        // Both modelled identifier forms read the same persisted gateway, so neither path
        // may report a different choice.
        for (form, identifier) in [("bare id", id.clone()), ("ARN", as_label(&arn))] {
            let read = get_gateway(&lattice, &identifier).await;
            assert_eq!(
                read.status, 200,
                "case {case:?} by {form}: expected 200, got {} / {}",
                read.status, read.body
            );
            assert_eq!(
                reported_resolution(&read).as_str(),
                Some(resolution),
                "case {case:?} by {form}: expected the persisted choice, got {}",
                read.body
            );
        }

        // The listing reports the same choice for the same gateway.
        let listed = list_gateways(&lattice).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: list expected 200, got {} / {}",
            listed.status, listed.body
        );
        let items = listed.body["items"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: list expected items, got {}", listed.body));
        let entry = items
            .iter()
            .find(|item| item["id"].as_str() == Some(id.as_str()))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the listing must carry the persisted gateway {id}, got {}",
                    listed.body
                )
            });
        assert_eq!(
            entry[DNS_RESOLUTION_MEMBER].as_str(),
            Some(resolution),
            "case {case:?}: the listed entry must carry the persisted choice, got {entry}"
        );
    }

    // Each case persisted its own gateway, each keeping its own choice: a second create
    // did not overwrite the first one's member.
    let listed = list_gateways(&lattice).await;
    let items = listed.body["items"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("final list: expected items, got {}", listed.body));
    assert_eq!(
        items.len(),
        DNS_RESOLUTIONS.len(),
        "final list: expected one gateway per case, got {}",
        listed.body
    );
    let mut reported: Vec<&str> = items
        .iter()
        .map(|item| {
            item[DNS_RESOLUTION_MEMBER].as_str().unwrap_or_else(|| {
                panic!("final list: every gateway must report its choice, got {item}")
            })
        })
        .collect();
    reported.sort_unstable();
    let mut expected: Vec<&str> = DNS_RESOLUTIONS.to_vec();
    expected.sort_unstable();
    assert_eq!(
        reported, expected,
        "final list: expected every case's own choice, got {}",
        listed.body
    );
}

/// A DNS-resolution value the member's enum does not admit is the modelled 400, and
/// nothing is persisted for the refused create.
#[tokio::test]
async fn a_dns_resolution_outside_the_modelled_enum_is_refused_and_persists_nothing() {
    let lattice = Lattice::new();

    // (case, the value the create submits) — each outside the enum above: a plausible
    // third token, one of the admitted values in the wrong case, and the empty string.
    let rows: [(&str, &str); 3] = [
        ("a token the enum does not carry", "PRIVATE"),
        ("an admitted value in lower case", "in_vpc"),
        ("the empty string", ""),
    ];

    for (index, (case, value)) in rows.into_iter().enumerate() {
        let name = format!("gw-dns-refused-{index}");
        let answer = create_gateway(&lattice, gateway_body(&name, Some(value))).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected the ValidationException wire code, got {:?} / {}",
            answer.error_code,
            answer.body
        );
        let listed = list_gateways(&lattice).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: list expected 200, got {} / {}",
            listed.status, listed.body
        );
        let items = listed.body["items"].as_array().cloned().unwrap_or_default();
        assert!(
            items.is_empty(),
            "case {case:?}: a refused create must persist no gateway, got {}",
            listed.body
        );
    }

    // The name a refused create named is still free, so nothing of it was written.
    let accepted = create_gateway(
        &lattice,
        gateway_body("gw-dns-refused-0", Some(DNS_RESOLUTIONS[0])),
    )
    .await;
    assert_eq!(
        accepted.status, 201,
        "after the refusals: the name a refused create used must still be free, got {} / {}",
        accepted.status, accepted.body
    );
    assert_eq!(
        reported_resolution(&accepted).as_str(),
        Some(DNS_RESOLUTIONS[0]),
        "after the refusals: expected the choice this create was given, got {}",
        accepted.body
    );
}
