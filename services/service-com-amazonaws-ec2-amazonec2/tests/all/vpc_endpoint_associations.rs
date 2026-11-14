// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeVpcEndpointAssociations`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=DescribeVpcEndpointAssociations`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing name in the
//! request's SigV4 credential scope, so every call here carries one.
//!
//! The rule under test is that the operation REPORTS THE STORE: the set it answers with is
//! the account's VPC endpoint associations, narrowed by the `VpcEndpointIds` and `Filters`
//! the request names and cut into `MaxResults`-sized pages. Every table row is a distinct
//! input along what that generalises over — which filter, which spelling of an
//! accessibility, how many values, which endpoint ids, which page size, which member of the
//! request is at fault — and an account with no association (the empty store) is one row
//! among them rather than the specification.
//!
//! An association belongs to an endpoint that points at a resource configuration or a
//! service network, so the endpoints the associations here hang off are written through
//! [`Ec2State::insert_vpc_endpoint`] while `CreateVpcEndpoint` holds no store for either
//! kind of target; the interface endpoint one test creates through the API is the other side
//! of the same rule — an endpoint type that has no association is reported as having none.

use ec2::ops::types::{DnsEntry, Tag, VpcEndpoint};
use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, VpcEndpointAssociationSpec};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, error_message, filters, indexed};
use super::wire::{child_text, document, param, Param};

/// The Region every call here is signed for. The association store is per account rather
/// than per Region, so the Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// The ec2Query action under test.
const ACTION: &str = "DescribeVpcEndpointAssociations";

/// The element the reported set arrives in
/// (`DescribeVpcEndpointAssociationsResult$VpcEndpointAssociations`'s `xmlName`).
const SET_ELEMENT: &str = "vpcEndpointAssociationSet";

/// One call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, ACTION, params).await
}

/// The association ids a successful response reports, in the order it reported them.
///
/// Read through the XML parser rather than by substring, so an `<id>` of the reported set is
/// never confused with an identically-named member of anything else in the envelope.
fn reported_ids(case: &str, body: &str) -> Vec<String> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == SET_ELEMENT)
        .map(|set| {
            set.children()
                .filter(|item| item.is_element() && item.tag_name().name() == "item")
                .map(|item| {
                    child_text(item, "id").unwrap_or_else(|| {
                        panic!("case {case:?}: an association with no <id> in {body}")
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The ids a call that has to succeed reports.
async fn described_ids(router: &MultiServiceRouter, case: &str, params: &[Param]) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    reported_ids(case, &body)
}

/// One `MaxResults` parameter.
fn max_results_param(max_results: i64) -> Vec<Param> {
    vec![param("MaxResults", &max_results.to_string())]
}

/// A tag, as the seeded associations carry them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// A DNS entry, as the seeded associations carry them.
fn dns_entry(dns_name: &str, hosted_zone_id: &str) -> DnsEntry {
    DnsEntry {
        dns_name: Some(dns_name.to_string()),
        hosted_zone_id: Some(hosted_zone_id.to_string()),
    }
}

/// The ARN of a VPC Lattice service network, the resource
/// `VpcEndpointAssociation$ServiceNetworkArn` names.
fn service_network_arn(id: &str) -> String {
    format!(
        "arn:aws:vpc-lattice:{REGION}:{}:servicenetwork/{id}",
        engine::emulator_account_id()
    )
}

/// The ARN of a resource configuration, the resource
/// `VpcEndpointAssociation$AssociatedResourceArn` names — in the `<type>/<id>` form an ARN's
/// resource part usually takes.
fn resource_configuration_arn(id: &str) -> String {
    format!(
        "arn:aws:vpc-lattice:{REGION}:{}:resourceconfiguration/{id}",
        engine::emulator_account_id()
    )
}

/// The same ARN in the OTHER form an ARN's resource part may take, `<type>:<id>` (AWS General
/// Reference, "Amazon Resource Names"), so the `associated-resource-id` filter is tested
/// against both separators rather than only the one the first form uses.
fn colon_resource_configuration_arn(id: &str) -> String {
    format!(
        "arn:aws:vpc-lattice:{REGION}:{}:resourceconfiguration:{id}",
        engine::emulator_account_id()
    )
}

/// The ARN of a resource configuration group, the resource
/// `VpcEndpointAssociation$ResourceConfigurationGroupArn` names.
fn group_arn(id: &str) -> String {
    format!(
        "arn:aws:vpc-lattice:{REGION}:{}:resourceconfiguration/{id}",
        engine::emulator_account_id()
    )
}

/// What one seeded association holds. The members are the modelled
/// [`ec2::ops::types::VpcEndpointAssociation`]'s, written through the one store seam so the
/// association id is minted rather than chosen here.
struct AssociationRow {
    /// Which seeded endpoint the association belongs to: an index into [`Endpoints::ids`].
    endpoint: usize,
    service_network: Option<&'static str>,
    service_network_name: Option<&'static str>,
    /// The id of the resource configuration the association reaches, and whether its ARN is
    /// written in the `/` or the `:` resource form.
    associated_resource: Option<(&'static str, bool)>,
    group: Option<&'static str>,
    /// `AssociatedResourceAccessibility`, one of the reported values of
    /// `ec2::state::ASSOCIATED_RESOURCE_ACCESSIBILITY_FILTER_VALUES`.
    accessibility: &'static str,
    failure_code: Option<&'static str>,
    failure_reason: Option<&'static str>,
    dns: Option<(&'static str, &'static str)>,
    private_dns: Option<(&'static str, &'static str)>,
    tags: Vec<Tag>,
}

/// The endpoints the seeded associations hang off.
struct Endpoints {
    /// The ids, in the order [`seed_endpoints`] wrote them.
    ids: Vec<String>,
}

/// The VPC endpoint types an association can belong to: an endpoint of a service network and
/// one of a resource configuration (`VpcEndpointType`'s `ServiceNetwork` and `Resource` —
/// `ec2::ops::types::VPC_ENDPOINT_TYPE_VALUES`), which is what
/// `DescribeVpcEndpointAssociations` describes "the VPC resources … or service networks
/// associated with the VPC endpoint" of.
const ASSOCIATED_ENDPOINT_TYPES: [&str; 2] = ["ServiceNetwork", "Resource"];

/// Write one VPC endpoint per type above into the store, the way the operation that creates
/// an endpoint to a service network or a resource configuration would, and report the ids
/// they were given.
fn seed_endpoints(engine: &engine::InMemoryEngine) -> Endpoints {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let ids = ASSOCIATED_ENDPOINT_TYPES
        .iter()
        .map(|endpoint_type| {
            guard
                .insert_vpc_endpoint(VpcEndpoint {
                    vpc_endpoint_type: Some((*endpoint_type).to_string()),
                    state: Some(ec2::state::VPC_ENDPOINT_STATE_AVAILABLE.to_string()),
                    ..VpcEndpoint::default()
                })
                .unwrap_or_else(|| {
                    panic!("case {endpoint_type:?}: the store must take the seeded endpoint")
                })
        })
        .collect();
    Endpoints { ids }
}

/// The associations the filter and paging tables are written against. They differ along
/// every attribute the documented filters select on — the endpoint, the accessibility, the
/// service network, the associated resource and its configuration group — and along every
/// member that is merely reported: the service network name, the failure code and reason,
/// the two DNS entries and the tags. The first endpoint holds three and the second two, so a
/// filter that selects on one endpoint reports more than a single row.
fn seeded_rows() -> Vec<AssociationRow> {
    vec![
        AssociationRow {
            endpoint: 0,
            service_network: Some("sn-00000000000000001"),
            service_network_name: Some("payments"),
            associated_resource: Some(("rcfg-00000000000000011", false)),
            group: None,
            accessibility: ec2::state::ASSOCIATED_RESOURCE_AVAILABLE,
            failure_code: None,
            failure_reason: None,
            dns: Some(("payments.vpce.example", "Z0000000000001")),
            private_dns: None,
            tags: vec![tag("Name", "payments"), tag("Owner", "TeamA")],
        },
        AssociationRow {
            endpoint: 0,
            service_network: Some("sn-00000000000000001"),
            service_network_name: Some("payments"),
            associated_resource: Some(("rcfg-00000000000000012", false)),
            group: Some("rcfg-group-00000000000000001"),
            accessibility: ec2::state::ASSOCIATED_RESOURCE_PENDING,
            failure_code: None,
            failure_reason: None,
            dns: None,
            private_dns: None,
            tags: Vec::new(),
        },
        AssociationRow {
            endpoint: 0,
            service_network: Some("sn-00000000000000002"),
            service_network_name: Some("orders"),
            associated_resource: Some(("rcfg-00000000000000013", false)),
            group: None,
            accessibility: ec2::state::ASSOCIATED_RESOURCE_FAILED,
            failure_code: Some("ResourceNotAvailable"),
            failure_reason: Some("The resource configuration is not available"),
            dns: None,
            private_dns: None,
            tags: vec![tag("Env", "test"), tag("Name", "orders")],
        },
        AssociationRow {
            endpoint: 1,
            service_network: None,
            service_network_name: None,
            associated_resource: Some(("rcfg-00000000000000014", false)),
            group: Some("rcfg-group-00000000000000002"),
            accessibility: ec2::state::ASSOCIATED_RESOURCE_AVAILABLE,
            failure_code: None,
            failure_reason: None,
            dns: Some(("reports.vpce.example", "Z0000000000002")),
            private_dns: Some(("reports.internal.example", "Z0000000000003")),
            tags: vec![tag("Name", "reports")],
        },
        AssociationRow {
            endpoint: 1,
            service_network: None,
            service_network_name: None,
            // The `<type>:<id>` ARN form, so the `associated-resource-id` filter is
            // exercised on both separators an ARN's resource part may use.
            associated_resource: Some(("rcfg-00000000000000015", true)),
            group: None,
            accessibility: ec2::state::ASSOCIATED_RESOURCE_PENDING,
            failure_code: None,
            failure_reason: None,
            dns: None,
            private_dns: None,
            tags: Vec::new(),
        },
    ]
}

/// The spec one row is written through.
fn spec(row: &AssociationRow, endpoints: &Endpoints) -> VpcEndpointAssociationSpec {
    VpcEndpointAssociationSpec {
        vpc_endpoint_id: endpoints.ids[row.endpoint].clone(),
        service_network_arn: row.service_network.map(service_network_arn),
        service_network_name: row.service_network_name.map(str::to_string),
        associated_resource_arn: row.associated_resource.map(|(id, colon_form)| {
            if colon_form {
                colon_resource_configuration_arn(id)
            } else {
                resource_configuration_arn(id)
            }
        }),
        resource_configuration_group_arn: row.group.map(group_arn),
        associated_resource_accessibility: row.accessibility.to_string(),
        failure_code: row.failure_code.map(str::to_string),
        failure_reason: row.failure_reason.map(str::to_string),
        dns_entry: row.dns.map(|(name, zone)| dns_entry(name, zone)),
        private_dns_entry: row.private_dns.map(|(name, zone)| dns_entry(name, zone)),
        tags: row.tags.clone(),
    }
}

/// Write the seeded endpoints and the associations on them, and report both.
fn seed(engine: &engine::InMemoryEngine) -> (Endpoints, Vec<String>) {
    let endpoints = seed_endpoints(engine);
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let mut ids = Vec::new();
    for (index, row) in seeded_rows().iter().enumerate() {
        ids.push(
            guard
                .insert_vpc_endpoint_association(spec(row, &endpoints))
                .unwrap_or_else(|| {
                    panic!("row {index}: the store must take the seeded association")
                }),
        );
    }
    drop(guard);
    (endpoints, ids)
}

/// The ids at `indices`, in store order.
fn ids_at(seeded: &[String], indices: &[usize]) -> Vec<String> {
    indices.iter().map(|index| seeded[*index].clone()).collect()
}

/// How many associations the store holds.
fn stored_count(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<Ec2State>()
        .lock()
        .vpc_endpoint_associations
        .len()
}

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// Every accessibility the rows pin is one the filter table publishes, so the table above is
/// written against that table rather than against prose.
#[test]
fn every_seeded_accessibility_is_one_the_filter_table_publishes() {
    for row in seeded_rows() {
        let accessibility = row.accessibility;
        assert!(
            ec2::state::ASSOCIATED_RESOURCE_ACCESSIBILITY_FILTER_VALUES
                .iter()
                .any(|(_, reported)| *reported == accessibility),
            "case {accessibility:?}: expected a reported AssociatedResourceAccessibility, got \
             {accessibility:?}"
        );
    }
}

/// An account with no endpoint of a service network or a resource configuration holds no
/// association, so the operation answers HTTP 200 with an empty set — and describing is a
/// READ: two calls report the same thing and the store is untouched.
#[tokio::test]
async fn an_account_with_no_association_reports_an_empty_set_and_describing_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "fresh account: expected HTTP 200, got {status} with body {body}"
    );
    let reported = reported_ids("fresh account", &body);
    assert!(
        reported.is_empty(),
        "fresh account: expected no association, got {reported:?} from {body}"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "fresh account: expected no pagination token, got {body}"
    );

    let again = described_ids(&router, "a second call", &[]).await;
    assert!(
        again.is_empty(),
        "a second call: expected no association, got {again:?}"
    );
    assert_eq!(
        stored_count(&engine),
        0,
        "describing associations must not write to the store"
    );

    // The same answer for an endpoint that EXISTS and has no association: an interface
    // endpoint created through the API has none, which is reported as an empty set rather
    // than as an error.
    let interface_endpoint = create_interface_endpoint(&router).await;
    let narrowed = described_ids(
        &router,
        "an interface endpoint",
        &indexed("VpcEndpointId", &[interface_endpoint.as_str()]),
    )
    .await;
    assert!(
        narrowed.is_empty(),
        "an interface endpoint: expected no association, got {narrowed:?}"
    );
    assert_eq!(
        stored_count(&engine),
        0,
        "describing associations must not write to the store"
    );
}

/// An interface endpoint created through `CreateVpcEndpoint`, in a VPC and subnet created the
/// same way, as its id — an endpoint this emulator really stores, for the "an endpoint type
/// with no association" row above.
async fn create_interface_endpoint(router: &MultiServiceRouter) -> String {
    let case = "an interface endpoint";
    let vpc = super::ec2_query::call(
        router,
        REGION,
        "CreateVpc",
        &[param("CidrBlock", "10.60.0.0/16")],
    )
    .await;
    assert_eq!(
        vpc.0, 200,
        "case {case:?}: expected HTTP 200 from CreateVpc, got {} with body {}",
        vpc.0, vpc.1
    );
    let vpc_id = element(&vpc.1, "vpcId")
        .unwrap_or_else(|| panic!("case {case:?}: no <vpcId> in {}", vpc.1));
    let endpoint = super::ec2_query::call(
        router,
        REGION,
        "CreateVpcEndpoint",
        &[
            param("VpcId", &vpc_id),
            param("VpcEndpointType", "Interface"),
            param("ServiceName", &format!("com.amazonaws.{REGION}.s3")),
            // This endpoint is only something for the association report to be about, so it
            // asks for no private hosted zone: a VPC you create has DNS hostnames off, which
            // is what an endpoint associating one needs.
            param("PrivateDnsEnabled", "false"),
        ],
    )
    .await;
    assert_eq!(
        endpoint.0, 200,
        "case {case:?}: expected HTTP 200 from CreateVpcEndpoint, got {} with body {}",
        endpoint.0, endpoint.1
    );
    element(&endpoint.1, "vpcEndpointId")
        .unwrap_or_else(|| panic!("case {case:?}: no <vpcEndpointId> in {}", endpoint.1))
}

/// Every association the store holds is reported, in store order, with every member of the
/// modelled shape — and one written after a call shows up in the next one, so the answer is
/// computed from the store rather than fixed.
#[tokio::test]
async fn every_stored_association_is_reported_and_a_later_write_is_visible() {
    let (router, engine) = super::ec2_query::emulator();
    let (endpoints, seeded) = seed(&engine);

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "all associations: expected HTTP 200, got {status} with body {body}"
    );
    let reported = reported_ids("all associations", &body);
    assert_eq!(
        reported, seeded,
        "all associations: expected every stored association in store order, got {reported:?}"
    );

    // Every member of the reported shape comes from the stored association.
    for (index, row) in seeded_rows().iter().enumerate() {
        let case = format!("row {index}");
        let mut expected: Vec<(&str, String)> = vec![
            ("id", seeded[index].clone()),
            ("vpcEndpointId", endpoints.ids[row.endpoint].clone()),
            (
                "associatedResourceAccessibility",
                row.accessibility.to_string(),
            ),
        ];
        if let Some(id) = row.service_network {
            expected.push(("serviceNetworkArn", service_network_arn(id)));
        }
        if let Some(name) = row.service_network_name {
            expected.push(("serviceNetworkName", name.to_string()));
        }
        if let Some((id, colon_form)) = row.associated_resource {
            let arn = if colon_form {
                colon_resource_configuration_arn(id)
            } else {
                resource_configuration_arn(id)
            };
            expected.push(("associatedResourceArn", arn));
        }
        if let Some(id) = row.group {
            expected.push(("resourceConfigurationGroupArn", group_arn(id)));
        }
        if let Some(code) = row.failure_code {
            expected.push(("failureCode", code.to_string()));
        }
        if let Some(reason) = row.failure_reason {
            expected.push(("failureReason", reason.to_string()));
        }
        for (member, value) in expected {
            assert!(
                body.contains(&format!("<{member}>{value}</{member}>")),
                "case {case:?}: expected <{member}>{value}</{member}>, got {body}"
            );
        }
        for (set, entry) in [("dnsEntry", row.dns), ("privateDnsEntry", row.private_dns)] {
            match entry {
                None => {}
                Some((dns_name, hosted_zone_id)) => {
                    assert!(
                        body.contains(&format!("<dnsName>{dns_name}</dnsName>")),
                        "case {case:?}: expected the {set} name {dns_name:?} to be reported, \
                         got {body}"
                    );
                    assert!(
                        body.contains(&format!(
                            "<hostedZoneId>{hosted_zone_id}</hostedZoneId>"
                        )),
                        "case {case:?}: expected the {set} hosted zone {hosted_zone_id:?} to \
                         be reported, got {body}"
                    );
                }
            }
        }
        for stored_tag in &row.tags {
            let key = stored_tag.key.clone().unwrap_or_default();
            let value = stored_tag.value.clone().unwrap_or_default();
            assert!(
                body.contains(&format!("<key>{key}</key>")),
                "case {case:?}: expected the tag key {key:?} to be reported, got {body}"
            );
            assert!(
                body.contains(&format!("<value>{value}</value>")),
                "case {case:?}: expected the tag value {value:?} to be reported, got {body}"
            );
        }
    }

    // Each association id is the `vpce-assoc-` form the store mints, never a value a request
    // chose.
    for (index, id) in seeded.iter().enumerate() {
        assert!(
            super::wire::is_long_id(ec2::state::VPC_ENDPOINT_ASSOCIATION_ID_PREFIX, id),
            "row {index}: expected a minted {}- id, got {id:?}",
            ec2::state::VPC_ENDPOINT_ASSOCIATION_ID_PREFIX
        );
    }

    let late_id = engine
        .state::<Ec2State>()
        .lock()
        .insert_vpc_endpoint_association(VpcEndpointAssociationSpec {
            vpc_endpoint_id: endpoints.ids[1].clone(),
            service_network_arn: None,
            service_network_name: None,
            associated_resource_arn: Some(resource_configuration_arn("rcfg-00000000000000099")),
            resource_configuration_group_arn: None,
            associated_resource_accessibility: ec2::state::ASSOCIATED_RESOURCE_AVAILABLE
                .to_string(),
            failure_code: None,
            failure_reason: None,
            dns_entry: None,
            private_dns_entry: None,
            tags: vec![tag("Name", "late")],
        })
        .expect("after a later write: the store must take the association");
    let after = described_ids(&router, "after a later write", &[]).await;
    let mut expected = seeded.clone();
    expected.push(late_id.clone());
    assert_eq!(
        after, expected,
        "after a later write: expected {late_id:?} to be reported too, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Each documented filter selects on its own attribute, values within one filter are OR'd,
/// and the comparison is exact and case-sensitive.
#[tokio::test]
async fn each_documented_filter_selects_the_associations_whose_attribute_equals_a_value() {
    let (router, engine) = super::ec2_query::emulator();
    let (endpoints, seeded) = seed(&engine);

    // One row per filter name `DescribeVpcEndpointAssociationsRequest$Filters` documents,
    // plus rows for several values, a value nothing holds, and the wrong case.
    let cases: Vec<(&str, Vec<String>, Vec<usize>)> = vec![
        (
            "vpc-endpoint-id",
            vec![endpoints.ids[0].clone()],
            vec![0, 1, 2],
        ),
        (
            "vpc-endpoint-id",
            vec![endpoints.ids[1].clone()],
            vec![3, 4],
        ),
        (
            "vpc-endpoint-id",
            vec![endpoints.ids[1].clone(), endpoints.ids[0].clone()],
            vec![0, 1, 2, 3, 4],
        ),
        ("association-id", vec![seeded[3].clone()], vec![3]),
        (
            "association-id",
            vec![seeded[4].clone(), seeded[1].clone()],
            vec![1, 4],
        ),
        (
            "association-id",
            vec![seeded[0].to_uppercase()],
            Vec::new(),
        ),
        // The filter's own spelling of the state, and the value the response reports for it:
        // "When the state is accessible, it returns AVAILABLE. When the state is
        // inaccessible, it returns PENDING or FAILED."
        (
            "associated-resource-accessibility",
            vec!["accessible".to_string()],
            vec![0, 3],
        ),
        (
            "associated-resource-accessibility",
            vec!["inaccessible".to_string()],
            vec![1, 2, 4],
        ),
        (
            "associated-resource-accessibility",
            vec![ec2::state::ASSOCIATED_RESOURCE_AVAILABLE.to_string()],
            vec![0, 3],
        ),
        (
            "associated-resource-accessibility",
            vec![ec2::state::ASSOCIATED_RESOURCE_PENDING.to_string()],
            vec![1, 4],
        ),
        (
            "associated-resource-accessibility",
            vec![ec2::state::ASSOCIATED_RESOURCE_FAILED.to_string()],
            vec![2],
        ),
        (
            "associated-resource-accessibility",
            vec!["Accessible".to_string()],
            Vec::new(),
        ),
        (
            "service-network-arn",
            vec![service_network_arn("sn-00000000000000001")],
            vec![0, 1],
        ),
        (
            "service-network-arn",
            vec![service_network_arn("sn-00000000000000002")],
            vec![2],
        ),
        (
            "service-network-arn",
            vec![service_network_arn("sn-00000000000000009")],
            Vec::new(),
        ),
        (
            "resource-configuration-group-arn",
            vec![group_arn("rcfg-group-00000000000000001")],
            vec![1],
        ),
        (
            "resource-configuration-group-arn",
            vec![group_arn("rcfg-group-00000000000000002")],
            vec![3],
        ),
        // The ID of the associated resource configuration, which is the resource part of its
        // ARN under either separator — never the whole ARN.
        (
            "associated-resource-id",
            vec!["rcfg-00000000000000011".to_string()],
            vec![0],
        ),
        (
            "associated-resource-id",
            vec!["rcfg-00000000000000015".to_string()],
            vec![4],
        ),
        (
            "associated-resource-id",
            vec![
                "rcfg-00000000000000012".to_string(),
                "rcfg-00000000000000014".to_string(),
            ],
            vec![1, 3],
        ),
        (
            "associated-resource-id",
            vec![resource_configuration_arn("rcfg-00000000000000011")],
            Vec::new(),
        ),
    ];

    for (name, values, expected) in cases {
        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        let case = format!("{name}={values:?}");
        let reported = described_ids(&router, &case, &filters(&[(name, &borrowed)])).await;
        assert_eq!(
            reported,
            ids_at(&seeded, &expected),
            "case {case:?}: expected the associations whose {name} matches, got {reported:?}"
        );
    }

    // "If you specify multiple filters, the filters are joined with an AND" (`Filter`).
    let both = filters(&[
        ("vpc-endpoint-id", &[endpoints.ids[0].as_str()]),
        ("associated-resource-accessibility", &["accessible"]),
    ]);
    let reported = described_ids(&router, "two filters", &both).await;
    assert_eq!(
        reported,
        ids_at(&seeded, &[0]),
        "case \"two filters\": expected only the association both filters hold for, got \
         {reported:?}"
    );
}

/// A filter name this operation does not document is refused rather than ignored — including
/// the tag filters a SIBLING describe documents and this one does not, and a documented name
/// in the wrong case.
#[tokio::test]
async fn a_filter_name_this_operation_does_not_document_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    for name in [
        "tag:Name",
        "tag-key",
        "vpc-endpoint-state",
        "Association-Id",
        "association_id",
        "",
    ] {
        let (status, body) = describe(&router, &filters(&[(name, &["anything"])])).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );
        assert!(
            error_message(&body).contains(&format!("The filter '{name}' is invalid")),
            "case {name:?}: the refusal must name the filter, got {body}"
        );
    }
}

/// The request-controlled factors of the store scan are bounded: each of the crate's caps is
/// accepted at its published value and refused one past it, so no request can ask for
/// unbounded filtering work.
#[tokio::test]
async fn the_filter_factors_are_accepted_at_their_caps_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    let (endpoints, _) = seed(&engine);

    let many_filters = |count: usize| -> Vec<Param> {
        let entries: Vec<(&str, &[&str])> = (0..count)
            .map(|_| ("associated-resource-accessibility", &["accessible"][..]))
            .collect();
        filters(&entries)
    };
    let many_values = |count: usize| -> Vec<Param> {
        let values: Vec<String> = (0..count).map(|index| format!("rcfg-{index}")).collect();
        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        filters(&[("associated-resource-id", &borrowed)])
    };
    let one_value_of = |bytes: usize| -> Vec<Param> {
        let value = "a".repeat(bytes);
        filters(&[("associated-resource-id", &[value.as_str()])])
    };

    let cases: Vec<(String, Vec<Param>, u16)> = vec![
        (
            format!("{} filters", ec2::state::MAX_FILTERS),
            many_filters(ec2::state::MAX_FILTERS),
            200,
        ),
        (
            format!("{} filters", ec2::state::MAX_FILTERS + 1),
            many_filters(ec2::state::MAX_FILTERS + 1),
            400,
        ),
        (
            format!("{} filter values", ec2::state::MAX_FILTER_VALUES),
            many_values(ec2::state::MAX_FILTER_VALUES),
            200,
        ),
        (
            format!("{} filter values", ec2::state::MAX_FILTER_VALUES + 1),
            many_values(ec2::state::MAX_FILTER_VALUES + 1),
            400,
        ),
        (
            format!("a {}-byte filter value", ec2::state::MAX_FILTER_VALUE_BYTES),
            one_value_of(ec2::state::MAX_FILTER_VALUE_BYTES),
            200,
        ),
        (
            format!(
                "a {}-byte filter value",
                ec2::state::MAX_FILTER_VALUE_BYTES + 1
            ),
            one_value_of(ec2::state::MAX_FILTER_VALUE_BYTES + 1),
            400,
        ),
    ];

    for (case, params, expected_status) in cases {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "InvalidParameterValue",
                "case {case:?}: expected InvalidParameterValue, got {body}"
            );
            assert!(
                reported_ids(&case, &body).is_empty(),
                "case {case:?}: a refused request reports no association, got {body}"
            );
        }
    }

    // The id-count cap is read BEFORE the ids are resolved, so a list one past it is refused
    // for its size rather than for the endpoints it names, while a list AT the cap gets as
    // far as resolving them.
    let endpoint_ids = |count: usize| -> Vec<Param> {
        let ids: Vec<String> = (0..count)
            .map(|index| {
                if index == 0 {
                    endpoints.ids[0].clone()
                } else {
                    format!("vpce-{index:017x}")
                }
            })
            .collect();
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        indexed("VpcEndpointId", &borrowed)
    };
    for (case, count, expected_code) in [
        (
            format!("{} ids", ec2::state::MAX_RESOURCE_IDS),
            ec2::state::MAX_RESOURCE_IDS,
            "InvalidVpcEndpointId.NotFound",
        ),
        (
            format!("{} ids", ec2::state::MAX_RESOURCE_IDS + 1),
            ec2::state::MAX_RESOURCE_IDS + 1,
            "InvalidParameterValue",
        ),
    ] {
        let (status, body) = describe(&router, &endpoint_ids(count)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// VpcEndpointIds
// ---------------------------------------------------------------------------

/// The ids narrow the report to the associations of the endpoints named, in store order
/// whatever order they were asked for; an id that is no endpoint id at all is malformed, and
/// one that names no endpoint of the account is not found.
#[tokio::test]
async fn the_endpoint_ids_narrow_the_report_to_the_associations_of_those_endpoints() {
    let (router, engine) = super::ec2_query::emulator();
    let (endpoints, seeded) = seed(&engine);

    // Rows differ in count and in the order the ids are asked for.
    let cases: Vec<(Vec<usize>, Vec<usize>)> = vec![
        (vec![0], vec![0, 1, 2]),
        (vec![1], vec![3, 4]),
        (vec![1, 0], vec![0, 1, 2, 3, 4]),
        (vec![0, 0], vec![0, 1, 2]),
    ];
    for (asked, expected) in cases {
        let ids: Vec<&str> = asked
            .iter()
            .map(|index| endpoints.ids[*index].as_str())
            .collect();
        let case = format!("endpoints {asked:?}");
        let reported = described_ids(&router, &case, &indexed("VpcEndpointId", &ids)).await;
        assert_eq!(
            reported,
            ids_at(&seeded, &expected),
            "case {case:?}: expected the associations of the endpoints named, in store order, \
             got {reported:?}"
        );
    }

    // An id that is not a VPC endpoint id at all: refused as malformed, whether or not any
    // association would have matched it.
    for malformed in [
        "vpce-not-an-id",
        "vpc-00000000000000001",
        "vpce-svc-00000000000000001",
        "VPCE-00000000000000001",
        "vpce-",
    ] {
        let (status, body) = describe(&router, &indexed("VpcEndpointId", &[malformed])).await;
        assert_eq!(
            status, 400,
            "case {malformed:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidVpcEndpointId.Malformed",
            "case {malformed:?}: expected InvalidVpcEndpointId.Malformed, got {body}"
        );
        assert!(
            error_message(&body).contains(&format!("Invalid id: \"{malformed}\"")),
            "case {malformed:?}: the refusal must name the id, got {body}"
        );
    }

    // A well-formed id that names no endpoint of the account, alone and beside a stored one:
    // an error rather than an empty report, so a dangling reference is distinguishable from
    // an endpoint with no association.
    let unknown = "vpce-0000000000000beef";
    for (case, asked) in [
        ("an unknown id alone", vec![unknown.to_string()]),
        (
            "an unknown id beside a stored one",
            vec![unknown.to_string(), endpoints.ids[0].clone()],
        ),
    ] {
        let borrowed: Vec<&str> = asked.iter().map(String::as_str).collect();
        let (status, body) = describe(&router, &indexed("VpcEndpointId", &borrowed)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidVpcEndpointId.NotFound",
            "case {case:?}: expected InvalidVpcEndpointId.NotFound, got {body}"
        );
        assert!(
            error_message(&body).contains(&format!("The VPC endpoint ID '{unknown}' does not exist")),
            "case {case:?}: the refusal must name the id, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Whatever the page size, walking the pages covers every association exactly once, in store
/// order, and the last page carries no token.
#[tokio::test]
async fn paging_covers_every_association_exactly_once_for_each_page_size() {
    let (router, engine) = super::ec2_query::emulator();
    let (_, seeded) = seed(&engine);

    // `MaxResults2` is modelled `range 1..100`, so the rows are its published lower bound, a
    // size that divides the store exactly, one that does not, and the upper bound, which
    // holds everything.
    for max_results in [1, 5, 2, 3, 100] {
        let case = format!("MaxResults={max_results}");
        let mut params = max_results_param(max_results);
        let mut walked: Vec<String> = Vec::new();
        let mut pages = 0;
        loop {
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = reported_ids(&case, &body);
            assert!(
                page.len() <= usize::try_from(max_results).unwrap_or_default(),
                "case {case:?}: expected at most {max_results} associations in a page, got \
                 {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= seeded.len() + 1,
                "case {case:?}: paging must terminate, walked {walked:?}"
            );
            match element(&body, "nextToken") {
                Some(token) => {
                    params = max_results_param(max_results);
                    params.push(param("NextToken", &token));
                }
                None => break,
            }
        }
        assert_eq!(
            walked, seeded,
            "case {case:?}: expected every association exactly once in store order, got \
             {walked:?}"
        );
    }
}

/// A page is a cut of the FILTERED set, not of the whole store: `MaxResults` and a filter
/// compose.
#[tokio::test]
async fn a_page_is_cut_from_the_filtered_set() {
    let (router, engine) = super::ec2_query::emulator();
    let (endpoints, seeded) = seed(&engine);

    let page_params = || {
        let mut params = filters(&[("vpc-endpoint-id", &[endpoints.ids[0].as_str()])]);
        params.extend(max_results_param(2));
        params
    };

    let (status, body) = describe(&router, &page_params()).await;
    assert_eq!(
        status, 200,
        "filtered first page: expected HTTP 200, got {status} with body {body}"
    );
    let first = reported_ids("filtered first page", &body);
    assert_eq!(
        first,
        ids_at(&seeded, &[0, 1]),
        "filtered first page: expected the first two associations of that endpoint, got \
         {first:?}"
    );
    let token = element(&body, "nextToken")
        .expect("filtered first page: a third association follows, so a token must be issued");

    let mut next = page_params();
    next.push(param("NextToken", &token));
    let second = described_ids(&router, "filtered second page", &next).await;
    assert_eq!(
        second,
        ids_at(&seeded, &[2]),
        "filtered second page: expected the remaining association of that endpoint, got \
         {second:?}"
    );
}

/// `MaxResults` outside the range the model publishes is refused at the boundary; each
/// published bound itself is accepted.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // The published bounds (`MaxResults2`, `range 1..100`) and one past each.
    for (max_results, expected_status) in [(0, 400), (1, 200), (100, 200), (101, 400)] {
        let (status, body) = describe(&router, &max_results_param(max_results)).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} with \
             body {body}"
        );
    }
}

/// A `NextToken` this service did not issue is refused with `InvalidPaginationToken`, rather
/// than read as a cursor.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let (_, seeded) = seed(&engine);

    // Rows: free text, punctuation, a hex string this service never minted, the bare
    // association id a caller might guess the token is, and that id hex-encoded (the shape
    // the token takes, without the marker and checksum a minted token carries).
    let cases = [
        "not-a-token".to_string(),
        "###".to_string(),
        "6161".to_string(),
        seeded[0].clone(),
        ::hex::encode(&seeded[0]),
        ::hex::encode(format!("ec2-1:0000000000000000:{}", seeded[0])),
    ];
    for token in cases {
        let (status, body) = describe(&router, &[param("NextToken", &token)]).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got {body}"
        );
        assert!(
            error_message(&body).contains(&format!("Next token '{token}' is invalid")),
            "case {token:?}: the refusal must name the refused token, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------

/// `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — and describes
/// nothing; `DryRun=false` is an ordinary call.
#[tokio::test]
async fn dry_run_reports_the_permitted_outcome_without_describing() {
    let (router, engine) = super::ec2_query::emulator();
    let (_, seeded) = seed(&engine);

    let (status, body) = describe(&router, &[param("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "DryRun=true: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "DryRun=true: expected DryRunOperation, got {body}"
    );
    assert!(
        reported_ids("DryRun=true", &body).is_empty(),
        "DryRun=true: a dry run reports no association, got {body}"
    );

    let reported = described_ids(&router, "DryRun=false", &[param("DryRun", "false")]).await;
    assert_eq!(
        reported, seeded,
        "DryRun=false: expected every stored association, got {reported:?}"
    );
}
