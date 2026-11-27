// SPDX-License-Identifier: CC0-1.0

//! Elastic Load Balancing's published default-quota catalog.

use super::*;

/// The `AWS/Usage` display name Elastic Load Balancing publishes its resource counts
/// under, which is the `Service` dimension of every metered load-balancing quota.
const ELB_USAGE_SERVICE: &str = "Elastic Load Balancing";

/// The `AWS/Usage` resource count of one plane of this service, under the series
/// CloudWatch publishes that plane's count as.
///
/// The per-Region planes of the service go through [`resource_count_plane`], which also
/// fills in the window those counts are published over; a plane of one balancer, target
/// group, rule or trust store carries the metric alone, because the service publishes no
/// window for it.
fn elb_plane_metric(resource: &str) -> MetricInfo {
    usage_metric(
        ELB_USAGE_SERVICE,
        &[
            ("Class", "None"),
            ("Resource", resource),
            ("Type", "Resource"),
        ],
    )
}

/// The series CloudWatch publishes one load-balancing plane's resource count under, built
/// from the plane's own published name.
///
/// SOURCE: the recorded default-quota listing of `elasticloadbalancing`, whose metered
/// rows carry the quota's own name with the spaces taken out as the `Resource` dimension
/// (`ListenersPerApplicationLoadBalancer` for `Listeners per Application Load Balancer`,
/// `TargetGroupsPerApplicationLoadBalancer` for `Target Groups per Application Load
/// Balancer`). That is the same form [`super::super::descriptive_quota_code`] builds a name
/// into, so the series goes through it rather than through a second copy of the transform.
/// A plane CloudWatch publishes under a series of another name states that series at the
/// call site instead.
fn plane_series(quota_name: &str) -> String {
    crate::state::descriptive_quota_code(quota_name)
}

/// The sentence Elastic Load Balancing publishes for one of its planes: the plane's own
/// phrase, WITHOUT a closing full stop — which is the form [`maximum_number_phrase`]
/// builds, so this service's rows are worded through that one shared builder.
///
/// SOURCE: the recorded default-quota listing of `elasticloadbalancing`, whose rule and
/// target-group planes of an Application Load Balancer read `The maximum number of rules
/// per Application Load Balancer` and `The maximum number of target groups per Application
/// Load Balancer` — no trailing stop, and no clause beyond the plane itself. It is the form
/// of the whole catalog, so every row of this service carries it. It is this service's own
/// hook for that form: a row of another wording states it for itself at the call site.
fn elb_description(counted: &str) -> String {
    maximum_number_phrase(counted)
}

/// Elastic Load Balancing's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `elasticloadbalancing` service
/// code (the "Quotas for your Application Load Balancers", "…Network Load Balancers",
/// "…Gateway Load Balancers" and "…Classic Load Balancers" pages of the Elastic Load
/// Balancing User Guide), and the recorded default-quota listing of that code for
/// which `QuotaCode` names which quota, for each code's adjustability and for the
/// usage metric each metered row carries. Elastic Load Balancing publishes its planes
/// PER LOAD BALANCER TYPE — a type is what a listener, rule or target allowance is
/// bought under — so a row is named for the type it is counted on, and the per-Region
/// planes of each type are the ones CloudWatch publishes an `AWS/Usage` resource count
/// over a window for. A plane counted inside one balancer, target group, rule or trust
/// store publishes its count too ([`elb_plane_metric`]), but no window.
///
/// The recorded listing publishes the listener plane of an Application Load Balancer under
/// `L-B6DF7632`, so that is the code it is published under here, and the CERTIFICATE planes
/// of an Application and of a Network Load Balancer under `L-9365A611` and `L-52964454`.
/// `L-37E1617B` is the revocation ENTRIES of a trust store — the entries its lists carry
/// together, a different quota from the lists themselves, which that listing publishes
/// under `L-332EE142` — and `L-8A66D0E6` is the Region's RESERVED Application Load Balancer
/// capacity, published with nothing reserved. It publishes the listener plane of a CLASSIC
/// Load Balancer under `L-1A491844` and the per-Availability Zone target plane of a Network
/// Load Balancer under `L-B211E961`. The two Gateway Load Balancer planes the certificate
/// bindings displaced, the per-Region Network Load Balancer plane the Classic listener
/// binding displaced and the per-Region target group plane the revocation-list binding
/// displaced take the codes those bindings vacated, each with that code's published
/// adjustability; the per-Availability Zone subnet plane of a load balancer and the
/// condition-wildcard plane of a rule name no other code the listing states and are left to
/// the default-quota-set gap — a code identifies ONE quota (see
/// [`super::descriptive_quota_code`]).
///
/// Adjustability is published PER CODE and is not a property of the plane: the recorded
/// listing reports the trust-store, certificate, condition, security-group, tag and
/// reserved-capacity planes as adjustable, so each row carries the adjustability the
/// listing reports for its own code rather than the one its constructor defaults to.
pub(in crate::state) fn elasticloadbalancing_quotas() -> Vec<QuotaRow> {
    // One per-Region plane of a load-balancer type, metered like every other counted
    // plane ([`resource_count_plane`]) and worded the way this service words a plane of
    // the account's Region.
    let metered = |code: &str, name: &str, value: f64, resource: &str, counted: &str| {
        resource_count_plane(code, name, value, ELB_USAGE_SERVICE, resource, counted).describe(
            &elb_description(&format!("{counted} in this account in the current Region")),
        )
    };
    // One plane counted INSIDE a single balancer, target group, rule or trust store: the
    // plane's count, under the series its own name names, and no window. `counted` is the
    // plane's phrase as the service's sentence reads it.
    let plane = |code: &str, name: &str, value: f64, counted: &str| {
        quota(name, value)
            .code(code)
            .metric_without_period(elb_plane_metric(&plane_series(name)))
            .describe(&elb_description(counted))
    };
    // One plane the service publishes no count for: the limit alone, in the same voice.
    let unmetered = |code: &str, name: &str, value: f64, counted: &str| {
        quota(name, value)
            .code(code)
            .describe(&elb_description(counted))
    };
    vec![
        metered(
            "L-057ECE54",
            "Application Load Balancers per Region",
            50.0,
            "ApplicationLoadBalancersPerRegion",
            "Application Load Balancers",
        ),
        metered(
            "L-0BB7B635",
            "Classic Load Balancers per Region",
            20.0,
            "ClassicLoadBalancersPerRegion",
            "Classic Load Balancers",
        ),
        // The listener plane of a Classic Load Balancer, published under the code the
        // recorded listing states for it, with that plane's own resource count; the
        // per-Region Network Load Balancer plane this catalog had served here takes the
        // code this binding vacated.
        plane(
            "L-1A491844",
            "Listeners per Classic Load Balancer",
            100.0,
            "listeners per Classic Load Balancer",
        ),
        // The network-interface plane of a VPC: the ENIs every Network Load Balancer of
        // the VPC takes together, metered by CloudWatch like the per-Region planes. The
        // service words this one as a TOTAL rather than a maximum, and spells the
        // interfaces the way its own catalog carries them.
        metered(
            "L-23568085",
            "Network Load Balancer ENIs per VPC",
            1200.0,
            "NetworkLoadBalancersENIsPerVPC",
            "Network Load Balancer ENIs per VPC",
        )
        .describe("The total number of Network Load Balancer eni's per VPC"),
        // The revocation-LIST plane of a trust store, published under the code the recorded
        // listing states for it: the lists a trust store may hold, which the service words
        // with the lists capitalised the way its own quota name does. The per-Region target
        // group plane this catalog had served here takes the code this binding vacated.
        unmetered(
            "L-332EE142",
            "Revocation lists per trust store",
            30.0,
            "Revocation lists per trust store",
        ),
        // The revocation-ENTRY plane of a trust store — the entries the lists it holds
        // carry together, which is a different quota from the lists themselves — published
        // under the code the recorded listing states for it and as adjustable. The entries
        // are capitalised in the sentence the way the quota's own name capitalises them.
        unmetered(
            "L-37E1617B",
            "Revocation entries per trust store",
            500_000.0,
            "Revocation entries per trust store",
        ),
        unmetered(
            "L-43FE5A42",
            "Listeners per Network Load Balancer",
            50.0,
            "listeners per Network Load Balancer",
        ),
        // The certificate plane of a Network Load Balancer, published under the code the
        // recorded listing states for it, with that plane's own resource count.
        plane(
            "L-52964454",
            "Certificates per Network Load Balancer",
            25.0,
            "certificates per Network Load Balancer",
        ),
        // The per-Region Network Load Balancer plane the Classic listener binding above
        // displaced: it takes the code that binding vacated.
        metered(
            "L-53DA6B97",
            "Network Load Balancers per Region",
            50.0,
            "NetworkLoadBalancersPerRegion",
            "Network Load Balancers",
        ),
        metered(
            "L-57A373D6",
            "Gateway Load Balancers per Region",
            100.0,
            "GatewayLoadBalancersPerRegion",
            "Gateway Load Balancers",
        ),
        // The two Gateway Load Balancer planes the certificate bindings above displaced:
        // each takes a code that binding vacated, keeping the adjustability and the
        // resource count the recorded listing reports for THAT code.
        plane(
            "L-69A177A2",
            "Listeners per Gateway Load Balancer",
            1.0,
            "listeners per Gateway Load Balancer",
        ),
        plane(
            "L-723DCCB6",
            "Targets per Availability Zone per Gateway Load Balancer",
            300.0,
            "targets per Availability Zone per Gateway Load Balancer",
        ),
        unmetered(
            "L-7A15E3C5",
            "Targets per Network Load Balancer",
            3000.0,
            "targets per Network Load Balancer",
        ),
        unmetered(
            "L-7C45F98A",
            "Target groups per action per Application Load Balancer",
            5.0,
            "target groups per action per Application Load Balancer",
        ),
        plane(
            "L-7E6692B2",
            "Targets per Application Load Balancer",
            1000.0,
            "targets per Application Load Balancer",
        ),
        // The routing-rule plane of an Application Load Balancer, which CloudWatch
        // publishes under a series of its own name rather than the plane's.
        quota("Rules per Application Load Balancer", 100.0)
            .code("L-7EED9B64")
            .metric_without_period(elb_plane_metric("RoutingRulesPerApplicationLoadBalancer"))
            .describe(&elb_description("rules per Application Load Balancer")),
        plane(
            "L-822D1B1B",
            "Target Groups per Application Load Balancer",
            100.0,
            "target groups per Application Load Balancer",
        )
        .fixed(),
        // The RESERVED capacity plane of the Region: the Application Load Balancer
        // capacity units held for the account rather than the balancers themselves, which
        // AWS publishes with nothing reserved. The units are named in the sentence the way
        // the quota's own name capitalises them. CloudWatch carries its count under a
        // series that spells the units out rather than under the initialism the quota's own
        // name parenthesises, so the series is named here.
        quota(
            "Reserved Application Load Balancer Capacity Units (LCU) per Region",
            0.0,
        )
        .code("L-8A66D0E6")
        .metric_without_period(elb_plane_metric(
            "ReservedApplicationLoadBalancerCapacityUnitsPerRegion",
        ))
        .describe(&elb_description(
            "Reserved Application Load Balancer Capacity Units (LCU) per Region",
        )),
        // The certificate plane of an Application Load Balancer, published under the code
        // the recorded listing states for it, with that plane's own resource count.
        plane(
            "L-9365A611",
            "Certificates per Application Load Balancer",
            25.0,
            "certificates per Application Load Balancer",
        ),
        unmetered(
            "L-9E6F9012",
            "Targets per target group per Application Load Balancer",
            1000.0,
            "targets per target group per Application Load Balancer",
        ),
        plane(
            "L-A012D4E8",
            "Targets per target group per Network Load Balancer",
            1000.0,
            "targets per target group per Network Load Balancer",
        ),
        plane(
            "L-A0D0B863",
            "Registered instances per Classic Load Balancer",
            1000.0,
            "instances registered with one Classic Load Balancer",
        ),
        unmetered("L-A8F7060B", "Condition values per rule", 5.0, "condition values per rule"),
        // The per-Availability Zone target plane of a Network Load Balancer, published
        // under the code the recorded listing states for it, with that plane's own resource
        // count. The condition-wildcard plane of a rule this catalog had served here names
        // no other code that listing states, so it is left to the default-quota-set gap.
        plane(
            "L-B211E961",
            "Targets per Availability Zone per Network Load Balancer",
            500.0,
            "targets per Availability Zone per Network Load Balancer",
        ),
        plane(
            "L-B22855CB",
            "Load balancers per target group",
            1.0,
            "load balancers one target group can be used by",
        ),
        plane(
            "L-B6DF7632",
            "Listeners per Application Load Balancer",
            50.0,
            "listeners per Application Load Balancer",
        ),
        plane(
            "L-CE3125E5",
            "Trust stores per Region",
            10.0,
            "trust stores in this account in the current Region",
        ),
        plane(
            "L-E9E9831D",
            "Certificate authority certificates per trust store",
            25.0,
            "certificate authority certificates per trust store",
        ),
        // The per-Region target group plane the revocation-list binding above displaced: it
        // takes the code that binding vacated, with that code's published adjustability.
        metered(
            "L-EBC0C08B",
            "Target groups per Region",
            3000.0,
            "TargetGroupsPerRegion",
            "target groups",
        )
        .fixed(),
        plane(
            "L-EEF1AD04",
            "Security groups per load balancer",
            5.0,
            "security groups per load balancer",
        ),
        unmetered("L-F31E307C", "Tags per load balancer", 50.0, "tags per load balancer"),
    ]
}
