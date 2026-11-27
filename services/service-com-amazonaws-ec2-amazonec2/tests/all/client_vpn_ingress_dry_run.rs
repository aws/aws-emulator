// SPDX-License-Identifier: CC0-1.0

//! The `DryRun` flag on the INGRESS AUTHORIZATION RULE operations of a Client VPN endpoint:
//! `AuthorizeClientVpnIngress`, which adds one rule, and `RevokeClientVpnIngress`, which takes
//! one off again — read back through `DescribeClientVpnAuthorizationRules`.
//!
//! Driven over the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=<Operation>`, answered
//! with XML (README.md / OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2`
//! signing name in the request's SigV4 credential scope, so every call here carries one.
//!
//! The rules under test:
//!
//! 1. `DryRun` "checks whether you have the required permissions for the action, WITHOUT
//!    ACTUALLY MAKING THE REQUEST, and provides an error response. If you have the required
//!    permissions, the error response is DryRunOperation"
//!    (`AuthorizeClientVpnIngressRequest$DryRun`, `RevokeClientVpnIngressRequest$DryRun`): the
//!    flag is a question about the CALLER, so a request that sets it is answered that one
//!    verdict ahead of resolving or validating anything the request names — whatever else
//!    about the request would have been served or refused — and nothing is written.
//! 2. `DryRun` unset, or set to `false`, is no dry run at all: such a request is performed, so
//!    an authorize adds its rule and a revoke takes one off, each reported by the describe.
//!
//! Every table row below is a distinct input along what rule 1 generalises over: which of the
//! two operations carries the flag, and what the rest of the request would otherwise have met
//! — a grant that would be served (to every client, and to one group), a duplicate of a stored
//! rule, a rule that is not there, an endpoint the account does not hold, a parameter the
//! request sent empty, a network that is no CIDR block, and a pair of parameters that cannot
//! be carried together.

use ec2::ops::types::ClientVpnEndpoint;
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, CLIENT_VPN_AUTHORIZATION_RULE_STATUS_AUTHORIZING,
    CLIENT_VPN_AUTHORIZATION_RULE_STATUS_REVOKING, DRY_RUN_OPERATION, DRY_RUN_OPERATION_MESSAGE,
    DRY_RUN_OPERATION_STATUS,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message};

/// The Region every call here is signed for. The Client VPN stores are per account rather
/// than per Region, so the Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// The actions this suite drives.
const AUTHORIZE: &str = "AuthorizeClientVpnIngress";
const REVOKE: &str = "RevokeClientVpnIngress";
const DESCRIBE: &str = "DescribeClientVpnAuthorizationRules";

/// A well-formed endpoint id this account holds no endpoint for.
const ABSENT_ENDPOINT: &str = "cvpn-endpoint-0123456789abcdef1";

/// The VPC the seeded endpoint is attached to.
const ENDPOINT_VPC: &str = "vpc-00000000000000001";

/// The two rules every case below starts from: one network granted to every client, one
/// granted to a single identity-provider group.
const SEEDED_ALL_GROUPS_NETWORK: &str = "10.0.0.0/16";
const SEEDED_GROUP_NETWORK: &str = "192.168.4.0/24";
const SEEDED_GROUP: &str = "engineering";

/// One form field.
fn p(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// One call of `action` carrying `params`, as `(status, body)`.
async fn call(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// Write one Client VPN endpoint, the way `CreateClientVpnEndpoint` would, and answer the
/// `ClientVpnEndpointId` it was minted with.
fn seed_endpoint(engine: &engine::InMemoryEngine) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_client_vpn_endpoint(ClientVpnEndpoint {
            description: Some("the endpoint every dry run names".to_string()),
            client_cidr_block: Some("10.100.0.0/22".to_string()),
            vpc_id: Some(ENDPOINT_VPC.to_string()),
            ..Default::default()
        })
        .expect("the store must take the Client VPN endpoint")
}

/// The form fields of one authorize: the endpoint, the network, and who the network is
/// granted to — one group, or every client when `group` is `None`.
fn authorize_params(
    endpoint_id: &str,
    network: &str,
    group: Option<&str>,
) -> Vec<(String, String)> {
    let mut params = vec![
        p("ClientVpnEndpointId", endpoint_id),
        p("TargetNetworkCidr", network),
    ];
    match group {
        Some(group) => params.push(p("AccessGroupId", group)),
        None => params.push(p("AuthorizeAllGroups", "true")),
    }
    params
}

/// The form fields of one revoke, named the same way.
fn revoke_params(endpoint_id: &str, network: &str, group: Option<&str>) -> Vec<(String, String)> {
    let mut params = vec![
        p("ClientVpnEndpointId", endpoint_id),
        p("TargetNetworkCidr", network),
    ];
    match group {
        Some(group) => params.push(p("AccessGroupId", group)),
        None => params.push(p("RevokeAllGroups", "true")),
    }
    params
}

/// `params` with the `DryRun` field `flag` spells appended.
fn with_dry_run(mut params: Vec<(String, String)>, flag: &str) -> Vec<(String, String)> {
    params.push(p("DryRun", flag));
    params
}

/// The rules `DescribeClientVpnAuthorizationRules` reports for `endpoint_id`, as the sorted
/// set of networks they cover and the sorted set of groups they name — enough to see a rule
/// added or taken off, whichever of the two it is.
async fn reported_rules(
    router: &MultiServiceRouter,
    case: &str,
    endpoint_id: &str,
) -> (Vec<String>, Vec<String>) {
    let (status, body) = call(router, DESCRIBE, &[p("ClientVpnEndpointId", endpoint_id)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {DESCRIBE}, got {status} with body {body}"
    );
    let mut networks = elements(&body, "destinationCidr");
    networks.sort();
    let mut groups = elements(&body, "groupId");
    groups.sort();
    (networks, groups)
}

/// Add the two rules every case starts from, over the wire, and answer the endpoint they are
/// on.
async fn seeded_endpoint(router: &MultiServiceRouter, engine: &engine::InMemoryEngine) -> String {
    let endpoint = seed_endpoint(engine);
    for (network, group) in [
        (SEEDED_ALL_GROUPS_NETWORK, None),
        (SEEDED_GROUP_NETWORK, Some(SEEDED_GROUP)),
    ] {
        let params = authorize_params(&endpoint, network, group);
        let (status, body) = call(router, AUTHORIZE, &params).await;
        assert_eq!(
            status, 200,
            "seeding {network}: expected HTTP 200 from {AUTHORIZE}, got {status} with body {body}"
        );
    }
    endpoint
}

/// The rules the seeding above leaves on the endpoint: what every dry run must leave behind.
fn seeded_rules() -> (Vec<String>, Vec<String>) {
    let mut networks = vec![
        SEEDED_ALL_GROUPS_NETWORK.to_string(),
        SEEDED_GROUP_NETWORK.to_string(),
    ];
    networks.sort();
    (networks, vec![SEEDED_GROUP.to_string()])
}

/// One dry-run row: what the rest of the request would otherwise have met, the action, and the
/// form fields it carries.
struct DryRunRow {
    case: &'static str,
    action: &'static str,
    params: Vec<(String, String)>,
}

/// Rule 1: the flag is answered the permitted verdict ahead of everything else the request
/// names, and the endpoint's rules are exactly the ones it already had.
#[tokio::test]
async fn a_dry_run_is_answered_the_permitted_verdict_and_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let endpoint = seeded_endpoint(&router, &engine).await;

    let rows = vec![
        DryRunRow {
            case: "an authorize that would be served, granted to every client",
            action: AUTHORIZE,
            params: authorize_params(&endpoint, "172.16.0.0/16", None),
        },
        DryRunRow {
            case: "an authorize that would be served, granted to one group",
            action: AUTHORIZE,
            params: authorize_params(&endpoint, "172.17.0.0/16", Some("finance")),
        },
        DryRunRow {
            case: "an authorize that would duplicate a rule the endpoint already has",
            action: AUTHORIZE,
            params: authorize_params(&endpoint, SEEDED_ALL_GROUPS_NETWORK, None),
        },
        DryRunRow {
            case: "an authorize naming an endpoint the account does not hold",
            action: AUTHORIZE,
            params: authorize_params(ABSENT_ENDPOINT, "172.18.0.0/16", None),
        },
        DryRunRow {
            // The target network sent EMPTY rather than left out: a required parameter the
            // request omits is refused at the request boundary, ahead of dispatch and so ahead
            // of the flag, while one sent empty reaches the handler — which is where the flag
            // is answered.
            case: "an authorize whose target network is empty",
            action: AUTHORIZE,
            params: vec![
                p("ClientVpnEndpointId", &endpoint),
                p("TargetNetworkCidr", ""),
                p("AuthorizeAllGroups", "true"),
            ],
        },
        DryRunRow {
            case: "an authorize whose target network is no CIDR block at all",
            action: AUTHORIZE,
            params: authorize_params(&endpoint, "10.0.0.0/33", None),
        },
        DryRunRow {
            case: "an authorize naming both every client and one group",
            action: AUTHORIZE,
            params: {
                let mut params = authorize_params(&endpoint, "172.19.0.0/16", Some("finance"));
                params.push(p("AuthorizeAllGroups", "true"));
                params
            },
        },
        DryRunRow {
            case: "an authorize naming nobody to grant the network to",
            action: AUTHORIZE,
            params: vec![
                p("ClientVpnEndpointId", &endpoint),
                p("TargetNetworkCidr", "172.20.0.0/16"),
            ],
        },
        DryRunRow {
            case: "a revoke that would be served, of the all-groups rule",
            action: REVOKE,
            params: revoke_params(&endpoint, SEEDED_ALL_GROUPS_NETWORK, None),
        },
        DryRunRow {
            case: "a revoke that would be served, of one group's rule",
            action: REVOKE,
            params: revoke_params(&endpoint, SEEDED_GROUP_NETWORK, Some(SEEDED_GROUP)),
        },
        DryRunRow {
            case: "a revoke of a rule the endpoint does not have",
            action: REVOKE,
            params: revoke_params(&endpoint, "10.9.0.0/16", None),
        },
        DryRunRow {
            case: "a revoke naming an endpoint the account does not hold",
            action: REVOKE,
            params: revoke_params(ABSENT_ENDPOINT, SEEDED_ALL_GROUPS_NETWORK, None),
        },
    ];

    for row in rows {
        let case = row.case;
        let (status, body) = call(&router, row.action, &with_dry_run(row.params, "true")).await;
        assert_eq!(
            status, DRY_RUN_OPERATION_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_OPERATION_STATUS} from {action}, got {status} \
             with body {body}",
            action = row.action
        );
        let code = error_code(&body);
        assert_eq!(
            code, DRY_RUN_OPERATION,
            "case {case:?}: expected the code {DRY_RUN_OPERATION:?}, got {code:?} in {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, DRY_RUN_OPERATION_MESSAGE,
            "case {case:?}: expected the message {DRY_RUN_OPERATION_MESSAGE:?}, got {message:?}"
        );

        let reported = reported_rules(&router, case, &endpoint).await;
        assert_eq!(
            reported,
            seeded_rules(),
            "case {case:?}: expected the endpoint's rules to be untouched, got {reported:?}"
        );
    }
}

/// Rule 2: the flag unset, and the flag set to `false`, are both ordinary calls — so the
/// authorize adds its rule and the revoke takes one off, each reported by the describe.
#[tokio::test]
async fn a_request_that_does_not_set_the_flag_is_performed() {
    // One row per spelling of "no dry run": the field left out, and the field carrying the
    // only value of a Smithy `Boolean` that is not the flag.
    for (case, flag) in [
        ("the DryRun field left out", None),
        ("DryRun=false", Some("false")),
    ] {
        let (router, engine) = super::ec2_query::emulator();
        let endpoint = seeded_endpoint(&router, &engine).await;
        let added = "172.21.0.0/16";

        let params = authorize_params(&endpoint, added, None);
        let params = match flag {
            None => params,
            Some(flag) => with_dry_run(params, flag),
        };
        let (status, body) = call(&router, AUTHORIZE, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {AUTHORIZE}, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "code").as_deref(),
            Some(CLIENT_VPN_AUTHORIZATION_RULE_STATUS_AUTHORIZING),
            "case {case:?}: the authorize answers the rule authorizing, got {body}"
        );
        let (networks, _) = reported_rules(&router, case, &endpoint).await;
        assert!(
            networks.iter().any(|network| network == added),
            "case {case:?}: expected {added} among the rules, got {networks:?}"
        );

        let params = revoke_params(&endpoint, added, None);
        let params = match flag {
            None => params,
            Some(flag) => with_dry_run(params, flag),
        };
        let (status, body) = call(&router, REVOKE, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {REVOKE}, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "code").as_deref(),
            Some(CLIENT_VPN_AUTHORIZATION_RULE_STATUS_REVOKING),
            "case {case:?}: the revoke answers the rule revoking, got {body}"
        );
        let reported = reported_rules(&router, case, &endpoint).await;
        assert_eq!(
            reported,
            seeded_rules(),
            "case {case:?}: expected only the rules the case started from, got {reported:?}"
        );
    }
}
