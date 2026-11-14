// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeVpcEndpointServices`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=DescribeVpcEndpointServices`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). The Region in the SigV4 `Credential=` scope is the
//! Region the service answers for, so every call here carries one.
//!
//! The rule under test: the operation reports the endpoint services a caller can create a
//! VPC endpoint to in the Regions the request asks about — the services Amazon Web Services
//! offers there, which are facts of the Region and therefore derived rather than stored,
//! together with the account's OWN stored service configurations hosted in those Regions —
//! narrowed by `ServiceNames` and the documented filters and cut into `MaxResults`-sized
//! pages, with `ServiceNames` and `ServiceDetails` reporting the same selection.
//!
//! Every table row is a distinct input along what that rule generalises over: which Region
//! the call asks about, which service (interface-only, gateway-only, both), which
//! documented filter and how many values, how many names, which source the service came
//! from, and which page size. No single request is the specification.

use std::collections::BTreeSet;

use ec2::ops::types::{PrivateDnsNameConfiguration, ServiceConfiguration, ServiceTypeDetail, Tag};
use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{error_code, error_message, filters, indexed};
use super::wire::{Param, child, child_text, document, param, set_items};

/// The Region most calls here are signed for.
const REGION: &str = "us-east-1";

/// The action every call below drives.
const ACTION: &str = "DescribeVpcEndpointServices";

/// One `DescribeVpcEndpointServices` call signed for `region`, as its status and body.
async fn describe_in(
    router: &MultiServiceRouter,
    region: &str,
    params: &[Param],
) -> (u16, String) {
    super::ec2_query::call(router, region, ACTION, params).await
}

/// One call signed for [`REGION`].
async fn describe(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    describe_in(router, REGION, params).await
}

/// One call signed for `region` that has to succeed, and the body it answered.
async fn ok_in(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    params: &[Param],
) -> String {
    let (status, body) = describe_in(router, region, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    body
}

/// One successful call signed for [`REGION`].
async fn ok(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    ok_in(case, router, REGION, params).await
}

/// One endpoint service as a response reports it: every member of `ServiceDetail`, read
/// through the XML parser so a nested `privateDnsNameSet/privateDnsName` is never confused
/// with the service's own `privateDnsName`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    name: String,
    id: String,
    types: Vec<String>,
    region: String,
    owner: String,
    zones: Vec<String>,
    zone_ids: Vec<String>,
    base_dns_names: Vec<String>,
    private_dns_name: Option<String>,
    private_dns_names: Vec<String>,
    policy_supported: Option<String>,
    acceptance_required: Option<String>,
    manages_endpoints: Option<String>,
    payer_responsibility: Option<String>,
    verification_state: Option<String>,
    ip_address_types: Vec<String>,
    tags: Vec<(String, String)>,
}

/// The text of every plain `<item>` of the `set` child of `node`.
fn items(node: roxmltree::Node<'_, '_>, set: &str) -> Vec<String> {
    set_items(node, set)
        .into_iter()
        .map(|item| item.text().unwrap_or_default().to_string())
        .collect()
}

/// The `member` of every `<item>` of the `set` child of `node` — for a set whose items are
/// structures (`serviceType`, `privateDnsNameSet`).
fn item_members(node: roxmltree::Node<'_, '_>, set: &str, member: &str) -> Vec<String> {
    set_items(node, set)
        .into_iter()
        .filter_map(|item| child_text(item, member))
        .collect()
}

/// Every service a `DescribeVpcEndpointServices` body reports, in the order it reported
/// them.
fn reported(case: &str, body: &str) -> Vec<Reported> {
    let parsed = document(case, body);
    let root = parsed.root_element();
    let Some(set) = child(root, "serviceDetailSet") else {
        return Vec::new();
    };
    set.children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .map(|item| Reported {
            name: child_text(item, "serviceName").unwrap_or_default(),
            id: child_text(item, "serviceId").unwrap_or_default(),
            types: item_members(item, "serviceType", "serviceType"),
            region: child_text(item, "serviceRegion").unwrap_or_default(),
            owner: child_text(item, "owner").unwrap_or_default(),
            zones: items(item, "availabilityZoneSet"),
            zone_ids: items(item, "availabilityZoneIdSet"),
            base_dns_names: items(item, "baseEndpointDnsNameSet"),
            private_dns_name: child_text(item, "privateDnsName"),
            private_dns_names: item_members(item, "privateDnsNameSet", "privateDnsName"),
            policy_supported: child_text(item, "vpcEndpointPolicySupported"),
            acceptance_required: child_text(item, "acceptanceRequired"),
            manages_endpoints: child_text(item, "managesVpcEndpoints"),
            payer_responsibility: child_text(item, "payerResponsibility"),
            verification_state: child_text(item, "privateDnsNameVerificationState"),
            ip_address_types: items(item, "supportedIpAddressTypeSet"),
            tags: set_items(item, "tagSet")
                .into_iter()
                .map(|tag| {
                    (
                        child_text(tag, "key").unwrap_or_default(),
                        child_text(tag, "value").unwrap_or_default(),
                    )
                })
                .collect(),
        })
        .collect()
}

/// The names the envelope's `serviceNameSet` carries, in the order it reported them.
fn reported_names(case: &str, body: &str) -> Vec<String> {
    let parsed = document(case, body);
    items(parsed.root_element(), "serviceNameSet")
}

/// The `nextToken` of a response, or `None` when it carries none.
fn next_token(case: &str, body: &str) -> Option<String> {
    let parsed = document(case, body);
    child_text(parsed.root_element(), "nextToken").filter(|token| !token.is_empty())
}

// ---------------------------------------------------------------------------
// What a Region's catalogue reports
// ---------------------------------------------------------------------------

/// One row of the catalogue table: a service of the catalogue and what it must report.
struct CatalogueCase {
    /// The service's label under the Region.
    label: &'static str,
    /// The `ServiceType` members it must report, in order.
    types: &'static [&'static str],
    /// Whether it must report a private DNS name — an interface endpoint's private hosted
    /// zone answers on the service's default public name, and a gateway endpoint has none.
    private_dns: bool,
}

/// The rows: the two services the blocked recordings name, and four the issue did not cite —
/// an interface service with a dotted label, a plain interface service, the one service
/// reachable through either kind of endpoint, and a gateway-only service.
const CATALOGUE_CASES: [CatalogueCase; 6] = [
    CatalogueCase {
        label: "appstream.streaming",
        types: &["Interface"],
        private_dns: true,
    },
    CatalogueCase {
        label: "iot.data",
        types: &["Interface"],
        private_dns: true,
    },
    CatalogueCase {
        label: "ecr.dkr",
        types: &["Interface"],
        private_dns: true,
    },
    CatalogueCase {
        label: "logs",
        types: &["Interface"],
        private_dns: true,
    },
    CatalogueCase {
        label: "s3",
        types: &["Gateway", "Interface"],
        private_dns: true,
    },
    CatalogueCase {
        label: "dynamodb",
        types: &["Gateway"],
        private_dns: false,
    },
];

/// The Regions the table is driven in: three Regions of three partitions' naming, so nothing
/// about the report is tied to one Region.
const REGIONS: [&str; 3] = ["us-east-1", "eu-west-2", "ap-southeast-2"];

/// Whether `value` is a derived endpoint service id: `vpce-svc-` and 8 lowercase hexadecimal
/// digits, the shorter of the two forms an EC2 resource id takes — the store mints 17 for a
/// service configuration a caller created, so a derived id can never be mistaken for one.
fn is_derived_service_id(value: &str) -> bool {
    value
        .strip_prefix("vpce-svc-")
        .is_some_and(|suffix| suffix.len() == 8 && suffix.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Every row, in every Region, reports the service of THAT Region: the name, the Region, the
/// owner alias, the endpoint types, the zones the Region has, the DNS names an endpoint to it
/// answers under, and the flags a service Amazon Web Services offers carries.
#[tokio::test]
async fn each_catalogued_service_is_reported_for_the_region_asked_about() {
    let router = super::ec2_query::emulator().0;

    for region in REGIONS {
        for row in &CATALOGUE_CASES {
            let name = format!("com.amazonaws.{region}.{}", row.label);
            let case = format!("{name} asked about in {region}");
            let body = ok_in(&case, &router, region, &indexed("ServiceName", &[&name])).await;
            let services = reported(&case, &body);
            assert_eq!(
                services.len(),
                1,
                "case {case:?}: expected exactly one service, got {services:?}"
            );
            let service = &services[0];
            assert_eq!(
                service.name, name,
                "case {case:?}: expected the service name {name}, got {:?}",
                service.name
            );
            assert_eq!(
                reported_names(&case, &body),
                vec![name.clone()],
                "case {case:?}: serviceNameSet must report the same selection, got {body}"
            );
            assert!(
                is_derived_service_id(&service.id),
                "case {case:?}: expected a derived vpce-svc- id, got {:?}",
                service.id
            );
            assert_eq!(
                service.types,
                row.types.iter().map(|t| (*t).to_string()).collect::<Vec<_>>(),
                "case {case:?}: expected the service types {:?}, got {:?}",
                row.types,
                service.types
            );
            assert_eq!(
                service.region, region,
                "case {case:?}: expected the service Region {region}, got {:?}",
                service.region
            );
            assert_eq!(
                service.owner, "amazon",
                "case {case:?}: expected the owner alias of a service Amazon Web Services \
                 offers, got {:?}",
                service.owner
            );
            // The service is available in the zones the Region HAS, which
            // `DescribeAvailabilityZones` reports for the same Region.
            let zones = available_zones(&router, region).await;
            assert_eq!(
                service.zones, zones,
                "case {case:?}: expected the Region's own zones {zones:?}, got {:?}",
                service.zones
            );
            assert_eq!(
                service.zone_ids.len(),
                zones.len(),
                "case {case:?}: expected one zone id per zone, got {:?}",
                service.zone_ids
            );
            // "The DNS names for the service": an interface endpoint's own names sit under
            // the service's base name below `vpce.amazonaws.com`; a gateway endpoint mints
            // none and is reached through the service's public name.
            let mut expected_dns: Vec<String> = Vec::new();
            if row.types.contains(&"Interface") {
                expected_dns.push(format!("{}.{region}.vpce.amazonaws.com", row.label));
            }
            if row.types.contains(&"Gateway") {
                expected_dns.push(format!("{}.{region}.amazonaws.com", row.label));
            }
            expected_dns.sort();
            let mut base_dns_names = service.base_dns_names.clone();
            base_dns_names.sort();
            assert_eq!(
                base_dns_names, expected_dns,
                "case {case:?}: expected the base endpoint DNS names {expected_dns:?}, got {:?}",
                service.base_dns_names
            );
            let expected_private = row
                .private_dns
                .then(|| format!("{}.{region}.amazonaws.com", row.label));
            assert_eq!(
                service.private_dns_name, expected_private,
                "case {case:?}: expected the private DNS name {expected_private:?}, got {:?}",
                service.private_dns_name
            );
            assert_eq!(
                service.private_dns_names,
                expected_private.iter().cloned().collect::<Vec<_>>(),
                "case {case:?}: privateDnsNameSet must carry the same name, got {:?}",
                service.private_dns_names
            );
            assert_eq!(
                service.verification_state,
                row.private_dns.then(|| "verified".to_string()),
                "case {case:?}: a service's own private name needs nothing verified, got {:?}",
                service.verification_state
            );
            assert_eq!(
                service.policy_supported.as_deref(),
                Some("true"),
                "case {case:?}: an endpoint to a service of the catalogue takes a policy, got \
                 {:?}",
                service.policy_supported
            );
            assert_eq!(
                service.acceptance_required.as_deref(),
                Some("false"),
                "case {case:?}: nothing accepts a connection to a service Amazon Web Services \
                 offers, got {:?}",
                service.acceptance_required
            );
            assert_eq!(
                service.manages_endpoints.as_deref(),
                Some("false"),
                "case {case:?}: the endpoint a caller creates is the caller's, got {:?}",
                service.manages_endpoints
            );
            assert_eq!(
                service.payer_responsibility.as_deref(),
                Some("ServiceOwner"),
                "case {case:?}: expected the one PayerResponsibility member, got {:?}",
                service.payer_responsibility
            );
            // "The supported IP address types" is about what the SERVICE answers on: an
            // interface endpoint service answers on both IPv4 and IPv6, while a service
            // reached only through a gateway endpoint is reached through its route-table
            // entry, which is an IPv4 prefix list.
            let expected_ip_types = if row.types.contains(&"Interface") {
                vec!["ipv4".to_string(), "ipv6".to_string()]
            } else {
                vec!["ipv4".to_string()]
            };
            assert_eq!(
                service.ip_address_types, expected_ip_types,
                "case {case:?}: expected the address types {expected_ip_types:?}, got {:?}",
                service.ip_address_types
            );
            assert!(
                service.tags.is_empty(),
                "case {case:?}: nobody can tag a service Amazon Web Services offers, got {:?}",
                service.tags
            );
        }
    }
}

/// The zone names `DescribeAvailabilityZones` reports for `region` — the zones the Region
/// HAS, read through the API rather than written out here, so the catalogue's zones are
/// compared against the one derivation this emulator has of them.
async fn available_zones(router: &MultiServiceRouter, region: &str) -> Vec<String> {
    let (status, body) =
        super::ec2_query::call(router, region, "DescribeAvailabilityZones", &[]).await;
    assert_eq!(
        status, 200,
        "DescribeAvailabilityZones for {region} must succeed, got {status}: {body}"
    );
    super::ec2_query::elements(&body, "zoneName")
}

/// One API whose data plane is reached through a DEDICATED PrivateLink service of the Region
/// rather than through that API's plain endpoint service: the label of the dedicated service,
/// and the label of the plain one it must not be confused with.
///
/// Source: the PrivateLink service list, which publishes the CloudWatch Logs syslog-ingestion
/// service as `com.amazonaws.<region>.syslog-logs` beside the plain API service
/// `com.amazonaws.<region>.logs`. A client provisioning syslog ingestion discovers the
/// Region's name for it here, so a catalogue that publishes only the plain service leaves it
/// with no endpoint to create.
const DEDICATED_SERVICES: [(&str, &str); 1] = [("syslog-logs", "logs")];

/// A Region publishes each dedicated interface service of [`DEDICATED_SERVICES`] under its own
/// name, beside — and distinct from — the plain service of the same API: a walk of the
/// Region's interface services finds both names, each resolves on its own, and the two carry
/// different ids and different endpoint DNS names.
#[tokio::test]
async fn a_dedicated_interface_service_is_published_beside_the_plain_one() {
    let router = super::ec2_query::emulator().0;

    for region in REGIONS {
        // The whole selection a client discovers with, walked to exhaustion the way a client
        // pages it: the Region's interface services.
        let case = format!("the interface services of {region}");
        let mut names: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut params = filters(&[("service-type", &["Interface"])]);
            params.push(param("MaxResults", "5"));
            if let Some(token) = token.as_deref() {
                params.push(param("NextToken", token));
            }
            let body = ok_in(&case, &router, region, &params).await;
            names.extend(
                reported(&case, &body)
                    .into_iter()
                    .map(|service| service.name),
            );
            match next_token(&case, &body) {
                None => break,
                Some(next) => token = Some(next),
            }
        }

        for (dedicated, plain) in DEDICATED_SERVICES {
            let dedicated_name = format!("com.amazonaws.{region}.{dedicated}");
            let plain_name = format!("com.amazonaws.{region}.{plain}");
            assert!(
                names.contains(&dedicated_name),
                "case {case:?}: expected the dedicated service {dedicated_name} in the walk, \
                 got {names:?}"
            );
            assert!(
                names.contains(&plain_name),
                "case {case:?}: expected the plain service {plain_name} beside it, got {names:?}"
            );

            // Each name resolves on its own, and the two are different services: different
            // ids, and the endpoint DNS names an endpoint to one answers under are not the
            // other's.
            let selected = ok_in(
                &case,
                &router,
                region,
                &indexed("ServiceName", &[&dedicated_name, &plain_name]),
            )
            .await;
            let services = reported(&case, &selected);
            assert_eq!(
                services.len(),
                2,
                "case {case:?}: both names must resolve, got {services:?}"
            );
            let dedicated_service = services
                .iter()
                .find(|service| service.name == dedicated_name)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: {dedicated_name} must be reported, got {services:?}")
                });
            let plain_service = services
                .iter()
                .find(|service| service.name == plain_name)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: {plain_name} must be reported, got {services:?}")
                });
            assert_eq!(
                dedicated_service.types,
                vec!["Interface".to_string()],
                "case {case:?}: {dedicated_name} is reached through an interface endpoint, got \
                 {:?}",
                dedicated_service.types
            );
            assert_eq!(
                dedicated_service.region, region,
                "case {case:?}: {dedicated_name} is a service of {region}, got {:?}",
                dedicated_service.region
            );
            assert_ne!(
                dedicated_service.id, plain_service.id,
                "case {case:?}: the dedicated service and the plain one are two services, got \
                 the one id {:?}",
                dedicated_service.id
            );
            assert_ne!(
                dedicated_service.base_dns_names, plain_service.base_dns_names,
                "case {case:?}: an endpoint to {dedicated_name} answers under its own base \
                 names, got {:?}",
                dedicated_service.base_dns_names
            );
        }
    }
}

/// Every service of a Region's report is a service OF that Region, named once, with an id of
/// its own — and a second identical call reports exactly the same thing, a describe being a
/// read.
#[tokio::test]
async fn a_regions_report_is_stable_and_its_services_are_distinct() {
    let router = super::ec2_query::emulator().0;
    let case = "the whole catalogue of one Region";

    let first = ok(case, &router, &[]).await;
    let services = reported(case, &first);
    assert!(
        services.len() > 1,
        "case {case:?}: expected a Region to offer several services, got {services:?}"
    );
    let names: Vec<String> = services.iter().map(|service| service.name.clone()).collect();
    assert_eq!(
        reported_names(case, &first),
        names,
        "case {case:?}: serviceNameSet must report the detailed selection, got {first}"
    );
    let unique_names: BTreeSet<&String> = names.iter().collect();
    assert_eq!(
        unique_names.len(),
        names.len(),
        "case {case:?}: a service name must be unique within a Region, got {names:?}"
    );
    let unique_ids: BTreeSet<&String> = services.iter().map(|service| &service.id).collect();
    assert_eq!(
        unique_ids.len(),
        services.len(),
        "case {case:?}: two services of one Region must not share an id, got {services:?}"
    );
    for service in &services {
        // Every service of the report is a service of the Region asked about, under one of
        // the prefixes an endpoint service name of Amazon Web Services is published with.
        let expected_prefixes = [
            format!("com.amazonaws.{REGION}."),
            format!("aws.api.{REGION}."),
            format!("api.aws.{REGION}."),
        ];
        assert!(
            expected_prefixes
                .iter()
                .any(|prefix| service.name.starts_with(prefix)),
            "case {case:?}: expected a service of {REGION}, got {:?}",
            service.name
        );
        assert!(
            !service.types.is_empty(),
            "case {case:?}: every service is reachable through some endpoint type, got {service:?}"
        );
    }

    let second = ok(case, &router, &[]).await;
    assert_eq!(
        reported(case, &second),
        services,
        "case {case:?}: a second call must report the same services, got {second}"
    );
}

// ---------------------------------------------------------------------------
// ServiceNames
// ---------------------------------------------------------------------------

/// `ServiceNames` narrows the report to the names given, in any number, and the selection is
/// the same whichever order they were given in.
#[tokio::test]
async fn service_names_narrows_the_report() {
    let router = super::ec2_query::emulator().0;

    // Rows: no name at all (the whole Region), one name, two names, and the same two the
    // other way round.
    let rows: Vec<(&str, Vec<&str>)> = vec![
        ("no name names every service", Vec::new()),
        (
            "one name",
            vec!["com.amazonaws.us-east-1.appstream.streaming"],
        ),
        (
            "two names",
            vec![
                "com.amazonaws.us-east-1.iot.data",
                "com.amazonaws.us-east-1.s3",
            ],
        ),
        (
            "the same two in the other order",
            vec![
                "com.amazonaws.us-east-1.s3",
                "com.amazonaws.us-east-1.iot.data",
            ],
        ),
    ];

    let every = reported("every service", &ok("every service", &router, &[]).await).len();
    for (case, names) in rows {
        let body = ok(case, &router, &indexed("ServiceName", &names)).await;
        let got: Vec<String> = reported(case, &body)
            .into_iter()
            .map(|service| service.name)
            .collect();
        if names.is_empty() {
            assert_eq!(
                got.len(),
                every,
                "case {case:?}: expected every service of the Region, got {got:?}"
            );
            continue;
        }
        let mut expected: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
        expected.sort();
        assert_eq!(
            got, expected,
            "case {case:?}: expected exactly the named services, got {got:?}"
        );
    }
}

/// A name no Region asked about offers is refused with `InvalidServiceName`, whatever makes
/// it unknown — and the refusal names the value at fault.
#[tokio::test]
async fn an_unknown_service_name_is_refused() {
    let router = super::ec2_query::emulator().0;

    // Rows: not an endpoint service name at all, a well-formed name of a service that does
    // not exist, a name of ANOTHER Region than the call asks about, and one unknown name
    // beside a known one.
    let rows: Vec<(&str, Vec<&str>, &str)> = vec![
        ("not a service name", vec!["nonsense"], "nonsense"),
        (
            "a service the catalogue does not offer",
            vec!["com.amazonaws.us-east-1.not-a-real-service"],
            "com.amazonaws.us-east-1.not-a-real-service",
        ),
        (
            "a service of another Region",
            vec!["com.amazonaws.eu-west-1.s3"],
            "com.amazonaws.eu-west-1.s3",
        ),
        (
            "one unknown name beside a known one",
            vec!["com.amazonaws.us-east-1.s3", "com.amazonaws.us-east-1.nope"],
            "com.amazonaws.us-east-1.nope",
        ),
    ];

    for (case, names, at_fault) in rows {
        let (status, body) = describe(&router, &indexed("ServiceName", &names)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidServiceName",
            "case {case:?}: expected InvalidServiceName, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The Vpc Endpoint Service '{at_fault}' does not exist"),
            "case {case:?}: expected the refusal to name the value at fault, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// ServiceRegions
// ---------------------------------------------------------------------------

/// `ServiceRegions` is what the report is about; a request naming none asks about the Region
/// it was signed for.
#[tokio::test]
async fn service_regions_selects_the_regions_asked_about() {
    let router = super::ec2_query::emulator().0;

    // Rows: no Region (the signed one), the signed Region named explicitly, another Region,
    // two Regions at once, and the same Region twice — which is one Region.
    let rows: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("no Region names the signed one", Vec::new(), vec![REGION]),
        ("the signed Region named", vec![REGION], vec![REGION]),
        ("another Region", vec!["eu-west-1"], vec!["eu-west-1"]),
        (
            "two Regions at once",
            vec!["eu-west-1", "ap-south-1"],
            vec!["eu-west-1", "ap-south-1"],
        ),
        (
            "one Region named twice",
            vec!["eu-west-1", "eu-west-1"],
            vec!["eu-west-1"],
        ),
    ];

    let per_region = reported("every service", &ok("every service", &router, &[]).await).len();
    for (case, asked, expected_regions) in rows {
        let body = ok(case, &router, &indexed("ServiceRegion", &asked)).await;
        let services = reported(case, &body);
        assert_eq!(
            services.len(),
            per_region * expected_regions.len(),
            "case {case:?}: expected one catalogue per Region asked about, got {} services",
            services.len()
        );
        let regions: BTreeSet<String> = services
            .iter()
            .map(|service| service.region.clone())
            .collect();
        let wanted: BTreeSet<String> = expected_regions
            .iter()
            .map(|region| (*region).to_string())
            .collect();
        assert_eq!(
            regions, wanted,
            "case {case:?}: expected the services of {wanted:?}, got {regions:?}"
        );
        for service in &services {
            // A name of any published family carries the Region hosting the service:
            // `com.amazonaws.<region>.<label>`, `aws.api.<region>.<label>` and
            // `api.aws.<region>.<label>`.
            let prefixes = [
                format!("com.amazonaws.{}.", service.region),
                format!("aws.api.{}.", service.region),
                format!("api.aws.{}.", service.region),
            ];
            assert!(
                prefixes
                    .iter()
                    .any(|prefix| service.name.starts_with(prefix)),
                "case {case:?}: a service's name carries its Region, got {:?}",
                service.name
            );
        }
    }
}

/// A `ServiceRegions` value that names no Region of the Region catalog names no services
/// either, and is refused rather than read as a Region that happens to offer none.
#[tokio::test]
async fn a_service_region_that_names_no_region_is_refused() {
    let router = super::ec2_query::emulator().0;

    // Rows: the case-shifted spelling, two spellings that are not Region codes at all, and a
    // well-formed code no Region has.
    for case in ["Us-East-1", "us_east_1", "not a region", "us-east-9"] {
        let (status, body) = describe(&router, &indexed("ServiceRegion", &[case])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// One row of the filter table: the `Filters` the request carries, and the services the
/// report must then hold.
struct FilterCase {
    case: &'static str,
    /// The filter entries, as the harness's `filters` builder takes them.
    entries: Vec<(&'static str, &'static [&'static str])>,
    /// The service names the report must carry, in the order it reports them, or `None` for
    /// "every service of the Region" — a filter that narrows nothing.
    expected: Option<Vec<&'static str>>,
}

/// The rows: each documented filter in turn, a filter with two values, two filters ANDed, a
/// wildcard, and filters whose values nothing holds.
fn filter_cases() -> Vec<FilterCase> {
    vec![
        FilterCase {
            case: "service-name selects the one service",
            entries: vec![("service-name", &["com.amazonaws.us-east-1.logs"])],
            expected: Some(vec!["com.amazonaws.us-east-1.logs"]),
        },
        FilterCase {
            case: "service-name over two values",
            entries: vec![(
                "service-name",
                &["com.amazonaws.us-east-1.logs", "com.amazonaws.us-east-1.kms"],
            )],
            expected: Some(vec![
                "com.amazonaws.us-east-1.kms",
                "com.amazonaws.us-east-1.logs",
            ]),
        },
        FilterCase {
            case: "service-name with a wildcard",
            entries: vec![("service-name", &["*.appstream.*"])],
            expected: Some(vec![
                "com.amazonaws.us-east-1.appstream.api",
                "com.amazonaws.us-east-1.appstream.streaming",
            ]),
        },
        FilterCase {
            case: "service-type selects the gateway services",
            entries: vec![("service-type", &["Gateway"])],
            expected: Some(vec![
                "com.amazonaws.us-east-1.dynamodb",
                "com.amazonaws.us-east-1.s3",
            ]),
        },
        FilterCase {
            case: "service-type ANDed with service-name",
            entries: vec![("service-type", &["Gateway"]), ("service-name", &["*.s3"])],
            expected: Some(vec!["com.amazonaws.us-east-1.s3"]),
        },
        FilterCase {
            case: "a service-type nothing is reachable through here",
            entries: vec![("service-type", &["GatewayLoadBalancer"])],
            expected: Some(Vec::new()),
        },
        FilterCase {
            case: "owner selects every service of the catalogue",
            entries: vec![("owner", &["amazon"])],
            expected: None,
        },
        FilterCase {
            case: "owner nothing holds",
            entries: vec![("owner", &["aws-marketplace"])],
            expected: Some(Vec::new()),
        },
        FilterCase {
            case: "service-region selects the signed Region",
            entries: vec![("service-region", &["us-east-1"])],
            expected: None,
        },
        FilterCase {
            case: "service-region of another Region selects nothing",
            entries: vec![("service-region", &["eu-west-1"])],
            expected: Some(Vec::new()),
        },
        FilterCase {
            case: "supported-ip-address-types selects every service",
            entries: vec![("supported-ip-address-types", &["ipv4"])],
            expected: None,
        },
        FilterCase {
            // Every interface endpoint service answers on IPv6 too, so the type selects
            // every service but the ones reachable only through a gateway endpoint: ANDing
            // it with `service-type` leaves the one service reachable BOTH ways.
            case: "an address type only the interface services support",
            entries: vec![
                ("supported-ip-address-types", &["ipv6"]),
                ("service-type", &["Gateway"]),
            ],
            expected: Some(vec!["com.amazonaws.us-east-1.s3"]),
        },
        FilterCase {
            case: "an address type no service here supports",
            entries: vec![("supported-ip-address-types", &["dualstack"])],
            expected: Some(Vec::new()),
        },
        FilterCase {
            case: "tag-key selects nothing, no service of the catalogue being tagged",
            entries: vec![("tag-key", &["Owner"])],
            expected: Some(Vec::new()),
        },
        FilterCase {
            case: "a tag the catalogue does not carry",
            entries: vec![("tag:Owner", &["TeamA"])],
            expected: Some(Vec::new()),
        },
    ]
}

/// Every documented filter selects on the attribute it names, ORed over its values, ANDed
/// with the other filters, with EC2's `*` wildcard.
#[tokio::test]
async fn documented_filters_narrow_the_report() {
    let router = super::ec2_query::emulator().0;

    let every: Vec<String> = reported("every service", &ok("every service", &router, &[]).await)
        .into_iter()
        .map(|service| service.name)
        .collect();
    for row in filter_cases() {
        let case = row.case;
        let body = ok(case, &router, &filters(&row.entries)).await;
        let got: Vec<String> = reported(case, &body)
            .into_iter()
            .map(|service| service.name)
            .collect();
        let wanted: Vec<String> = match row.expected {
            None => every.clone(),
            Some(names) => names.iter().map(|name| (*name).to_string()).collect(),
        };
        assert_eq!(
            got, wanted,
            "case {case:?}: expected {wanted:?}, got {got:?}"
        );
        assert_eq!(
            reported_names(case, &body),
            got,
            "case {case:?}: serviceNameSet must report the same selection, got {body}"
        );
    }
}

/// A filter this operation does not document is refused whether or not any service would
/// have matched it — so an unsupported filter can never read as "no results".
#[tokio::test]
async fn an_undocumented_filter_is_refused() {
    let router = super::ec2_query::emulator().0;

    // Rows: a filter of a SIBLING operation, a misspelling, the case-shifted spelling of a
    // documented name, and a `tag:` prefix with no key.
    for case in ["service-id", "service-names", "Service-Name", "tag:"] {
        let (status, body) = describe(&router, &filters(&[(case, &["anything"])])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The filter '{case}' is invalid"),
            "case {case:?}: expected the refusal to name the filter, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Paging
// ---------------------------------------------------------------------------

/// A `MaxResults` walk reports every service of the selection exactly once, in one stable
/// order, and the last page carries no token.
#[tokio::test]
async fn pages_walk_every_service_exactly_once() {
    let router = super::ec2_query::emulator().0;

    let whole: Vec<String> = reported("the whole report", &ok("the whole report", &router, &[]).await)
        .into_iter()
        .map(|service| service.name)
        .collect();

    // Rows: a page of one, a page that divides the catalogue unevenly, a page larger than it,
    // and a page above the maximum this service serves — which is cut down to it rather than
    // refused ("if the value is greater than 1,000, we return only 1,000 items").
    for page_size in [1usize, 7, 1_000, 5_000] {
        let case = format!("pages of {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut params = vec![param("MaxResults", &page_size.to_string())];
            if let Some(token) = token.as_deref() {
                params.push(param("NextToken", token));
            }
            let body = ok(&case, &router, &params).await;
            let names: Vec<String> = reported(&case, &body)
                .into_iter()
                .map(|service| service.name)
                .collect();
            assert!(
                names.len() <= page_size.min(1_000),
                "case {case:?}: a page holds at most the page size, got {} names",
                names.len()
            );
            walked.extend(names);
            pages += 1;
            assert!(
                pages <= whole.len() + 1,
                "case {case:?}: the walk must terminate, got {pages} pages"
            );
            match next_token(&case, &body) {
                None => break,
                Some(next) => token = Some(next),
            }
        }
        assert_eq!(
            walked, whole,
            "case {case:?}: the walk must report every service exactly once, in order"
        );
    }
}

/// A token this service did not mint is refused rather than read as a position.
#[tokio::test]
async fn a_token_this_service_did_not_mint_is_refused() {
    let router = super::ec2_query::emulator().0;

    for case in ["not-a-token", "", "com.amazonaws.us-east-1.s3", "%%%"] {
        let (status, body) = describe(&router, &[param("NextToken", case)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {case:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}

/// A page of no items is not a page: a `MaxResults` below the minimum this operation
/// publishes is refused in EC2's own sentence for that violation, which names the value at
/// fault and the minimum it has to reach, rather than answered with a full page. A size
/// ABOVE the maximum is not refused — the member documents its own answer for that, which
/// the walk above covers.
#[tokio::test]
async fn a_page_size_below_one_is_refused() {
    let router = super::ec2_query::emulator().0;

    for case in ["0", "-1", "-1000"] {
        let (status, body) = describe(&router, &[param("MaxResults", case)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!(
                "Value ( {case} ) for parameter maxResults is invalid. \
                 Expecting a value greater than 1."
            ),
            "case {case:?}: expected the refusal to name the size at fault, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// The account's own services
// ---------------------------------------------------------------------------

/// One service configuration of the account, as `CreateVpcEndpointServiceConfiguration` would
/// leave it: hosted in `region`, accepting connections only once its owner has accepted them,
/// and tagged.
fn own_service(region: &str, zones: &[String], tag_value: &str) -> ServiceConfiguration {
    ServiceConfiguration {
        service_type: Some(vec![ServiceTypeDetail {
            service_type: Some("Interface".to_string()),
        }]),
        service_state: Some("Available".to_string()),
        availability_zones: Some(zones.to_vec()),
        availability_zone_ids: Some(Vec::new()),
        acceptance_required: Some(true),
        manages_vpc_endpoints: Some(false),
        base_endpoint_dns_names: Some(vec![format!("{region}.vpce.amazonaws.com")]),
        private_dns_name: Some(format!("service.{region}.example.com")),
        private_dns_name_configuration: Some(PrivateDnsNameConfiguration {
            state: Some("pendingVerification".to_string()),
            ..Default::default()
        }),
        payer_responsibility: Some("ServiceOwner".to_string()),
        supported_ip_address_types: Some(vec!["ipv4".to_string()]),
        tags: Some(vec![Tag {
            key: Some("Owner".to_string()),
            value: Some(tag_value.to_string()),
        }]),
        ..Default::default()
    }
}

/// The account's own endpoint services are reported beside the Region's catalogue, with what
/// their OWNER decided — and only in the Region that hosts them.
#[tokio::test]
async fn an_account_service_configuration_is_reported_beside_the_catalogue() {
    let (router, engine) = super::ec2_query::emulator();
    let case = "the account's own endpoint service";

    let zones = available_zones(&router, REGION).await;
    let (service_id, service_name) = {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let service_id = guard
            .insert_vpc_endpoint_service(own_service(REGION, &zones, "TeamA"))
            .expect("the store must have room for the seeded service configuration");
        let service_name = format!("com.amazonaws.vpce.{REGION}.{service_id}");
        if let Some(stored) = guard.vpc_endpoint_services.get_mut(&service_id) {
            stored.service_name = Some(service_name.clone());
        }
        (service_id, service_name)
    };

    let body = ok(case, &router, &indexed("ServiceName", &[&service_name])).await;
    let services = reported(case, &body);
    assert_eq!(
        services.len(),
        1,
        "case {case:?}: expected the one stored service, got {services:?}"
    );
    let service = &services[0];
    assert_eq!(
        service.id, service_id,
        "case {case:?}: the stored service keeps the id the store minted, got {:?}",
        service.id
    );
    assert_eq!(
        service.owner,
        engine::emulator_account_id(),
        "case {case:?}: the account owns the service it created, got {:?}",
        service.owner
    );
    assert_eq!(
        service.region, REGION,
        "case {case:?}: the service is hosted in the Region its name carries, got {:?}",
        service.region
    );
    assert_eq!(
        service.acceptance_required.as_deref(),
        Some("true"),
        "case {case:?}: the owner required acceptance, got {:?}",
        service.acceptance_required
    );
    assert_eq!(
        service.policy_supported.as_deref(),
        Some("false"),
        "case {case:?}: an endpoint policy is for a service Amazon Web Services offers, got \
         {:?}",
        service.policy_supported
    );
    assert_eq!(
        service.verification_state.as_deref(),
        Some("pendingVerification"),
        "case {case:?}: the stored verification state is reported, got {:?}",
        service.verification_state
    );
    assert_eq!(
        service.tags,
        vec![("Owner".to_string(), "TeamA".to_string())],
        "case {case:?}: the service's own tags are reported, got {:?}",
        service.tags
    );

    // It is reported in the Region that hosts it, among that Region's catalogue, and the
    // documented tag and owner filters select it there.
    let whole = ok(case, &router, &[]).await;
    let names: Vec<String> = reported(case, &whole)
        .into_iter()
        .map(|reported| reported.name)
        .collect();
    assert!(
        names.contains(&service_name),
        "case {case:?}: the stored service is one of the Region's services, got {names:?}"
    );
    for (filter_case, entries) in [
        ("tag:Owner", vec![("tag:Owner", &["TeamA"][..])]),
        ("tag-key", vec![("tag-key", &["Owner"][..])]),
        (
            "owner",
            vec![("owner", &[engine::emulator_account_id()][..])],
        ),
    ] {
        let body = ok(filter_case, &router, &filters(&entries)).await;
        let selected: Vec<String> = reported(filter_case, &body)
            .into_iter()
            .map(|reported| reported.name)
            .collect();
        assert_eq!(
            selected,
            vec![service_name.clone()],
            "case {filter_case:?}: expected only the account's own service, got {selected:?}"
        );
    }

    // And it is NOT a service of another Region: the Region hosting it is the one its name
    // carries.
    let elsewhere = ok(case, &router, &indexed("ServiceRegion", &["eu-west-1"])).await;
    let elsewhere_names: Vec<String> = reported(case, &elsewhere)
        .into_iter()
        .map(|reported| reported.name)
        .collect();
    assert!(
        !elsewhere_names.contains(&service_name),
        "case {case:?}: a service of {REGION} is not offered in eu-west-1, got \
         {elsewhere_names:?}"
    );
}

// ---------------------------------------------------------------------------
// DryRun, and the catalogue against a created endpoint
// ---------------------------------------------------------------------------

/// "If you have the required permissions, the error response is DryRunOperation" — and
/// nothing is reported.
#[tokio::test]
async fn dry_run_reports_nothing_and_answers_dry_run_operation() {
    let router = super::ec2_query::emulator().0;
    let case = "DryRun";

    let (status, body) = describe(&router, &[param("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "case {case:?}: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case {case:?}: expected DryRunOperation, got {body}"
    );
    assert!(
        reported(case, &body).is_empty(),
        "case {case:?}: a dry run reports no service, got {body}"
    );
}

/// The base endpoint DNS name reported for a service is the name an endpoint TO that service
/// answers under: the names `CreateVpcEndpoint` mints end in exactly it.
#[tokio::test]
async fn a_created_endpoints_dns_names_sit_under_the_reported_base_name() {
    let router = super::ec2_query::emulator().0;
    let case = "an interface endpoint to a service of the catalogue";
    let service_name = format!("com.amazonaws.{REGION}.logs");

    let body = ok(case, &router, &indexed("ServiceName", &[&service_name])).await;
    let services = reported(case, &body);
    let base = services
        .first()
        .and_then(|service| service.base_dns_names.first().cloned())
        .unwrap_or_else(|| panic!("case {case:?}: no base endpoint DNS name in {body}"));

    let vpc = ok_action(case, &router, "CreateVpc", &[param("CidrBlock", "10.50.0.0/16")]).await;
    let vpc_id = super::ec2_query::element(&vpc, "vpcId")
        .unwrap_or_else(|| panic!("case {case:?}: no vpcId in {vpc}"));
    let subnet = ok_action(
        case,
        &router,
        "CreateSubnet",
        &[
            param("VpcId", &vpc_id),
            param("CidrBlock", "10.50.1.0/24"),
            param("AvailabilityZone", &format!("{REGION}a")),
        ],
    )
    .await;
    let subnet_id = super::ec2_query::element(&subnet, "subnetId")
        .unwrap_or_else(|| panic!("case {case:?}: no subnetId in {subnet}"));
    let endpoint = ok_action(
        case,
        &router,
        "CreateVpcEndpoint",
        &[
            param("VpcId", &vpc_id),
            param("ServiceName", &service_name),
            param("VpcEndpointType", "Interface"),
            param("SubnetId.1", &subnet_id),
            // This row is about the names the endpoint answers ON, so it asks for no private
            // hosted zone: the service's own public name is served by that zone rather than
            // sitting under the endpoint's base name, and a VPC you create has the DNS
            // hostnames such a zone needs turned off.
            param("PrivateDnsEnabled", "false"),
        ],
    )
    .await;

    let dns_names = super::ec2_query::elements(&endpoint, "dnsName");
    assert!(
        !dns_names.is_empty(),
        "case {case:?}: an interface endpoint answers on some name, got {endpoint}"
    );
    for dns_name in &dns_names {
        assert!(
            dns_name.ends_with(&base),
            "case {case:?}: expected {dns_name:?} to sit under the reported base name \
             {base:?}"
        );
    }
}

/// One call of another action that has to succeed, and the body it answered.
async fn ok_action(
    case: &str,
    router: &MultiServiceRouter,
    action: &str,
    params: &[Param],
) -> String {
    let (status, body) = super::ec2_query::call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}
