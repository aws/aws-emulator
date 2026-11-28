// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the AWS managed permission LIBRARY that `ListPermissions`
//! enumerates.
//!
//! The rule the rows below pin: `ListPermissions` enumerates the whole managed-permission
//! library Amazon Web Services publishes for its shareable resource types — the library
//! belongs to Amazon Web Services rather than to an account, so an unfiltered read in any
//! account reports one `ResourceSharePermissionSummary` per published permission, each
//! carrying its account-less ARN, its name, the resource type it governs, `AWS_MANAGED`,
//! `ATTACHABLE`, the version the library publishes it at with `defaultVersion` true, and
//! whether it is that resource type's default. A
//! read scoped with `resourceType` keeps the permissions published for that one type,
//! matched without regard to case or to the separators inside the resource code, and a
//! shareable type the library publishes nothing for selects none of them.
//!
//! The rows vary along what the rule generalises over: the whole library rather than one
//! entry, cut into pages of several sizes; the families the library publishes for a single
//! resource type beside the types that take a single permission; four spellings of a
//! resource type; both `permissionType` filters beside no filter at all; and the
//! round trip of every enumerated ARN through `GetPermission`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_ram_amazonresourcesharing::state::{
    aws_managed_permission_arn, resource_type_matches, AWS_MANAGED_PERMISSIONS,
    FIRST_PERMISSION_VERSION, PERMISSION_FEATURE_SET_STANDARD, PERMISSION_STATUS_ATTACHABLE,
    PERMISSION_TYPE_AWS_MANAGED, PERMISSION_TYPE_CUSTOMER_MANAGED, RESOURCE_TYPES,
};
use smithy_server_runtime::MultiServiceRouter;

// Every suite in this crate calls the service the one way: through `tests/all/harness.rs`.
use super::harness::*;

async fn list_permissions(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/listpermissions", Some(body)).await
}

async fn get_permission(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/getpermission", Some(body)).await
}

async fn create_permission(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/createpermission", Some(body)).await
}

/// The permissions a read reported.
fn permissions_of(case: &str, body: &Value) -> Vec<Value> {
    body.get("permissions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: response carried no permissions, got {body:?}"))
}

/// A string member of a reported summary.
fn field(case: &str, permission: &Value, member: &str) -> String {
    permission
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} member, got {permission:?}"))
        .to_string()
}

/// A boolean member of a reported summary.
fn flag(case: &str, permission: &Value, member: &str) -> bool {
    permission
        .get(member)
        .and_then(Value::as_bool)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} member, got {permission:?}"))
}

/// Every summary an unfiltered read reports, keyed by ARN, read one page of `page_size`
/// at a time until the read reports no `nextToken`.
///
/// The page budget is the library's own size plus the pages a one-entry page size needs,
/// so a read that never stops fails the case instead of looping.
async fn library_by_arn(
    router: &MultiServiceRouter,
    case: &str,
    request: &Value,
    page_size: usize,
) -> BTreeMap<String, Value> {
    let mut by_arn: BTreeMap<String, Value> = BTreeMap::new();
    let mut token: Option<String> = None;
    let budget = AWS_MANAGED_PERMISSIONS.len() + 2;
    for page in 0..=budget {
        let mut body = request.clone();
        body["maxResults"] = json!(page_size);
        if let Some(resume) = &token {
            body["nextToken"] = json!(resume);
        }
        let (status, reported) = list_permissions(router, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: page {page} expected HTTP 200, got {status} with {reported:?}"
        );
        let permissions = permissions_of(case, &reported);
        assert!(
            permissions.len() <= page_size,
            "case {case:?}: page {page} reported {} entries, more than the {page_size} asked \
             for",
            permissions.len()
        );
        for permission in permissions {
            let arn = field(case, &permission, "arn");
            assert!(
                by_arn.insert(arn.clone(), permission).is_none(),
                "case {case:?}: {arn} was reported twice across pages"
            );
        }
        token = reported
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            return by_arn;
        }
    }
    panic!(
        "case {case:?}: the read never reported a last page within {budget} pages of \
         {page_size}"
    );
}

/// Assert one reported summary against the library entry it must have been projected
/// from.
fn assert_summary(case: &str, permission: &Value, name: &str) {
    let entry = AWS_MANAGED_PERMISSIONS
        .iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("case {case:?}: the library publishes no permission {name:?}"));
    assert_eq!(
        field(case, permission, "name"),
        entry.name,
        "case {case:?}: expected the published name, got {permission:?}"
    );
    assert_eq!(
        field(case, permission, "resourceType"),
        entry.resource_type,
        "case {case:?}: expected the resource type the permission governs, got {permission:?}"
    );
    assert_eq!(
        field(case, permission, "permissionType"),
        PERMISSION_TYPE_AWS_MANAGED,
        "case {case:?}: a permission Amazon Web Services publishes reports AWS_MANAGED, got \
         {permission:?}"
    );
    assert_eq!(
        field(case, permission, "status"),
        PERMISSION_STATUS_ATTACHABLE,
        "case {case:?}: a published permission can be attached to a share, got {permission:?}"
    );
    assert_eq!(
        field(case, permission, "featureSet"),
        PERMISSION_FEATURE_SET_STANDARD,
        "case {case:?}: expected the STANDARD feature set, got {permission:?}"
    );
    assert_eq!(
        field(case, permission, "version"),
        entry.version.to_string(),
        "case {case:?}: expected the version the library publishes, got {permission:?}"
    );
    assert!(
        entry.version >= FIRST_PERMISSION_VERSION,
        "case {case:?}: a published version is numbered from {FIRST_PERMISSION_VERSION} upward, \
         got {}",
        entry.version
    );
    assert!(
        flag(case, permission, "defaultVersion"),
        "case {case:?}: the published version is the default one, got {permission:?}"
    );
    assert_eq!(
        flag(case, permission, "isResourceTypeDefault"),
        entry.is_resource_type_default,
        "case {case:?}: expected the resource-type default flag, got {permission:?}"
    );
}

#[tokio::test]
async fn an_unfiltered_read_enumerates_the_whole_published_library() {
    let router = emulator();
    let expected: BTreeSet<String> = AWS_MANAGED_PERMISSIONS
        .iter()
        .map(|entry| aws_managed_permission_arn(entry.name))
        .collect();
    assert!(
        expected.len() > 100,
        "the library this service publishes must be the whole catalog rather than a \
         hand-picked subset, got {} entries",
        expected.len()
    );

    // The page sizes span the published bounds of `maxResults` (1..500) and sizes between
    // them: the library comes back whole however the read is cut into pages.
    for page_size in [1usize, 7, 64, 500] {
        let case = format!("maxResults {page_size}");
        let reported = library_by_arn(&router, &case, &json!({}), page_size).await;
        let arns: BTreeSet<String> = reported.keys().cloned().collect();
        assert_eq!(
            arns, expected,
            "case {case:?}: expected every published permission ARN, got {} of them",
            arns.len()
        );
        for entry in AWS_MANAGED_PERMISSIONS {
            let arn = aws_managed_permission_arn(entry.name);
            let permission = reported
                .get(&arn)
                .unwrap_or_else(|| panic!("case {case:?}: {arn} was not enumerated"));
            assert_summary(&case, permission, entry.name);
        }
    }
}

#[tokio::test]
async fn every_enumerated_permission_reads_back_through_get_permission() {
    let router = emulator();
    let case = "round trip";
    let reported = library_by_arn(&router, case, &json!({}), 500).await;
    for (arn, summary) in &reported {
        let (status, read) = get_permission(&router, &json!({"permissionArn": arn})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an enumerated ARN must resolve, got {status} with {read:?} for {arn}"
        );
        let detail = read
            .get("permission")
            .unwrap_or_else(|| panic!("case {case:?}: no permission for {arn}, got {read:?}"));
        for member in ["arn", "name", "resourceType", "permissionType", "version"] {
            assert_eq!(
                field(case, detail, member),
                field(case, summary, member),
                "case {case:?}: the read of {arn} reports another {member} than the enumeration"
            );
        }
    }
}

#[tokio::test]
async fn the_library_publishes_the_families_the_permission_library_records() {
    // What the library's membership must cover, named here rather than read off the
    // catalog: one row per published permission of a family, with the resource type it
    // governs. Source: the managed permissions the RAM permission library publishes,
    // which an unfiltered `ListPermissions` enumerates in any account. The rows are
    // families the issue that filed this suite did not cite — Oracle Database@AWS, AWS
    // End User Messaging, EventBridge, SageMaker AI hubs, Lake Formation-enabled Glue —
    // beside one single-permission type.
    let published: &[(&str, &str)] = &[
        ("AWSRAMDefaultPermissionODBNetwork", "odb:OdbNetwork"),
        (
            "AWSRAMDefaultPermissionODBCloudExadataInfrastructure",
            "odb:CloudExadataInfrastructure",
        ),
        (
            "AWSRAMDefaultPermissionODBExascaleDbStorageVault",
            "odb:ExascaleDbStorageVault",
        ),
        ("AWSRAMPermissionSMSVoicePhoneNumber", "sms-voice:PhoneNumber"),
        ("AWSRAMPermissionSMSVoicePool", "sms-voice:Pool"),
        ("AWSRAMPermissionSMSVoiceSenderId", "sms-voice:SenderId"),
        ("AWSRAMEventBridgeEventBusV2FullAccess", "events:EventBusv2"),
        ("AWSRAMEventBridgeEventBusV2PublishOnly", "events:EventBusv2"),
        ("AWSRAMPermissionSagemakerHubReadOnly", "sagemaker:Hub"),
        ("AWSRAMPermissionSagemakerHubFullAccess", "sagemaker:Hub"),
        ("AWSRAMLFEnabledGlueDatabaseReadWrite", "glue:Database"),
        ("AWSRAMPermissionLFTagGlueTableReadWrite", "glue:Table"),
        (
            "AWSRAMPermissionResourceExplorerViews",
            "resource-explorer-2:View",
        ),
    ];

    let router = emulator();
    let case = "published families";
    let reported = library_by_arn(&router, case, &json!({}), 500).await;
    for (name, resource_type) in published {
        let arn = aws_managed_permission_arn(name);
        let permission = reported.get(&arn).unwrap_or_else(|| {
            panic!("case {name:?}: the library publishes it, so a read must enumerate {arn}")
        });
        assert_eq!(
            field(name, permission, "resourceType"),
            *resource_type,
            "case {name:?}: expected the resource type the permission governs, got {permission:?}"
        );
        assert_summary(name, permission, name);
    }

    // A name the library publishes nothing under is enumerated nowhere, so a client
    // cannot discover an ARN that resolves to nothing.
    for absent in [
        "AWSRAMDefaultPermissionODBNetworkV2",
        "AWSRAMPermissionSMSVoiceNeverPublished",
        "awsrameventbridgeeventbusv2fullaccess",
    ] {
        let arn = aws_managed_permission_arn(absent);
        assert!(
            !reported.contains_key(&arn),
            "case {absent:?}: a name the library does not publish must not be enumerated"
        );
    }
}

#[tokio::test]
async fn a_resource_type_scoped_read_keeps_the_permissions_published_for_that_type() {
    let router = emulator();
    // The spellings a client scopes a read with: the display name, the same name
    // lowercased, the spelling a resource ARN carries (the separators inside the resource
    // code dropped or hyphenated) and the name shouted — the member "is not case
    // sensitive" (`ListPermissions$resourceType`). The types span one the library
    // publishes a family for, one it publishes a single permission for, one whose service
    // code carries a hyphen, and one the library publishes nothing for.
    let types = [
        "acm-pca:CertificateAuthority",
        "glue:Catalog",
        "odb:OdbNetwork",
        "sms-voice:OptOutList",
        "ec2:TransitGatewayMulticastDomain",
        "resource-explorer-2:View",
        // A resource type `ListResourceTypes` reports that no published permission
        // governs: the page is empty rather than refused.
        "ec2:NetworkInsightsAccessScope",
    ];
    for resource_type in types {
        for wanted in [
            resource_type.to_string(),
            resource_type.to_ascii_lowercase(),
            resource_type.to_ascii_uppercase(),
            resource_type.replace('-', ""),
        ] {
            let case = format!("resourceType {wanted:?}");
            let mut expected: Vec<String> = AWS_MANAGED_PERMISSIONS
                .iter()
                .filter(|entry| resource_type_matches(entry.resource_type, &wanted))
                .map(|entry| aws_managed_permission_arn(entry.name))
                .collect();
            expected.sort();

            let (status, reported) =
                list_permissions(&router, &json!({"resourceType": wanted})).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {reported:?}"
            );
            let mut arns: Vec<String> = permissions_of(&case, &reported)
                .iter()
                .map(|permission| field(&case, permission, "arn"))
                .collect();
            arns.sort();
            assert_eq!(
                arns, expected,
                "case {case:?}: expected the permissions published for that resource type, got \
                 {arns:?}"
            );
            for permission in permissions_of(&case, &reported) {
                let reported_type = field(&case, &permission, "resourceType");
                assert!(
                    resource_type_matches(&reported_type, &wanted),
                    "case {case:?}: a scoped page reports only that resource type, got \
                     {reported_type:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_library_publishes_these_revisions_and_resource_type_defaults() {
    // The published VERSION and the resource-type DEFAULT of a permission, named here as
    // literals rather than read off the catalog, so a renumbered entry or a default moved
    // onto the wrong permission fails a case. Source: the version the RAM permission
    // library publishes each of these permissions at, and which of the permissions it
    // publishes for one resource type is that type's default. The rows span every shape the
    // rule has: an entry at its first version, entries at a revision, both permissions of a
    // resource type the library publishes two of, and the entries the library publishes for
    // one resource type where the default is NOT the first-listed or the most permissive one.
    let published: &[(&str, &str, &str, bool)] = &[
        ("AWSRAMDefaultPermissionAppMesh", "appmesh:Mesh", "3", true),
        (
            "AWSRAMDefaultPermissionAppMeshPreview",
            "appmesh-preview:Mesh",
            "3",
            true,
        ),
        (
            "AWSRAMDefaultPermissionAgentRegistryReadOnly",
            "agent-registry:Registry",
            "2",
            true,
        ),
        (
            "AWSRAMPermissionAgentRegistryForConsumer",
            "agent-registry:Registry",
            "1",
            false,
        ),
        (
            "AWSRAMDefaultPermissionCapacityReservation",
            "ec2:CapacityReservation",
            "2",
            true,
        ),
        (
            "AWSRAMDefaultPermissionTransitGateway",
            "ec2:TransitGateway",
            "3",
            true,
        ),
        (
            "AWSRAMPermissionGlueDatabaseReadWrite",
            "glue:Database",
            "3",
            false,
        ),
        ("AWSRAMDefaultPermissionODBNetwork", "odb:OdbNetwork", "2", true),
        (
            "AWSRAMDefaultPermissionRefactorSpacesEnvironment",
            "refactor-spaces:Environment",
            "2",
            true,
        ),
        (
            "AWSRAMDefaultPermissionS3Outposts",
            "s3-outposts:Outpost",
            "2",
            true,
        ),
        (
            "AWSRAMDefaultPermissionSSMIncidentsResponsePlan",
            "ssm-incidents:ResponsePlan",
            "2",
            true,
        ),
        // An Amazon DataZone domain: the library has revised the domain's own default and
        // each of the permissions published beside it a different number of times.
        (
            "AWSRAMDefaultPermissionAmazonDataZoneDomain",
            "datazone:Domain",
            "10",
            true,
        ),
        (
            "AWSRAMPermissionAmazonDataZoneDomainFullAccessWithPortalAccess",
            "datazone:Domain",
            "7",
            false,
        ),
        (
            "AWSRAMPermissionsAmazonDatazoneDomainExtendedServiceAccess",
            "datazone:Domain",
            "10",
            false,
        ),
        (
            "AWSRAMPermissionsAmazonDatazoneDomainExtendedServiceWithPortalAccess",
            "datazone:Domain",
            "8",
            false,
        ),
        (
            "AWSRAMSageMakerServicePrincipalPermissionAmazonDataZoneDomain",
            "datazone:Domain",
            "3",
            false,
        ),
        // A resource type the library publishes two permissions for: exactly one of the
        // pair is the type's default, and the other reports false.
        (
            "AWSRAMDefaultPermissionsIpamPool",
            "ec2:IpamPool",
            "4",
            true,
        ),
        (
            "AWSRAMPermissionIpamPoolByoipCidrImport",
            "ec2:IpamPool",
            "1",
            false,
        ),
        (
            "AWSRAMDefaultPermissionNetworkFirewallPolicy",
            "network-firewall:FirewallPolicy",
            "1",
            true,
        ),
        (
            "AWSRAMDefaultPermissionNetworkFirewallStatefulRuleGroup",
            "network-firewall:StatefulRulegroup",
            "2",
            true,
        ),
        (
            "AWSRAMDefaultPermissionNetworkFirewallStatefulRuleGroupWithDescribe",
            "network-firewall:StatefulRulegroup",
            "1",
            false,
        ),
        (
            "AWSRAMDefaultPermissionOutpostsOutpost",
            "outposts:Outpost",
            "1",
            true,
        ),
        (
            "AWSRAMDefaultPermissionOutpostsSite",
            "outposts:Site",
            "1",
            true,
        ),
        (
            "AWSRAMPermissionVpcLatticeServiceNetworkReadWrite",
            "vpc-lattice:ServiceNetwork",
            "2",
            true,
        ),
        // An Amazon EventBridge event bus: the subscribe-only permission is the type's
        // default, not the full-access one, and the type is spelled the way the owning
        // service publishes it.
        (
            "AWSRAMEventBridgeEventBusV2SubscribeOnly",
            "events:EventBusv2",
            "1",
            true,
        ),
        (
            "AWSRAMEventBridgeEventBusV2FullAccess",
            "events:EventBusv2",
            "1",
            false,
        ),
        // Amazon S3 Access Grants and the SageMaker AI hubs: again the default is not the
        // read-only or the most permissive entry of the family.
        (
            "AWSRAMPermissionAccessGrantsData",
            "s3:AccessGrants",
            "2",
            true,
        ),
        (
            "AWSRAMPermissionAccessGrantsReadAccess",
            "s3:AccessGrants",
            "2",
            false,
        ),
        (
            "AWSRAMPermissionSagemakerHubReadAndUse",
            "sagemaker:Hub",
            "2",
            true,
        ),
        (
            "AWSRAMPermissionSagemakerHubReadOnly",
            "sagemaker:Hub",
            "1",
            false,
        ),
        // AWS Service Catalog AppRegistry: the plural type spelling the owning service
        // publishes, with the read-only permission as the default.
        (
            "AWSRAMPermissionServiceCatalogAppRegistryApplicationReadOnly",
            "servicecatalog:Applications",
            "1",
            true,
        ),
        (
            "AWSRAMPermissionServiceCatalogAppRegistryAttributeGroupReadOnly",
            "servicecatalog:AttributeGroups",
            "1",
            true,
        ),
        // A permission whose name names one service and whose resource type is another's:
        // the server-manageability permission governs a Dedicated Host.
        (
            "AWSRAMDefaultPermissionServerManageability",
            "ec2:DedicatedHost",
            "1",
            true,
        ),
    ];

    let router = emulator();
    let case = "published revisions";
    let reported = library_by_arn(&router, case, &json!({}), 500).await;
    for (name, resource_type, version, is_resource_type_default) in published {
        let arn = aws_managed_permission_arn(name);
        let permission = reported
            .get(&arn)
            .unwrap_or_else(|| panic!("case {name:?}: the library publishes it, so {arn} must be \
                                       enumerated"));
        assert_eq!(
            field(name, permission, "resourceType"),
            *resource_type,
            "case {name:?}: expected the resource type the library publishes it for, got \
             {permission:?}"
        );
        assert_eq!(
            field(name, permission, "version"),
            *version,
            "case {name:?}: expected the version the library publishes, got {permission:?}"
        );
        assert!(
            flag(name, permission, "defaultVersion"),
            "case {name:?}: the published version is the default one, got {permission:?}"
        );
        assert_eq!(
            flag(name, permission, "isResourceTypeDefault"),
            *is_resource_type_default,
            "case {name:?}: expected isResourceTypeDefault {is_resource_type_default}, got \
             {permission:?}"
        );
    }
}

#[tokio::test]
async fn a_scoped_read_reports_the_permissions_the_library_publishes_for_two_named_types() {
    // The expectation named as literals rather than computed from the catalog, so the
    // matching this scoping rests on is pinned: the ARNs below are what a read scoped to
    // each type must report, in every spelling of the type. Source: the permissions the
    // RAM permission library publishes for a private certificate authority (its default,
    // the blank and typed end-entity issuance permissions, one subordinate-CA issuance
    // permission per published path length, and the revocation permission) and for an
    // Oracle Database@AWS network (one).
    let scoped: &[(&str, &[&str])] = &[
        (
            "acm-pca:CertificateAuthority",
            &[
                "AWSRAMBlankEndEntityCertificateAPICSRPassthroughIssuanceCertificateAuthority",
                "AWSRAMBlankEndEntityCertificateAPIPassthroughIssuanceCertificateAuthority",
                "AWSRAMDefaultPermissionCertificateAuthority",
                "AWSRAMEndEntityClientAuthCertificateIssuanceCertificateAuthority",
                "AWSRAMEndEntityServerAuthCertificateIssuanceCertificateAuthority",
                "AWSRAMRevokeCertificateCertificateAuthority",
                "AWSRAMSubordinateCACertificatePathLen0IssuanceCertificateAuthority",
            ],
        ),
        ("odb:OdbNetwork", &["AWSRAMDefaultPermissionODBNetwork"]),
    ];

    let router = emulator();
    for (resource_type, names) in scoped {
        let mut expected: Vec<String> = names
            .iter()
            .map(|name| aws_managed_permission_arn(name))
            .collect();
        expected.sort();
        for wanted in [
            resource_type.to_string(),
            resource_type.to_ascii_lowercase(),
            resource_type.to_ascii_uppercase(),
            resource_type.replace('-', ""),
        ] {
            let case = format!("named rows for {wanted:?}");
            let (status, reported) =
                list_permissions(&router, &json!({"resourceType": wanted})).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {reported:?}"
            );
            let mut arns: Vec<String> = permissions_of(&case, &reported)
                .iter()
                .map(|permission| field(&case, permission, "arn"))
                .collect();
            arns.sort();
            assert_eq!(
                arns, expected,
                "case {case:?}: expected exactly the permissions the library publishes for that \
                 type, got {arns:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_library_publishes_one_default_per_resource_type_it_governs() {
    let router = emulator();
    // Over every resource type the library publishes for, not only the types the recording
    // touched: at least one of that type's published permissions is its default, and a
    // scoped read reports it. The flag is a per-entry published fact rather than a count
    // this service derives, so a type the library publishes two defaults for reports two.
    let governed: BTreeSet<&str> = AWS_MANAGED_PERMISSIONS
        .iter()
        .map(|entry| entry.resource_type)
        .collect();
    assert!(
        governed.len() > 30,
        "the library governs the shareable resource types, got {} of them",
        governed.len()
    );
    for resource_type in governed {
        let case = format!("default of {resource_type:?}");
        // The type is one this service shares, which is what `ListResourceTypes` reports.
        assert!(
            RESOURCE_TYPES
                .iter()
                .any(|known| known.resource_type == resource_type),
            "case {case:?}: a governed resource type is one this service shares"
        );
        let (status, reported) =
            list_permissions(&router, &json!({"resourceType": resource_type})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {reported:?}"
        );
        let permissions = permissions_of(&case, &reported);
        let mut defaults: Vec<String> = permissions
            .iter()
            .filter(|permission| flag(&case, permission, "isResourceTypeDefault"))
            .map(|permission| field(&case, permission, "name"))
            .collect();
        defaults.sort();
        let mut published_defaults: Vec<String> = AWS_MANAGED_PERMISSIONS
            .iter()
            .filter(|entry| entry.resource_type == resource_type && entry.is_resource_type_default)
            .map(|entry| entry.name.to_string())
            .collect();
        published_defaults.sort();
        assert!(
            !defaults.is_empty(),
            "case {case:?}: a governed resource type has a default permission, got \
             {defaults:?}"
        );
        assert_eq!(
            defaults, published_defaults,
            "case {case:?}: expected the defaults the library publishes for the type, got \
             {defaults:?}"
        );
    }
}

#[tokio::test]
async fn the_permission_type_filter_tells_the_library_from_an_accounts_own_permissions() {
    let router = emulator();
    let case = "permissionType filters";
    let (status, created) = create_permission(
        &router,
        &json!({
            "name": "LibraryNeighbour",
            "resourceType": "codebuild:Project",
            "policyTemplate": r#"{"Effect":"ALLOW","Action":["codebuild:StartBuild"]}"#,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the subject create expected 200, got {status} with {created:?}"
    );
    let mine = format!("arn:aws:ram:{REGION}:{ACCOUNT}:permission/LibraryNeighbour");

    let published: BTreeSet<String> = AWS_MANAGED_PERMISSIONS
        .iter()
        .map(|entry| aws_managed_permission_arn(entry.name))
        .collect();

    // No filter and `ALL` report both populations; `AWS_MANAGED` reports the library
    // exactly; `CUSTOMER_MANAGED` reports none of it.
    for (filter, carries_library, carries_mine) in [
        (None, true, true),
        (Some("ALL"), true, true),
        (Some(PERMISSION_TYPE_AWS_MANAGED), true, false),
        (Some(PERMISSION_TYPE_CUSTOMER_MANAGED), false, true),
    ] {
        let case = format!("permissionType {filter:?}");
        let mut request = json!({});
        if let Some(value) = filter {
            request["permissionType"] = json!(value);
        }
        let reported = library_by_arn(&router, &case, &request, 500).await;
        let arns: BTreeSet<String> = reported.keys().cloned().collect();
        if carries_library {
            assert!(
                published.is_subset(&arns),
                "case {case:?}: expected the whole published library, got {} entries",
                arns.len()
            );
        } else {
            assert!(
                published.is_disjoint(&arns),
                "case {case:?}: expected none of the published library, got {} entries",
                arns.len()
            );
        }
        assert_eq!(
            arns.contains(&mine),
            carries_mine,
            "case {case:?}: this account's own permission must{} be reported",
            if carries_mine { "" } else { " not" }
        );
        for permission in reported.values() {
            let reported_type = field(&case, permission, "permissionType");
            if let Some(wanted) = filter.filter(|value| *value != "ALL") {
                assert_eq!(
                    reported_type, wanted,
                    "case {case:?}: a filtered page reports only that permission type, got \
                     {permission:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_enumeration_reports_the_same_library_to_a_later_caller() {
    let router = emulator();
    // A read writes nothing: what the library reports does not drift between calls, and
    // reading it does not consume it.
    let case = "repeat enumeration";
    let first = library_by_arn(&router, case, &json!({}), 500).await;
    let second = library_by_arn(&router, case, &json!({}), 500).await;
    assert_eq!(
        first, second,
        "case {case:?}: a second enumeration reported another library"
    );
}
