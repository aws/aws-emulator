// SPDX-License-Identifier: CC0-1.0

//! What OpenSearch Serverless TELLS a caller whose managed endpoint cannot be placed is what
//! Amazon EC2 answered, not a sentence this service wrote.
//!
//! RULE: a VPC and a subnet are Amazon EC2's resources, so the placement a `CreateVpcEndpoint`
//! or `UpdateVpcEndpoint` names is resolved by CALLING EC2, and when that call comes back
//! refused the `ValidationException` the caller is answered with states EC2's OWN sentence
//! about the very id the request carried, verbatim, followed by the envelope the SDK layer
//! that relayed the call states around a failed call: the service it was made to, the status
//! that service answered with, that answer's own request id and how many attempts it took. The
//! subnets are resolved FIRST, because they are where the request places the endpoint's
//! network interfaces, so a request whose subnets EC2 holds none of is answered about them
//! even when the VPC beside them resolves to nothing too.
//!
//! The expectation is never typed out here: every case asks EC2 the same thing the service
//! asks it and reads the sentence and the status off THAT answer, so the assertion is EC2's
//! wording rather than this suite's, and the envelope is the only fixed text the suite states.
//!
//! The rows differ along what the rule generalises over: which member names the unresolvable
//! id (`subnetIds`, `vpcId`, `addSubnetIds`), the FORM of the value (EC2's 17-hex id form, its
//! older 8-hex form, and the all-zero form), WHERE in a list it sits, how many ids the request
//! names, and whether the VPC beside the subnets resolves.
//!
//! Sources: `smithy_query(query="operation", operation="CreateVpcEndpoint,UpdateVpcEndpoint")`
//! on the opensearchserverless model (both model `ValidationException`, which this service
//! answers HTTP 400 over `awsJson1_0`); `smithy_query(query="shape", shape="SubnetIds,VpcId")`
//! for the id forms each member admits. The service pair, the two wires and the unresolvable
//! id lists are `aoss_vpc_endpoint_placement_resolves_in_ec2.rs`'s beside this file, so both
//! suites stand up one estate and ask EC2 one way.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::aoss_vpc_endpoint_placement_resolves_in_ec2::{
    aoss, await_active, ec2_answer, ec2_vpc, emulator, unresolvable_subnet_ids,
    unresolvable_vpc_ids, Vpc, ACTIVE, CLIENT_ERROR,
};
use crate::query_wire::elements;

/// The error this service refuses an unplaceable endpoint with.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// What EC2 answers a read that NAMES `id`: the status and the sentence it states about that
/// id — the expectation every case below is built from, read off EC2 itself rather than
/// written out here.
///
/// `parameter` is the member the read names the id in (`SubnetId.1`, `VpcId.1`), and `action`
/// the read the service makes to resolve that kind of id. The sentence is read with the
/// document's own escaping undone — EC2 quotes the id in apostrophes, which travel the
/// ec2Query wire as `&apos;`.
async fn ec2_refusal(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    parameter: &str,
    id: &str,
) -> (u16, String) {
    let (status, body) = ec2_answer(router, action, &[(parameter, id)]).await;
    assert_eq!(
        status, CLIENT_ERROR,
        "case {case:?}: EC2 must refuse {action} naming {id:?}, got {status} with {body}"
    );
    let reason = elements(&body, "Message")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: EC2's refusal states no message: {body}"));
    (status, reason)
}

/// The envelope the SDK layer that relayed a call states around a failed call, split at the
/// request id: what precedes it, and what follows it.
///
/// Stated here because it is the only fixed text of the expectation — the sentence before it
/// is EC2's and the id inside it belongs to the relayed call, so it is read back rather than
/// predicted.
fn envelope_parts(status: u16) -> (String, String) {
    (
        format!(" (Service: Ec2, Status Code: {status}, Request ID: "),
        ") (SDK Attempt Count: 1)".to_string(),
    )
}

/// The request id a relayed refusal reports, asserting the whole message IS `reason` inside
/// the envelope for a call EC2 answered `status`.
fn relayed_request_id(case: &str, stated: &str, reason: &str, status: u16) -> String {
    let (head, tail) = envelope_parts(status);
    let opening = format!("{reason}{head}");
    let id = stated
        .strip_prefix(opening.as_str())
        .and_then(|rest| rest.strip_suffix(tail.as_str()))
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected EC2's own sentence inside the relay envelope, \
                 {opening:?}<request id>{tail:?}, got {stated:?}"
            )
        });
    assert!(
        !id.is_empty() && !id.contains(' '),
        "case {case:?}: expected the envelope to name the relayed call's request id, got {id:?} \
         in {stated:?}"
    );
    id.to_string()
}

/// One row of the create table: a request that cannot be placed, and the EC2 read whose
/// answer words its refusal.
struct Unplaceable {
    /// What the row varies, named in every assertion it makes.
    case: String,
    /// The `vpcId` the create names.
    vpc_id: String,
    /// The `subnetIds` the create names.
    subnet_ids: Vec<String>,
    /// The EC2 read the service makes to resolve the id this row is refused on.
    action: &'static str,
    /// The member that read names the id in.
    parameter: &'static str,
    /// The id EC2 is asked about — the one the refusal must be worded from.
    named: String,
}

/// Every way a create's placement fails to exist, over both members and all three id forms.
fn unplaceable_creates(held: &Vpc) -> Vec<Unplaceable> {
    let mut rows = Vec::new();
    for (form, unknown) in unresolvable_subnet_ids() {
        for (shape, subnet_ids) in [
            ("the only subnetIds entry", vec![unknown.to_string()]),
            (
                "the LAST of two subnetIds entries",
                vec![held.subnets[0].clone(), unknown.to_string()],
            ),
            (
                "the FIRST of three subnetIds entries",
                vec![
                    unknown.to_string(),
                    held.subnets[0].clone(),
                    held.subnets[1].clone(),
                ],
            ),
        ] {
            rows.push(Unplaceable {
                case: format!("{shape}, in {form}"),
                vpc_id: held.id.clone(),
                subnet_ids,
                action: "DescribeSubnets",
                parameter: "SubnetId.1",
                named: unknown.to_string(),
            });
        }
        // The precedence the recording pins: a request whose subnets resolve to nothing is
        // answered about THEM even though the VPC it names resolves to nothing either.
        rows.push(Unplaceable {
            case: format!("subnetIds and the vpcId beside it, both unresolvable, in {form}"),
            vpc_id: unresolvable_vpc_ids()[0].1.to_string(),
            subnet_ids: vec![unknown.to_string()],
            action: "DescribeSubnets",
            parameter: "SubnetId.1",
            named: unknown.to_string(),
        });
    }
    for (form, unknown) in unresolvable_vpc_ids() {
        rows.push(Unplaceable {
            case: format!("a vpcId EC2 holds no VPC of, in {form}"),
            vpc_id: unknown.to_string(),
            subnet_ids: held.subnets[..1].to_vec(),
            action: "DescribeVpcs",
            parameter: "VpcId.1",
            named: unknown.to_string(),
        });
    }
    rows
}

#[tokio::test]
async fn an_unplaceable_create_states_ec2s_own_sentence_inside_the_relay_envelope() {
    let (router, _engine) = emulator();
    let held = ec2_vpc(&router, 80, &["us-west-2a", "us-west-2b"]).await;

    let mut request_ids: Vec<String> = Vec::new();
    for (index, row) in unplaceable_creates(&held).iter().enumerate() {
        let case = row.case.as_str();
        let (status, reason) =
            ec2_refusal(&router, case, row.action, row.parameter, &row.named).await;
        let refused = aoss(
            &router,
            "CreateVpcEndpoint",
            json!({
                "name": format!("relayed-refusal-{index}"),
                "vpcId": row.vpc_id,
                "subnetIds": row.subnet_ids,
            }),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (CLIENT_ERROR, VALIDATION_EXCEPTION),
            "case {case:?}: expected {CLIENT_ERROR} {VALIDATION_EXCEPTION}, got {} with {}",
            refused.status,
            refused.body
        );
        request_ids.push(relayed_request_id(case, refused.message(), &reason, status));
    }

    // Each refusal reports the request id of ITS OWN relayed call, so no two state the same
    // one: the id belongs to the call that was made, not to anything in the request.
    let mut distinct = request_ids.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        request_ids.len(),
        "case \"every relayed refusal\": expected one request id per relayed call, got \
         {request_ids:?}"
    );
}

#[tokio::test]
async fn an_unplaceable_update_states_ec2s_own_sentence_inside_the_relay_envelope() {
    let (router, _engine) = emulator();
    let held = ec2_vpc(&router, 81, &["us-west-2a", "us-west-2b"]).await;

    let created = aoss(
        &router,
        "CreateVpcEndpoint",
        json!({
            "name": "relayed-update-endpoint",
            "vpcId": held.id,
            "subnetIds": held.subnets[..1].to_vec(),
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup: the create must be accepted, got {} with {}",
        created.status, created.body
    );
    let id = created
        .body
        .get("createVpcEndpointDetail")
        .and_then(|detail| detail.get("id"))
        .and_then(Value::as_str)
        .expect("setup: the create reports an endpoint id")
        .to_string();
    await_active(&router, "setup", &id).await;

    // `addSubnetIds` is resolved by the same gate, so an id EC2 holds no subnet of is answered
    // with EC2's own sentence here too — over each id form, and over an add list of one and of
    // two.
    for (form, unknown) in unresolvable_subnet_ids() {
        for (shape, adding) in [
            ("one added subnet", vec![unknown.to_string()]),
            (
                "two added subnets, the resolvable one first",
                vec![held.subnets[1].clone(), unknown.to_string()],
            ),
        ] {
            let case = format!("addSubnetIds naming a subnet EC2 does not hold ({shape}, {form})");
            let (status, reason) =
                ec2_refusal(&router, &case, "DescribeSubnets", "SubnetId.1", unknown).await;
            let refused = aoss(
                &router,
                "UpdateVpcEndpoint",
                json!({ "id": id, "addSubnetIds": adding }),
            )
            .await;
            assert_eq!(
                (refused.status, refused.error_code().as_str()),
                (CLIENT_ERROR, VALIDATION_EXCEPTION),
                "case {case:?}: expected {CLIENT_ERROR} {VALIDATION_EXCEPTION}, got {} with {}",
                refused.status,
                refused.body
            );
            relayed_request_id(&case, refused.message(), &reason, status);
        }
    }

    // And the subnet EC2 DOES hold is still added, so the gate refuses only what EC2 refused,
    // and the reader reports the endpoint back in its terminal status.
    let updated = aoss(
        &router,
        "UpdateVpcEndpoint",
        json!({ "id": id, "addSubnetIds": [held.subnets[1].clone()] }),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "case \"addSubnetIds naming EC2's own subnet\": expected 200, got {} with {}",
        updated.status, updated.body
    );
    let detail = await_active(&router, "after the accepted update", &id).await;
    let mut reported: Vec<String> = detail
        .get("subnetIds")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    reported.sort();
    let mut expected = held.subnets.clone();
    expected.sort();
    assert_eq!(
        reported, expected,
        "case \"addSubnetIds naming EC2's own subnet\": expected both of EC2's subnets to be \
         reported in the {ACTIVE} endpoint, got {detail}"
    );
}
