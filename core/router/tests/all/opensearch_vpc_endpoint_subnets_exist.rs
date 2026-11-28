// SPDX-License-Identifier: CC0-1.0

//! A service-managed OpenSearch VPC endpoint is created in subnets the account HOLDS.
//!
//! RULE: `VPCOptions$SubnetIds` names subnets of the service that owns them, so an
//! identifier that service answers NO subnet for names nothing a placement could be created
//! in: the call is refused with the modelled `ValidationException` (HTTP 400) stating
//! `The subnet ID '<id>' does not exist` — the fixed text around the identifier from the
//! REQUEST in single quotes — and nothing is written. Because the subnets are what the
//! placement is derived from, they are judged BEFORE anything else the request names: before
//! the security groups, and before the `DomainArn` is resolved, so a call that names both an
//! unknown subnet and a domain this account does not hold is told about the subnet. The same
//! call naming subnets the account does hold is served (200) and the endpoint settles in
//! them. Both calls that take `VpcOptions` for an endpoint are judged by it:
//! `CreateVpcEndpoint` and `UpdateVpcEndpoint`.
//!
//! A subnet is Amazon EC2's resource, so whether the account holds one is EC2's to say: this
//! suite lives beside the router because every VPC, subnet and group below is minted by the
//! real EC2 operations on the SAME engine that serves OpenSearch Service, so every refusal is
//! grounded in what EC2 actually answered and every accepted identifier is one EC2 handed
//! out. The wire harness is the one [`crate::opensearch_vpc_endpoint_vpc_membership`]
//! publishes, and this suite is the subnet half of what
//! [`crate::opensearch_vpc_endpoint_security_groups_exist`] drives for security groups.
//!
//! The rows differ along what the rule generalises over: which identifier form the unknown
//! value takes (the long and the short one EC2 issues), how many subnets the call names, where
//! in the list the unknown one sits, and what ELSE the same request gets wrong (an unknown
//! security group, a domain the account does not hold).

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::opensearch_vpc_endpoint_vpc_membership::{
    emulator, opensearch, security_group, vpc_with_subnets, Reply, PREFIX,
};

/// The engine release the domain below runs — one the service's catalog publishes for the
/// instance type it uses.
const ENGINE_VERSION: &str = "OpenSearch_2.11";

/// Well-formed identifiers no `CreateSubnet` below handed out: EC2 answers for no subnet
/// under either, in each of the two identifier widths it issues.
const UNKNOWN_LONG: &str = "subnet-0baadf00d00000001";
const UNKNOWN_SHORT: &str = "subnet-0baadf01";

/// A well-formed security-group identifier EC2 answers for no group, for the row that gets
/// BOTH halves of the placement wrong.
const UNKNOWN_GROUP: &str = "sg-0baadf00d00000002";

/// How long a poll waits for a span the clock owes an endpoint or a domain to fall due.
///
/// Generous on purpose: the condition is what is waited for, not a duration, so a loaded host
/// only makes the loop go round more times.
const READY_DEADLINE: Duration = Duration::from_secs(120);

/// How long one turn of a poll loop pauses before looking again.
const POLL_PAUSE: Duration = Duration::from_millis(50);

/// How AWS words a refusal of an identifier it holds no subnet for: fixed text and the value
/// from the request in single quotes. Spelled out here rather than taken from the crate, so a
/// change to either part fails this suite.
fn subnet_does_not_exist(value: &str) -> String {
    format!("The subnet ID '{value}' does not exist")
}

/// `CreateVpcEndpoint` for `domain_arn`, as it answered.
async fn create_endpoint(
    router: &MultiServiceRouter,
    domain_arn: &str,
    subnets: &[String],
    groups: &[String],
) -> Reply {
    opensearch(
        router,
        "POST",
        &format!("{PREFIX}/opensearch/vpcEndpoints"),
        Some(json!({
            "DomainArn": domain_arn,
            "VpcOptions": { "SubnetIds": subnets, "SecurityGroupIds": groups },
        })),
    )
    .await
}

/// `UpdateVpcEndpoint` with the `VpcOptions` a row names, as it answered.
async fn update_endpoint(
    router: &MultiServiceRouter,
    id: &str,
    subnets: &[String],
    groups: &[String],
) -> Reply {
    opensearch(
        router,
        "POST",
        &format!("{PREFIX}/opensearch/vpcEndpoints/update"),
        Some(json!({
            "VpcEndpointId": id,
            "VpcOptions": { "SubnetIds": subnets, "SecurityGroupIds": groups },
        })),
    )
    .await
}

/// The endpoint `DescribeVpcEndpoints` reports for `id`.
async fn read_endpoint(router: &MultiServiceRouter, id: &str) -> Value {
    let read = opensearch(
        router,
        "POST",
        &format!("{PREFIX}/opensearch/vpcEndpoints/describe"),
        Some(json!({ "VpcEndpointIds": [id] })),
    )
    .await;
    assert_eq!(
        read.status, 200,
        "reading endpoint {id:?}: expected 200, got {} with {}",
        read.status, read.body
    );
    read.body["VpcEndpoints"][0].clone()
}

/// How many endpoints `ListVpcEndpoints` reports for this account.
async fn listed_endpoint_count(router: &MultiServiceRouter) -> usize {
    let listed = opensearch(
        router,
        "GET",
        &format!("{PREFIX}/opensearch/vpcEndpoints"),
        None,
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "a listing of this account's endpoints must be served, got {} with {}",
        listed.status, listed.body
    );
    listed.body["VpcEndpointSummaryList"]
        .as_array()
        .map_or(0, Vec::len)
}

/// The `SubnetIds` an endpoint reports, in the order it reports them.
fn endpoint_subnets(endpoint: &Value) -> Vec<String> {
    endpoint["VpcOptions"]["SubnetIds"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|value| value.as_str().map(str::to_owned))
        .collect()
}

/// Look again until the endpoint rests in `ACTIVE`, so the next change is served on it, or a
/// generous deadline passes.
async fn await_active_endpoint(router: &MultiServiceRouter, id: &str) -> Value {
    let deadline = Instant::now() + READY_DEADLINE;
    loop {
        let endpoint = read_endpoint(router, id).await;
        if endpoint["Status"] == json!("ACTIVE") {
            return endpoint;
        }
        assert!(
            Instant::now() < deadline,
            "the endpoint {id:?} never became active: {endpoint}"
        );
        tokio::time::sleep(POLL_PAUSE).await;
    }
}

/// Look again until the domain reports itself created with nothing in flight.
async fn await_settled_domain(router: &MultiServiceRouter, name: &str) -> Value {
    let deadline = Instant::now() + READY_DEADLINE;
    loop {
        let read = opensearch(
            router,
            "GET",
            &format!("{PREFIX}/opensearch/domain/{name}"),
            None,
        )
        .await;
        let status = read.body["DomainStatus"].clone();
        if status["Created"] == json!(true) && status["Processing"] == json!(false) {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "the domain {name:?} never settled: {status}"
        );
        tokio::time::sleep(POLL_PAUSE).await;
    }
}

/// Assert `answer` is the modelled refusal naming `offending` as a subnet that does not
/// exist.
fn assert_refused(answer: &Reply, offending: &str, case: &str) {
    assert_eq!(
        (answer.status, answer.error_code()),
        (400, "ValidationException"),
        "case {case:?}: expected a 400 ValidationException, got {} with {}",
        answer.status,
        answer.body
    );
    assert_eq!(
        answer.message(),
        subnet_does_not_exist(offending),
        "case {case:?}: expected the sentence naming {offending:?}, got {}",
        answer.body
    );
}

#[tokio::test]
async fn a_vpc_endpoint_call_refuses_a_subnet_the_account_does_not_hold() {
    let router = emulator();

    // One VPC with three subnets and a group of that VPC — every identifier below is one EC2
    // handed out, or one it answers for nothing about.
    let (vpc_id, subnets) =
        vpc_with_subnets(&router, "10.160", &["us-west-2a", "us-west-2b", "us-west-2c"]).await;
    let group = security_group(&router, &vpc_id, "opensearch-subnet-rule").await;
    let groups = vec![group.clone()];

    let domain = "subnet-exists-domain";
    let created = opensearch(
        &router,
        "POST",
        &format!("{PREFIX}/opensearch/domain"),
        Some(json!({
            "DomainName": domain,
            "EngineVersion": ENGINE_VERSION,
            "ClusterConfig": { "InstanceType": "t3.small.search", "InstanceCount": 1 },
            "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 10 },
            "VPCOptions": { "SubnetIds": [subnets[0].clone()], "SecurityGroupIds": groups },
        })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup: CreateDomain({domain:?}) must be served, got {} with {}",
        created.status, created.body
    );
    let domain_arn = await_settled_domain(&router, domain).await["ARN"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: the domain reported no ARN"))
        .to_owned();
    // An ARN of the same form naming a domain this account does not hold, built from the one
    // the service minted so only the domain name differs.
    let absent_domain_arn = domain_arn.replace(domain, "subnet-rule-no-such-domain");

    // THE CREATE half: a create naming an identifier EC2 holds no subnet for mints nothing,
    // over the rows that differ in which identifier width the unknown value takes, how many
    // subnets the call names and where the unknown one sits.
    let held_before = listed_endpoint_count(&router).await;
    let refused_rows: [(&str, Vec<String>, &str); 4] = [
        (
            "an identifier of the long form the account holds no subnet for",
            vec![UNKNOWN_LONG.to_string()],
            UNKNOWN_LONG,
        ),
        (
            "an identifier of the short form the account holds no subnet for",
            vec![UNKNOWN_SHORT.to_string()],
            UNKNOWN_SHORT,
        ),
        (
            "an unknown identifier after a subnet the account holds",
            vec![subnets[0].clone(), UNKNOWN_LONG.to_string()],
            UNKNOWN_LONG,
        ),
        (
            "an unknown identifier before a subnet the account holds",
            vec![UNKNOWN_SHORT.to_string(), subnets[1].clone()],
            UNKNOWN_SHORT,
        ),
    ];
    for (case, named, offending) in &refused_rows {
        let refused = create_endpoint(&router, &domain_arn, named, &groups).await;
        assert_refused(&refused, offending, case);
        assert_eq!(
            listed_endpoint_count(&router).await,
            held_before,
            "case {case:?}: a refused create must mint no endpoint"
        );
    }

    // ... and the subnets are judged BEFORE the rest of the request: a call that also names a
    // group the account does not hold, and one whose `DomainArn` names no domain of this
    // account, is still told about the subnet.
    let also_wrong: [(&str, &str, Vec<String>); 2] = [
        (
            "an unknown subnet beside an unknown security group",
            domain_arn.as_str(),
            vec![UNKNOWN_GROUP.to_string()],
        ),
        (
            "an unknown subnet on a domain this account does not hold",
            absent_domain_arn.as_str(),
            groups.clone(),
        ),
    ];
    let unknown_one = vec![UNKNOWN_LONG.to_string()];
    for (case, arn, named_groups) in &also_wrong {
        let refused = create_endpoint(&router, arn, &unknown_one, named_groups).await;
        assert_refused(&refused, UNKNOWN_LONG, case);
        assert_eq!(
            listed_endpoint_count(&router).await,
            held_before,
            "case {case:?}: a refused create must mint no endpoint"
        );
    }

    // ... while a create naming subnets the account DOES hold is served.
    let own_subnets = vec![subnets[0].clone(), subnets[1].clone()];
    let served = create_endpoint(&router, &domain_arn, &own_subnets, &groups).await;
    assert_eq!(
        served.status, 200,
        "case a create naming two subnets the account holds: expected it to be served, got {} \
         with {}",
        served.status, served.body
    );
    let id = served.body["VpcEndpoint"]["VpcEndpointId"]
        .as_str()
        .unwrap_or_else(|| panic!("the create reported no VpcEndpointId: {}", served.body))
        .to_owned();
    let active = await_active_endpoint(&router, &id).await;
    assert_eq!(
        endpoint_subnets(&active),
        own_subnets,
        "the endpoint must carry the subnets the create named, got {active}"
    );

    // THE CHANGE half of the same rule: every refused change leaves the endpoint standing —
    // its options and the status it was resting in — and a change naming a subnet the account
    // holds is served and settles in it.
    for (case, named, offending) in &refused_rows {
        let before = read_endpoint(&router, &id).await;
        let answer = update_endpoint(&router, &id, named, &groups).await;
        assert_refused(&answer, offending, case);
        let after = read_endpoint(&router, &id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused change must leave the endpoint standing, got {after}"
        );
    }

    let moved = vec![subnets[2].clone()];
    let answer = update_endpoint(&router, &id, &moved, &groups).await;
    assert_eq!(
        answer.status, 200,
        "case a change naming a third subnet of the same VPC: expected it to be served, got \
         {} with {}",
        answer.status, answer.body
    );
    let settled = await_active_endpoint(&router, &id).await;
    assert_eq!(
        endpoint_subnets(&settled),
        moved,
        "the endpoint must carry the subnet the change named, got {settled}"
    );
}
