// SPDX-License-Identifier: CC0-1.0

//! Which load balancers carry a `Scheme`, over the wire.
//!
//! One rule: the FAMILY decides whether a load balancer has a scheme at all. A family the
//! member is documented for carries one from the moment it exists — the value the request
//! named, else the one its documentation fixes as the default ("the default is an
//! Internet-facing load balancer") — and the family it is documented AGAINST ("you can't
//! specify a scheme for a Gateway Load Balancer", `CreateLoadBalancerInput$Scheme`) carries
//! none: nothing is defaulted in for it, a read reports no `Scheme` member at all, and a
//! create that names one is refused the modelled `InvalidConfigurationRequest` and stores
//! nothing.
//!
//! Why the member's ABSENCE is the assertion: `LoadBalancer$Scheme` is optional, so an absent
//! member is how the service says this load balancer has no scheme — a client that branches on
//! `Scheme` to tell internet-facing from internal load balancers reads a value for every load
//! balancer that has one and nothing for the one that cannot.
//!
//! The rows span what the rule generalises over: every family the model's
//! `LoadBalancerTypeEnum` names, every value its `LoadBalancerSchemeEnum` names, and a request
//! that spells no scheme out at all — crossed against the two outcomes, the scheme reported
//! and the create refused. Every assertion is made on what a later `DescribeLoadBalancers`
//! reports as well as on what the create answered, so what is under test is the stored record
//! rather than one echo.
//!
//! The wire harness (`emulator`, `call`, `action`, `p`) comes from `provisioning_loop` and the
//! message reader from `target_group_attributes_and_modification`; nothing here restates them.

use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::ops::types::LOAD_BALANCER_SCHEME_ENUM_VALUES;
use smithy_server_runtime::MultiServiceRouter;

use crate::provisioning_loop::{action, call, emulator, p, Reply};
use crate::target_group_attributes_and_modification::unescaped;

/// The awsQuery code a configuration no load balancer can hold is refused with
/// (`InvalidConfigurationRequestException`), and the status it is answered under.
const INVALID_CONFIGURATION: &str = "InvalidConfigurationRequest";
const REFUSED_STATUS: u16 = 400;

/// The code a read of a load balancer that is not there is refused with, which is what says a
/// refused create stored nothing.
const LOAD_BALANCER_NOT_FOUND: &str = "LoadBalancerNotFound";

/// The value `Scheme`'s own documentation fixes as its default: "The default is an
/// Internet-facing load balancer." Written out here rather than read from the crate, so the
/// VALUE the service reports is under test and not only its propagation.
const DOCUMENTED_DEFAULT: &str = "internet-facing";

/// What a row expects of the load balancer it asks for.
enum Expected {
    /// The create is served and a read reports this scheme.
    Carries(&'static str),
    /// The create is served and a read reports NO `Scheme` member.
    CarriesNone,
    /// The create is refused and stores nothing.
    Refused,
}

/// One row: the case, the `Type` the request names, the `Scheme` it names (`None` where it
/// spells none out) and what must then hold.
struct Row {
    case: &'static str,
    lb_type: &'static str,
    scheme: Option<&'static str>,
    expected: Expected,
}

const ROWS: &[Row] = &[
    Row {
        case: "an application load balancer naming no scheme",
        lb_type: "application",
        scheme: None,
        expected: Expected::Carries(DOCUMENTED_DEFAULT),
    },
    Row {
        case: "an internet-facing application load balancer",
        lb_type: "application",
        scheme: Some("internet-facing"),
        expected: Expected::Carries("internet-facing"),
    },
    Row {
        case: "an internal application load balancer",
        lb_type: "application",
        scheme: Some("internal"),
        expected: Expected::Carries("internal"),
    },
    Row {
        case: "a network load balancer naming no scheme",
        lb_type: "network",
        scheme: None,
        expected: Expected::Carries(DOCUMENTED_DEFAULT),
    },
    Row {
        case: "an internal network load balancer",
        lb_type: "network",
        scheme: Some("internal"),
        expected: Expected::Carries("internal"),
    },
    Row {
        case: "a gateway load balancer naming no scheme",
        lb_type: "gateway",
        scheme: None,
        expected: Expected::CarriesNone,
    },
    Row {
        case: "a gateway load balancer naming the internal scheme",
        lb_type: "gateway",
        scheme: Some("internal"),
        expected: Expected::Refused,
    },
    Row {
        case: "a gateway load balancer naming the internet-facing scheme",
        lb_type: "gateway",
        scheme: Some("internet-facing"),
        expected: Expected::Refused,
    },
];

/// The sentence a create naming a scheme for a family that carries none is refused with: what
/// cannot be specified, and the type of load balancer it was asked for.
fn unsupported_scheme(lb_type: &str) -> String {
    format!("You cannot specify a scheme with a load balancer of type '{lb_type}'")
}

/// Ask `CreateLoadBalancer` for the load balancer `row` describes, under `name`.
///
/// Two subnets, which every family can be placed in — an Application Load Balancer needs at
/// least two — so the placement is never what a row turns on.
async fn create(router: &MultiServiceRouter, name: &str, row: &Row) -> Reply {
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", name));
    form.push(p("Type", row.lb_type));
    if let Some(scheme) = row.scheme {
        form.push(p("Scheme", scheme));
    }
    form.push(p("Subnets.member.1", "subnet-11111111"));
    form.push(p("Subnets.member.2", "subnet-22222222"));
    call(router, &form).await
}

/// What `DescribeLoadBalancers` answers for the load balancer called `name`.
async fn describe(router: &MultiServiceRouter, name: &str) -> Reply {
    let mut form = action("DescribeLoadBalancers");
    form.push(p("Names.member.1", name));
    call(router, &form).await
}

#[tokio::test]
async fn a_load_balancer_carries_a_scheme_only_where_its_family_has_one() {
    let (router, _engine) = emulator();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let name = format!("scheme-family-{index}");
        let created = create(&router, &name, row).await;

        match row.expected {
            Expected::Refused => {
                assert_eq!(
                    (created.status, created.error_code().as_str()),
                    (REFUSED_STATUS, INVALID_CONFIGURATION),
                    "case {case:?}: expected {REFUSED_STATUS} {INVALID_CONFIGURATION}, got {} \
                     body {}",
                    created.status,
                    created.body
                );
                let stated = created
                    .text("Message")
                    .map(|text| unescaped(&text))
                    .unwrap_or_else(|| panic!("case {case:?}: no message in {}", created.body));
                assert_eq!(
                    stated,
                    unsupported_scheme(row.lb_type),
                    "case {case:?}: expected {:?}, got {stated:?}",
                    unsupported_scheme(row.lb_type)
                );
                // A refused create stores nothing, so the name it asked for answers no read.
                let read = describe(&router, &name).await;
                assert_eq!(
                    (read.status, read.error_code().as_str()),
                    (REFUSED_STATUS, LOAD_BALANCER_NOT_FOUND),
                    "case {case:?}: a refused create must store no load balancer, got {} body {}",
                    read.status,
                    read.body
                );
                continue;
            }
            Expected::Carries(_) | Expected::CarriesNone => assert_eq!(
                created.status, 200,
                "case {case:?}: expected the create to be served, got {} body {}",
                created.status, created.body
            ),
        }

        let expected_scheme = match row.expected {
            Expected::Carries(scheme) => Some(scheme),
            _ => None,
        };
        // The create's own answer and a later read agree on the scheme the load balancer
        // carries — and on the ABSENCE of the member for the family that carries none.
        let read = describe(&router, &name).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the stored load balancer to be reported, got {} body {}",
            read.status, read.body
        );
        for (source, reply) in [("the create", &created), ("a later read", &read)] {
            assert_eq!(
                reply.text("Scheme").as_deref(),
                expected_scheme,
                "case {case:?}: {source} must report {expected_scheme:?} for Scheme, got body {}",
                reply.body
            );
            assert_eq!(
                reply.text("Type").as_deref(),
                Some(row.lb_type),
                "case {case:?}: {source} must report the family asked for, got body {}",
                reply.body
            );
        }
    }
}

/// Every value the model's own `LoadBalancerSchemeEnum` catalogue names is refused on the
/// family that carries no scheme — the rule is about the MEMBER being named at all, not about
/// which of its values was named — and each refusal leaves the name free for the gateway load
/// balancer that names none.
#[tokio::test]
async fn no_scheme_the_model_names_is_accepted_on_a_gateway_load_balancer() {
    let (router, _engine) = emulator();
    let name = "scheme-gateway-free";

    for scheme in LOAD_BALANCER_SCHEME_ENUM_VALUES {
        let row = Row {
            case: "a gateway load balancer naming a scheme of the model's catalogue",
            lb_type: "gateway",
            scheme: Some(scheme),
            expected: Expected::Refused,
        };
        let refused = create(&router, name, &row).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (REFUSED_STATUS, INVALID_CONFIGURATION),
            "case scheme {scheme:?}: expected {REFUSED_STATUS} {INVALID_CONFIGURATION}, got {} \
             body {}",
            refused.status,
            refused.body
        );
    }

    let row = Row {
        case: "a gateway load balancer naming no scheme",
        lb_type: "gateway",
        scheme: None,
        expected: Expected::CarriesNone,
    };
    let created = create(&router, name, &row).await;
    assert_eq!(
        created.status, 200,
        "case after the refusals: expected the create to be served, got {} body {}",
        created.status, created.body
    );
    let read = describe(&router, name).await;
    assert_eq!(
        (read.status, read.text("Scheme")),
        (200, None),
        "case after the refusals: the stored gateway load balancer must report no Scheme, got \
         body {}",
        read.body
    );
}
