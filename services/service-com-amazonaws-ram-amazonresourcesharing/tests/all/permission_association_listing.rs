// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListPermissionAssociations`.
//!
//! The rule the rows below pin: a managed permission and a resource share are associated
//! exactly while that share is governed by that permission, so a read of a permission's
//! associations reports the same pairs a read of each share's permissions reports from
//! the other side — the permissions a share holds by name, each at the version that
//! attachment carries, plus the default managed permission of every resource type the
//! share shares that none of those applies to — each pair once, in (permission ARN, share
//! ARN) order, one page of at most `maxResults` entries at a time with a `nextToken` only
//! while a further page exists, and each pair reported with the version the share is
//! governed by, whether that is the permission's current default version, the resource
//! type the permission applies to, the share's own `featureSet` and the association's
//! status.
//!
//! So the rows vary along what the rule generalises over:
//!
//! * WHICH permission is associated: a customer managed one a create named, an AWS
//!   managed one derived from a resource type the share shares, and an AWS managed one
//!   that is not its type's default attached by name;
//! * WHAT a share carries: nothing at all, one permission, a permission plus a derived
//!   default of another type, and two derived defaults;
//! * WHICH VERSION the share is governed by: the one it attached, which a later version
//!   of the permission does not move (so its association stops being the default one),
//!   and the one a replacing attach updates it to;
//! * WHICH FILTER narrows the pairs: each input member in turn, taking a value that keeps
//!   everything, one that keeps some pairs and one that keeps none;
//! * HOW MANY pairs one page holds: both published bounds of `maxResults`, sizes between
//!   them and one past each;
//! * WHAT STOPS a pair being reported: a share whose delete removed it, and a permission a
//!   removal took back out of a share's permission set;
//! * WHAT a request this read cannot serve answers: a `permissionArn` that is not a
//!   managed permission ARN, a version number below the first one, and a `nextToken` this
//!   read did not issue.

use serde_json::{json, Value};
use service_com_amazonaws_ram_amazonresourcesharing::state::{
    aws_managed_permission_arn, encode_arn_cursor, encode_permission_association_cursor,
    AWS_MANAGED_PERMISSIONS, FEATURE_SET_STANDARD, FIRST_PERMISSION_VERSION,
    SHARE_PERMISSION_STATUS_ASSOCIATED,
};
use smithy_server_runtime::MultiServiceRouter;

// Every suite in this crate calls the service the one way: through `tests/all/harness.rs`.
use super::harness::*;

/// A resource type `ListResourceTypes` reports that a customer managed permission can be
/// created for, so a share of one can be governed by a permission of this account's own.
const PROJECT_TYPE: &str = "codebuild:Project";
/// A CodeBuild project: a resource of [`PROJECT_TYPE`].
const PROJECT: &str = "arn:aws:codebuild:us-west-2:000000000000:project/example";
/// An action of the service that owns [`PROJECT_TYPE`], for a permission's policy.
const PROJECT_ACTION: &str = "codebuild:BatchGetProjects";
/// The permission Amazon Web Services publishes as [`PROJECT_TYPE`]'s default.
const PROJECT_TYPE_DEFAULT: &str = "AWSRAMDefaultPermissionCodeBuildProject";

/// The resource type the harness's local resource belongs to.
const SUBNET_TYPE: &str = "ec2:Subnet";
/// The permission Amazon Web Services publishes as [`SUBNET_TYPE`]'s default.
const SUBNET_TYPE_DEFAULT: &str = "AWSRAMDefaultPermissionSubnet";

/// A Glue database: a resource of a third resource type, so a share can share two types.
const GLUE_DATABASE: &str = "arn:aws:glue:us-west-2:000000000000:database/analytics";
/// The resource type [`GLUE_DATABASE`] belongs to.
const GLUE_DATABASE_TYPE: &str = "glue:Database";
/// A permission the catalog publishes for [`GLUE_DATABASE_TYPE`] that is NOT that type's
/// default, and at a version above the first one, so attaching it by name is observable
/// beside the derived default and the version filters have more than one value to pick
/// between.
const GLUE_DATABASE_READ_WRITE: &str = "AWSRAMPermissionGlueDatabaseReadWrite";

/// A shareable resource type nothing below shares, for the filter row that must keep
/// nothing (`RESOURCE_TYPES` publishes it).
const UNSHARED_TYPE: &str = "ec2:Volume";

/// One association's key: the permission ARN and the resource share ARN it relates.
type Pair = (String, String);

/// One row of the filter table: what it is called, the request it makes, and the pairs
/// that request must report.
type FilterRow = (&'static str, Value, Vec<Pair>);

async fn list_associations(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/listpermissionassociations", Some(body)).await
}

async fn list_share_permissions(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/listresourcesharepermissions", Some(body)).await
}

async fn attach(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(
        router,
        "POST",
        "/associateresourcesharepermission",
        Some(body),
    )
    .await
}

async fn remove(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(
        router,
        "POST",
        "/disassociateresourcesharepermission",
        Some(body),
    )
    .await
}

/// A policy template a `CreatePermission` of this suite carries: one allowed action of
/// the service that owns the permission's resource type.
fn policy_template(action: &str) -> String {
    json!({"Effect": "ALLOW", "Action": [action]}).to_string()
}

/// Mint a customer managed permission for `resource_type` and answer its ARN.
async fn customer_permission(
    router: &MultiServiceRouter,
    name: &str,
    resource_type: &str,
    action: &str,
) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/createpermission",
        Some(&json!({
            "name": name,
            "resourceType": resource_type,
            "policyTemplate": policy_template(action),
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: creating the permission expected 200, got {status} with {created:?}"
    );
    created
        .get("permission")
        .and_then(|permission| permission.get("arn"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {name:?}: the create reported no permission ARN, got {created:?}")
        })
        .to_string()
}

/// Write another version of `permission_arn`, which becomes its default version.
async fn write_version(router: &MultiServiceRouter, case: &str, permission_arn: &str, action: &str) {
    let (status, written) = call(
        router,
        "POST",
        "/createpermissionversion",
        Some(&json!({
            "permissionArn": permission_arn,
            "policyTemplate": policy_template(action),
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: writing a version of {permission_arn} expected 200, got {status} with \
         {written:?}"
    );
}

/// Create a share that must succeed and answer its ARN.
async fn share_with(router: &MultiServiceRouter, case: &str, body: &Value) -> String {
    let created = create_ok(router, case, body).await;
    arn_of(case, &created)
}

/// The associations a read reported.
fn associations_of(case: &str, body: &Value) -> Vec<Value> {
    body.get("permissions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: response carried no permissions, got {body:?}"))
}

/// The (permission ARN, share ARN) pairs a read reported, in the order it reported them.
fn pairs_of(case: &str, body: &Value) -> Vec<Pair> {
    associations_of(case, body)
        .iter()
        .map(|association| {
            (
                field(case, association, "arn"),
                field(case, association, "resourceShareArn"),
            )
        })
        .collect()
}

/// A string member of a reported association.
fn field(case: &str, association: &Value, member: &str) -> String {
    association
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} member, got {association:?}"))
        .to_string()
}

/// A boolean member of a reported association.
fn flag(case: &str, association: &Value, member: &str) -> bool {
    association
        .get(member)
        .and_then(Value::as_bool)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} member, got {association:?}"))
}

/// The `nextToken` a read reported, if any.
fn token_of(body: &Value) -> Option<String> {
    body.get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// A read that must succeed, with everything it reported.
async fn list_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let (status, reported) = list_associations(router, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {reported:?}"
    );
    reported
}

/// Every association this store holds, read in one page.
async fn all_pairs(router: &MultiServiceRouter, case: &str) -> Vec<Pair> {
    let reported = list_ok(router, case, &json!({"maxResults": 500})).await;
    assert_eq!(
        token_of(&reported),
        None,
        "case {case:?}: one page held every association, so no nextToken is reported"
    );
    pairs_of(case, &reported)
}

/// The version the AWS managed permission catalog publishes `name` at, which is therefore
/// the version a share attaching it is governed by ([`AWS_MANAGED_PERMISSIONS`]).
fn catalog_version(name: &str) -> i32 {
    AWS_MANAGED_PERMISSIONS
        .iter()
        .find(|entry| entry.name == name)
        .map(|entry| entry.version)
        .unwrap_or_else(|| panic!("the catalog publishes {name}"))
}

/// The permission ARNs a read of ONE share's permissions reported, sorted.
async fn share_permission_arns(
    router: &MultiServiceRouter,
    case: &str,
    share_arn: &str,
) -> Vec<String> {
    let (status, reported) =
        list_share_permissions(router, &json!({"resourceShareArn": share_arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: reading the share's permissions expected 200, got {status} with \
         {reported:?}"
    );
    let mut arns: Vec<String> = associations_of(case, &reported)
        .iter()
        .map(|permission| field(case, permission, "arn"))
        .collect();
    arns.sort();
    arns
}

#[tokio::test]
async fn every_permission_governing_a_share_is_reported_as_an_association_once() {
    let router = emulator();
    let project_permission =
        customer_permission(&router, "assoc-project", PROJECT_TYPE, PROJECT_ACTION).await;
    let spare_permission =
        customer_permission(&router, "assoc-spare", PROJECT_TYPE, PROJECT_ACTION).await;
    let subnet_default = aws_managed_permission_arn(SUBNET_TYPE_DEFAULT);
    let glue_default = aws_managed_permission_arn("AWSRAMDefaultPermissionGlueDatabase");
    let project_default = aws_managed_permission_arn(PROJECT_TYPE_DEFAULT);

    // What a share is governed by, and so which permissions it is associated with.
    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "a share with no resources and no permissions",
            json!({"name": "assoc-empty"}),
            Vec::new(),
        ),
        (
            "a share that derives its shared type's default permission",
            json!({"name": "assoc-subnet", "resourceArns": [LOCAL_RESOURCE]}),
            vec![subnet_default.clone()],
        ),
        (
            "a share created naming a customer managed permission",
            json!({
                "name": "assoc-customer",
                "resourceArns": [PROJECT],
                "permissionArns": [project_permission.clone()],
            }),
            vec![project_permission.clone()],
        ),
        (
            "a share sharing two resource types",
            json!({
                "name": "assoc-two-types",
                "resourceArns": [LOCAL_RESOURCE, GLUE_DATABASE],
            }),
            vec![subnet_default.clone(), glue_default.clone()],
        ),
        (
            "a share whose named permission applies to a type it does not share",
            json!({
                "name": "assoc-mixed",
                "resourceArns": [LOCAL_RESOURCE],
                "permissionArns": [spare_permission.clone()],
            }),
            vec![spare_permission.clone(), subnet_default.clone()],
        ),
        (
            "a share whose shared type's default is attached by name",
            json!({
                "name": "assoc-named-default",
                "resourceArns": [PROJECT],
                "permissionArns": [project_default.clone()],
            }),
            vec![project_default.clone()],
        ),
    ];

    let mut expected: Vec<Pair> = Vec::new();
    let mut shares: Vec<(&str, String, Vec<String>)> = Vec::new();
    for (case, body, permissions) in rows {
        let share_arn = share_with(&router, case, &body).await;
        for permission_arn in &permissions {
            expected.push((permission_arn.clone(), share_arn.clone()));
        }
        shares.push((case, share_arn, permissions));
    }
    // The pairs come back in (permission ARN, share ARN) order, each of them once.
    expected.sort();

    let case = "every association";
    assert_eq!(
        all_pairs(&router, case).await,
        expected,
        "case {case:?}: every (permission, share) pair is reported once, in order"
    );

    // The same relation read from the other side has to agree, share by share.
    for (case, share_arn, permissions) in &shares {
        let mut from_the_share = permissions.clone();
        from_the_share.sort();
        assert_eq!(
            share_permission_arns(&router, case, share_arn).await,
            from_the_share,
            "case {case:?}: a read of the share's permissions reports the same permissions as \
             the read of their associations"
        );
    }

    // Every member of a reported association is the value the pair it names holds.
    let case = "the members of an association";
    let reported = list_ok(&router, case, &json!({"maxResults": 500})).await;
    for association in associations_of(case, &reported) {
        let permission_arn = field(case, &association, "arn");
        let expected_type = if permission_arn == subnet_default {
            SUBNET_TYPE
        } else if permission_arn == glue_default {
            GLUE_DATABASE_TYPE
        } else {
            PROJECT_TYPE
        };
        let expected_version = if permission_arn.starts_with("arn:aws:ram::aws:") {
            catalog_version(
                permission_arn
                    .rsplit('/')
                    .next()
                    .expect("a permission ARN names its permission"),
            )
        } else {
            FIRST_PERMISSION_VERSION
        };
        assert_eq!(
            field(case, &association, "resourceType"),
            expected_type,
            "case {case:?}: {permission_arn} applies to {expected_type}, got {association:?}"
        );
        assert_eq!(
            field(case, &association, "permissionVersion"),
            expected_version.to_string(),
            "case {case:?}: {permission_arn} is attached at its default version, got \
             {association:?}"
        );
        assert!(
            flag(case, &association, "defaultVersion"),
            "case {case:?}: {permission_arn} is attached at its default version, got \
             {association:?}"
        );
        assert_eq!(
            field(case, &association, "status"),
            SHARE_PERMISSION_STATUS_ASSOCIATED,
            "case {case:?}: an association a share is governed by reports that it is \
             associated, got {association:?}"
        );
        assert_eq!(
            field(case, &association, "featureSet"),
            FEATURE_SET_STANDARD,
            "case {case:?}: the association reports the share's featureSet, got {association:?}"
        );
        assert!(
            association.get("lastUpdatedTime").and_then(Value::as_f64) > Some(0.0),
            "case {case:?}: the association reports when it was last updated, got \
             {association:?}"
        );
    }
}

#[tokio::test]
async fn an_association_reports_the_version_of_the_permission_the_share_uses() {
    let router = emulator();
    let permission =
        customer_permission(&router, "version-project", PROJECT_TYPE, PROJECT_ACTION).await;
    let first = share_with(
        &router,
        "the share governed by the first version",
        &json!({
            "name": "version-first",
            "resourceArns": [PROJECT],
            "permissionArns": [permission.clone()],
        }),
    )
    .await;
    // A later version becomes the permission's default version, and "existing resource
    // shares continue to use their original permission versions".
    write_version(
        &router,
        "a second version of the permission",
        &permission,
        "codebuild:ListBuildsForProject",
    )
    .await;
    let second = share_with(
        &router,
        "the share governed by the second version",
        &json!({
            "name": "version-second",
            "resourceArns": [PROJECT],
            "permissionArns": [permission.clone()],
        }),
    )
    .await;

    let case = "two shares on two versions of one permission";
    let reported = list_ok(&router, case, &json!({"permissionArn": permission.clone()})).await;
    let versions: Vec<(String, String, bool)> = associations_of(case, &reported)
        .iter()
        .map(|association| {
            (
                field(case, association, "resourceShareArn"),
                field(case, association, "permissionVersion"),
                flag(case, association, "defaultVersion"),
            )
        })
        .collect();
    let mut expected = vec![
        (first.clone(), FIRST_PERMISSION_VERSION.to_string(), false),
        (
            second.clone(),
            (FIRST_PERMISSION_VERSION + 1).to_string(),
            true,
        ),
    ];
    expected.sort();
    let mut actual = versions.clone();
    actual.sort();
    assert_eq!(
        actual, expected,
        "case {case:?}: each share is reported at the version it is governed by, with \
         defaultVersion only for the permission's current default"
    );

    // A replacing attach updates the outdated share to the default version, which both
    // sides of the relation then report.
    let case = "an attach that updates the outdated share";
    let (status, updated) = attach(
        &router,
        &json!({
            "resourceShareArn": first.clone(),
            "permissionArn": permission.clone(),
            "replace": true,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {updated:?}"
    );
    let reported = list_ok(&router, case, &json!({"permissionArn": permission.clone()})).await;
    for association in associations_of(case, &reported) {
        assert_eq!(
            field(case, &association, "permissionVersion"),
            (FIRST_PERMISSION_VERSION + 1).to_string(),
            "case {case:?}: every share is now governed by the default version, got \
             {association:?}"
        );
        assert!(
            flag(case, &association, "defaultVersion"),
            "case {case:?}: every share is now governed by the default version, got \
             {association:?}"
        );
    }
}

#[tokio::test]
async fn each_filter_keeps_the_associations_it_names() {
    let router = emulator();
    let permission =
        customer_permission(&router, "filter-project", PROJECT_TYPE, PROJECT_ACTION).await;
    let subnet_default = aws_managed_permission_arn(SUBNET_TYPE_DEFAULT);
    let glue_read_write = aws_managed_permission_arn(GLUE_DATABASE_READ_WRITE);

    let project_share = share_with(
        &router,
        "the share that shares a project",
        &json!({
            "name": "filter-project-share",
            "resourceArns": [PROJECT],
            "permissionArns": [permission.clone()],
        }),
    )
    .await;
    let subnet_share = share_with(
        &router,
        "the share that shares a subnet",
        &json!({"name": "filter-subnet-share", "resourceArns": [LOCAL_RESOURCE]}),
    )
    .await;
    // A share that shares a subnet AND holds a permission for another resource type, so
    // the `resourceType` filter — which is about the resources the SHARE shares — keeps
    // both of its associations.
    let mixed_share = share_with(
        &router,
        "the share that shares a subnet and holds a project permission",
        &json!({
            "name": "filter-mixed-share",
            "resourceArns": [LOCAL_RESOURCE],
            "permissionArns": [permission.clone()],
        }),
    )
    .await;
    // A later version of the permission leaves the shares above on the first one.
    write_version(
        &router,
        "a second version of the filtered permission",
        &permission,
        "codebuild:ListBuildsForProject",
    )
    .await;
    let glue_share = share_with(
        &router,
        "the share that shares a glue database",
        &json!({
            "name": "filter-glue-share",
            "resourceArns": [GLUE_DATABASE],
            "permissionArns": [glue_read_write.clone()],
        }),
    )
    .await;

    let everything: Vec<Pair> = all_pairs(&router, "no filter at all").await;
    let rows: Vec<FilterRow> = vec![
        (
            "permissionArn naming the customer managed permission",
            json!({"permissionArn": permission.clone()}),
            vec![
                (permission.clone(), project_share.clone()),
                (permission.clone(), mixed_share.clone()),
            ],
        ),
        (
            "permissionArn naming a permission nothing is governed by",
            json!({"permissionArn": aws_managed_permission_arn(PROJECT_TYPE_DEFAULT)}),
            Vec::new(),
        ),
        (
            "permissionVersion naming the version both shares attached",
            json!({"permissionVersion": FIRST_PERMISSION_VERSION}),
            everything
                .iter()
                .filter(|(arn, _)| arn != &glue_read_write)
                .cloned()
                .collect(),
        ),
        (
            "permissionVersion naming the catalog version of the attached permission",
            json!({"permissionVersion": catalog_version(GLUE_DATABASE_READ_WRITE)}),
            vec![(glue_read_write.clone(), glue_share.clone())],
        ),
        (
            "permissionVersion naming a version nothing uses",
            json!({"permissionVersion": 99}),
            Vec::new(),
        ),
        (
            "defaultVersion true, which keeps only the shares on the default version",
            json!({"defaultVersion": true}),
            everything
                .iter()
                .filter(|(arn, _)| arn != &permission)
                .cloned()
                .collect(),
        ),
        (
            "defaultVersion false, which keeps associations on any version",
            json!({"defaultVersion": false}),
            everything.clone(),
        ),
        (
            "featureSet the shares carry",
            json!({"featureSet": FEATURE_SET_STANDARD}),
            everything.clone(),
        ),
        (
            "featureSet no share carries",
            json!({"featureSet": "CREATED_FROM_POLICY"}),
            Vec::new(),
        ),
        (
            "associationStatus the associations carry",
            json!({"associationStatus": SHARE_PERMISSION_STATUS_ASSOCIATED}),
            everything.clone(),
        ),
        (
            "associationStatus no association carries",
            json!({"associationStatus": "DISASSOCIATED"}),
            Vec::new(),
        ),
        (
            "resourceType the subnet shares share",
            json!({"resourceType": SUBNET_TYPE}),
            vec![
                (permission.clone(), mixed_share.clone()),
                (subnet_default.clone(), subnet_share.clone()),
                (subnet_default.clone(), mixed_share.clone()),
            ],
        ),
        (
            "resourceType spelled as a resource's own ARN spells it",
            json!({"resourceType": "codebuild:project"}),
            vec![(permission.clone(), project_share.clone())],
        ),
        (
            "resourceType no share shares",
            json!({"resourceType": UNSHARED_TYPE}),
            Vec::new(),
        ),
        (
            "two filters at once, which keep the pairs both name",
            json!({
                "permissionArn": permission.clone(),
                "resourceType": SUBNET_TYPE,
            }),
            vec![(permission.clone(), mixed_share.clone())],
        ),
        (
            "resourceShareArn naming the share that holds one permission",
            json!({"resourceShareArn": project_share.clone()}),
            vec![(permission.clone(), project_share.clone())],
        ),
        (
            "resourceShareArn naming the share that holds two",
            json!({"resourceShareArn": mixed_share.clone()}),
            vec![
                (permission.clone(), mixed_share.clone()),
                (subnet_default.clone(), mixed_share.clone()),
            ],
        ),
        (
            "resourceShareArn naming a share this account does not hold",
            json!({
                "resourceShareArn":
                    format!(
                        "arn:aws:ram:{REGION}:{ACCOUNT}:resource-share/\
                         00000000-0000-0000-0000-000000000000"
                    ),
            }),
            Vec::new(),
        ),
        (
            "resourceShareArn and permissionArn, which keep the one pair both name",
            json!({
                "resourceShareArn": mixed_share.clone(),
                "permissionArn": subnet_default.clone(),
            }),
            vec![(subnet_default.clone(), mixed_share.clone())],
        ),
        (
            "resourceOwner SELF, the documented default, which keeps the shares this \
             account owns",
            json!({"resourceOwner": "SELF"}),
            everything.clone(),
        ),
        (
            "resourceOwner OTHER-ACCOUNTS, which keeps the shares owned elsewhere",
            json!({"resourceOwner": "OTHER-ACCOUNTS"}),
            Vec::new(),
        ),
    ];

    for (case, mut body, mut wanted) in rows {
        body["maxResults"] = json!(500);
        let reported = list_ok(&router, case, &body).await;
        let mut actual = pairs_of(case, &reported);
        actual.sort();
        wanted.sort();
        assert_eq!(
            actual, wanted,
            "case {case:?}: the filter keeps exactly the associations it names"
        );
    }
}

#[tokio::test]
async fn paging_reports_every_association_once_in_order() {
    let router = emulator();
    let permission =
        customer_permission(&router, "paging-project", PROJECT_TYPE, PROJECT_ACTION).await;
    // Several shares governed by the same permission and by a derived default, so the
    // pairs a page cuts through run over both ARNs of the key.
    for index in 0..3 {
        share_with(
            &router,
            "a paged share",
            &json!({
                "name": format!("paging-share-{index}"),
                "resourceArns": [PROJECT, LOCAL_RESOURCE],
                "permissionArns": [permission.clone()],
            }),
        )
        .await;
    }
    let all = all_pairs(&router, "every association").await;
    assert_eq!(
        all.len(),
        6,
        "three shares governed by two permissions each hold six associations, got {all:?}"
    );

    // The published bounds of `maxResults` (1..500) and sizes between them: every
    // association comes back once, in one order, however it is cut into pages.
    for size in [1usize, 2, 5, all.len(), 500] {
        let case = format!("maxResults {size}");
        let mut seen: Vec<Pair> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut body = json!({"maxResults": size});
            if let Some(resume) = &token {
                body["nextToken"] = json!(resume);
            }
            let reported = list_ok(&router, &case, &body).await;
            let page = pairs_of(&case, &reported);
            assert!(
                page.len() <= size,
                "case {case:?}: a page reported {} entries, more than the {size} asked for",
                page.len()
            );
            seen.extend(page);
            token = token_of(&reported);
            pages += 1;
            assert!(
                pages <= all.len() + 1,
                "case {case:?}: paging did not finish after {pages} pages, got {seen:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, all,
            "case {case:?}: paging reported every association exactly once, in order"
        );
    }

    // One past each published bound is refused by the modelled range, not clamped.
    for size in [0, 501] {
        let case = format!("maxResults {size}");
        let (status, refused) = list_associations(&router, &json!({"maxResults": size})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a page size outside 1..500 is refused, got {status} with {refused:?}"
        );
    }
}

#[tokio::test]
async fn an_association_a_later_call_took_away_is_no_longer_reported() {
    let router = emulator();
    let permission =
        customer_permission(&router, "gone-project", PROJECT_TYPE, PROJECT_ACTION).await;
    let kept = share_with(
        &router,
        "the share that keeps its permission",
        &json!({"name": "gone-kept", "permissionArns": [permission.clone()]}),
    )
    .await;
    let removed_from = share_with(
        &router,
        "the share the permission is removed from",
        &json!({"name": "gone-removed", "permissionArns": [permission.clone()]}),
    )
    .await;
    let deleted = share_with(
        &router,
        "the share that is deleted",
        &json!({"name": "gone-deleted", "permissionArns": [permission.clone()]}),
    )
    .await;

    let case = "three shares governed by one permission";
    let mut expected = vec![
        (permission.clone(), kept.clone()),
        (permission.clone(), removed_from.clone()),
        (permission.clone(), deleted.clone()),
    ];
    expected.sort();
    assert_eq!(
        all_pairs(&router, case).await,
        expected,
        "case {case:?}: every share the permission governs is associated with it"
    );

    let case = "a removal takes the permission out of one share";
    let (status, answered) = remove(
        &router,
        &json!({"resourceShareArn": removed_from.clone(), "permissionArn": permission.clone()}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {answered:?}"
    );

    let case = "a delete removes the share";
    let (status, answered) = delete(&router, &deleted, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {answered:?}"
    );

    let case = "what is left associated";
    assert_eq!(
        all_pairs(&router, case).await,
        vec![(permission.clone(), kept.clone())],
        "case {case:?}: neither the removed permission nor the deleted share is associated"
    );
}

#[tokio::test]
async fn a_request_this_read_cannot_serve_is_refused() {
    let router = emulator();
    let permission =
        customer_permission(&router, "refusal-project", PROJECT_TYPE, PROJECT_ACTION).await;
    let share_arn = share_with(
        &router,
        "a share for the refusal rows",
        &json!({"name": "refusal-share", "permissionArns": [permission.clone()]}),
    )
    .await;

    let rows: Vec<(&str, Value, &str, Option<String>)> = vec![
        (
            "a permissionArn that is not an ARN",
            json!({"permissionArn": "permission/abc"}),
            "MalformedArnException",
            None,
        ),
        (
            "a permissionArn that names a resource share",
            json!({"permissionArn": share_arn.clone()}),
            "MalformedArnException",
            None,
        ),
        (
            "a permissionVersion of zero",
            json!({"permissionVersion": 0}),
            "InvalidParameterException",
            Some(format!(
                "permissionVersion must be {FIRST_PERMISSION_VERSION} or greater"
            )),
        ),
        (
            "a negative permissionVersion",
            json!({"permissionVersion": -1}),
            "InvalidParameterException",
            Some(format!(
                "permissionVersion must be {FIRST_PERMISSION_VERSION} or greater"
            )),
        ),
        (
            "a nextToken this read did not issue",
            json!({"nextToken": "not-a-token"}),
            "InvalidNextTokenException",
            None,
        ),
        (
            "an empty nextToken",
            json!({"nextToken": ""}),
            "InvalidNextTokenException",
            None,
        ),
        (
            "a nextToken naming one ARN rather than a pair",
            json!({"nextToken": encode_arn_cursor(&permission)}),
            "InvalidNextTokenException",
            None,
        ),
        (
            "a nextToken whose pair is the wrong way round",
            json!({"nextToken": encode_permission_association_cursor(&share_arn, &permission)}),
            "InvalidNextTokenException",
            None,
        ),
        (
            "a resourceShareArn that is not an ARN",
            json!({"resourceShareArn": "resource-share/abc"}),
            "MalformedArnException",
            None,
        ),
        (
            "a resourceShareArn that names a managed permission",
            json!({"resourceShareArn": permission.clone()}),
            "MalformedArnException",
            None,
        ),
    ];

    for (case, body, expected_code, expected_message) in rows {
        let (status, refused) = list_associations(&router, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            expected_code,
            "case {case:?}: expected {expected_code}, got {refused:?}"
        );
        if let Some(message) = expected_message {
            assert_eq!(
                error_message(&refused),
                message,
                "case {case:?}: expected the modelled message, got {refused:?}"
            );
        }
    }

    // The token a page DID issue resumes the read at the pair it names.
    let case = "the token a page issued";
    let second = share_with(
        &router,
        "a second share for the resume row",
        &json!({"name": "refusal-share-two", "permissionArns": [permission.clone()]}),
    )
    .await;
    let first_page = list_ok(&router, case, &json!({"maxResults": 1})).await;
    let resume = token_of(&first_page).unwrap_or_else(|| {
        panic!("case {case:?}: a first page of one of two carries a nextToken, got {first_page:?}")
    });
    let next_page = list_ok(
        &router,
        case,
        &json!({"maxResults": 1, "nextToken": resume}),
    )
    .await;
    let mut both = pairs_of(case, &first_page);
    both.extend(pairs_of(case, &next_page));
    let mut expected = vec![
        (permission.clone(), share_arn.clone()),
        (permission.clone(), second.clone()),
    ];
    expected.sort();
    both.sort();
    assert_eq!(
        both, expected,
        "case {case:?}: the two pages together hold every association, each once"
    );
}
