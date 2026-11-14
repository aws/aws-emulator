// SPDX-License-Identifier: CC0-1.0

//! The `Cluster` structure every cluster operation reports, end to end over the
//! awsQuery wire.
//!
//! The rule under test: a reported `Cluster` carries every member this service
//! can derive from the cluster's own persisted configuration — where it is (its
//! subnet group, VPC, VPC security groups and the VPC endpoint its connection
//! endpoint is reached through), what it is (its node topology, public key,
//! database revision, encryption key, AQUA configuration and parameter group)
//! and what is scheduled for it (its next maintenance window, its deferred
//! windows, its pending changes) — and the create path, the modify path and the
//! read path report the SAME member set for one cluster.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize. The rule is asserted
//! over a TABLE whose rows differ along what it generalises over: the cluster
//! type and node count, whether the request names a subnet group, how many VPC
//! security groups it names, whether the cluster is encrypted and with whose key,
//! which parameter group it joins, and what it asks of the retired AQUA
//! parameter.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};

/// `call` takes borrowed pairs, so a case that builds owned parameters borrows
/// them through this.
fn borrowed(params: &[(String, String)]) -> Vec<(&str, &str)> {
    params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

/// The INSIDE of the `<Cluster>…</Cluster>` subtree of a response, which is the
/// whole of what this suite compares between the create path and the read path.
///
/// The open tag itself is left out: the serializer puts the service's XML
/// namespace on it in one envelope and not in the other, and the namespace is
/// not part of the cluster's structure.
fn cluster_block(xml: &str) -> String {
    // Matched as a whole element name, so the `<Clusters>` list wrapper of a
    // `DescribeClusters` body cannot be mistaken for the `<Cluster>` inside it.
    let start = xml
        .find("<Cluster>")
        .or_else(|| xml.find("<Cluster "))
        .unwrap_or_else(|| panic!("the response carries a Cluster element: {xml}"));
    let open_end = xml[start..]
        .find('>')
        .map(|offset| start + offset + 1)
        .unwrap_or_else(|| panic!("the Cluster element's open tag ends: {xml}"));
    let end = xml
        .find("</Cluster>")
        .expect("the response carries a closing Cluster element");
    xml[open_end..end].to_string()
}

/// The two texts around the first byte at which they differ, so a failure names
/// the member that diverged rather than printing two whole documents.
fn first_difference(left: &str, right: &str) -> (String, String) {
    let common = left
        .bytes()
        .zip(right.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    let window = |text: &str| {
        let from = common.saturating_sub(40).min(text.len());
        let to = (common + 80).min(text.len());
        text.get(from..to).unwrap_or(text).to_string()
    };
    (window(left), window(right))
}

/// One row of the structure table.
struct Case {
    /// The case name, named in every assertion of the row.
    case: &'static str,
    /// The `CreateCluster` parameters beyond the three required ones.
    params: Vec<(String, String)>,
    /// The `ClusterNode$NodeRole` of every node, in the order they must come
    /// back.
    node_roles: Vec<String>,
    /// The `ClusterSubnetGroupName` the cluster must report.
    subnet_group: String,
    /// The `VpcSecurityGroupId` of every VPC security group it must report, or
    /// EMPTY for a row that names none — which must report the default security
    /// group of its own VPC, the VPC its subnet group's subnets are in.
    security_groups: Vec<String>,
    /// The `KmsKeyId` it must report, or `None` when it must report none.
    kms_key_id: Option<String>,
    /// The `ParameterGroupName` of the parameter group it joined.
    parameter_group: String,
}

/// The compute nodes of a multi-node cluster, in the order Redshift numbers
/// them: the leader node first, then `count` compute nodes from zero.
fn multi_node_roles(count: usize) -> Vec<String> {
    std::iter::once(state::NODE_ROLE_LEADER.to_string())
        .chain((0..count).map(|index| format!("COMPUTE-{index}")))
        .collect()
}

/// The three members every `CreateCluster` must carry, for one identifier.
fn required(identifier: &str) -> Vec<(String, String)> {
    vec![
        ("ClusterIdentifier".to_string(), identifier.to_string()),
        ("NodeType".to_string(), "ra3.xlplus".to_string()),
        ("MasterUsername".to_string(), "admin".to_string()),
    ]
}

/// The admin password the rows that carry one use: it must never reach the wire,
/// not even as the pending change that reports it.
const PASSWORD: &str = "Structur3-Passw0rd";

#[tokio::test]
async fn a_reported_cluster_carries_every_member_derived_from_its_configuration() {
    let emulator = Emulator::new();

    let default_subnet_group = state::DEFAULT_CLUSTER_SUBNET_GROUP.to_string();

    let cases: Vec<Case> = vec![
        Case {
            case: "a single-node cluster is its own shared node",
            params: vec![("ClusterType".to_string(), "single-node".to_string())],
            node_roles: vec![state::NODE_ROLE_SHARED.to_string()],
            subnet_group: default_subnet_group.clone(),
            security_groups: Vec::new(),
            // "Default: true" is this service's documented encryption default,
            // and an encrypted cluster that named no key is encrypted under the
            // key Amazon Web Services owns.
            kms_key_id: Some(state::AWS_OWNED_KMS_KEY.to_string()),
            parameter_group: state::cluster_default_parameter_group(),
        },
        Case {
            case: "a two-node cluster has a leader and two compute nodes",
            params: vec![
                ("ClusterType".to_string(), "multi-node".to_string()),
                ("NumberOfNodes".to_string(), "2".to_string()),
            ],
            node_roles: multi_node_roles(2),
            subnet_group: default_subnet_group.clone(),
            security_groups: Vec::new(),
            kms_key_id: Some(state::AWS_OWNED_KMS_KEY.to_string()),
            parameter_group: state::cluster_default_parameter_group(),
        },
        Case {
            case: "an eight-node cluster numbers every compute node",
            params: vec![
                ("ClusterType".to_string(), "multi-node".to_string()),
                ("NumberOfNodes".to_string(), "8".to_string()),
            ],
            node_roles: multi_node_roles(8),
            subnet_group: default_subnet_group.clone(),
            security_groups: Vec::new(),
            kms_key_id: Some(state::AWS_OWNED_KMS_KEY.to_string()),
            parameter_group: state::cluster_default_parameter_group(),
        },
        Case {
            case: "a named subnet group puts the cluster in that group's VPC",
            params: vec![(
                "ClusterSubnetGroupName".to_string(),
                "analytics-subnets".to_string(),
            )],
            node_roles: vec![state::NODE_ROLE_SHARED.to_string()],
            subnet_group: "analytics-subnets".to_string(),
            security_groups: Vec::new(),
            kms_key_id: Some(state::AWS_OWNED_KMS_KEY.to_string()),
            parameter_group: state::cluster_default_parameter_group(),
        },
        Case {
            case: "two named VPC security groups are both reported",
            params: vec![
                (
                    "VpcSecurityGroupIds.VpcSecurityGroupId.1".to_string(),
                    "sg-0123456789abcdef0".to_string(),
                ),
                (
                    "VpcSecurityGroupIds.VpcSecurityGroupId.2".to_string(),
                    "sg-00fedcba9876543210".to_string(),
                ),
            ],
            node_roles: vec![state::NODE_ROLE_SHARED.to_string()],
            subnet_group: default_subnet_group.clone(),
            security_groups: vec![
                "sg-0123456789abcdef0".to_string(),
                "sg-00fedcba9876543210".to_string(),
            ],
            kms_key_id: Some(state::AWS_OWNED_KMS_KEY.to_string()),
            parameter_group: state::cluster_default_parameter_group(),
        },
        Case {
            case: "an unencrypted cluster reports no encryption key",
            params: vec![("Encrypted".to_string(), "false".to_string())],
            node_roles: vec![state::NODE_ROLE_SHARED.to_string()],
            subnet_group: default_subnet_group.clone(),
            security_groups: Vec::new(),
            kms_key_id: None,
            parameter_group: state::cluster_default_parameter_group(),
        },
        Case {
            case: "a customer managed key is reported as named",
            params: vec![(
                "KmsKeyId".to_string(),
                "arn:aws:kms:us-west-2:123456789012:key/7c9eb2f9-0000-4000-8000-0123456789ab"
                    .to_string(),
            )],
            node_roles: vec![state::NODE_ROLE_SHARED.to_string()],
            subnet_group: default_subnet_group.clone(),
            security_groups: Vec::new(),
            kms_key_id: Some(
                "arn:aws:kms:us-west-2:123456789012:key/7c9eb2f9-0000-4000-8000-0123456789ab"
                    .to_string(),
            ),
            parameter_group: state::cluster_default_parameter_group(),
        },
        Case {
            case: "a named parameter group is the group the cluster joins",
            params: vec![
                (
                    "ClusterParameterGroupName".to_string(),
                    "structure-group".to_string(),
                ),
                // "This parameter is retired. It does not set the AQUA
                // configuration status", so asking for `enabled` changes
                // nothing the cluster reports.
                ("AquaConfigurationStatus".to_string(), "enabled".to_string()),
            ],
            node_roles: vec![state::NODE_ROLE_SHARED.to_string()],
            subnet_group: default_subnet_group.clone(),
            security_groups: Vec::new(),
            kms_key_id: Some(state::AWS_OWNED_KMS_KEY.to_string()),
            parameter_group: "structure-group".to_string(),
        },
    ];

    // The group the last row names must exist before a cluster joins it, so the
    // round trip through `DescribeClusterParameterGroups` can find it.
    let created_group = emulator
        .call(
            "CreateClusterParameterGroup",
            &[
                ("ParameterGroupName", "structure-group"),
                (
                    "ParameterGroupFamily",
                    state::DEFAULT_PARAMETER_GROUP_FAMILY,
                ),
                ("Description", "the group one row names"),
            ],
        )
        .await;
    assert_eq!(
        created_group.status, 200,
        "case seed structure-group: expected 200, got {} with body {}",
        created_group.status, created_group.body
    );

    // Likewise the subnet group one row names: a cluster is only launched into a
    // group the account holds, over subnets EC2 minted for it.
    let account_subnets = emulator.subnet_ids("seed analytics-subnets", 1).await;
    let created_subnet_group = emulator
        .call(
            "CreateClusterSubnetGroup",
            &[
                ("ClusterSubnetGroupName", "analytics-subnets"),
                ("Description", "the subnets one row names"),
                ("SubnetIds.SubnetIdentifier.1", &account_subnets[0]),
            ],
        )
        .await;
    assert_eq!(
        created_subnet_group.status, 200,
        "case seed analytics-subnets: expected 200, got {} with body {}",
        created_subnet_group.status, created_subnet_group.body
    );

    for (index, case) in cases.iter().enumerate() {
        let identifier = format!("structure-{index:02}");
        let mut params = required(&identifier);
        params.push(("MasterUserPassword".to_string(), PASSWORD.to_string()));
        params.extend(case.params.iter().cloned());
        let created = emulator.call("CreateCluster", &borrowed(&params)).await;
        assert_eq!(
            created.status, 200,
            "case {}: expected 200, got {} with body {}",
            case.case, created.status, created.body
        );

        // The VPC the cluster must report is the one its subnet group reports:
        // asked of the group itself rather than computed here.
        let group = emulator
            .call(
                "DescribeClusterSubnetGroups",
                &[("ClusterSubnetGroupName", case.subnet_group.as_str())],
            )
            .await;
        let subnet_group_vpc = group.text("VpcId").unwrap_or_else(|| {
            panic!(
                "case {}: the subnet group {} must report the VPC its subnets are in, got body {}",
                case.case, case.subnet_group, group.body
            )
        });
        assert_structure(&created, case, &identifier, &subnet_group_vpc);

        // The read path reports the SAME structure the create path did: the
        // whole `Cluster` subtree, member for member.
        let described = emulator
            .call("DescribeClusters", &[("ClusterIdentifier", &identifier)])
            .await;
        assert_eq!(
            described.status, 200,
            "case {} read back: expected 200, got {} with body {}",
            case.case, described.status, described.body
        );
        let created_block = cluster_block(&created.body);
        let described_block = cluster_block(&described.body);
        assert!(
            described_block == created_block,
            "case {} read back: expected the created structure; first difference created={:?} described={:?}",
            case.case,
            first_difference(&created_block, &described_block).0,
            first_difference(&created_block, &described_block).1
        );

        // The group the cluster says it joined is a group this service holds, so
        // a follow-up lookup of it finds one.
        let group = emulator
            .call(
                "DescribeClusterParameterGroups",
                &[("ParameterGroupName", case.parameter_group.as_str())],
            )
            .await;
        assert_eq!(
            (group.status, group.text("ParameterGroupName").as_deref()),
            (200, Some(case.parameter_group.as_str())),
            "case {}: expected the parameter group {} to exist, got {} with body {}",
            case.case,
            case.parameter_group,
            group.status,
            group.body
        );
    }

    // Once every row's cluster is serving it publishes its connection endpoint:
    // the hostname that carries its own name, the port it listens on and the VPC
    // interface endpoint it is reached through, whose network interface is in the
    // cluster's own Availability Zone.
    emulator.advance_clock(state::PROVISIONING_SPAN);
    for (index, case) in cases.iter().enumerate() {
        let name = case.case;
        let identifier = format!("structure-{index:02}");
        let serving = emulator
            .call("DescribeClusters", &[("ClusterIdentifier", &identifier)])
            .await;
        let address = serving
            .text("Address")
            .unwrap_or_else(|| panic!("case {name}: expected an endpoint address once serving, got {}", serving.body));
        assert!(
            address.starts_with(&format!("{identifier}."))
                && address.ends_with(".redshift.amazonaws.com"),
            "case {name}: expected a Redshift endpoint host for the identifier, got {address:?}"
        );
        assert_eq!(
            serving.text("Port").as_deref(),
            Some(state::DEFAULT_PORT.to_string().as_str()),
            "case {name}: expected the default port, got {:?}",
            serving.text("Port")
        );
        let endpoint_id = serving
            .text("VpcEndpointId")
            .unwrap_or_else(|| panic!("case {name}: expected a VpcEndpointId, got {}", serving.body));
        assert!(
            endpoint_id.starts_with("vpce-"),
            "case {name}: expected a VPC endpoint id, got {endpoint_id:?}"
        );
        let interface_id = serving
            .text("NetworkInterfaceId")
            .unwrap_or_else(|| panic!("case {name}: expected a NetworkInterfaceId"));
        let subnet_id = serving
            .text("SubnetId")
            .unwrap_or_else(|| panic!("case {name}: expected a SubnetId"));
        assert!(
            interface_id.starts_with("eni-") && subnet_id.starts_with("subnet-"),
            "case {name}: expected an eni/subnet pair, got {interface_id:?} / {subnet_id:?}"
        );
        let zones = all_text(&serving.body, "AvailabilityZone");
        assert!(
            zones.len() == 2 && zones[0] == zones[1],
            "case {name}: expected the endpoint interface to be in the cluster's zone, got {zones:?}"
        );
        let reported_vpcs = all_text(&serving.body, "VpcId");
        // The VPC the endpoint is in is the cluster's own: the VPC its subnet
        // group reports, asked of that group rather than computed here.
        let group = emulator
            .call(
                "DescribeClusterSubnetGroups",
                &[("ClusterSubnetGroupName", case.subnet_group.as_str())],
            )
            .await;
        let vpc_id = group.text("VpcId").unwrap_or_else(|| {
            panic!(
                "case {name}: the subnet group {} must report the VPC its subnets are in, got \
                 body {}",
                case.subnet_group, group.body
            )
        });
        assert!(
            reported_vpcs.len() == 2 && reported_vpcs.iter().all(|reported| *reported == vpc_id),
            "case {name}: expected the endpoint to be in the cluster's VPC {vpc_id:?}, got \
             {reported_vpcs:?}"
        );
    }
}

/// Assert the whole rule on one reported cluster: the row's own expectations,
/// then every member the rule says a cluster always carries.
///
/// `subnet_group_vpc` is the `VpcId` the cluster's own subnet group reports — the
/// VPC its subnets belong to, as the service that owns subnets answered for them
/// — which is the VPC the cluster must report too.
fn assert_structure(answer: &Answer, case: &Case, identifier: &str, subnet_group_vpc: &str) {
    let name = case.case;

    assert_eq!(
        all_text(&answer.body, "NodeRole"),
        case.node_roles,
        "case {name}: expected the node roles {:?}, got {:?}",
        case.node_roles,
        all_text(&answer.body, "NodeRole")
    );
    // Every node reports both of its addresses, and no two nodes share one.
    let private: Vec<String> = all_text(&answer.body, "PrivateIPAddress");
    let public: Vec<String> = all_text(&answer.body, "PublicIPAddress");
    assert_eq!(
        (private.len(), public.len()),
        (case.node_roles.len(), case.node_roles.len()),
        "case {name}: expected one private and one public address per node, got {private:?} / {public:?}"
    );
    let mut distinct = private.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        private.len(),
        "case {name}: expected a distinct private address per node, got {private:?}"
    );
    for address in private.iter().chain(public.iter()) {
        let octets: Vec<&str> = address.split('.').collect();
        assert!(
            octets.len() == 4 && octets.iter().all(|octet| octet.parse::<u8>().is_ok()),
            "case {name}: expected a dotted-quad node address, got {address:?}"
        );
    }

    assert_eq!(
        answer.text("ClusterSubnetGroupName").as_deref(),
        Some(case.subnet_group.as_str()),
        "case {name}: expected the subnet group {:?}, got {:?}",
        case.subnet_group,
        answer.text("ClusterSubnetGroupName")
    );
    // The cluster's VPC is its subnet group's VPC, and the VPC its connection
    // endpoint is reached through is the same one.
    let vpc_id = subnet_group_vpc;
    let reported_vpcs = all_text(&answer.body, "VpcId");
    assert!(
        !reported_vpcs.is_empty() && reported_vpcs.iter().all(|reported| reported == vpc_id),
        "case {name}: expected every VpcId to be {vpc_id:?}, got {reported_vpcs:?}"
    );

    // The VPC security groups: the ones the row named, or the default security
    // group of the cluster's own VPC when it named none.
    let expected_groups: Vec<String> = if case.security_groups.is_empty() {
        vec![state::default_vpc_security_group_id(subnet_group_vpc)]
    } else {
        case.security_groups.clone()
    };
    assert_eq!(
        all_text(&answer.body, "VpcSecurityGroupId"),
        expected_groups,
        "case {name}: expected the VPC security groups {expected_groups:?}, got {:?}",
        all_text(&answer.body, "VpcSecurityGroupId")
    );

    assert_eq!(
        answer.text("KmsKeyId"),
        case.kms_key_id,
        "case {name}: expected KmsKeyId {:?}, got {:?}",
        case.kms_key_id,
        answer.text("KmsKeyId")
    );

    assert_eq!(
        all_text(&answer.body, "ParameterGroupName"),
        vec![case.parameter_group.clone()],
        "case {name}: expected the parameter group {:?}, got {:?}",
        case.parameter_group,
        all_text(&answer.body, "ParameterGroupName")
    );

    // The SSH public key of the cluster, in the form Redshift reports.
    let public_key = answer
        .text("ClusterPublicKey")
        .unwrap_or_else(|| panic!("case {name}: expected a ClusterPublicKey, got {}", answer.body));
    assert!(
        public_key.starts_with("ssh-rsa ") && public_key.ends_with(" Amazon-Redshift"),
        "case {name}: expected an ssh-rsa public key for Amazon-Redshift, got {public_key:?}"
    );

    // The database revision: a number, and the same cluster always reports the
    // same one.
    let revision = answer
        .text("ClusterRevisionNumber")
        .unwrap_or_else(|| panic!("case {name}: expected a ClusterRevisionNumber"));
    assert!(
        revision.chars().all(|digit| digit.is_ascii_digit()) && !revision.is_empty(),
        "case {name}: expected a numeric ClusterRevisionNumber, got {revision:?}"
    );

    // The managed VPC endpoint the cluster is reached through is part of the
    // connection endpoint, and a cluster still being provisioned publishes
    // NONE of it: the whole endpoint appears only once the cluster is serving,
    // which the loop above asserts past the provisioning span.
    assert_eq!(
        answer.text("VpcEndpointId"),
        None,
        "case {name}: expected no VPC endpoint while the cluster is creating, got {:?}",
        answer.text("VpcEndpointId")
    );
    assert_eq!(
        answer.text("Address"),
        None,
        "case {name}: expected no endpoint address while the cluster is creating, got {:?}",
        answer.text("Address")
    );
    let zones = all_text(&answer.body, "AvailabilityZone");
    assert!(
        zones.len() == 1,
        "case {name}: expected only the cluster's own zone while it is creating, got {zones:?}"
    );

    // The retired AQUA configuration: the automatic configuration status, with
    // AQUA not in use, whatever the request asked for.
    assert_eq!(
        (
            answer.text("AquaConfigurationStatus").as_deref(),
            answer.text("AquaStatus").as_deref()
        ),
        (Some("auto"), Some("disabled")),
        "case {name}: expected the automatic AQUA configuration, got {:?} / {:?}",
        answer.text("AquaConfigurationStatus"),
        answer.text("AquaStatus")
    );

    // A cluster still coming up is only intermittently available for queries.
    assert_eq!(
        answer.text("ClusterAvailabilityStatus").as_deref(),
        Some(state::availability_status(state::STATUS_CREATING)),
        "case {name}: expected ClusterAvailabilityStatus={}, got {:?}",
        state::availability_status(state::STATUS_CREATING),
        answer.text("ClusterAvailabilityStatus")
    );

    // The next maintenance window start is the next occurrence of the cluster's
    // own window, so it is in the future.
    let window = answer
        .text("PreferredMaintenanceWindow")
        .unwrap_or_else(|| panic!("case {name}: expected a PreferredMaintenanceWindow"));
    let start = answer
        .text("NextMaintenanceWindowStartTime")
        .unwrap_or_else(|| {
            panic!("case {name}: expected a NextMaintenanceWindowStartTime, got {}", answer.body)
        });
    let hhmm = window
        .split('-')
        .next()
        .and_then(|from| from.split_once(':').map(|(_, time)| time.to_string()))
        .unwrap_or_default();
    assert!(
        start.contains(&format!("T{hhmm}")),
        "case {name}: expected the next window start at {hhmm} of the window {window:?}, got {start:?}"
    );

    // The members a cluster with nothing deferred and nothing pending still
    // reports: an empty group of deferred windows, and the pending admin
    // password change this create asked for — as the mask, never the password.
    assert!(
        answer.body.contains("DeferredMaintenanceWindows"),
        "case {name}: expected a DeferredMaintenanceWindows element, got {}",
        answer.body
    );
    assert_eq!(
        answer.text("MasterUserPassword").as_deref(),
        Some(state::REDACTED_MASTER_USER_PASSWORD),
        "case {name}: expected the pending password change to be masked, got {:?}",
        answer.text("MasterUserPassword")
    );
    assert!(
        !answer.body.contains(PASSWORD),
        "case {name}: the admin password must never reach the wire, got {}",
        answer.body
    );

    assert_eq!(
        answer.text("ClusterIdentifier").as_deref(),
        Some(identifier),
        "case {name}: expected the identifier back, got {:?}",
        answer.text("ClusterIdentifier")
    );
}

#[tokio::test]
async fn the_availability_status_is_the_published_mapping_of_the_cluster_status() {
    // Every `ClusterStatus` the model publishes, from `smithy_query(query="doc",
    // shape="Cluster", member="ClusterStatus")`, against the
    // `ClusterAvailabilityStatus` the five values of `smithy_query(query="doc",
    // shape="Cluster", member="ClusterAvailabilityStatus")` give it. The rows
    // include statuses this emulator never mints, which is the point: the
    // mapping is over the whole published vocabulary.
    let cases = [
        ("available", "Available"),
        ("available, prep-for-resize", "Available"),
        ("available, resize-cleanup", "Available"),
        ("cancelling-resize", "Modifying"),
        ("creating", "Modifying"),
        ("deleting", "Unavailable"),
        ("final-snapshot", "Modifying"),
        ("hardware-failure", "Failed"),
        ("incompatible-hsm", "Failed"),
        ("incompatible-network", "Failed"),
        ("incompatible-parameters", "Failed"),
        ("incompatible-restore", "Failed"),
        ("modifying", "Modifying"),
        ("paused", "Unavailable"),
        ("rebooting", "Modifying"),
        ("renaming", "Modifying"),
        ("resizing", "Modifying"),
        ("rotating-keys", "Modifying"),
        ("storage-full", "Unavailable"),
        ("updating-hsm", "Modifying"),
        // A status no published vocabulary holds is not available for queries.
        ("something-nobody-published", "Unavailable"),
    ];
    for (status, expected) in cases {
        assert_eq!(
            state::availability_status(status),
            expected,
            "case status {status}: expected {expected}, got {}",
            state::availability_status(status)
        );
    }

    // End to end: the status a cluster reports and the availability status beside
    // it move together, across the clock-owed transition and into a delete.
    let emulator = Emulator::new();
    let created = emulator.create_minimal("availability-demo").await;
    assert_eq!(
        (
            created.text("ClusterStatus").as_deref(),
            created.text("ClusterAvailabilityStatus").as_deref()
        ),
        (Some("creating"), Some("Modifying")),
        "case a cluster coming up: expected creating / Modifying, got {:?} / {:?}",
        created.text("ClusterStatus"),
        created.text("ClusterAvailabilityStatus")
    );

    emulator.advance_clock(state::PROVISIONING_SPAN);
    let settled = emulator
        .call(
            "DescribeClusters",
            &[("ClusterIdentifier", "availability-demo")],
        )
        .await;
    assert_eq!(
        (
            settled.text("ClusterStatus").as_deref(),
            settled.text("ClusterAvailabilityStatus").as_deref()
        ),
        (Some("available"), Some("Available")),
        "case a settled cluster: expected available / Available, got {:?} / {:?}",
        settled.text("ClusterStatus"),
        settled.text("ClusterAvailabilityStatus")
    );
    // The password change the create reported as pending was applied by the time
    // the cluster is available, so nothing is pending on it any more.
    assert_eq!(
        settled.text("MasterUserPassword"),
        None,
        "case a settled cluster: expected no pending password change, got {:?}",
        settled.text("MasterUserPassword")
    );

    let deleted = emulator
        .call(
            "DeleteCluster",
            &[
                ("ClusterIdentifier", "availability-demo"),
                ("SkipFinalClusterSnapshot", "true"),
            ],
        )
        .await;
    assert_eq!(
        (
            deleted.text("ClusterStatus").as_deref(),
            deleted.text("ClusterAvailabilityStatus").as_deref()
        ),
        (Some("deleting"), Some("Unavailable")),
        "case a deleted cluster: expected deleting / Unavailable, got {:?} / {:?}",
        deleted.text("ClusterStatus"),
        deleted.text("ClusterAvailabilityStatus")
    );
}

#[tokio::test]
async fn the_next_maintenance_window_start_is_the_next_occurrence_of_the_window() {
    // The window form is `ddd:hh24:mi-ddd:hh24:mi` (`smithy_query(query="doc",
    // shape="CreateClusterMessage", member="PreferredMaintenanceWindow")`), so
    // the rows walk every weekday the form admits, both bounds of the clock, and
    // the forms it does not admit.
    //
    // 2026-01-01T00:00:00Z — a Thursday — is the instant every row measures
    // from, so the expected weekday and the expected distance are both exact.
    let thursday = aws_smithy_types::DateTime::from_secs(1_767_225_600);
    const SECONDS_PER_DAY: i64 = 24 * 60 * 60;

    // (case, window, the days ahead of the Thursday the start must fall on, the
    // second of the day it must fall at)
    let cases: Vec<(&str, &str, i64, i64)> = vec![
        ("the same weekday, later in the day", "thu:05:30-thu:06:00", 0, 5 * 3600 + 1800),
        ("the next day", "fri:00:00-fri:00:30", 1, 0),
        ("two days ahead", "sat:23:59-sun:00:29", 2, 23 * 3600 + 59 * 60),
        ("three days ahead", "sun:12:00-sun:12:30", 3, 12 * 3600),
        ("four days ahead", "mon:01:00-mon:01:30", 4, 3600),
        ("five days ahead", "tue:18:45-tue:19:15", 5, 18 * 3600 + 45 * 60),
        ("six days ahead", "wed:06:15-wed:06:45", 6, 6 * 3600 + 15 * 60),
        // The window opens exactly at the instant measured from, so the next
        // occurrence is the week after.
        ("exactly now rolls on a week", "thu:00:00-thu:00:30", 7, 0),
    ];
    for (case, window, days_ahead, second_of_day) in cases {
        let start = state::next_maintenance_window_start(window, thursday)
            .unwrap_or_else(|| panic!("case {case}: expected a next start for {window:?}"));
        assert_eq!(
            start.secs(),
            thursday.secs() + days_ahead * SECONDS_PER_DAY + second_of_day,
            "case {case}: expected the next {window:?} start, got {}",
            start.secs()
        );
        assert!(
            start.secs() > thursday.secs()
                && start.secs() <= thursday.secs() + 7 * SECONDS_PER_DAY,
            "case {case}: expected a start inside the next week, got {}",
            start.secs()
        );
    }

    // A window the documented form does not admit reports no next start rather
    // than a made-up one — including a field that is not inside its own range,
    // which must be refused BEFORE it reaches the arithmetic.
    for (case, window) in [
        ("no day at all", "05:30-06:00"),
        ("a day nobody names", "xyz:05:30-xyz:06:00"),
        ("an hour past the clock", "thu:24:00-thu:24:30"),
        ("a minute past the clock", "thu:05:60-thu:06:30"),
        ("a negative hour", "thu:-1:00-thu:01:00"),
        ("a negative minute", "thu:05:-1-thu:06:00"),
        (
            "an hour that would overflow the arithmetic",
            "thu:-9000000000000000000:00-thu:06:00",
        ),
        (
            "a minute that would overflow the arithmetic",
            "thu:05:9223372036854775807-thu:06:00",
        ),
        ("not a time", "thu:ab:cd-thu:ef:gh"),
        ("nothing at all", ""),
    ] {
        assert_eq!(
            state::next_maintenance_window_start(window, thursday),
            None,
            "case {case}: expected no next start for {window:?}, got {:?}",
            state::next_maintenance_window_start(window, thursday)
        );
    }

    // End to end: a cluster created with a window reports the next start of THAT
    // window, and a modify that moves the window moves the start with it.
    let emulator = Emulator::new();
    let created = emulator
        .call(
            "CreateCluster",
            &[
                ("ClusterIdentifier", "window-demo"),
                ("NodeType", "ra3.xlplus"),
                ("MasterUsername", "admin"),
                ("PreferredMaintenanceWindow", "tue:05:30-tue:06:00"),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case create window-demo: expected 200, got {} with body {}",
        created.status, created.body
    );
    let start = created
        .text("NextMaintenanceWindowStartTime")
        .expect("case create window-demo: expected a NextMaintenanceWindowStartTime");
    assert!(
        start.contains("T05:30"),
        "case create window-demo: expected a start at 05:30, got {start:?}"
    );

    let modified = emulator
        .call(
            "ModifyCluster",
            &[
                ("ClusterIdentifier", "window-demo"),
                ("PreferredMaintenanceWindow", "sat:21:15-sat:21:45"),
            ],
        )
        .await;
    let moved = modified
        .text("NextMaintenanceWindowStartTime")
        .expect("case modify window-demo: expected a NextMaintenanceWindowStartTime");
    assert!(
        moved.contains("T21:15"),
        "case modify window-demo: expected a start at 21:15, got {moved:?}"
    );
    // And the read path agrees with the modify that moved it.
    let described = emulator
        .call("DescribeClusters", &[("ClusterIdentifier", "window-demo")])
        .await;
    assert_eq!(
        described.text("NextMaintenanceWindowStartTime").as_deref(),
        Some(moved.as_str()),
        "case read window-demo: expected the moved start {moved:?}, got {:?}",
        described.text("NextMaintenanceWindowStartTime")
    );
}

#[tokio::test]
async fn relocation_is_reported_as_the_request_asked_and_on_by_default() {
    // `AvailabilityZoneRelocation` is "the option to enable relocation for an
    // Amazon Redshift cluster between Availability Zones", and
    // `AvailabilityZoneRelocationStatus` reports it. Each row is a request, and
    // the status the cluster must report for it.
    let emulator = Emulator::new();
    let cases: Vec<(&str, Option<&str>, &str)> = vec![
        ("a silent request", None, "enabled"),
        ("relocation asked for", Some("true"), "enabled"),
        ("relocation turned off", Some("false"), "disabled"),
    ];
    for (index, (case, requested, expected)) in cases.iter().enumerate() {
        let identifier = format!("relocation-{index:02}");
        let mut params = vec![
            ("ClusterIdentifier", identifier.as_str()),
            ("NodeType", "ra3.xlplus"),
            ("MasterUsername", "admin"),
        ];
        if let Some(value) = requested {
            params.push(("AvailabilityZoneRelocation", value));
        }
        let created = emulator.call("CreateCluster", &params).await;
        assert_eq!(
            created.text("AvailabilityZoneRelocationStatus").as_deref(),
            Some(*expected),
            "case {case}: expected AvailabilityZoneRelocationStatus={expected}, got {:?}",
            created.text("AvailabilityZoneRelocationStatus")
        );

        // A modify that names the option moves the status; the read path follows.
        let flipped = if *expected == "enabled" { "false" } else { "true" };
        let modified = emulator
            .call(
                "ModifyCluster",
                &[
                    ("ClusterIdentifier", identifier.as_str()),
                    ("AvailabilityZoneRelocation", flipped),
                ],
            )
            .await;
        let after = if flipped == "true" { "enabled" } else { "disabled" };
        assert_eq!(
            modified.text("AvailabilityZoneRelocationStatus").as_deref(),
            Some(after),
            "case {case} modified to {flipped}: expected {after}, got {:?}",
            modified.text("AvailabilityZoneRelocationStatus")
        );
        let described = emulator
            .call(
                "DescribeClusters",
                &[("ClusterIdentifier", identifier.as_str())],
            )
            .await;
        assert_eq!(
            described.text("AvailabilityZoneRelocationStatus").as_deref(),
            Some(after),
            "case {case} read back: expected {after}, got {:?}",
            described.text("AvailabilityZoneRelocationStatus")
        );
    }
}

#[tokio::test]
async fn a_cluster_is_refused_above_the_published_node_count_bound() {
    // The node topology a cluster reports is one record per node, so the node
    // count a request may name is BOUNDED: `MAX_NODES_PER_CLUSTER` nodes, above
    // which every operation that writes a cluster answers its modelled
    // `NumberOfNodesPerClusterLimitExceededFault` (awsQuery code
    // `NumberOfNodesPerClusterLimitExceeded`, HTTP 400) and writes nothing.
    //
    // The rows walk the bound: inside it, exactly on it, one past it, and the
    // largest value the modelled `Integer` admits.
    let emulator = Emulator::new();
    let cases: Vec<(&str, String, bool)> = vec![
        ("well inside the bound", "4".to_string(), true),
        (
            "exactly on the bound",
            state::MAX_NODES_PER_CLUSTER.to_string(),
            true,
        ),
        (
            "one past the bound",
            (state::MAX_NODES_PER_CLUSTER + 1).to_string(),
            false,
        ),
        ("the largest Integer", i32::MAX.to_string(), false),
    ];

    for (index, (case, nodes, accepted)) in cases.iter().enumerate() {
        let identifier = format!("nodebound-{index:02}");
        let created = emulator
            .call(
                "CreateCluster",
                &[
                    ("ClusterIdentifier", identifier.as_str()),
                    ("NodeType", "ra3.xlplus"),
                    ("MasterUsername", "admin"),
                    ("ClusterType", "multi-node"),
                    ("NumberOfNodes", nodes.as_str()),
                ],
            )
            .await;
        if *accepted {
            assert_eq!(
                created.status, 200,
                "case create {case}: expected 200, got {} with body {}",
                created.status, created.body
            );
            // One leader node plus the compute nodes the request asked for.
            let expected: usize = nodes.parse::<usize>().expect("a node count") + 1;
            assert_eq!(
                all_text(&created.body, "NodeRole").len(),
                expected,
                "case create {case}: expected {expected} nodes, got {:?}",
                all_text(&created.body, "NodeRole")
            );
        } else {
            assert_eq!(
                (created.status, created.error_code()),
                (
                    400,
                    Some("NumberOfNodesPerClusterLimitExceeded".to_string())
                ),
                "case create {case}: expected 400 NumberOfNodesPerClusterLimitExceeded, got {} / {:?}",
                created.status,
                created.error_code()
            );
            // The refused create wrote nothing.
            let described = emulator
                .call(
                    "DescribeClusters",
                    &[("ClusterIdentifier", identifier.as_str())],
                )
                .await;
            assert_eq!(
                (described.status, described.error_code()),
                (404, Some("ClusterNotFound".to_string())),
                "case create {case}: expected the cluster not to exist, got {} with body {}",
                described.status,
                described.body
            );
        }
    }

    // The same bound on the modify path, against a cluster that is inside it.
    let grown = emulator
        .call(
            "ModifyCluster",
            &[
                ("ClusterIdentifier", "nodebound-00"),
                ("NumberOfNodes", &(state::MAX_NODES_PER_CLUSTER + 1).to_string()),
            ],
        )
        .await;
    assert_eq!(
        (grown.status, grown.error_code()),
        (
            400,
            Some("NumberOfNodesPerClusterLimitExceeded".to_string())
        ),
        "case modify past the bound: expected 400 NumberOfNodesPerClusterLimitExceeded, got {} / {:?}",
        grown.status,
        grown.error_code()
    );
    let unchanged = emulator
        .call("DescribeClusters", &[("ClusterIdentifier", "nodebound-00")])
        .await;
    assert_eq!(
        unchanged.text("NumberOfNodes").as_deref(),
        Some("4"),
        "case modify past the bound: expected the node count to be untouched, got {:?}",
        unchanged.text("NumberOfNodes")
    );
}
