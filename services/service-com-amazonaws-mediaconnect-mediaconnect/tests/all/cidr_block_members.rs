// SPDX-License-Identifier: CC0-1.0

//! Every request member MediaConnect documents as a range of IP addresses, end to end
//! over HTTP: a value that is not a Classless Inter-Domain Routing (CIDR) block is refused
//! with 400 `BadRequestException` and NOTHING is stored, while every well-formed block is
//! stored and read back.
//!
//! The rule is asserted over a TABLE of values rather than over one request, and over
//! every member the rule holds for: the gateway's egress range
//! (`CreateGatewayRequest$EgressCidrBlocks`) and each of its networks
//! (`GatewayNetwork$CidrBlock`), the range a source admits contributions from
//! (`SetSourceRequest$WhitelistCidr` on CreateFlow and AddFlowSources,
//! `UpdateFlowSourceRequest$WhitelistCidr`) and the range an output admits requests from
//! (`AddOutputRequest$CidrAllowList` on CreateFlow and AddFlowOutputs,
//! `UpdateFlowOutputRequest$CidrAllowList`).
//!
//! The rows differ along what the rule generalises over: the form of the value (no prefix
//! length, a prefix length past the published bound, an octet past the published bound, the
//! wrong number of octets, an address family this service's CIDR members do not carry,
//! surrounding whitespace, text that is no address at all, the empty string), and — on the
//! accepted side — each published bound of the prefix length and a block whose host bits
//! are set.
//!
//! Every case drives the real request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], so what they assert is what a MediaConnect client
//! sees on the wire.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::flow_outputs::add_flow_outputs;
use crate::flow_sources::add_flow_sources;
use crate::update_flow_output::{at_path, update_flow_output};
use crate::update_flow_source::update_flow_source;

/// The values no CIDR member admits.
///
/// Each row is one way a string fails the form the model publishes for this service's
/// CIDR members — `PublicRouterNetworkInterfaceRule$Cidr` carries
/// `@pattern("^(?:[0-9]{1,3}\\.){3}[0-9]{1,3}/([0-9]|[12][0-9]|3[0-2])$")`
/// (`smithy_query(query="shape",shape="PublicRouterNetworkInterfaceRule")`) — so the list
/// spans the form rather than repeating one bad value, including a prefix length ONE past
/// the published bound of 32.
fn malformed_cidr_blocks() -> Vec<&'static str> {
    vec![
        "banana",
        "10.0.0.0",
        "10.0.0.0/",
        "10.0.0.0/33",
        "10.0.0.0/64",
        "10.0.0.0/-1",
        "10.0.0.0/016",
        "10.0.0.0/sixteen",
        "10.0.0/16",
        "10.0.0.0.0/16",
        "10.0.0.256/24",
        "10.0.0.0/16/24",
        "2001:db8::/32",
        " 10.0.0.0/16",
        "10.0.0.0/16 ",
        "",
    ]
}

/// The values every CIDR member admits: the block the documentation gives as its example,
/// each published bound of the prefix length (0 and 32), and a block whose host bits are
/// set — which the published pattern admits, so this emulator must too.
fn well_formed_cidr_blocks() -> Vec<&'static str> {
    vec![
        "10.0.0.0/16",
        "0.0.0.0/0",
        "255.255.255.255/32",
        "203.0.113.5/24",
        "198.51.100.7/32",
    ]
}

/// Assert `response` is this service's refusal of a request whose CONTENT was
/// unacceptable, and that its detail names `member` and the value it carried.
fn assert_cidr_refusal(
    case: &str,
    member: &str,
    value: &str,
    status: u16,
    code: Option<&str>,
    response: &Value,
) {
    assert_eq!(
        (status, code),
        (400, Some("BadRequestException")),
        "case {case:?}: {member} {value:?} must be refused 400 BadRequestException, got \
         {status} {code:?}: {response}"
    );
    let message = response["message"].as_str().unwrap_or_default();
    let Some(detail) = message.strip_prefix(&format!("{}\n", mc::VALIDATION_FAILED_PREFIX)) else {
        panic!(
            "case {case:?}: {member} {value:?} is refused in this service's validation \
             envelope, got {message:?}"
        );
    };
    assert!(
        detail.contains(member),
        "case {case:?}: the detail names the member that carried the value, got {detail:?}"
    );
    assert!(
        detail.contains(&format!("{value:?}")),
        "case {case:?}: the detail quotes the value that was refused, got {detail:?}"
    );
}

/// A CreateGateway body whose egress range is `egress` and whose single network's range is
/// `network`.
fn gateway_body(name: &str, egress: &str, network: &str) -> Value {
    json!({
        "name": name,
        "egressCidrBlocks": [egress],
        "networks": [{ "name": "net-1", "cidrBlock": network }],
    })
}

/// Every gateway the store holds, as `(GatewayArn, Name)`.
async fn listed_gateway_names(emulator: &Emulator, case: &str) -> Vec<String> {
    let (status, code, list) = emulator
        .send("GET", "/v1/gateways?maxResults=1000", None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: ListGateways must answer 200, got {status} {code:?}: {list}"
    );
    list["gateways"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|gateway| gateway["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// CreateGateway refuses a value that is not a CIDR block in EITHER of its range members,
/// and creates no gateway: the ranges configure the gateway, so a request that names one
/// the service cannot parse configures nothing.
#[tokio::test]
async fn create_gateway_refuses_malformed_ranges_and_stores_nothing() {
    let emulator = Emulator::new();
    let good = "10.0.0.0/16";

    for (index, value) in malformed_cidr_blocks().into_iter().enumerate() {
        for (member, name, body) in [
            (
                "EgressCidrBlocks[0]",
                format!("gw-egress-{index}"),
                gateway_body(&format!("gw-egress-{index}"), value, good),
            ),
            (
                "Networks[0].CidrBlock",
                format!("gw-network-{index}"),
                gateway_body(&format!("gw-network-{index}"), good, value),
            ),
        ] {
            let case = format!("{member} = {value:?}");
            let (status, code, response) = emulator.send("POST", "/v1/gateways", Some(body)).await;
            assert_cidr_refusal(&case, member, value, status, code.as_deref(), &response);

            let names = listed_gateway_names(&emulator, &case).await;
            assert!(
                !names.contains(&name),
                "case {case:?}: a refused create stores no gateway, got {names:?}"
            );
        }
    }
}

/// And every well-formed block is stored: the create answers 201 and DescribeGateway
/// reports the ranges the request named, so the refusal above turns away malformed values
/// only.
#[tokio::test]
async fn create_gateway_stores_every_well_formed_range() {
    let emulator = Emulator::new();

    for (index, value) in well_formed_cidr_blocks().into_iter().enumerate() {
        let case = format!("a gateway whose ranges are {value:?}");
        let name = format!("gw-ok-{index}");
        let (status, code, created) = emulator
            .send("POST", "/v1/gateways", Some(gateway_body(&name, value, value)))
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: create expected 201, got {status} {code:?}: {created}"
        );
        let gateway_arn = created["gateway"]["gatewayArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create answers an ARN: {created}"))
            .to_string();

        let (status, code, described) = emulator
            .send("GET", &format!("/v1/gateways/{gateway_arn}"), None)
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: describe expected 200, got {status} {code:?}: {described}"
        );
        assert_eq!(
            described["gateway"]["egressCidrBlocks"],
            json!([value]),
            "case {case:?}: the stored egress range is the one requested, got {described}"
        );
        assert_eq!(
            described["gateway"]["networks"],
            json!([{ "name": "net-1", "cidrBlock": value }]),
            "case {case:?}: the stored network range is the one requested, got {described}"
        );
    }
}

/// CreateFlow refuses a malformed range in a source's `WhitelistCidr` or an output's
/// `CidrAllowList`, and stores no flow.
#[tokio::test]
async fn create_flow_refuses_malformed_ranges_and_stores_nothing() {
    let emulator = Emulator::new();
    let good = "10.0.0.0/16";

    for (index, value) in malformed_cidr_blocks().into_iter().enumerate() {
        for (member, name, body) in [
            (
                "Sources[0].WhitelistCidr",
                format!("flow-source-{index}"),
                json!({
                    "name": format!("flow-source-{index}"),
                    "source": { "name": "src", "protocol": "rtp", "whitelistCidr": value },
                }),
            ),
            (
                "Outputs[0].CidrAllowList[0]",
                format!("flow-output-{index}"),
                json!({
                    "name": format!("flow-output-{index}"),
                    "source": { "name": "src", "protocol": "rtp", "whitelistCidr": good },
                    "outputs": [{
                        "name": "out-1",
                        "protocol": "rtp",
                        "destination": "198.51.100.10",
                        "port": 5000,
                        "cidrAllowList": [value],
                    }],
                }),
            ),
        ] {
            let case = format!("{member} = {value:?}");
            let (status, code, response) = emulator.create_flow(body).await;
            assert_cidr_refusal(&case, member, value, status, code.as_deref(), &response);

            let (status, _, list) = emulator.list_flows("?maxResults=1000").await;
            assert_eq!(status, 200, "case {case:?}: ListFlows must answer 200: {list}");
            let names: Vec<String> = list["flows"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|flow| flow["name"].as_str().unwrap_or_default().to_string())
                .collect();
            assert!(
                !names.contains(&name),
                "case {case:?}: a refused create stores no flow, got {names:?}"
            );
        }
    }
}

/// The flow a case adds to or updates: one `rtp` source and one output, both with a
/// well-formed range, and its `(FlowArn, SourceArn, OutputArn)`.
///
/// Source failover is ENABLED so the flow has room for a second source — "if the state is
/// active, the flow can have one or two sources" (`FailoverConfig$State`) — because the
/// AddFlowSources cases below must reach the range check rather than the source bound.
async fn flow_with_source_and_output(emulator: &Emulator, name: &str) -> (String, String, String) {
    let (status, code, created) = emulator
        .create_flow(json!({
            "name": name,
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" },
            "sourceFailoverConfig": { "state": "ENABLED" },
            "outputs": [{
                "name": "out-1",
                "protocol": "rtp",
                "destination": "198.51.100.10",
                "port": 5000,
                "cidrAllowList": ["10.0.0.0/16"],
            }],
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "the fixture flow must be created, got {status} {code:?}: {created}"
    );
    let flow = &created["flow"];
    (
        flow["flowArn"].as_str().unwrap_or_default().to_string(),
        flow["source"]["sourceArn"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        flow["outputs"][0]["outputArn"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    )
}

/// The add paths refuse a malformed range too, and append nothing: AddFlowSources carries
/// the same `SetSourceRequest` CreateFlow does, and AddFlowOutputs the same
/// `AddOutputRequest`, so one value cannot be admitted by one path and refused by another.
#[tokio::test]
async fn the_add_paths_refuse_malformed_ranges_and_append_nothing() {
    let emulator = Emulator::new();
    let (flow_arn, ..) = flow_with_source_and_output(&emulator, "flow-adds").await;

    for value in malformed_cidr_blocks() {
        let case = format!("AddFlowSources with WhitelistCidr {value:?}");
        let (status, code, response) = add_flow_sources(
            &emulator,
            &flow_arn,
            json!([{ "name": "added", "protocol": "rtp", "whitelistCidr": value }]),
        )
        .await;
        assert_cidr_refusal(
            &case,
            "Sources[0].WhitelistCidr",
            value,
            status,
            code.as_deref(),
            &response,
        );

        let case = format!("AddFlowOutputs with CidrAllowList {value:?}");
        let (status, code, response) = add_flow_outputs(
            &emulator,
            &flow_arn,
            json!([{
                "name": "added",
                "protocol": "rtp",
                "destination": "198.51.100.11",
                "port": 5001,
                "cidrAllowList": [value],
            }]),
        )
        .await;
        assert_cidr_refusal(
            &case,
            "Outputs[0].CidrAllowList[0]",
            value,
            status,
            code.as_deref(),
            &response,
        );
    }

    // Nothing above was appended: the flow still holds exactly what the create stored.
    let (status, code, described) = emulator.describe_flow(&flow_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "DescribeFlow must answer 200, got {status} {code:?}: {described}"
    );
    assert_eq!(
        described["flow"]["sources"].as_array().map(Vec::len),
        Some(1),
        "a refused add appends no source, got {described}"
    );
    assert_eq!(
        described["flow"]["outputs"].as_array().map(Vec::len),
        Some(1),
        "a refused add appends no output, got {described}"
    );
}

/// The update paths refuse a malformed range and write NOTHING: the stored source and
/// output keep the ranges they already carried, so a refused update leaves the record
/// exactly as it was.
#[tokio::test]
async fn the_update_paths_refuse_malformed_ranges_and_write_nothing() {
    let emulator = Emulator::new();
    let (flow_arn, source_arn, output_arn) =
        flow_with_source_and_output(&emulator, "flow-updates").await;

    for value in malformed_cidr_blocks() {
        let case = format!("UpdateFlowSource with WhitelistCidr {value:?}");
        let (status, code, response) = update_flow_source(
            &emulator,
            &flow_arn,
            &source_arn,
            json!({ "whitelistCidr": value, "description": "should not be written" }),
        )
        .await;
        assert_cidr_refusal(
            &case,
            "WhitelistCidr",
            value,
            status,
            code.as_deref(),
            &response,
        );

        let case = format!("UpdateFlowOutput with CidrAllowList {value:?}");
        let (status, code, response) = update_flow_output(
            &emulator,
            &flow_arn,
            &output_arn,
            json!({ "cidrAllowList": [value], "description": "should not be written" }),
        )
        .await;
        assert_cidr_refusal(
            &case,
            "CidrAllowList[0]",
            value,
            status,
            code.as_deref(),
            &response,
        );

        let (_, _, described) = emulator.describe_flow(&flow_arn).await;
        let source = &described["flow"]["sources"][0];
        assert_eq!(
            source["whitelistCidr"].as_str(),
            Some("10.0.0.0/16"),
            "case {case:?}: a refused update leaves the stored range alone, got {source}"
        );
        assert!(
            source["description"].is_null(),
            "case {case:?}: a refused update writes none of its other members, got {source}"
        );
        let output = &described["flow"]["outputs"][0];
        assert_eq!(
            at_path(output, "transport.cidrAllowList"),
            &json!(["10.0.0.0/16"]),
            "case {case:?}: a refused update leaves the stored list alone, got {output}"
        );
        assert!(
            output["description"].is_null(),
            "case {case:?}: a refused update writes none of its other members, got {output}"
        );
    }
}

/// And every well-formed range the update paths carry IS written: DescribeFlow reports
/// what the update stored, so the refusals above turn away malformed values only.
#[tokio::test]
async fn the_update_paths_store_every_well_formed_range() {
    let emulator = Emulator::new();
    let (flow_arn, source_arn, output_arn) =
        flow_with_source_and_output(&emulator, "flow-accepted").await;

    for value in well_formed_cidr_blocks() {
        let case = format!("an update whose range is {value:?}");
        let (status, code, updated) = update_flow_source(
            &emulator,
            &flow_arn,
            &source_arn,
            json!({ "whitelistCidr": value }),
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: UpdateFlowSource expected 202, got {status} {code:?}: {updated}"
        );
        let (status, code, updated) = update_flow_output(
            &emulator,
            &flow_arn,
            &output_arn,
            json!({ "cidrAllowList": [value] }),
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: UpdateFlowOutput expected 202, got {status} {code:?}: {updated}"
        );

        let (_, _, described) = emulator.describe_flow(&flow_arn).await;
        assert_eq!(
            described["flow"]["sources"][0]["whitelistCidr"].as_str(),
            Some(value),
            "case {case:?}: the stored source range is the one requested, got {described}"
        );
        assert_eq!(
            at_path(&described["flow"]["outputs"][0], "transport.cidrAllowList"),
            &json!([value]),
            "case {case:?}: the stored output list is the one requested, got {described}"
        );
    }
}
