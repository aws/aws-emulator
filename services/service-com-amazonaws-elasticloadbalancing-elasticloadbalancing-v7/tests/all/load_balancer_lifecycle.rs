// SPDX-License-Identifier: CC0-1.0

//! The Classic load balancer (`AccessPoint`) lifecycle, end to end over the wire.
//!
//! Every assertion here goes through the awsQuery wire protocol on the multi-service
//! router — the same path `core/router` serves — so what is checked is the emulator's
//! HTTP behaviour, not a handler called directly.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! request:
//!
//! * `CreateLoadBalancer` persists a record keyed by `LoadBalancerName` and answers a
//!   `DNSName` — the address this emulator answers that load balancer's traffic on — that
//!   the record keeps for its lifetime, while `CanonicalHostedZoneName` carries the name
//!   Elastic Load Balancing's own zone gives it
//!   (`<name>-<id>.<region>.elb.amazonaws.com`, `internal-`-prefixed for the `internal`
//!   scheme).
//! * every later operation computes its answer from that stored record: the describes
//!   project it, register/deregister mutate it and answer the resulting set, and delete
//!   removes it.

use super::wire::{call, emulator, kid, kid_text, member_texts, members, param, parse, Element};

/// The `InstanceId` of each member of an `Instances` list.
fn instance_ids(node: &Element) -> Vec<String> {
    members(node, "Instances")
        .into_iter()
        .filter_map(|member| kid_text(member, "InstanceId"))
        .collect()
}

fn sorted(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values
}

/// One listener of a case's create request.
#[derive(Debug, Clone)]
struct ListenerSpec {
    protocol: &'static str,
    load_balancer_port: i32,
    /// `None` exercises the documented default: the front-end protocol.
    instance_protocol: Option<&'static str>,
    instance_port: i32,
    ssl_certificate_id: Option<&'static str>,
}

/// One row of the lifecycle table.
#[derive(Debug, Clone)]
struct Case {
    name: &'static str,
    region: &'static str,
    scheme: Option<&'static str>,
    listeners: Vec<ListenerSpec>,
    availability_zones: Vec<&'static str>,
    subnets: Vec<&'static str>,
    security_groups: Vec<&'static str>,
    tags: Vec<(&'static str, &'static str)>,
    /// Registered in one call; a repeated id proves the stored set is deduplicated.
    register: Vec<&'static str>,
    deregister: Vec<&'static str>,
}

impl Case {
    fn create_params(&self) -> Vec<(String, String)> {
        let mut params = vec![("LoadBalancerName".to_string(), self.name.to_string())];
        if let Some(scheme) = self.scheme {
            params.push(("Scheme".to_string(), scheme.to_string()));
        }
        for (index, listener) in self.listeners.iter().enumerate() {
            let slot = index + 1;
            params.push((
                format!("Listeners.member.{slot}.Protocol"),
                listener.protocol.to_string(),
            ));
            params.push((
                format!("Listeners.member.{slot}.LoadBalancerPort"),
                listener.load_balancer_port.to_string(),
            ));
            params.push((
                format!("Listeners.member.{slot}.InstancePort"),
                listener.instance_port.to_string(),
            ));
            if let Some(instance_protocol) = listener.instance_protocol {
                params.push((
                    format!("Listeners.member.{slot}.InstanceProtocol"),
                    instance_protocol.to_string(),
                ));
            }
            if let Some(certificate) = listener.ssl_certificate_id {
                params.push((
                    format!("Listeners.member.{slot}.SSLCertificateId"),
                    certificate.to_string(),
                ));
            }
        }
        for (index, zone) in self.availability_zones.iter().enumerate() {
            params.push((
                format!("AvailabilityZones.member.{}", index + 1),
                (*zone).to_string(),
            ));
        }
        for (index, subnet) in self.subnets.iter().enumerate() {
            params.push((format!("Subnets.member.{}", index + 1), (*subnet).to_string()));
        }
        for (index, group) in self.security_groups.iter().enumerate() {
            params.push((
                format!("SecurityGroups.member.{}", index + 1),
                (*group).to_string(),
            ));
        }
        for (index, (key, value)) in self.tags.iter().enumerate() {
            let slot = index + 1;
            params.push((format!("Tags.member.{slot}.Key"), (*key).to_string()));
            params.push((format!("Tags.member.{slot}.Value"), (*value).to_string()));
        }
        params
    }

    /// The instance ids the register call leaves registered, deduplicated.
    fn registered(&self) -> Vec<String> {
        let mut unique: Vec<String> = Vec::new();
        for id in &self.register {
            if !unique.iter().any(|held| held == id) {
                unique.push((*id).to_string());
            }
        }
        unique
    }

    fn remaining(&self) -> Vec<String> {
        self.registered()
            .into_iter()
            .filter(|id| !self.deregister.contains(&id.as_str()))
            .collect()
    }

    /// The `InstanceProtocol` a listener ends up with: the request's, else the
    /// front-end `Protocol` (the model's documented default).
    fn instance_protocol(listener: &ListenerSpec) -> String {
        listener
            .instance_protocol
            .unwrap_or(listener.protocol)
            .to_string()
    }
}

/// The published Route 53 hosted zone ids of the Classic Elastic Load Balancing
/// endpoint, from the AWS General Reference's "Elastic Load Balancing endpoints and
/// quotas" page. Deliberately includes regions the issue did not cite.
const PUBLISHED_HOSTED_ZONE_IDS: &[(&str, &str)] = &[
    ("us-east-1", "Z35SXDOTRQ7X7K"),
    ("eu-west-1", "Z32O12XQLNTSW2"),
    ("ap-southeast-2", "Z1GM3OXH4ZPM65"),
];

/// The published `DNSName` is the address a CLIENT dials, and the name Elastic Load
/// Balancing's own zone would give a load balancer resolves to nothing on the machine this
/// emulator runs on — so what is published is the `host:port` of the socket this emulator
/// answers that load balancer's traffic on. The zone name is what
/// `CanonicalHostedZoneName` carries ([`assert_hosted_zone_name`]), and the whole published
/// form is pinned in `classic_data_plane.rs`.
fn assert_dns_name(case: &Case, dns_name: &str) {
    assert!(
        dns_name.parse::<std::net::SocketAddr>().is_ok(),
        "case {:?}: expected an address a client can dial, got {dns_name:?}",
        case.name
    );
    assert!(
        !dns_name.contains("elb.amazonaws.com"),
        "case {:?}: the published address must not be a host in AWS's own zone, got \
         {dns_name:?}",
        case.name
    );
}

/// `CanonicalHostedZoneName` is "the DNS name of the load balancer" as the Route 53 hosted
/// zone a client points an alias at holds it: `<name>-<id>.<region>.elb.amazonaws.com`,
/// prefixed `internal-` for the `internal` scheme.
fn assert_hosted_zone_name(case: &Case, hosted_zone_name: &str) {
    let prefix = if case.scheme == Some("internal") {
        format!("internal-{}-", case.name)
    } else {
        format!("{}-", case.name)
    };
    let suffix = format!(".{}.elb.amazonaws.com", case.region);
    assert!(
        hosted_zone_name.starts_with(&prefix) && hosted_zone_name.ends_with(&suffix),
        "case {:?}: expected a hosted zone name '{prefix}<id>{suffix}', got \
         {hosted_zone_name:?}",
        case.name
    );
    let id = &hosted_zone_name[prefix.len()..hosted_zone_name.len() - suffix.len()];
    assert!(
        id.len() == 10 && id.bytes().all(|byte| byte.is_ascii_digit()),
        "case {:?}: expected a 10-digit id in the hosted zone name, got {id:?}",
        case.name
    );
}

fn assert_hosted_zone_id(case: &Case, zone_id: &str) {
    if let Some((_, published)) = PUBLISHED_HOSTED_ZONE_IDS
        .iter()
        .find(|(region, _)| *region == case.region)
    {
        assert_eq!(
            zone_id, *published,
            "case {:?}: expected the published hosted zone id of {}, got {zone_id:?}",
            case.name, case.region
        );
    } else {
        assert!(
            zone_id.len() == 14
                && zone_id.starts_with('Z')
                && zone_id[1..]
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit()),
            "case {:?}: expected a well-formed hosted zone id for the uncovered region {}, \
             got {zone_id:?}",
            case.name,
            case.region
        );
    }
}

/// The lifecycle table: rows differ in region, scheme, listener count and protocol
/// family, placement (zones vs subnets), security groups, tags, and how many instances
/// are registered and then deregistered.
fn lifecycle_cases() -> Vec<Case> {
    vec![
        Case {
            name: "web-elb",
            region: "us-east-1",
            scheme: None,
            listeners: vec![ListenerSpec {
                protocol: "HTTP",
                load_balancer_port: 80,
                instance_protocol: Some("HTTP"),
                instance_port: 8080,
                ssl_certificate_id: None,
            }],
            availability_zones: vec!["us-east-1a"],
            subnets: vec![],
            security_groups: vec![],
            tags: vec![],
            register: vec!["i-1111111111111111", "i-2222222222222222"],
            deregister: vec!["i-1111111111111111"],
        },
        Case {
            name: "internal-two-listener-elb",
            region: "eu-west-1",
            scheme: Some("internal"),
            listeners: vec![
                ListenerSpec {
                    protocol: "HTTPS",
                    load_balancer_port: 443,
                    instance_protocol: None,
                    instance_port: 8443,
                    ssl_certificate_id: Some(
                        "arn:aws:iam::000000000000:server-certificate/example",
                    ),
                },
                ListenerSpec {
                    protocol: "HTTP",
                    load_balancer_port: 8080,
                    instance_protocol: Some("HTTP"),
                    instance_port: 8081,
                    ssl_certificate_id: None,
                },
            ],
            availability_zones: vec![],
            subnets: vec!["subnet-aaaa1111", "subnet-bbbb2222"],
            security_groups: vec!["sg-11112222", "sg-33334444"],
            tags: vec![("team", "platform"), ("env", "staging")],
            register: vec!["i-3333333333333333"],
            deregister: vec!["i-3333333333333333"],
        },
        Case {
            name: "tcp-elb",
            region: "ap-southeast-2",
            scheme: Some("internet-facing"),
            listeners: vec![
                ListenerSpec {
                    protocol: "TCP",
                    load_balancer_port: 25,
                    instance_protocol: None,
                    instance_port: 2525,
                    ssl_certificate_id: None,
                },
                ListenerSpec {
                    protocol: "SSL",
                    load_balancer_port: 465,
                    instance_protocol: Some("TCP"),
                    instance_port: 2526,
                    ssl_certificate_id: Some(
                        "arn:aws:iam::000000000000:server-certificate/smtp",
                    ),
                },
                ListenerSpec {
                    protocol: "TCP",
                    load_balancer_port: 1024,
                    instance_protocol: Some("TCP"),
                    instance_port: 65535,
                    ssl_certificate_id: None,
                },
            ],
            availability_zones: vec!["ap-southeast-2a", "ap-southeast-2b"],
            // Both placement members at once: `Subnets` is documented as one subnet per
            // Availability Zone named in `AvailabilityZones`, so this is a valid VPC
            // request and both lists must be stored and reported as given.
            subnets: vec!["subnet-cccc3333"],
            security_groups: vec!["sg-55556666"],
            tags: vec![("only", "one")],
            // The repeated id proves the stored set is deduplicated.
            register: vec![
                "i-4444444444444444",
                "i-5555555555555555",
                "i-4444444444444444",
                "i-6666666666666666",
            ],
            deregister: vec!["i-5555555555555555", "i-6666666666666666"],
        },
        Case {
            // A region the published hosted-zone table does not cover.
            name: "gov-elb",
            region: "us-gov-west-1",
            scheme: Some("internal"),
            listeners: vec![ListenerSpec {
                protocol: "HTTP",
                load_balancer_port: 1,
                instance_protocol: None,
                instance_port: 1,
                ssl_certificate_id: None,
            }],
            availability_zones: vec!["us-gov-west-1a"],
            subnets: vec![],
            security_groups: vec![],
            tags: vec![],
            register: vec![],
            deregister: vec![],
        },
    ]
}

#[tokio::test]
async fn a_load_balancer_is_created_used_read_back_and_deleted() {
    let router = emulator();

    for case in lifecycle_cases() {
        // CREATE — answers the generated DNS name and persists the record.
        let created = call(
            &router,
            case.region,
            "CreateLoadBalancer",
            &case.create_params(),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreateLoadBalancer must succeed, got {}",
            case.name, created.body
        );
        let created_doc = parse(&created.body);
        let dns_name = kid_text(created.result(&created_doc), "DNSName")
            .unwrap_or_else(|| panic!("case {:?}: no DNSName in {}", case.name, created.body));
        assert_dns_name(&case, &dns_name);

        // READ BACK — every reported member comes from the stored record.
        let described = call(
            &router,
            case.region,
            "DescribeLoadBalancers",
            &[("LoadBalancerNames.member.1".to_string(), case.name.to_string())],
        )
        .await;
        let described_doc = parse(&described.body);
        let result = described.result(&described_doc);
        let descriptions = members(result, "LoadBalancerDescriptions");
        assert_eq!(
            descriptions.len(),
            1,
            "case {:?}: expected exactly one description, got {}",
            case.name,
            described.body
        );
        let description = descriptions[0];

        assert_eq!(
            kid_text(description, "LoadBalancerName").as_deref(),
            Some(case.name),
            "case {:?}: wrong LoadBalancerName in {}",
            case.name,
            described.body
        );
        assert_eq!(
            kid_text(description, "DNSName").as_deref(),
            Some(dns_name.as_str()),
            "case {:?}: the DNSName must be the one create answered, got {}",
            case.name,
            described.body
        );
        assert_hosted_zone_name(
            &case,
            &kid_text(description, "CanonicalHostedZoneName").unwrap_or_default(),
        );
        assert_hosted_zone_id(
            &case,
            &kid_text(description, "CanonicalHostedZoneNameID").unwrap_or_default(),
        );
        assert_eq!(
            kid_text(description, "Scheme").as_deref(),
            Some(case.scheme.unwrap_or("internet-facing")),
            "case {:?}: wrong Scheme in {}",
            case.name,
            described.body
        );
        let created_time = kid_text(description, "CreatedTime").unwrap_or_default();
        assert!(
            created_time.len() >= 4 && created_time[..4].bytes().all(|b| b.is_ascii_digit()),
            "case {:?}: expected an ISO-8601 CreatedTime, got {created_time:?}",
            case.name
        );
        assert_eq!(
            member_texts(description, "AvailabilityZones"),
            case.availability_zones,
            "case {:?}: wrong AvailabilityZones in {}",
            case.name,
            described.body
        );
        assert_eq!(
            member_texts(description, "Subnets"),
            case.subnets,
            "case {:?}: wrong Subnets in {}",
            case.name,
            described.body
        );
        assert_eq!(
            member_texts(description, "SecurityGroups"),
            case.security_groups,
            "case {:?}: wrong SecurityGroups in {}",
            case.name,
            described.body
        );

        // The listeners exactly as created, each wrapped with its PolicyNames.
        let listener_descriptions = members(description, "ListenerDescriptions");
        assert_eq!(
            listener_descriptions.len(),
            case.listeners.len(),
            "case {:?}: wrong listener count in {}",
            case.name,
            described.body
        );
        for (reported, expected) in listener_descriptions.iter().zip(&case.listeners) {
            let listener = kid(reported, "Listener").unwrap_or_else(|| {
                panic!("case {:?}: no Listener in {}", case.name, described.body)
            });
            assert_eq!(
                kid_text(listener, "Protocol").as_deref(),
                Some(expected.protocol),
                "case {:?}: wrong listener Protocol in {}",
                case.name,
                described.body
            );
            assert_eq!(
                kid_text(listener, "LoadBalancerPort"),
                Some(expected.load_balancer_port.to_string()),
                "case {:?}: wrong LoadBalancerPort in {}",
                case.name,
                described.body
            );
            assert_eq!(
                kid_text(listener, "InstancePort"),
                Some(expected.instance_port.to_string()),
                "case {:?}: wrong InstancePort in {}",
                case.name,
                described.body
            );
            assert_eq!(
                kid_text(listener, "InstanceProtocol"),
                Some(Case::instance_protocol(expected)),
                "case {:?}: InstanceProtocol must default to the front-end protocol, got {}",
                case.name,
                described.body
            );
            assert_eq!(
                kid_text(listener, "SSLCertificateId").as_deref(),
                expected.ssl_certificate_id,
                "case {:?}: wrong SSLCertificateId in {}",
                case.name,
                described.body
            );
            assert!(
                member_texts(reported, "PolicyNames").is_empty(),
                "case {:?}: a newly created listener carries no policies, got {}",
                case.name,
                described.body
            );
        }

        // The default health check: a TCP check against the first listener's instance
        // port, with the Classic Load Balancer default thresholds.
        let health_check = kid(description, "HealthCheck").unwrap_or_else(|| {
            panic!("case {:?}: no HealthCheck in {}", case.name, described.body)
        });
        assert_eq!(
            kid_text(health_check, "Target"),
            Some(format!("TCP:{}", case.listeners[0].instance_port)),
            "case {:?}: wrong default HealthCheck Target in {}",
            case.name,
            described.body
        );
        for (member, expected) in [
            ("Interval", "30"),
            ("Timeout", "5"),
            ("UnhealthyThreshold", "2"),
            ("HealthyThreshold", "10"),
        ] {
            assert_eq!(
                kid_text(health_check, member).as_deref(),
                Some(expected),
                "case {:?}: wrong default HealthCheck {member} in {}",
                case.name,
                described.body
            );
        }

        // REGISTER — answers the FULL current set, read back from state.
        if !case.register.is_empty() {
            let mut params = Vec::new();
            params.push(("LoadBalancerName".to_string(), case.name.to_string()));
            for (index, instance) in case.register.iter().enumerate() {
                params.push((
                    format!("Instances.member.{}.InstanceId", index + 1),
                    (*instance).to_string(),
                ));
            }
            let registered = call(
                &router,
                case.region,
                "RegisterInstancesWithLoadBalancer",
                &params,
            )
            .await;
            let registered_doc = parse(&registered.body);
            assert_eq!(
                sorted(instance_ids(registered.result(&registered_doc))),
                sorted(case.registered()),
                "case {:?}: register must answer the full deduplicated set, got {}",
                case.name,
                registered.body
            );

            // Registering an already-registered instance changes nothing.
            let again = call(
                &router,
                case.region,
                "RegisterInstancesWithLoadBalancer",
                &[
                    ("LoadBalancerName".to_string(), case.name.to_string()),
                    (
                        "Instances.member.1.InstanceId".to_string(),
                        case.register[0].to_string(),
                    ),
                ],
            )
            .await;
            let again_doc = parse(&again.body);
            assert_eq!(
                sorted(instance_ids(again.result(&again_doc))),
                sorted(case.registered()),
                "case {:?}: re-registering must not duplicate an instance, got {}",
                case.name,
                again.body
            );
        }

        // INSTANCE HEALTH — one entry per registered instance, derived from state.
        let health = call(
            &router,
            case.region,
            "DescribeInstanceHealth",
            &[("LoadBalancerName".to_string(), case.name.to_string())],
        )
        .await;
        let health_doc = parse(&health.body);
        let states = members(health.result(&health_doc), "InstanceStates");
        assert_eq!(
            sorted(
                states
                    .iter()
                    .filter_map(|state| kid_text(state, "InstanceId"))
                    .collect()
            ),
            sorted(case.registered()),
            "case {:?}: instance health must cover exactly the registered set, got {}",
            case.name,
            health.body
        );
        for state in &states {
            assert_eq!(
                (
                    kid_text(state, "State"),
                    kid_text(state, "ReasonCode"),
                    kid_text(state, "Description"),
                ),
                (
                    Some("OutOfService".to_string()),
                    Some("ELB".to_string()),
                    Some("Instance registration is still in progress.".to_string()),
                ),
                "case {:?}: a just-registered instance reports its registration in progress, got {}",
                case.name,
                health.body
            );
        }

        // An instance that is NOT registered is reported, not omitted.
        let stranger = call(
            &router,
            case.region,
            "DescribeInstanceHealth",
            &[
                ("LoadBalancerName".to_string(), case.name.to_string()),
                (
                    "Instances.member.1.InstanceId".to_string(),
                    "i-0000000000000000".to_string(),
                ),
            ],
        )
        .await;
        let stranger_doc = parse(&stranger.body);
        let stranger_states = members(stranger.result(&stranger_doc), "InstanceStates");
        assert_eq!(
            stranger_states.len(),
            1,
            "case {:?}: an unregistered instance must be reported, got {}",
            case.name,
            stranger.body
        );
        assert_eq!(
            (
                kid_text(stranger_states[0], "State"),
                kid_text(stranger_states[0], "ReasonCode"),
                kid_text(stranger_states[0], "Description"),
            ),
            (
                Some("OutOfService".to_string()),
                Some("ELB".to_string()),
                Some("Instance is not currently registered with the LoadBalancer.".to_string()),
            ),
            "case {:?}: wrong unregistered-instance state in {}",
            case.name,
            stranger.body
        );

        // DEREGISTER — answers what is still registered, and the describes agree.
        if !case.deregister.is_empty() {
            let mut params = vec![("LoadBalancerName".to_string(), case.name.to_string())];
            for (index, instance) in case.deregister.iter().enumerate() {
                params.push((
                    format!("Instances.member.{}.InstanceId", index + 1),
                    (*instance).to_string(),
                ));
            }
            let deregistered = call(
                &router,
                case.region,
                "DeregisterInstancesFromLoadBalancer",
                &params,
            )
            .await;
            let deregistered_doc = parse(&deregistered.body);
            assert_eq!(
                sorted(instance_ids(deregistered.result(&deregistered_doc))),
                sorted(case.remaining()),
                "case {:?}: deregister must answer the remaining set, got {}",
                case.name,
                deregistered.body
            );
        }

        let after = call(
            &router,
            case.region,
            "DescribeLoadBalancers",
            &[("LoadBalancerNames.member.1".to_string(), case.name.to_string())],
        )
        .await;
        let after_doc = parse(&after.body);
        let after_description = members(after.result(&after_doc), "LoadBalancerDescriptions")[0];
        assert_eq!(
            sorted(instance_ids(after_description)),
            sorted(case.remaining()),
            "case {:?}: describe must list only the instances still registered, got {}",
            case.name,
            after.body
        );
        let after_health = call(
            &router,
            case.region,
            "DescribeInstanceHealth",
            &[("LoadBalancerName".to_string(), case.name.to_string())],
        )
        .await;
        let after_health_doc = parse(&after_health.body);
        assert_eq!(
            sorted(
                members(after_health.result(&after_health_doc), "InstanceStates")
                    .iter()
                    .filter_map(|state| kid_text(state, "InstanceId"))
                    .collect()
            ),
            sorted(case.remaining()),
            "case {:?}: instance health must follow the deregistration, got {}",
            case.name,
            after_health.body
        );

        // DELETE — removes the record, and is idempotent.
        for attempt in 1..=2 {
            let deleted = call(
                &router,
                case.region,
                "DeleteLoadBalancer",
                &[("LoadBalancerName".to_string(), case.name.to_string())],
            )
            .await;
            assert_eq!(
                deleted.status, 200,
                "case {:?}: DeleteLoadBalancer attempt {attempt} must succeed, got {}",
                case.name, deleted.body
            );
        }
        let gone = call(
            &router,
            case.region,
            "DescribeLoadBalancers",
            &[("LoadBalancerNames.member.1".to_string(), case.name.to_string())],
        )
        .await;
        assert_eq!(
            (gone.status, gone.error_code().as_str()),
            (400, "LoadBalancerNotFound"),
            "case {:?}: describing a deleted load balancer must not be found, got {}",
            case.name,
            gone.body
        );
    }

    // Nothing the table created survives, in any of the regions it used.
    for case in lifecycle_cases() {
        let all = call(&router, case.region, "DescribeLoadBalancers", &[]).await;
        let all_doc = parse(&all.body);
        assert!(
            members(all.result(&all_doc), "LoadBalancerDescriptions").is_empty(),
            "every load balancer in {} was deleted, so none may be listed, got {}",
            case.region,
            all.body
        );
    }
}

#[tokio::test]
async fn describe_load_balancers_pages_over_the_stored_order() {
    let router = emulator();

    // Created out of order, so the page order can only come from the store.
    let names = ["elb-04", "elb-01", "elb-07", "elb-02", "elb-06", "elb-03", "elb-05"];
    for name in names {
        let created = call(
            &router,
            "us-east-1",
            "CreateLoadBalancer",
            &[
                ("LoadBalancerName".to_string(), name.to_string()),
                ("Listeners.member.1.Protocol".to_string(), "HTTP".to_string()),
                (
                    "Listeners.member.1.LoadBalancerPort".to_string(),
                    "80".to_string(),
                ),
                ("Listeners.member.1.InstancePort".to_string(), "80".to_string()),
                (
                    "AvailabilityZones.member.1".to_string(),
                    "us-east-1a".to_string(),
                ),
            ],
        )
        .await;
        assert_eq!(created.status, 200, "creating {name} must succeed: {}", created.body);
    }
    let expected = sorted(names.iter().map(|name| (*name).to_string()).collect());

    // Each page size walks the same sequence exactly once.
    for page_size in [1usize, 2, 3, 7, 400] {
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut params = vec![("PageSize".to_string(), page_size.to_string())];
            if let Some(marker) = &marker {
                params.push(("Marker".to_string(), marker.clone()));
            }
            let page = call(&router, "us-east-1", "DescribeLoadBalancers", &params).await;
            let page_doc = parse(&page.body);
            let result = page.result(&page_doc);
            let batch: Vec<String> = members(result, "LoadBalancerDescriptions")
                .iter()
                .filter_map(|description| kid_text(description, "LoadBalancerName"))
                .collect();
            assert!(
                batch.len() <= page_size,
                "page size {page_size}: a page may not exceed it, got {batch:?}"
            );
            seen.extend(batch);
            marker = kid_text(result, "NextMarker");
            pages += 1;
            assert!(
                pages <= names.len() + 1,
                "page size {page_size}: pagination did not terminate, saw {seen:?}"
            );
            if marker.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "page size {page_size}: the pages must concatenate to the full stored order"
        );
    }

    // A `Marker` resumes AT the name it carries.
    let resumed = call(
        &router,
        "us-east-1",
        "DescribeLoadBalancers",
        &[
            ("Marker".to_string(), "elb-05".to_string()),
            ("PageSize".to_string(), "400".to_string()),
        ],
    )
    .await;
    let resumed_doc = parse(&resumed.body);
    assert_eq!(
        members(resumed.result(&resumed_doc), "LoadBalancerDescriptions")
            .iter()
            .filter_map(|description| kid_text(description, "LoadBalancerName"))
            .collect::<Vec<_>>(),
        vec!["elb-05".to_string(), "elb-06".to_string(), "elb-07".to_string()],
        "a Marker resumes at the name it carries, got {}",
        resumed.body
    );

    // One unknown name in the filter is not found, even beside known ones.
    let unknown = call(
        &router,
        "us-east-1",
        "DescribeLoadBalancers",
        &[
            ("LoadBalancerNames.member.1".to_string(), "elb-01".to_string()),
            (
                "LoadBalancerNames.member.2".to_string(),
                "no-such-elb".to_string(),
            ),
        ],
    )
    .await;
    assert_eq!(
        (unknown.status, unknown.error_code().as_str()),
        (400, "LoadBalancerNotFound"),
        "an unknown name in LoadBalancerNames must not be found, got {}",
        unknown.body
    );
}

/// The published quotas bound what one caller can make the store hold.
#[tokio::test]
async fn published_quotas_bound_what_a_caller_can_store() {
    /// A minimal, valid create body for `name`.
    fn minimal_create(name: &str) -> Vec<(String, String)> {
        vec![
            param("LoadBalancerName", name),
            param("Listeners.member.1.Protocol", "HTTP"),
            param("Listeners.member.1.LoadBalancerPort", "80"),
            param("Listeners.member.1.InstancePort", "80"),
            param("AvailabilityZones.member.1", "us-east-1a"),
        ]
    }

    // Load balancers per region: the 21st create is refused.
    let router = emulator();
    for index in 1..=20 {
        let created = call(
            &router,
            "us-east-1",
            "CreateLoadBalancer",
            &minimal_create(&format!("quota-elb-{index:02}")),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "load balancer {index} is within the quota: {}",
            created.body
        );
    }
    let over = call(
        &router,
        "us-east-1",
        "CreateLoadBalancer",
        &minimal_create("quota-elb-21"),
    )
    .await;
    assert_eq!(
        (over.status, over.error_code().as_str()),
        (400, "TooManyLoadBalancers"),
        "the 21st load balancer must be refused, got {}",
        over.body
    );

    // Listeners per load balancer: 100 is accepted, 101 is refused.
    for (count, expected_code) in [(100usize, None), (101, Some("ValidationError"))] {
        let router = emulator();
        let mut params = vec![
            param("LoadBalancerName", "listener-quota-elb"),
            param("AvailabilityZones.member.1", "us-east-1a"),
        ];
        for slot in 1..=count {
            params.push((
                format!("Listeners.member.{slot}.Protocol"),
                "HTTP".to_string(),
            ));
            params.push((
                format!("Listeners.member.{slot}.LoadBalancerPort"),
                slot.to_string(),
            ));
            params.push((
                format!("Listeners.member.{slot}.InstancePort"),
                (8000 + slot).to_string(),
            ));
        }
        let answer = call(&router, "us-east-1", "CreateLoadBalancer", &params).await;
        match expected_code {
            None => assert_eq!(
                answer.status, 200,
                "{count} listeners is within the quota: {}",
                answer.body
            ),
            Some(code) => assert_eq!(
                (answer.status, answer.error_code().as_str()),
                (400, code),
                "{count} listeners must be refused, got {}",
                answer.body
            ),
        }
    }

    // Tags per load balancer: 10 is accepted, 11 is refused with the modelled error.
    for (count, expected_code) in [(10usize, None), (11, Some("TooManyTags"))] {
        let router = emulator();
        let mut params = minimal_create("tag-quota-elb");
        for slot in 1..=count {
            params.push((format!("Tags.member.{slot}.Key"), format!("key-{slot}")));
            params.push((format!("Tags.member.{slot}.Value"), slot.to_string()));
        }
        let answer = call(&router, "us-east-1", "CreateLoadBalancer", &params).await;
        match expected_code {
            None => assert_eq!(
                answer.status, 200,
                "{count} tags is within the quota: {}",
                answer.body
            ),
            Some(code) => assert_eq!(
                (answer.status, answer.error_code().as_str()),
                (400, code),
                "{count} tags must be refused, got {}",
                answer.body
            ),
        }
    }

    // Security groups per load balancer: 5 is accepted, 6 is refused.
    for (count, expected_code) in [(5usize, None), (6, Some("ValidationError"))] {
        let router = emulator();
        let mut params = minimal_create("sg-quota-elb");
        for slot in 1..=count {
            params.push((
                format!("SecurityGroups.member.{slot}"),
                format!("sg-{slot:08}"),
            ));
        }
        let answer = call(&router, "us-east-1", "CreateLoadBalancer", &params).await;
        match expected_code {
            None => assert_eq!(
                answer.status, 200,
                "{count} security groups is within the bound: {}",
                answer.body
            ),
            Some(code) => assert_eq!(
                (answer.status, answer.error_code().as_str()),
                (400, code),
                "{count} security groups must be refused, got {}",
                answer.body
            ),
        }
    }

    // Placements (Availability Zones or subnets): 50 is accepted, 51 is refused.
    for member in ["AvailabilityZones", "Subnets"] {
        for (count, expected_code) in [(50usize, None), (51, Some("ValidationError"))] {
            let router = emulator();
            let mut params = vec![
                param("LoadBalancerName", "placement-quota-elb"),
                param("Listeners.member.1.Protocol", "HTTP"),
                param("Listeners.member.1.LoadBalancerPort", "80"),
                param("Listeners.member.1.InstancePort", "80"),
            ];
            for slot in 1..=count {
                params.push((format!("{member}.member.{slot}"), format!("place-{slot:03}")));
            }
            let answer = call(&router, "us-east-1", "CreateLoadBalancer", &params).await;
            match expected_code {
                None => assert_eq!(
                    answer.status, 200,
                    "{count} {member} entries is within the bound: {}",
                    answer.body
                ),
                Some(code) => assert_eq!(
                    (answer.status, answer.error_code().as_str()),
                    (400, code),
                    "{count} {member} entries must be refused, got {}",
                    answer.body
                ),
            }
        }
    }

    // Registered instances per load balancer: a batch over the quota is refused, and so
    // is one that would push the stored set past it.
    let router = emulator();
    let created = call(
        &router,
        "us-east-1",
        "CreateLoadBalancer",
        &minimal_create("instance-quota-elb"),
    )
    .await;
    assert_eq!(created.status, 200, "the fixture must be created: {}", created.body);

    let register_params = |count: usize, offset: usize| {
        let mut params = vec![param("LoadBalancerName", "instance-quota-elb")];
        for slot in 1..=count {
            params.push((
                format!("Instances.member.{slot}.InstanceId"),
                format!("i-{:016}", offset + slot),
            ));
        }
        params
    };

    let over_batch = call(
        &router,
        "us-east-1",
        "RegisterInstancesWithLoadBalancer",
        &register_params(1001, 0),
    )
    .await;
    assert_eq!(
        (over_batch.status, over_batch.error_code().as_str()),
        (400, "ValidationError"),
        "a batch of 1001 instances must be refused, got {}",
        over_batch.error_code()
    );

    let full = call(
        &router,
        "us-east-1",
        "RegisterInstancesWithLoadBalancer",
        &register_params(1000, 0),
    )
    .await;
    assert_eq!(
        full.status, 200,
        "1000 registered instances is within the quota: {}",
        full.error_code()
    );
    let one_too_many = call(
        &router,
        "us-east-1",
        "RegisterInstancesWithLoadBalancer",
        &register_params(1, 1000),
    )
    .await;
    assert_eq!(
        (one_too_many.status, one_too_many.error_code().as_str()),
        (400, "ValidationError"),
        "the 1001st registered instance must be refused, got {}",
        one_too_many.error_code()
    );
    // `LoadBalancerNames` is deduplicated before the bound applies, so a list that
    // repeats an existing name is ACCEPTED however long it is (AWS accepts it too) and
    // still describes that load balancer once; a list naming more DISTINCT load
    // balancers than exist is answered by the modelled not-found error, not by the
    // emulator's backstop.
    let router = emulator();
    let created = call(
        &router,
        "us-east-1",
        "CreateLoadBalancer",
        &minimal_create("names-bound-elb"),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the fixture must be created: {}",
        created.body
    );
    for count in [1usize, 20, 21, 50] {
        let mut params = Vec::new();
        for slot in 1..=count {
            params.push((
                format!("LoadBalancerNames.member.{slot}"),
                "names-bound-elb".to_string(),
            ));
        }
        let answer = call(&router, "us-east-1", "DescribeLoadBalancers", &params).await;
        let doc = parse(&answer.body);
        let described = members(answer.result(&doc), "LoadBalancerDescriptions");
        assert_eq!(
            (answer.status, described.len()),
            (200, 1),
            "{count} repetitions of one existing name must describe it once, got {}",
            answer.body
        );
    }
    let mut distinct = vec![(
        "LoadBalancerNames.member.1".to_string(),
        "names-bound-elb".to_string(),
    )];
    for slot in 2..=21 {
        distinct.push((
            format!("LoadBalancerNames.member.{slot}"),
            format!("absent-elb-{slot:02}"),
        ));
    }
    let unknown = call(&router, "us-east-1", "DescribeLoadBalancers", &distinct).await;
    assert_eq!(
        (unknown.status, unknown.error_code().as_str()),
        (400, "LoadBalancerNotFound"),
        "a name that does not exist must raise the modelled error, got {}",
        unknown.body
    );

    // Regions per store: the region comes from the caller's own credential scope, so the
    // number of per-region budgets it can open is capped at 64.
    let router = emulator();
    for index in 1..=64 {
        let created = call(
            &router,
            // Shaped like a published region name, so it is taken as one.
            &format!("zz-east-{index}"),
            "CreateLoadBalancer",
            &minimal_create("region-quota-elb"),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "region {index} is within the bound: {}",
            created.body
        );
    }
    let over_regions = call(
        &router,
        "zz-west-1",
        "CreateLoadBalancer",
        &minimal_create("region-quota-elb"),
    )
    .await;
    assert_eq!(
        (over_regions.status, over_regions.error_code().as_str()),
        (400, "ValidationException"),
        "the 65th region must be refused, got {}",
        over_regions.body
    );
}

/// A credential scope whose region field is not shaped like an AWS region name does not
/// open a store partition of its own: the request is attributed to the default region.
#[tokio::test]
async fn a_scope_region_that_names_no_region_is_attributed_to_the_default_region() {
    let router = emulator();
    // Each of these reaches the server as the `Credential=.../<region>/...` field, and
    // none of them is shaped like a published AWS region name.
    for bogus in ["NOT_A_REGION", "us", "us-east", "us-east-1-extra-extra", "....", "1-2-3"] {
        let created = call(
            &router,
            bogus,
            "CreateLoadBalancer",
            &[
                param("LoadBalancerName", "defaulted-elb"),
                param("Listeners.member.1.Protocol", "HTTP"),
                param("Listeners.member.1.LoadBalancerPort", "80"),
                param("Listeners.member.1.InstancePort", "80"),
                param("AvailabilityZones.member.1", "us-east-1a"),
            ],
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {bogus:?}: the create itself is valid: {}",
            created.body
        );
        let doc = parse(&created.body);
        let dns_name = kid_text(created.result(&doc), "DNSName").unwrap_or_default();
        assert!(
            dns_name.parse::<std::net::SocketAddr>().is_ok(),
            "case {bogus:?}: the published DNSName is the address the emulator answers this \
             load balancer on, got {dns_name:?}"
        );

        // It landed in the default region's partition, and nowhere else.
        let described = call(&router, "us-east-1", "DescribeLoadBalancers", &[]).await;
        let described_doc = parse(&described.body);
        let descriptions = members(described.result(&described_doc), "LoadBalancerDescriptions");
        assert_eq!(
            descriptions.len(),
            1,
            "case {bogus:?}: the record must be in the default region, got {}",
            described.body
        );
        // And the region it was attributed to is the one its hosted-zone name carries —
        // "the DNS name of the load balancer" as Route 53's hosted zone holds it, which is
        // where the region names a load balancer's own record.
        let hosted_zone_name = descriptions
            .first()
            .and_then(|description| kid_text(description, "CanonicalHostedZoneName"))
            .unwrap_or_default();
        assert!(
            hosted_zone_name.ends_with(".us-east-1.elb.amazonaws.com"),
            "case {bogus:?}: expected the default region in the hosted zone name, got \
             {hosted_zone_name:?}"
        );

        let deleted = call(
            &router,
            bogus,
            "DeleteLoadBalancer",
            &[param("LoadBalancerName", "defaulted-elb")],
        )
        .await;
        assert_eq!(
            deleted.status, 200,
            "case {bogus:?}: delete must succeed: {}",
            deleted.body
        );
    }
}

/// A load balancer is a regional resource: one name names a different load balancer in
/// each region, and a call sees only the records of the region it is signed for.
#[tokio::test]
async fn load_balancers_are_scoped_to_the_region_they_were_created_in() {
    let router = emulator();
    let shared_name = "shared-name-elb";
    let regions = ["us-east-1", "eu-west-1"];

    let mut dns_names: Vec<String> = Vec::new();
    for region in regions {
        let created = call(
            &router,
            region,
            "CreateLoadBalancer",
            &[
                param("LoadBalancerName", shared_name),
                param("Listeners.member.1.Protocol", "HTTP"),
                param("Listeners.member.1.LoadBalancerPort", "80"),
                param("Listeners.member.1.InstancePort", "80"),
                param("AvailabilityZones.member.1", &format!("{region}a")),
            ],
        )
        .await;
        assert_eq!(
            created.status, 200,
            "one name may be used in each region, so {region} must succeed: {}",
            created.body
        );
        let doc = parse(&created.body);
        dns_names.push(kid_text(created.result(&doc), "DNSName").unwrap_or_default());
    }
    assert_ne!(
        dns_names[0], dns_names[1],
        "each region's load balancer has its own DNS name, got {dns_names:?}"
    );

    // A describe sees only its own region's records.
    for (index, region) in regions.iter().enumerate() {
        let described = call(&router, region, "DescribeLoadBalancers", &[]).await;
        let doc = parse(&described.body);
        let descriptions = members(described.result(&doc), "LoadBalancerDescriptions");
        assert_eq!(
            descriptions.len(),
            1,
            "{region} holds exactly one load balancer, got {}",
            described.body
        );
        assert_eq!(
            kid_text(descriptions[0], "DNSName").as_deref(),
            Some(dns_names[index].as_str()),
            "{region} must report its own record, got {}",
            described.body
        );
    }

    // A region that holds no record of that name does not find it.
    let elsewhere = call(
        &router,
        "ap-southeast-2",
        "DescribeLoadBalancers",
        &[param("LoadBalancerNames.member.1", shared_name)],
    )
    .await;
    assert_eq!(
        (elsewhere.status, elsewhere.error_code().as_str()),
        (400, "LoadBalancerNotFound"),
        "a region holding no such name must not find it, got {}",
        elsewhere.body
    );

    // Registering in one region does not touch the other's record.
    let registered = call(
        &router,
        regions[0],
        "RegisterInstancesWithLoadBalancer",
        &[
            param("LoadBalancerName", shared_name),
            param("Instances.member.1.InstanceId", "i-7777777777777777"),
        ],
    )
    .await;
    assert_eq!(registered.status, 200, "register must succeed: {}", registered.body);
    let other_health = call(
        &router,
        regions[1],
        "DescribeInstanceHealth",
        &[param("LoadBalancerName", shared_name)],
    )
    .await;
    let other_health_doc = parse(&other_health.body);
    assert!(
        members(other_health.result(&other_health_doc), "InstanceStates").is_empty(),
        "registering in {} must not register in {}, got {}",
        regions[0],
        regions[1],
        other_health.body
    );

    // Deleting in one region leaves the other's record in place.
    let deleted = call(
        &router,
        regions[0],
        "DeleteLoadBalancer",
        &[param("LoadBalancerName", shared_name)],
    )
    .await;
    assert_eq!(deleted.status, 200, "delete must succeed: {}", deleted.body);
    let survivor = call(
        &router,
        regions[1],
        "DescribeLoadBalancers",
        &[param("LoadBalancerNames.member.1", shared_name)],
    )
    .await;
    let survivor_doc = parse(&survivor.body);
    assert_eq!(
        members(survivor.result(&survivor_doc), "LoadBalancerDescriptions").len(),
        1,
        "deleting in {} must not delete in {}, got {}",
        regions[0],
        regions[1],
        survivor.body
    );
    let gone = call(
        &router,
        regions[0],
        "DescribeLoadBalancers",
        &[param("LoadBalancerNames.member.1", shared_name)],
    )
    .await;
    assert_eq!(
        (gone.status, gone.error_code().as_str()),
        (400, "LoadBalancerNotFound"),
        "the deleted record must be gone from its own region, got {}",
        gone.body
    );
}

/// One refusal row: the request, and the status and awsQuery error code it must answer.
struct Refusal {
    what: &'static str,
    action: &'static str,
    params: Vec<(String, String)>,
    status: u16,
    code: &'static str,
}

#[tokio::test]
async fn invalid_requests_are_refused_with_the_modelled_error() {
    let router = emulator();

    // One existing load balancer, so the duplicate-name row has something to collide
    // with.
    let created = call(
        &router,
        "us-east-1",
        "CreateLoadBalancer",
        &[
            param("LoadBalancerName", "taken-elb"),
            param("Listeners.member.1.Protocol", "HTTP"),
            param("Listeners.member.1.LoadBalancerPort", "80"),
            param("Listeners.member.1.InstancePort", "80"),
            param("AvailabilityZones.member.1", "us-east-1a"),
        ],
    )
    .await;
    assert_eq!(created.status, 200, "the fixture must be created: {}", created.body);

    let refusals = vec![
        Refusal {
            what: "a second create with a name that already exists",
            action: "CreateLoadBalancer",
            params: vec![
                param("LoadBalancerName", "taken-elb"),
                param("Listeners.member.1.Protocol", "HTTP"),
                param("Listeners.member.1.LoadBalancerPort", "80"),
                param("Listeners.member.1.InstancePort", "80"),
                param("AvailabilityZones.member.1", "us-east-1a"),
            ],
            status: 400,
            code: "DuplicateLoadBalancerName",
        },
        Refusal {
            what: "a scheme outside the two documented values",
            action: "CreateLoadBalancer",
            params: vec![
                param("LoadBalancerName", "bad-scheme-elb"),
                param("Scheme", "external-facing"),
                param("Listeners.member.1.Protocol", "HTTP"),
                param("Listeners.member.1.LoadBalancerPort", "80"),
                param("Listeners.member.1.InstancePort", "80"),
                param("AvailabilityZones.member.1", "us-east-1a"),
            ],
            status: 400,
            code: "InvalidScheme",
        },
        Refusal {
            what: "a listener protocol the service does not route",
            action: "CreateLoadBalancer",
            params: vec![
                param("LoadBalancerName", "udp-elb"),
                param("Listeners.member.1.Protocol", "UDP"),
                param("Listeners.member.1.LoadBalancerPort", "53"),
                param("Listeners.member.1.InstancePort", "53"),
                param("AvailabilityZones.member.1", "us-east-1a"),
            ],
            status: 400,
            code: "ValidationError",
        },
        Refusal {
            what: "an HTTP front end with a TCP back end",
            action: "CreateLoadBalancer",
            params: vec![
                param("LoadBalancerName", "mixed-elb"),
                param("Listeners.member.1.Protocol", "HTTP"),
                param("Listeners.member.1.LoadBalancerPort", "80"),
                param("Listeners.member.1.InstanceProtocol", "TCP"),
                param("Listeners.member.1.InstancePort", "80"),
                param("AvailabilityZones.member.1", "us-east-1a"),
            ],
            status: 409,
            code: "InvalidConfigurationRequest",
        },
        Refusal {
            what: "two listeners claiming one load balancer port",
            action: "CreateLoadBalancer",
            params: vec![
                param("LoadBalancerName", "double-port-elb"),
                param("Listeners.member.1.Protocol", "HTTP"),
                param("Listeners.member.1.LoadBalancerPort", "80"),
                param("Listeners.member.1.InstancePort", "80"),
                param("Listeners.member.2.Protocol", "HTTP"),
                param("Listeners.member.2.LoadBalancerPort", "80"),
                param("Listeners.member.2.InstancePort", "8080"),
                param("AvailabilityZones.member.1", "us-east-1a"),
            ],
            status: 409,
            code: "InvalidConfigurationRequest",
        },
        Refusal {
            what: "one tag key given twice",
            action: "CreateLoadBalancer",
            params: vec![
                param("LoadBalancerName", "dup-tag-elb"),
                param("Listeners.member.1.Protocol", "HTTP"),
                param("Listeners.member.1.LoadBalancerPort", "80"),
                param("Listeners.member.1.InstancePort", "80"),
                param("AvailabilityZones.member.1", "us-east-1a"),
                param("Tags.member.1.Key", "env"),
                param("Tags.member.1.Value", "a"),
                param("Tags.member.2.Key", "env"),
                param("Tags.member.2.Value", "b"),
            ],
            status: 400,
            code: "DuplicateTagKeys",
        },
        Refusal {
            what: "a create that places the load balancer nowhere",
            action: "CreateLoadBalancer",
            params: vec![
                param("LoadBalancerName", "nowhere-elb"),
                param("Listeners.member.1.Protocol", "HTTP"),
                param("Listeners.member.1.LoadBalancerPort", "80"),
                param("Listeners.member.1.InstancePort", "80"),
            ],
            status: 400,
            code: "ValidationError",
        },
        Refusal {
            what: "registering with a load balancer that does not exist",
            action: "RegisterInstancesWithLoadBalancer",
            params: vec![
                param("LoadBalancerName", "no-such-elb"),
                param("Instances.member.1.InstanceId", "i-9999999999999999"),
            ],
            status: 400,
            code: "LoadBalancerNotFound",
        },
        Refusal {
            what: "deregistering from a load balancer that does not exist",
            action: "DeregisterInstancesFromLoadBalancer",
            params: vec![
                param("LoadBalancerName", "no-such-elb"),
                param("Instances.member.1.InstanceId", "i-9999999999999999"),
            ],
            status: 400,
            code: "LoadBalancerNotFound",
        },
        Refusal {
            what: "instance health of a load balancer that does not exist",
            action: "DescribeInstanceHealth",
            params: vec![param("LoadBalancerName", "no-such-elb")],
            status: 400,
            code: "LoadBalancerNotFound",
        },
        Refusal {
            what: "an instance carrying no instance id",
            action: "RegisterInstancesWithLoadBalancer",
            params: vec![
                param("LoadBalancerName", "taken-elb"),
                param("Instances.member.1", ""),
            ],
            status: 400,
            code: "InvalidInstance",
        },
    ];

    for refusal in &refusals {
        let answer = call(&router, "us-east-1", refusal.action, &refusal.params).await;
        assert_eq!(
            (answer.status, answer.error_code().as_str()),
            (refusal.status, refusal.code),
            "case {:?}: expected HTTP {} {}, got {}",
            refusal.what,
            refusal.status,
            refusal.code,
            answer.body
        );
    }

    // A refused create persisted nothing: only the fixture exists.
    let all = call(&router, "us-east-1", "DescribeLoadBalancers", &[]).await;
    let all_doc = parse(&all.body);
    assert_eq!(
        members(all.result(&all_doc), "LoadBalancerDescriptions")
            .iter()
            .filter_map(|description| kid_text(description, "LoadBalancerName"))
            .collect::<Vec<_>>(),
        vec!["taken-elb".to_string()],
        "a refused create must not persist a record, got {}",
        all.body
    );

    // The fixture's registered set is also untouched by the refused register calls.
    let health = call(
        &router,
        "us-east-1",
        "DescribeInstanceHealth",
        &[param("LoadBalancerName", "taken-elb")],
    )
    .await;
    let health_doc = parse(&health.body);
    assert!(
        members(health.result(&health_doc), "InstanceStates").is_empty(),
        "no instance was registered, so none may be reported, got {}",
        health.body
    );
}
