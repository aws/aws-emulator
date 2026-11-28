// SPDX-License-Identifier: CC0-1.0

//! Which S3 Access Grants INSTANCE an answer about a grant belongs to, driven
//! end to end over HTTP.
//!
//! The rule under test: an access grant is held IN one S3 Access Grants instance
//! — the instance of the account the request names, in the Region the request was
//! signed for — so every answer that reports a grant names that instance, with
//! the ARN the instance itself reports: each entry of `ListCallerAccessGrants`
//! carries it as `AccessGrantsInstanceArn`, and the credential `GetDataAccess`
//! vends carries the instance whose grant matched as `MatchedInstanceArn`. The
//! ARN is the INSTANCE's, not the grant's own (which is a subresource path under
//! it), and a grant held by another instance names that other instance.
//!
//! `InstanceScope` chooses which instances are searched — `SELF` the instance of
//! the account in the request, `ALL` that one together with every instance shared
//! with the caller's account through Resource Access Manager — and nothing shares
//! an S3 Access Grants instance with another account here, so both values answer
//! about the same one instance.
//!
//! Every row below differs along something the rule generalises over: the Region
//! whose instance holds the grants (which is the Region the ARN names), the form
//! of the registered location's scope, whether the grant narrows that location,
//! the permission granted, how many grants the instance holds, and which
//! `InstanceScope` the request asks for.
//!
//! The instance ARN each row compares against is the one the service itself
//! reports for that instance (`GetAccessGrantsInstance`), so what is pinned is
//! the RELATION between the answers rather than a second spelling of the ARN.
//!
//! Every assertion goes through the same front door a client uses: a buffered
//! `http::Request` dispatched by the multi-service router. The router, the request
//! builder, the XML readers and the calls that create an instance and register a
//! location are the sibling suites', reused rather than copied.

use std::collections::BTreeSet;

use crate::access_grants_data_plane::{create_instance, create_location, query_escape};
use crate::access_point_lifecycle::{
    call_in_region, xml_escape, Emulator, Reply, ACCOUNT, REGION, XMLNS,
};

use service_com_amazonaws_s3control_awss3controlservicev20180820::state::{
    DEFAULT_LOCATION_SCOPE, PERMISSION_READ, PERMISSION_READWRITE,
};

/// The `GranteeType` of an IAM user or role, which is the kind of grantee a
/// REQUEST's credential can be (`ListAccessGrantsRequest$GranteeType`).
const IAM_GRANTEE_TYPE: &str = "IAM";

/// Every `InstanceScope` value, from
/// `smithy_query(query="shape", shape="com.amazonaws.s3control#InstanceScope")`,
/// plus the absent member: a request may name no scope at all.
const INSTANCE_SCOPES: [Option<&str>; 3] = [None, Some("SELF"), Some("ALL")];

/// One row: the instance that holds the grants, the location registered in it,
/// and the grants created on that location.
struct InstanceCase {
    case: &'static str,
    region: &'static str,
    location_scope: &'static str,
    /// One entry per grant to create: how it narrows the location (`None` grants
    /// the whole scope), and what it grants.
    grants: Vec<(Option<&'static str>, &'static str)>,
}

/// The rows every assertion below runs over. Each takes its own Region, because
/// one account holds ONE instance per Region and a caller is authorized for one
/// account.
fn instance_cases() -> Vec<InstanceCase> {
    vec![
        InstanceCase {
            case: "one grant under the default location",
            region: REGION,
            location_scope: DEFAULT_LOCATION_SCOPE,
            grants: vec![(Some("ledger/*"), PERMISSION_READ)],
        },
        InstanceCase {
            case: "two grants under a bucket and prefix",
            region: "eu-central-1",
            location_scope: "s3://reports/daily/",
            grants: vec![
                (Some("2026/*"), PERMISSION_READWRITE),
                (Some("2025/*"), PERMISSION_READ),
            ],
        },
        InstanceCase {
            case: "a grant over a whole registered bucket",
            region: "ap-southeast-2",
            location_scope: "s3://archive/",
            grants: vec![(None, PERMISSION_READ)],
        },
        InstanceCase {
            case: "three grants under a bucket registered without its trailing slash",
            region: "us-east-1",
            location_scope: "s3://ledger",
            grants: vec![
                (Some("alpha/*"), PERMISSION_READ),
                (Some("beta/*"), PERMISSION_READWRITE),
                (Some("gamma/*"), PERMISSION_READ),
            ],
        },
    ]
}

/// The IAM identity every request of this suite speaks for: the user the
/// emulator's own long-term credential belongs to, which is what the service
/// resolves the caller to — so a grant to it is a grant the caller listing
/// reports.
fn caller_arn() -> String {
    ::service_common::caller_principal::ambient_user_arn()
}

/// Create a grant on `location_id` for the CALLER, narrowed by `sub_prefix` when
/// there is one.
///
/// The sibling data-plane suite's helper grants another role, which the caller
/// listing would then report nothing for; everything else about the request is
/// the same.
async fn create_caller_grant(
    emulator: &Emulator,
    region: &str,
    location_id: &str,
    sub_prefix: Option<&str>,
    permission: &str,
) -> Reply {
    let mut body = format!(
        "<CreateAccessGrantRequest xmlns=\"{XMLNS}\">\
         <AccessGrantsLocationId>{}</AccessGrantsLocationId>",
        xml_escape(location_id)
    );
    if let Some(sub_prefix) = sub_prefix {
        body.push_str(&format!(
            "<AccessGrantsLocationConfiguration><S3SubPrefix>{}</S3SubPrefix>\
             </AccessGrantsLocationConfiguration>",
            xml_escape(sub_prefix)
        ));
    }
    body.push_str(&format!(
        "<Grantee><GranteeType>{IAM_GRANTEE_TYPE}</GranteeType><GranteeIdentifier>{}\
         </GranteeIdentifier></Grantee><Permission>{permission}</Permission>\
         </CreateAccessGrantRequest>",
        xml_escape(&caller_arn())
    ));
    call_in_region(
        &emulator.router,
        region,
        "POST",
        "/v20180820/accessgrantsinstance/grant",
        ACCOUNT,
        &body,
    )
    .await
}

/// `ListCallerAccessGrants` in `region`, asking for `instance_scope` when the row
/// names one.
async fn list_caller_grants(
    emulator: &Emulator,
    region: &str,
    instance_scope: Option<&str>,
) -> Reply {
    let mut uri = "/v20180820/accessgrantsinstance/caller/grants".to_string();
    if let Some(instance_scope) = instance_scope {
        uri.push_str(&format!("?instanceScope={instance_scope}"));
    }
    call_in_region(&emulator.router, region, "GET", &uri, ACCOUNT, "").await
}

/// `GetDataAccess` for `target` with `permission`, asking for `instance_scope`
/// when the row names one.
async fn get_data_access(
    emulator: &Emulator,
    region: &str,
    target: &str,
    permission: &str,
    instance_scope: Option<&str>,
) -> Reply {
    let mut uri = format!(
        "/v20180820/accessgrantsinstance/dataaccess?target={}&permission={permission}",
        query_escape(target)
    );
    if let Some(instance_scope) = instance_scope {
        uri.push_str(&format!("&instanceScope={instance_scope}"));
    }
    call_in_region(&emulator.router, region, "GET", &uri, ACCOUNT, "").await
}

/// The ARN the service reports for the instance this account owns in `region`.
async fn instance_arn(emulator: &Emulator, case: &str, region: &str) -> String {
    let read = call_in_region(
        &emulator.router,
        region,
        "GET",
        "/v20180820/accessgrantsinstance",
        ACCOUNT,
        "",
    )
    .await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: expected GetAccessGrantsInstance to answer 200, got {} with body {}",
        read.status, read.body
    );
    read.text("AccessGrantsInstanceArn")
        .unwrap_or_else(|| panic!("case {case:?}: no AccessGrantsInstanceArn in {}", read.body))
}

/// An S3 URI path the grant on `scope` covers: a key inside the scope, whichever
/// form the scope takes.
fn target_under(scope: &str) -> String {
    match scope.strip_suffix('*') {
        Some(prefix) => format!("{prefix}object.txt"),
        None if scope.ends_with('/') => format!("{scope}object.txt"),
        None => format!("{scope}/object.txt"),
    }
}

/// What one row left in its instance: the instance's own ARN, and the grant
/// scopes and ARNs the creates reported.
struct Provisioned {
    instance_arn: String,
    /// One per grant: the scope it covers, the permission it grants, and the
    /// grant's own ARN.
    grants: Vec<(String, &'static str, String)>,
}

/// Stand up `case`'s instance, register its location and create its grants.
async fn provision(emulator: &Emulator, case: &InstanceCase) -> Provisioned {
    let created = create_instance(emulator, ACCOUNT, case.region).await;
    assert_eq!(
        created.status, 200,
        "case {:?}: expected CreateAccessGrantsInstance to answer 200, got {} with body {}",
        case.case, created.status, created.body
    );
    let registered = create_location(emulator, ACCOUNT, case.region, case.location_scope).await;
    assert_eq!(
        registered.status, 200,
        "case {:?}: expected CreateAccessGrantsLocation to answer 200, got {} with body {}",
        case.case, registered.status, registered.body
    );
    let location_id = registered
        .text("AccessGrantsLocationId")
        .unwrap_or_else(|| panic!("case {:?}: no AccessGrantsLocationId", case.case));

    let mut grants = Vec::new();
    for (sub_prefix, permission) in &case.grants {
        let grant =
            create_caller_grant(emulator, case.region, &location_id, *sub_prefix, permission).await;
        assert_eq!(
            grant.status, 200,
            "case {:?}: expected CreateAccessGrant to answer 200, got {} with body {}",
            case.case, grant.status, grant.body
        );
        let scope = grant
            .text("GrantScope")
            .unwrap_or_else(|| panic!("case {:?}: no GrantScope in {}", case.case, grant.body));
        let arn = grant
            .text("AccessGrantArn")
            .unwrap_or_else(|| panic!("case {:?}: no AccessGrantArn in {}", case.case, grant.body));
        grants.push((scope, *permission, arn));
    }
    Provisioned {
        instance_arn: instance_arn(emulator, case.case, case.region).await,
        grants,
    }
}

#[tokio::test]
async fn every_caller_grant_entry_names_the_instance_that_holds_it() {
    let emulator = Emulator::start();
    let cases = instance_cases();
    let mut named_instances: BTreeSet<String> = BTreeSet::new();

    for case in &cases {
        let provisioned = provision(&emulator, case).await;
        named_instances.insert(provisioned.instance_arn.clone());

        for instance_scope in INSTANCE_SCOPES {
            let listed = list_caller_grants(&emulator, case.region, instance_scope).await;
            assert_eq!(
                listed.status, 200,
                "case {:?} with scope {instance_scope:?}: expected ListCallerAccessGrants to \
                 answer 200, got {} with body {}",
                case.case, listed.status, listed.body
            );
            // Every entry of the page names the instance, and the page holds one
            // entry per grant that instance holds.
            let reported = listed.texts("AccessGrantsInstanceArn");
            assert_eq!(
                reported,
                vec![provisioned.instance_arn.clone(); case.grants.len()],
                "case {:?} with scope {instance_scope:?}: expected every entry to name the \
                 instance that holds the grant, got body {}",
                case.case,
                listed.body
            );
            // The instance's ARN, not the grant's own: a grant is addressed as a
            // subresource UNDER the instance.
            for (_, _, grant_arn) in &provisioned.grants {
                assert!(
                    !reported.contains(grant_arn),
                    "case {:?} with scope {instance_scope:?}: expected the instance's ARN \
                     rather than the grant's {grant_arn:?}, got {reported:?}",
                    case.case
                );
            }
        }
    }

    // Each row's grants are held by a DIFFERENT instance, so the ARNs the entries
    // name are as many as the rows: the member names the instance the answer is
    // about rather than one fixed value.
    assert_eq!(
        named_instances.len(),
        cases.len(),
        "expected one instance ARN per row, got {named_instances:?}"
    );
}

#[tokio::test]
async fn a_vended_credential_names_the_instance_whose_grant_matched() {
    let emulator = Emulator::start();
    let cases = instance_cases();

    for case in &cases {
        let provisioned = provision(&emulator, case).await;

        for (index, (scope, permission, grant_arn)) in provisioned.grants.iter().enumerate() {
            let target = target_under(scope);
            // One `InstanceScope` per grant, so the rows cover the absent member
            // and both values over the targets of this instance's grants.
            let instance_scope = INSTANCE_SCOPES[index % INSTANCE_SCOPES.len()];
            let vended =
                get_data_access(&emulator, case.region, &target, permission, instance_scope).await;
            assert_eq!(
                vended.status, 200,
                "case {:?} target {target:?} with scope {instance_scope:?}: expected \
                 GetDataAccess to answer 200, got {} with body {}",
                case.case, vended.status, vended.body
            );
            let matched = vended.text("MatchedInstanceArn").unwrap_or_default();
            assert_eq!(
                matched, provisioned.instance_arn,
                "case {:?} target {target:?} with scope {instance_scope:?}: expected the \
                 instance containing the matched grant, got {matched:?} in body {}",
                case.case, vended.body
            );
            assert_ne!(
                &matched, grant_arn,
                "case {:?} target {target:?}: expected the instance's ARN rather than the \
                 matched grant's, got body {}",
                case.case, vended.body
            );
        }

        // A target no grant covers is the refusal, which vends nothing and names
        // no instance.
        let uncovered = get_data_access(
            &emulator,
            case.region,
            "s3://uncovered-by-any-grant/object.txt",
            PERMISSION_READ,
            None,
        )
        .await;
        assert_eq!(
            uncovered.status, 403,
            "case {:?}: expected an uncovered target to be refused with 403, got {} with \
             body {}",
            case.case, uncovered.status, uncovered.body
        );
        assert_eq!(
            uncovered.text("MatchedInstanceArn"),
            None,
            "case {:?}: expected a refusal to name no instance, got body {}",
            case.case,
            uncovered.body
        );
    }
}
