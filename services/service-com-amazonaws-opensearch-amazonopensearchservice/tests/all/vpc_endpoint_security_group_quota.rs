// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how many security groups a service-managed VPC endpoint's
//! `VpcOptions` may name: the account's own security-groups-per-network-interface quota.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness comes from [`super::domain_lifecycle`]
//! and the endpoint helpers from [`super::vpc_endpoints`] and
//! [`super::vpc_endpoint_updates`]; this suite adds no second copy of either.
//!
//! The rule under test: the security groups a VPC placement names are attached to the
//! elastic network interface it is reached through, and `VPCOptions$SecurityGroupIds`
//! carries no modelled bound, so the ceiling is the value the ACCOUNT's "Security groups
//! per network interface" quota is in force at: a request naming more is refused with the
//! modelled `ValidationException`, the stored placement keeps exactly the groups it had,
//! and the same request is served once that quota is raised.
//!
//! The rows differ along what the rule generalises over: how many groups are named (every
//! count from one to the quota, and several past it), which call names them (the create
//! and the update of an endpoint, and the create of a VPC-access domain), and what the
//! account's quota is in force at.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::VPC_ENDPOINT_STATUS_ACTIVE;

use super::domain_lifecycle::{error_code, poll_until, Answer, Emulator};
use super::vpc_endpoint_updates::update_endpoint;
use super::vpc_endpoints::{
    create_endpoint, describe_endpoints, list_endpoints, member, minted, vpc_domain,
};

/// AWS's published DEFAULT value for the account quota this ceiling is: "Security groups
/// per network interface", `L-2AFB9258` of the `vpc` service code, whose default value is
/// 5 and which is adjustable (what `service-quotas list-aws-default-service-quotas
/// --service-code vpc` reports). Stated here rather than taken from the crate, so a change
/// to either side fails this suite.
const DEFAULT_GROUPS_PER_INTERFACE: usize = 5;

/// The Service Quotas identifiers of that quota, which the increase below is asked for.
const QUOTA_SERVICE_CODE: &str = "vpc";
const QUOTA_CODE: &str = "L-2AFB9258";

/// The subnet every placement below is created in: one subnet id, so what differs between
/// the rows is only the security groups.
const SUBNET: &str = "subnet-0aaa0000000000001";

/// `how_many` distinct security group ids, each of the form EC2 mints.
fn security_groups(how_many: usize) -> Vec<String> {
    (0..how_many)
        .map(|index| format!("sg-0bbb00000000{index:05}"))
        .collect()
}

/// `VpcOptions` naming `how_many` security groups in the one subnet.
fn vpc_options(how_many: usize) -> Value {
    json!({ "SubnetIds": [SUBNET], "SecurityGroupIds": security_groups(how_many) })
}

/// The security groups `DescribeVpcEndpoints` reports for one endpoint — the STORED
/// placement, which is what a refused change must leave alone.
async fn stored_groups(emulator: &Emulator, id: &str, case: &str) -> Vec<String> {
    let answer = describe_endpoints(emulator, &[id]).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: reading endpoint {id:?} must be served, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["VpcEndpoints"][0]["VpcOptions"]["SecurityGroupIds"]
        .as_array()
        .map(|groups| {
            groups
                .iter()
                .map(|group| group.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Look again until the endpoint `id` is the status the clock owed it, so a case waits
/// for the CONDITION rather than for a stretch of time.
async fn settled_endpoint(emulator: &Emulator, id: &str, case: &str) {
    poll_until(case, || async {
        let read = describe_endpoints(emulator, &[id]).await;
        read.body["VpcEndpoints"][0]["Status"] == json!(VPC_ENDPOINT_STATUS_ACTIVE)
    })
    .await;
}

/// One ACTIVE endpoint on a VPC domain, created with a single security group, answering
/// with its id.
async fn active_endpoint(emulator: &Emulator, domain: &str) -> String {
    let arn = vpc_domain(emulator, domain).await;
    let case = "the endpoint the quota rule is asserted on";
    let answer = create_endpoint(
        emulator,
        json!({ "DomainArn": arn, "VpcOptions": vpc_options(1) }),
    )
    .await;
    let id = member(&minted(&answer, case), "VpcEndpointId", case);
    settled_endpoint(emulator, &id, case).await;
    id
}

/// Assert `answer` is the modelled refusal of an over-quota placement, stating the limit
/// in force and the placement it was asked about.
fn assert_over_quota(answer: &Answer, case: &str, ceiling: usize, placement: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case}: expected HTTP 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        error_code(answer),
        "ValidationException",
        "case {case}: expected ValidationException, got {:?}",
        answer.body
    );
    let expected = format!(
        "You\u{2019}ve exceeded the limit for security groups. You can select up to {ceiling} \
         security groups for {placement}."
    );
    assert_eq!(
        answer.body["message"].as_str().unwrap_or_default(),
        expected,
        "case {case}: expected {expected:?}, got {:?}",
        answer.body
    );
}

/// What the refusal names each placement as.
const VPC_ENDPOINT: &str = "the VPC endpoint";
const DOMAIN: &str = "the domain";

#[tokio::test]
async fn an_update_naming_more_security_groups_than_one_interface_carries_is_refused() {
    let emulator = Emulator::new();
    let id = active_endpoint(&emulator, "sg-quota-update").await;
    let before = stored_groups(&emulator, &id, "the created endpoint").await;
    assert_eq!(
        before,
        security_groups(1),
        "case the created endpoint: expected the group the create named, got {before:?}"
    );

    // Past the ceiling: one group more than the quota, and well past it. Each is refused
    // and the endpoint keeps exactly the groups it had, read back through
    // `DescribeVpcEndpoints` rather than from the refused call.
    for how_many in [
        DEFAULT_GROUPS_PER_INTERFACE + 1,
        DEFAULT_GROUPS_PER_INTERFACE + 2,
        DEFAULT_GROUPS_PER_INTERFACE + 10,
    ] {
        let case = format!("an update naming {how_many} security groups");
        let answer = update_endpoint(&emulator, &id, &vpc_options(how_many)).await;
        assert_over_quota(
            &answer,
            &case,
            DEFAULT_GROUPS_PER_INTERFACE,
            VPC_ENDPOINT,
        );
        assert_eq!(
            stored_groups(&emulator, &id, &case).await,
            before,
            "case {case}: a refused update must leave the endpoint's groups as they were"
        );
    }

    // At the ceiling the update is served, so the endpoint can still be changed after the
    // refusals — and the groups it ends up with are the ones that update named.
    let at_the_ceiling = security_groups(DEFAULT_GROUPS_PER_INTERFACE);
    let served = update_endpoint(
        &emulator,
        &id,
        &vpc_options(DEFAULT_GROUPS_PER_INTERFACE),
    )
    .await;
    assert_eq!(
        served.status, 200,
        "case an update naming exactly the quota: expected 200, got {} with {:?}",
        served.status, served.body
    );
    settled_endpoint(&emulator, &id, "the update at the ceiling").await;
    assert_eq!(
        stored_groups(&emulator, &id, "the update at the ceiling").await,
        at_the_ceiling,
        "case the update at the ceiling: expected the groups it named"
    );
}

#[tokio::test]
async fn a_create_naming_more_security_groups_than_one_interface_carries_mints_nothing() {
    let emulator = Emulator::new();
    let arn = vpc_domain(&emulator, "sg-quota-create").await;

    // Every count from one to the quota is served; every count past it is refused and
    // mints no endpoint.
    for how_many in 1..=DEFAULT_GROUPS_PER_INTERFACE {
        let case = format!("a create naming {how_many} security groups");
        let answer = create_endpoint(
            &emulator,
            json!({ "DomainArn": arn, "VpcOptions": vpc_options(how_many) }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let id = member(&minted(&answer, &case), "VpcEndpointId", &case);
        assert_eq!(
            stored_groups(&emulator, &id, &case).await,
            security_groups(how_many),
            "case {case}: expected the endpoint to carry the groups the create named"
        );
    }

    let listed_before = listed_endpoint_ids(&emulator).await;
    for how_many in [
        DEFAULT_GROUPS_PER_INTERFACE + 1,
        DEFAULT_GROUPS_PER_INTERFACE + 4,
    ] {
        let case = format!("a create naming {how_many} security groups");
        let answer = create_endpoint(
            &emulator,
            json!({ "DomainArn": arn, "VpcOptions": vpc_options(how_many) }),
        )
        .await;
        assert_over_quota(
            &answer,
            &case,
            DEFAULT_GROUPS_PER_INTERFACE,
            VPC_ENDPOINT,
        );
        assert_eq!(
            listed_endpoint_ids(&emulator).await,
            listed_before,
            "case {case}: a refused create must mint no endpoint"
        );
    }

    // The same bound holds on the VPC-access DOMAIN whose interfaces carry those groups.
    let domain = emulator
        .create(json!({
            "DomainName": "sg-quota-domain",
            "ClusterConfig": { "InstanceType": "t3.small.search", "InstanceCount": 1 },
            "VPCOptions": vpc_options(DEFAULT_GROUPS_PER_INTERFACE + 1),
        }))
        .await;
    assert_over_quota(
        &domain,
        "a VPC-access domain naming one group past the quota",
        DEFAULT_GROUPS_PER_INTERFACE,
        DOMAIN,
    );
    let read = emulator.describe("sg-quota-domain").await;
    assert_ne!(
        read.status, 200,
        "case a VPC-access domain naming one group past the quota: a refused create must \
         provision no domain, got {:?}",
        read.body
    );
}

/// The ids of every endpoint this account holds, read through `ListVpcEndpoints`.
async fn listed_endpoint_ids(emulator: &Emulator) -> Vec<String> {
    let answer = list_endpoints(emulator, None).await;
    assert_eq!(
        answer.status, 200,
        "the privileged listing must be served, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["VpcEndpointSummaryList"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    entry["VpcEndpointId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// RULE: the ceiling is the ACCOUNT's quota rather than a number this service fixes, so
/// raising "Security groups per network interface" through Service Quotas' own
/// `RequestServiceQuotaIncrease` — the operation an account raises it with — makes the very
/// list that was refused acceptable, and a list past the RAISED value is still refused.
#[tokio::test]
async fn the_ceiling_is_the_account_quota_in_force() {
    let emulator = Emulator::with_service_quotas();
    let id = active_endpoint(&emulator, "sg-quota-account").await;
    let raised = DEFAULT_GROUPS_PER_INTERFACE + 2;
    // The same ceiling as the quota's own `Value`, which is a number rather than a count.
    let raised_value = f64::from(u32::try_from(raised).expect("the ceiling fits"));

    // The quota in force is the published default, so a list past it is refused.
    let refused = update_endpoint(&emulator, &id, &vpc_options(raised)).await;
    assert_over_quota(
        &refused,
        &format!("an update naming {raised} groups while the default is in force"),
        DEFAULT_GROUPS_PER_INTERFACE,
        VPC_ENDPOINT,
    );

    // The account raises it, through the service that owns the quota.
    let asked = emulator
        .quotas_call(
            "RequestServiceQuotaIncrease",
            json!({
                "ServiceCode": QUOTA_SERVICE_CODE,
                "QuotaCode": QUOTA_CODE,
                "DesiredValue": raised,
            }),
        )
        .await;
    assert_eq!(
        asked.status, 200,
        "case the quota increase request: expected 200, got {} with {:?}",
        asked.status, asked.body
    );
    // The decision the clock owes that request falls due on its own: the case waits for
    // the raised value to be IN FORCE, read through the operation that reports it.
    poll_until("the quota increase being decided", || async {
        let read = emulator
            .quotas_call(
                "GetServiceQuota",
                json!({ "ServiceCode": QUOTA_SERVICE_CODE, "QuotaCode": QUOTA_CODE }),
            )
            .await;
        read.body["Quota"]["Value"].as_f64() == Some(raised_value)
    })
    .await;
    let in_force = emulator
        .quotas_call(
            "GetServiceQuota",
            json!({ "ServiceCode": QUOTA_SERVICE_CODE, "QuotaCode": QUOTA_CODE }),
        )
        .await;
    assert_eq!(
        in_force.body["Quota"]["Value"].as_f64(),
        Some(raised_value),
        "case the quota after the increase: expected the raised value in force, got {:?}",
        in_force.body
    );

    // ... and the SAME update is now served, while one past the raised value is not.
    let served = update_endpoint(&emulator, &id, &vpc_options(raised)).await;
    assert_eq!(
        served.status, 200,
        "case the update that was refused before the increase: expected 200, got {} with {:?}",
        served.status, served.body
    );
    settled_endpoint(&emulator, &id, "the update under the raised quota").await;
    assert_eq!(
        stored_groups(&emulator, &id, "the update under the raised quota").await,
        security_groups(raised),
        "case the update under the raised quota: expected the groups it named"
    );

    let past_the_raised = update_endpoint(&emulator, &id, &vpc_options(raised + 1)).await;
    assert_over_quota(
        &past_the_raised,
        &format!("an update naming {} groups under the raised quota", raised + 1),
        raised,
        VPC_ENDPOINT,
    );
}
