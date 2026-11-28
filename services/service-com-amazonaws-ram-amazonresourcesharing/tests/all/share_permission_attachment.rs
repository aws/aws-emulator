// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `AssociateResourceSharePermission` and what it makes
//! `ListResourceSharePermissions` report.
//!
//! The rule the rows below pin: a managed permission attached to a resource share
//! becomes part of that share's permission set — a share is governed by the permissions
//! it was created with, the ones attached to it afterwards, and the default permission
//! of every resource type it shares that none of those applies to — and a read of the
//! share's permissions reports each of them at the version the share attached, with
//! `status` reporting the attachment to that share rather than the version's own
//! attachability.
//!
//! So the rows vary along what the rule generalises over:
//!
//! * WHICH permission is attached: a customer managed one a create minted, an AWS
//!   managed one that is not its resource type's default, and several of different
//!   resource types;
//! * WHAT the share already carries: nothing, a permission the create named, the derived
//!   default of a type it shares, and a shared resource type the catalog publishes no
//!   permission for at all;
//! * HOW MANY attachments one page holds: a page size below, at and above the number
//!   attached;
//! * WHICH VERSION of the permission the share is governed by: the one it attached, which
//!   a later version of the permission does not move, and the one a replacing attach
//!   updates it to;
//! * WHAT the `replace` parameter governs: it answers for the collision with another
//!   permission of the same resource type only, so an attach that collides with nothing
//!   is served however the request sets it;
//! * WHICH SHARE an attach reaches: the one it names, and no other share of the same
//!   resource;
//! * WHAT a `clientToken` answers for: a repeat of THIS operation's call, so a token
//!   another operation spent, or one a refused attach carried, still serves an attach;
//! * WHAT an attach that cannot be made answers: a value that is not an ARN of the
//!   resource it names, a share or permission nothing answers to, a deleted share, a
//!   version the permission does not carry or that is not its default, a second
//!   permission for a resource type the share already has one for, and a `clientToken`
//!   reused for a different request.
//!
//! Writing a version of a permission is what makes "the version the share attached"
//! observable at all, so this suite drives `CreatePermissionVersion` too (its own stub
//! issue: 20261103T040218Z-ram-createpermissionversion-stub.md).
//!
//! The REMOVAL half of the same rule is driven by the rows at the end of this file:
//! `DisassociateResourceSharePermission` takes a managed permission back out of a
//! share's permission set, and may do so only while the share shares no resources that
//! permission is the last one governing. Those rows vary along what the removal rule
//! generalises over too:
//!
//! * WHICH permission is removed: a resource type's default managed permission, an AWS
//!   managed permission that is not its type's default, and a customer managed one;
//! * WHICH READ observes the removal: a read of the share's own permissions, and the
//!   `permissionArn` filter of a read of the shares — both answer which permissions
//!   govern a share, so both have to stop reporting the one removed;
//! * WHICH SHARE a removal reaches: the one it names, and no other share governed by the
//!   same permission;
//! * WHETHER it is permanent: an attach after a removal governs the share by that
//!   permission again, and a resource newly shared derives its type's default again;
//! * WHAT a `clientToken` answers for: a repeat of THAT removal, whether the token was
//!   the caller's or the one the removal minted for a request that carried none — and
//!   not for a removal on a share that is gone, nor for a different removal.
//!
//! The BULK half of the same rule is driven by the rows at the very end of this file:
//! `ReplacePermissionAssociations` moves every resource share governed by one managed
//! permission onto another one, at that permission's current default version, as a
//! background task the call reports and `ListReplacePermissionAssociationsWork` reads
//! back (its own stub issue: 20261123T230154Z-ram-replacepermissionassociations-stub.md).
//! Those rows vary along what the bulk rule generalises over too:
//!
//! * WHICH permissions the task names: the same permission on both ends (which moves
//!   every share onto its current default version), two customer managed permissions, a
//!   customer managed one replaced by an AWS managed one, and an AWS managed one replaced
//!   by a customer managed one;
//! * WHICH VERSION the request restricts the task to: none at all, and one version of
//!   several — which moves the shares holding that version and leaves the others alone;
//! * WHICH SHARES the task reaches: the ones governed by the old permission, not a share
//!   governed by another permission, and not an attachment of another resource type on a
//!   share it does reach;
//! * WHEN the move is observable: not while the task is in flight, and on every later
//!   read once the clock has made it due;
//! * WHAT a `clientToken` answers for: a repeat of THAT request, which starts no second
//!   task, and not a different request that reuses the token;
//! * HOW the tasks are read back: by id, by status, and one page at a time;
//! * WHAT a replacement that cannot be made answers: a value that is not a permission
//!   ARN, a version below the first one or one the permission never carried, a permission
//!   nothing answers to, and two permissions for different resource types.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_ram_amazonresourcesharing::state::{
    aws_managed_permission_arn, last_permission_for_resource_type_message,
    permission_arn_rejection, permission_not_attached_message,
    permission_resource_type_mismatch_message, permission_version_below_first_message,
    share_arn_rejection, unknown_permission_message, unknown_permission_version_message,
    unknown_share_message, FIRST_PERMISSION_VERSION, IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE,
    MAX_ENTITIES_PER_CALL, MAX_PERMISSION_VERSIONS, PERMISSION_FEATURE_SET_STANDARD,
    PERMISSION_TYPE_CUSTOMER_MANAGED, REPLACE_PERMISSION_ASSOCIATIONS_COMPLETED_AFTER_MILLIS,
    REPLACE_WORK_STATUS_COMPLETED, REPLACE_WORK_STATUS_IN_PROGRESS,
    SHARE_PERMISSION_STATUS_ASSOCIATED,
};
use smithy_server_runtime::MultiServiceRouter;

// Every suite in this crate calls the service the one way: through `tests/all/harness.rs`.
use super::harness::*;

/// The resource type the recorded lifecycle shared a customer managed permission for.
/// `ListResourceTypes` reports it (`RESOURCE_TYPES`), so a share of one is governed by
/// the permission the AWS managed catalog publishes as its default
/// ([`COMPONENT_TYPE_DEFAULT`]) until a permission attached by name stands in for it.
const COMPONENT_TYPE: &str = "imagebuilder:Component";
/// An Image Builder component: a resource of [`COMPONENT_TYPE`].
const COMPONENT: &str = "arn:aws:imagebuilder:us-west-2:000000000000:component/hardening/1.0.0/1";
/// The permission Amazon Web Services publishes as [`COMPONENT_TYPE`]'s default, which
/// therefore governs a share that shares a component and names none of its own.
const COMPONENT_TYPE_DEFAULT: &str = "AWSRAMDefaultPermissionImageBuilderComponent";
/// A Glue database, a resource type the catalog publishes two permissions for.
const GLUE_DATABASE: &str = "arn:aws:glue:us-west-2:000000000000:database/analytics";

async fn attach(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(
        router,
        "POST",
        "/associateresourcesharepermission",
        Some(body),
    )
    .await
}

async fn list_permissions(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/listresourcesharepermissions", Some(body)).await
}

async fn create_permission_version(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/createpermissionversion", Some(body)).await
}

async fn get_permission(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/getpermission", Some(body)).await
}

/// A policy template allowing `action`, as both permission writes take one.
fn policy_template(action: &str) -> String {
    json!({"Effect": "ALLOW", "Action": [action]}).to_string()
}

/// The ARN of the AWS managed permission of this name.
fn managed(name: &str) -> String {
    aws_managed_permission_arn(name)
}

/// The ARN a customer managed permission of this name is minted under.
fn customer(name: &str) -> String {
    format!("arn:aws:ram:{REGION}:{ACCOUNT}:permission/{name}")
}

/// Mint a customer managed permission for `resource_type` and answer its ARN.
async fn mint(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    resource_type: &str,
    action: &str,
) -> String {
    let (status, minted) = call(
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
        "case {case:?}: expected 200 from the permission create, got {status} with {minted:?}"
    );
    customer(name)
}

/// Create a share and answer its ARN.
async fn share_with(router: &MultiServiceRouter, case: &str, request: &Value) -> String {
    let created = create_ok(router, case, request).await;
    arn_of(case, &created)
}

/// The permissions a read reported.
fn permissions_of(case: &str, body: &Value) -> Vec<Value> {
    body.get("permissions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: response carried no permissions, got {body:?}"))
}

/// A string member of a reported permission.
fn field(case: &str, permission: &Value, member: &str) -> String {
    permission
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} member, got {permission:?}"))
        .to_string()
}

/// A boolean member of a reported permission.
fn flag(case: &str, permission: &Value, member: &str) -> bool {
    permission
        .get(member)
        .and_then(Value::as_bool)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} member, got {permission:?}"))
}

/// Read every permission of `share_arn`, asserting HTTP 200 and that each one reports
/// its attachment to the share, and answer them in the order they were reported.
async fn listed(router: &MultiServiceRouter, case: &str, share_arn: &str) -> Vec<Value> {
    let (status, body) = list_permissions(router, &json!({"resourceShareArn": share_arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {body:?}"
    );
    let reported = permissions_of(case, &body);
    for permission in &reported {
        assert_eq!(
            field(case, permission, "status"),
            SHARE_PERMISSION_STATUS_ASSOCIATED,
            "case {case:?}: a permission the share holds reports its attachment, got \
             {permission:?}"
        );
    }
    reported
}

/// The ARNs of every permission `share_arn` reports.
async fn listed_arns(router: &MultiServiceRouter, case: &str, share_arn: &str) -> Vec<String> {
    listed(router, case, share_arn)
        .await
        .iter()
        .map(|permission| field(case, permission, "arn"))
        .collect()
}

/// Attach `permission_arn` to `share_arn`, asserting the attach succeeded.
async fn attach_ok(router: &MultiServiceRouter, case: &str, share_arn: &str, permission_arn: &str) {
    attach_served(router, case, share_arn, permission_arn, None).await;
}

/// The same, for a request that says something about `replace`: `None` omits the member,
/// `Some(..)` sends that value.
async fn attach_served(
    router: &MultiServiceRouter,
    case: &str,
    share_arn: &str,
    permission_arn: &str,
    replace: Option<bool>,
) {
    let mut request = json!({"resourceShareArn": share_arn, "permissionArn": permission_arn});
    if let Some(replace) = replace {
        request["replace"] = json!(replace);
    }
    let (status, answered) = attach(router, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 attaching {permission_arn} with replace {replace:?}, \
         got {status} with {answered:?}"
    );
    assert_eq!(
        answered.get("returnValue").and_then(Value::as_bool),
        Some(true),
        "case {case:?}: a successful attach reports returnValue true, got {answered:?}"
    );
    assert!(
        answered
            .get("clientToken")
            .and_then(Value::as_str)
            .is_some_and(|token| !token.is_empty()),
        "case {case:?}: an attach reports the idempotency token it used, got {answered:?}"
    );
}

/// One row of an attachment table: a share, the permissions attached to it afterwards
/// and the permission set it must then report.
struct AttachRow {
    case: &'static str,
    /// The resources the share shares, so which defaults it derives varies.
    resource_arns: Vec<&'static str>,
    /// The permissions the create request named.
    created_with: Vec<String>,
    /// The permissions attached after creation, in order.
    attached: Vec<String>,
    /// What every attach of this row says about `replace`: nothing at all, `false` or
    /// `true`.
    replace: Option<bool>,
    /// Every permission ARN the share must report afterwards.
    expected: Vec<String>,
}

impl AttachRow {
    /// Create this row's share, make its attaches and assert the share then reports
    /// exactly [`AttachRow::expected`], in the permission ARN order a read pages in.
    async fn run(&self, router: &MultiServiceRouter, name: &str) {
        let case = self.case;
        let share_arn = share_with(
            router,
            case,
            &json!({
                "name": name,
                "resourceArns": self.resource_arns,
                "permissionArns": self.created_with,
            }),
        )
        .await;
        // Before the attach the share carries only what it was created with and what it
        // derives, so the attach is what the row observes.
        let before = listed_arns(router, case, &share_arn).await;
        for permission_arn in &self.attached {
            attach_served(router, case, &share_arn, permission_arn, self.replace).await;
        }
        // A read reports a share's permissions in permission ARN order, so the
        // expectation is the row's set in that order.
        let mut wanted = self.expected.clone();
        wanted.sort();
        assert_eq!(
            listed_arns(router, case, &share_arn).await,
            wanted,
            "case {case:?}: attaching {:?} with replace {:?} to a share carrying {before:?} must \
             leave exactly {wanted:?}",
            self.attached,
            self.replace
        );
    }
}

#[tokio::test]
async fn an_attached_managed_permission_joins_the_shares_permission_set() {
    let router = emulator();
    let component_reader = mint(
        &router,
        "mint a component permission",
        "ComponentReader",
        COMPONENT_TYPE,
        "imagebuilder:GetComponent",
    )
    .await;

    let rows = [
        AttachRow {
            case: "a share that shares nothing takes a customer managed permission",
            resource_arns: vec![],
            created_with: vec![],
            attached: vec![component_reader.clone()],
            replace: None,
            expected: vec![component_reader.clone()],
        },
        AttachRow {
            case: "a share takes an AWS managed permission that is not a type default",
            resource_arns: vec![],
            created_with: vec![],
            attached: vec![managed("AWSRAMPermissionGlueDatabaseReadWrite")],
            replace: None,
            expected: vec![managed("AWSRAMPermissionGlueDatabaseReadWrite")],
        },
        AttachRow {
            case: "the permission attached for a shared type stands in for that type's default",
            resource_arns: vec![GLUE_DATABASE],
            created_with: vec![],
            attached: vec![managed("AWSRAMPermissionGlueDatabaseReadWrite")],
            replace: None,
            expected: vec![managed("AWSRAMPermissionGlueDatabaseReadWrite")],
        },
        AttachRow {
            case: "an attachment beside the derived default of another type",
            resource_arns: vec![LOCAL_RESOURCE],
            created_with: vec![],
            attached: vec![component_reader.clone()],
            replace: None,
            expected: vec![
                managed("AWSRAMDefaultPermissionSubnet"),
                component_reader.clone(),
            ],
        },
        AttachRow {
            case: "an attachment standing in for the published default of the type it shares",
            resource_arns: vec![COMPONENT],
            created_with: vec![],
            attached: vec![component_reader.clone()],
            replace: None,
            expected: vec![component_reader.clone()],
        },
        AttachRow {
            case: "two permissions of different resource types are both carried",
            resource_arns: vec![],
            created_with: vec![],
            attached: vec![
                managed("AWSRAMPermissionGlueDatabaseReadWrite"),
                managed("AWSRAMDefaultPermissionPrefixList"),
            ],
            replace: None,
            expected: vec![
                managed("AWSRAMDefaultPermissionPrefixList"),
                managed("AWSRAMPermissionGlueDatabaseReadWrite"),
            ],
        },
        AttachRow {
            case: "the same permission attached twice is carried once",
            resource_arns: vec![],
            created_with: vec![],
            attached: vec![component_reader.clone(), component_reader.clone()],
            replace: None,
            expected: vec![component_reader.clone()],
        },
        AttachRow {
            case: "an attachment beside a permission the create named",
            resource_arns: vec![],
            created_with: vec![managed("AWSRAMDefaultPermissionSubnet")],
            attached: vec![component_reader.clone()],
            replace: None,
            expected: vec![
                managed("AWSRAMDefaultPermissionSubnet"),
                component_reader.clone(),
            ],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        row.run(&router, &format!("attach-row-{index}")).await;
    }
}

#[tokio::test]
async fn an_attached_permission_reads_back_as_the_permission_that_was_attached() {
    let router = emulator();
    let case = "a customer managed permission attached to a share";
    let permission_arn = mint(
        &router,
        case,
        "ComponentConsumer",
        COMPONENT_TYPE,
        "imagebuilder:GetComponent",
    )
    .await;
    let share_arn = share_with(
        &router,
        case,
        &json!({"name": "component-share", "resourceArns": [COMPONENT]}),
    )
    .await;
    assert_eq!(
        listed_arns(&router, case, &share_arn).await,
        vec![managed(COMPONENT_TYPE_DEFAULT)],
        "case {case:?}: before the attach the share is governed by the published default of \
         the type it shares"
    );

    attach_ok(&router, case, &share_arn, &permission_arn).await;

    let reported = listed(&router, case, &share_arn).await;
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: the share carries the one permission attached, got {reported:?}"
    );
    let permission = &reported[0];
    assert_eq!(
        field(case, permission, "arn"),
        permission_arn,
        "case {case:?}: expected the attached permission's ARN, got {permission:?}"
    );
    assert_eq!(
        field(case, permission, "name"),
        "ComponentConsumer",
        "case {case:?}: expected the name the create stored, got {permission:?}"
    );
    assert_eq!(
        field(case, permission, "resourceType"),
        COMPONENT_TYPE,
        "case {case:?}: expected the resource type the create named, got {permission:?}"
    );
    assert_eq!(
        field(case, permission, "permissionType"),
        PERMISSION_TYPE_CUSTOMER_MANAGED,
        "case {case:?}: a permission a caller created reports CUSTOMER_MANAGED, got {permission:?}"
    );
    assert_eq!(
        field(case, permission, "featureSet"),
        PERMISSION_FEATURE_SET_STANDARD,
        "case {case:?}: expected the STANDARD feature set, got {permission:?}"
    );
    assert_eq!(
        field(case, permission, "version"),
        FIRST_PERMISSION_VERSION.to_string(),
        "case {case:?}: the version attached is the permission's default version, got \
         {permission:?}"
    );
    assert!(
        flag(case, permission, "defaultVersion"),
        "case {case:?}: the attached version is the permission's default version, got \
         {permission:?}"
    );
    assert!(
        !flag(case, permission, "isResourceTypeDefault"),
        "case {case:?}: a created permission is not its resource type's default, got \
         {permission:?}"
    );
    assert!(
        permission.get("lastUpdatedTime").is_some_and(Value::is_number),
        "case {case:?}: a reported permission carries a lastUpdatedTime, got {permission:?}"
    );
    assert_eq!(
        permission.get("tags").and_then(Value::as_array),
        Some(&vec![]),
        "case {case:?}: a permission carrying no tags reports an empty list, got {permission:?}"
    );

    // The read changed nothing a later caller sees.
    let (first_status, first) =
        list_permissions(&router, &json!({"resourceShareArn": share_arn.clone()})).await;
    let (second_status, second) =
        list_permissions(&router, &json!({"resourceShareArn": share_arn})).await;
    assert_eq!(
        (first_status, second_status),
        (200, 200),
        "case {case:?}: both reads expected 200, got {first_status} then {second_status}"
    );
    assert_eq!(
        first, second,
        "case {case:?}: a read reported something else the second time, got {second:?}"
    );
}

#[tokio::test]
async fn a_second_permission_for_one_resource_type_needs_the_replace_parameter() {
    let router = emulator();
    let case = "one permission per resource type";
    let first = mint(
        &router,
        case,
        "ComponentReaderOne",
        COMPONENT_TYPE,
        "imagebuilder:GetComponent",
    )
    .await;
    let second = mint(
        &router,
        case,
        "ComponentReaderTwo",
        COMPONENT_TYPE,
        "imagebuilder:ListComponents",
    )
    .await;
    let share_arn = share_with(
        &router,
        case,
        &json!({"name": "one-per-type", "resourceArns": [COMPONENT]}),
    )
    .await;
    attach_ok(&router, case, &share_arn, &first).await;

    let (status, refused) = attach(
        &router,
        &json!({"resourceShareArn": share_arn, "permissionArn": second}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 for a second permission of one type, got {status} with \
         {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidParameterException",
        "case {case:?}: expected InvalidParameterException, got {refused:?}"
    );
    assert_eq!(
        error_message(&refused),
        format!(
            "Resource share {share_arn} already has permission {first} associated for resource \
             type {COMPONENT_TYPE}. Set replace to true to replace it."
        ),
        "case {case:?}: the refusal names the type, the permission and the parameter, got \
         {refused:?}"
    );
    assert_eq!(
        listed_arns(&router, case, &share_arn).await,
        vec![first.clone()],
        "case {case:?}: a refused attach leaves the share's permission set alone"
    );

    // The same request with `replace` set does what it was refused for: the share ends
    // up governed by the second permission and no longer by the first.
    let case = "replace the permission of a resource type";
    let (status, replaced) = attach(
        &router,
        &json!({"resourceShareArn": share_arn, "permissionArn": second, "replace": true}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {replaced:?}"
    );
    assert_eq!(
        listed_arns(&router, case, &share_arn).await,
        vec![second.clone()],
        "case {case:?}: a replacing attach leaves only the permission it attached"
    );

    // A permission of ANOTHER resource type collides with nothing, so it needs no
    // replace of its own.
    let case = "a permission of another resource type";
    attach_ok(
        &router,
        case,
        &share_arn,
        &managed("AWSRAMPermissionGlueDatabaseReadWrite"),
    )
    .await;
    let mut wanted = vec![
        second,
        managed("AWSRAMPermissionGlueDatabaseReadWrite"),
    ];
    wanted.sort();
    assert_eq!(
        listed_arns(&router, case, &share_arn).await,
        wanted,
        "case {case:?}: the share carries one permission per resource type"
    );
}

#[tokio::test]
async fn an_attach_that_cannot_be_made_is_refused_and_attaches_nothing() {
    let router = emulator();
    let case = "a share to attach to";
    let permission_arn = mint(
        &router,
        case,
        "ComponentRefusals",
        COMPONENT_TYPE,
        "imagebuilder:GetComponent",
    )
    .await;
    let share_arn = share_with(
        &router,
        case,
        &json!({"name": "refusal-share", "resourceArns": [COMPONENT]}),
    )
    .await;
    let doomed = share_with(
        &router,
        case,
        &json!({"name": "deleted-share", "resourceArns": [COMPONENT]}),
    )
    .await;
    let (status, removed) = delete(&router, &doomed, None).await;
    assert_eq!(
        status, 200,
        "case \"delete a share\": expected 200, got {status} with {removed:?}"
    );
    let unknown_share =
        format!("arn:aws:ram:{REGION}:{ACCOUNT}:resource-share/11111111-2222-3333-4444-555555555555");
    let unknown_permission = customer("NeverCreated");

    // Each row: the request, the modelled error class it must be refused with, and the
    // message that class carries where this service words one.
    let rows: Vec<(&str, Value, &str, Option<String>)> = vec![
        (
            "a resourceShareArn that is not an ARN",
            json!({"resourceShareArn": "resource-share/abc", "permissionArn": permission_arn}),
            "MalformedArnException",
            None,
        ),
        (
            "a resourceShareArn naming another service",
            json!({
                "resourceShareArn": format!("arn:aws:ec2:{REGION}:{ACCOUNT}:resource-share/abc"),
                "permissionArn": permission_arn,
            }),
            "MalformedArnException",
            None,
        ),
        (
            "a permissionArn that names a share",
            json!({"resourceShareArn": share_arn, "permissionArn": share_arn}),
            "MalformedArnException",
            None,
        ),
        (
            "a share nothing was created under",
            json!({"resourceShareArn": unknown_share, "permissionArn": permission_arn}),
            "UnknownResourceException",
            Some(format!("ResourceShare {unknown_share} could not be found.")),
        ),
        (
            "a share that was deleted",
            json!({"resourceShareArn": doomed, "permissionArn": permission_arn}),
            "UnknownResourceException",
            Some(format!("ResourceShare {doomed} could not be found.")),
        ),
        (
            "a permission nothing was created under",
            json!({"resourceShareArn": share_arn, "permissionArn": unknown_permission}),
            "UnknownResourceException",
            Some(format!(
                "Permission {unknown_permission} could not be found."
            )),
        ),
        (
            "a permission version below the first one",
            json!({
                "resourceShareArn": share_arn,
                "permissionArn": permission_arn,
                "permissionVersion": 0,
            }),
            "InvalidParameterException",
            Some(format!(
                "permissionVersion must be {FIRST_PERMISSION_VERSION} or greater"
            )),
        ),
        (
            "a permission version the permission does not carry",
            json!({
                "resourceShareArn": share_arn,
                "permissionArn": permission_arn,
                "permissionVersion": FIRST_PERMISSION_VERSION + 1,
            }),
            "UnknownResourceException",
            Some(format!(
                "Permission {permission_arn} has no version {}.",
                FIRST_PERMISSION_VERSION + 1
            )),
        ),
        (
            "an empty clientToken",
            json!({
                "resourceShareArn": share_arn,
                "permissionArn": permission_arn,
                "clientToken": "",
            }),
            "InvalidClientTokenException",
            Some("clientToken must not be empty".to_string()),
        ),
    ];

    for (case, request, expected_code, expected_message) in rows {
        let (status, refused) = attach(&router, &request).await;
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
                "case {case:?}: wrong refusal message, got {refused:?}"
            );
        }
    }

    // Not one of those requests attached anything: the share is still governed by the
    // published default of the type it shares, and by nothing else.
    assert_eq!(
        listed_arns(&router, "refused attaches change nothing", &share_arn).await,
        vec![managed(COMPONENT_TYPE_DEFAULT)],
        "case \"refused attaches change nothing\": the share's permission set is untouched"
    );

    // A request naming the permission's own default version is not a refusal at all:
    // that is the version RAM attaches.
    let case = "the default version named explicitly";
    let (status, answered) = attach(
        &router,
        &json!({
            "resourceShareArn": share_arn,
            "permissionArn": permission_arn,
            "permissionVersion": FIRST_PERMISSION_VERSION,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {answered:?}"
    );
    assert_eq!(
        listed_arns(&router, case, &share_arn).await,
        vec![permission_arn],
        "case {case:?}: the share carries the permission that was attached"
    );
}

#[tokio::test]
async fn one_client_token_attaches_once_and_refuses_another_request() {
    let router = emulator();
    let case = "an attach repeated with one client token";
    let first = mint(
        &router,
        case,
        "ComponentIdempotent",
        COMPONENT_TYPE,
        "imagebuilder:GetComponent",
    )
    .await;
    let other = managed("AWSRAMPermissionGlueDatabaseReadWrite");
    let share_arn = share_with(
        &router,
        case,
        &json!({"name": "idempotent-share", "resourceArns": [COMPONENT]}),
    )
    .await;

    let request = json!({
        "resourceShareArn": share_arn,
        "permissionArn": first,
        "clientToken": "attach-token-1",
    });
    for attempt in 1..=2 {
        let (status, answered) = attach(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: attempt {attempt} expected HTTP 200, got {status} with {answered:?}"
        );
        assert_eq!(
            answered.get("clientToken").and_then(Value::as_str),
            Some("attach-token-1"),
            "case {case:?}: attempt {attempt} must report the token it was given, got {answered:?}"
        );
        assert_eq!(
            answered.get("returnValue").and_then(Value::as_bool),
            Some(true),
            "case {case:?}: attempt {attempt} expected returnValue true, got {answered:?}"
        );
    }
    assert_eq!(
        listed_arns(&router, case, &share_arn).await,
        vec![first.clone()],
        "case {case:?}: a repeated attach attaches the permission once"
    );

    let case = "one token reused for another request";
    let (status, refused) = attach(
        &router,
        &json!({
            "resourceShareArn": share_arn,
            "permissionArn": other,
            "clientToken": "attach-token-1",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidClientTokenException",
        "case {case:?}: expected InvalidClientTokenException, got {refused:?}"
    );
    assert_eq!(
        listed_arns(&router, case, &share_arn).await,
        vec![first],
        "case {case:?}: the refused request attached nothing"
    );

    // A token answers for the attachment the first call made, so it cannot answer for a
    // share that no longer exists: the repeat is refused like any other call naming it.
    let case = "the same token after the share is deleted";
    let (status, removed) = delete(&router, &share_arn, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from the delete, got {status} with {removed:?}"
    );
    let (status, refused) = attach(&router, &request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "UnknownResourceException",
        "case {case:?}: expected UnknownResourceException, got {refused:?}"
    );
    assert_eq!(
        error_message(&refused),
        format!("ResourceShare {share_arn} could not be found."),
        "case {case:?}: the refusal names the share it was asked for, got {refused:?}"
    );
}

#[tokio::test]
async fn a_page_holds_at_most_max_results_of_the_attached_permissions() {
    let router = emulator();
    let case = "page the attached permissions";
    let share_arn = share_with(
        &router,
        case,
        &json!({"name": "paged-attachments", "resourceArns": [COMPONENT]}),
    )
    .await;
    // Four permissions of four resource types, so one share carries four attachments:
    // enough for a page size below, at and above the number attached.
    let component_reader = mint(
        &router,
        case,
        "ComponentPaged",
        COMPONENT_TYPE,
        "imagebuilder:GetComponent",
    )
    .await;
    let mut every = vec![
        component_reader,
        managed("AWSRAMPermissionGlueDatabaseReadWrite"),
        managed("AWSRAMDefaultPermissionPrefixList"),
        managed("AWSRAMDefaultPermissionSubnet"),
    ];
    for permission_arn in &every {
        attach_ok(&router, case, &share_arn, permission_arn).await;
    }
    every.sort();
    assert_eq!(
        listed_arns(&router, case, &share_arn).await,
        every,
        "case {case:?}: the share reports every permission attached, in ARN order"
    );

    for page_size in [1usize, 2, 3, 4, 5, 500] {
        let case = format!("maxResults {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // A page of this size cannot need more than one request per permission plus the
        // last, empty one; anything beyond that is a token that does not advance.
        for _ in 0..=every.len() {
            let mut request = json!({"resourceShareArn": share_arn, "maxResults": page_size});
            if let Some(token) = token.clone() {
                request["nextToken"] = Value::String(token);
            }
            let (status, page) = list_permissions(&router, &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {page:?}"
            );
            let reported: Vec<String> = permissions_of(&case, &page)
                .iter()
                .map(|permission| field(&case, permission, "arn"))
                .collect();
            assert!(
                reported.len() <= page_size,
                "case {case:?}: a page holds at most {page_size} permissions, got {reported:?}"
            );
            walked.extend(reported);
            token = page
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            token, None,
            "case {case:?}: the walk must end with a page carrying no nextToken"
        );
        assert_eq!(
            walked, every,
            "case {case:?}: paging reports every attached permission exactly once, in ARN order"
        );
    }
}

/// The detail a permission write or read reported.
fn detail_of(case: &str, body: &Value) -> Value {
    body.get("permission")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: response carried no permission, got {body:?}"))
}

#[tokio::test]
async fn a_create_naming_more_permissions_than_one_call_accepts_is_refused() {
    let router = emulator();
    // Every permission ARN a create names becomes an attachment on the stored share, so
    // the list is bounded: at the bound the create is served, one past it refused.
    for (case, count, expected_status) in [
        ("at the bound", MAX_ENTITIES_PER_CALL, 200),
        ("one past the bound", MAX_ENTITIES_PER_CALL + 1, 400),
    ] {
        let permission_arns: Vec<String> = (0..count)
            .map(|index| customer(&format!("NeverCreated{index}")))
            .collect();
        let (status, answered) = create(
            &router,
            &json!({
                "name": format!("permission-count-{count}"),
                "permissionArns": permission_arns,
            }),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: a create naming {count} permissions expected HTTP \
             {expected_status}, got {status} with {answered:?}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&answered),
                "ResourceShareLimitExceededException",
                "case {case:?}: expected ResourceShareLimitExceededException, got {answered:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_share_reports_the_version_it_attached_while_the_permissions_default_moves_on() {
    let router = emulator();
    let case = "the version a share is governed by";
    let permission_arn = mint(
        &router,
        case,
        "ComponentVersioned",
        COMPONENT_TYPE,
        "imagebuilder:GetComponent",
    )
    .await;
    let share_arn = share_with(
        &router,
        case,
        &json!({"name": "versioned-share", "resourceArns": [COMPONENT]}),
    )
    .await;
    attach_ok(&router, case, &share_arn, &permission_arn).await;

    let attached = listed(&router, case, &share_arn).await;
    assert_eq!(
        (
            field(case, &attached[0], "version"),
            flag(case, &attached[0], "defaultVersion")
        ),
        (FIRST_PERMISSION_VERSION.to_string(), true),
        "case {case:?}: the share attached the permission's only version, which is its \
         default, got {attached:?}"
    );

    // A new version of the permission "is automatically set as the default version",
    // and "existing resource shares continue to use their original permission
    // versions" (CreatePermissionVersion's own documentation).
    let case = "a second version of the permission";
    let second_template = policy_template("imagebuilder:ListComponents");
    let (status, written) = create_permission_version(
        &router,
        &json!({"permissionArn": permission_arn, "policyTemplate": second_template}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {written:?}"
    );
    let detail = detail_of(case, &written);
    assert_eq!(
        (
            field(case, &detail, "version"),
            flag(case, &detail, "defaultVersion"),
            field(case, &detail, "permission"),
        ),
        (
            (FIRST_PERMISSION_VERSION + 1).to_string(),
            true,
            second_template.clone()
        ),
        "case {case:?}: the write reports the version it stored, as the new default, got \
         {detail:?}"
    );

    // Read back through GetPermission: a read that names no version reports the new
    // default version and the policy that version was written with.
    let (status, read) = get_permission(&router, &json!({"permissionArn": permission_arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the read-back, got {status} with {read:?}"
    );
    let read_detail = detail_of(case, &read);
    assert_eq!(
        (
            field(case, &read_detail, "version"),
            field(case, &read_detail, "permission"),
        ),
        ((FIRST_PERMISSION_VERSION + 1).to_string(), second_template),
        "case {case:?}: the permission reads back at its new default version, got \
         {read_detail:?}"
    );

    // The share is still governed by the version it attached, which is no longer the
    // permission's default.
    let outdated = listed(&router, case, &share_arn).await;
    assert_eq!(
        (
            field(case, &outdated[0], "version"),
            flag(case, &outdated[0], "defaultVersion")
        ),
        (FIRST_PERMISSION_VERSION.to_string(), false),
        "case {case:?}: the share keeps the version it attached, and reports that it is no \
         longer the default, got {outdated:?}"
    );

    // A replacing attach "updates an outdated version of the permission to the current
    // default version" (AssociateResourceSharePermission$permissionVersion).
    let case = "an outdated version named without replace";
    let (status, refused) = attach(
        &router,
        &json!({
            "resourceShareArn": share_arn,
            "permissionArn": permission_arn,
            "permissionVersion": FIRST_PERMISSION_VERSION,
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidParameterException",
        "case {case:?}: expected InvalidParameterException, got {refused:?}"
    );
    assert_eq!(
        error_message(&refused),
        format!(
            "permissionVersion {FIRST_PERMISSION_VERSION} is not the default version of \
             permission {permission_arn}, which is version {}. Specify the default version, or \
             set replace to true to update the resource share to it.",
            FIRST_PERMISSION_VERSION + 1
        ),
        "case {case:?}: the refusal names the version asked for, the default and the parameter, \
         got {refused:?}"
    );

    let case = "the share updated to the current default version";
    let (status, updated) = attach(
        &router,
        &json!({
            "resourceShareArn": share_arn,
            "permissionArn": permission_arn,
            "permissionVersion": FIRST_PERMISSION_VERSION + 1,
            "replace": true,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {updated:?}"
    );
    let current = listed(&router, case, &share_arn).await;
    assert_eq!(
        (
            field(case, &current[0], "version"),
            flag(case, &current[0], "defaultVersion")
        ),
        ((FIRST_PERMISSION_VERSION + 1).to_string(), true),
        "case {case:?}: the share now reports the version it was updated to, got {current:?}"
    );
}

#[tokio::test]
async fn a_permission_version_that_cannot_be_written_is_refused() {
    let router = emulator();
    let case = "a permission to version";
    let permission_arn = mint(
        &router,
        case,
        "ComponentVersionRefusals",
        COMPONENT_TYPE,
        "imagebuilder:GetComponent",
    )
    .await;
    let template = policy_template("imagebuilder:ListComponents");
    let unknown = customer("NeverCreated");
    let aws_managed = managed("AWSRAMDefaultPermissionSubnet");

    let rows: Vec<(&str, Value, &str, Option<String>)> = vec![
        (
            "a permissionArn that is not an ARN",
            json!({"permissionArn": "permission/abc", "policyTemplate": template}),
            "MalformedArnException",
            None,
        ),
        (
            "a permissionArn that names a resource share",
            json!({
                "permissionArn":
                    format!("arn:aws:ram:{REGION}:{ACCOUNT}:resource-share/abc"),
                "policyTemplate": template,
            }),
            "MalformedArnException",
            None,
        ),
        (
            "a permission nothing was created under",
            json!({"permissionArn": unknown, "policyTemplate": template}),
            "UnknownResourceException",
            Some(format!("Permission {unknown} could not be found.")),
        ),
        (
            "a permission Amazon Web Services manages",
            json!({"permissionArn": aws_managed, "policyTemplate": template}),
            "OperationNotPermittedException",
            Some(format!(
                "Permission {aws_managed} is an Amazon Web Services managed permission, \
                 which Amazon Web Services creates and manages. You can't modify it."
            )),
        ),
        (
            "a policyTemplate that is not JSON",
            json!({"permissionArn": permission_arn, "policyTemplate": "not json"}),
            "InvalidPolicyException",
            None,
        ),
        (
            "a policyTemplate whose action belongs to another service",
            json!({
                "permissionArn": permission_arn,
                "policyTemplate": policy_template("ec2:DescribeSubnets"),
            }),
            "InvalidPolicyException",
            None,
        ),
        (
            "a policyTemplate that does not allow",
            json!({
                "permissionArn": permission_arn,
                "policyTemplate":
                    json!({"Effect": "DENY", "Action": ["imagebuilder:GetComponent"]}).to_string(),
            }),
            "InvalidPolicyException",
            None,
        ),
        (
            "an empty clientToken",
            json!({
                "permissionArn": permission_arn,
                "policyTemplate": template,
                "clientToken": "",
            }),
            "InvalidClientTokenException",
            Some("clientToken must not be empty".to_string()),
        ),
    ];

    for (case, request, expected_code, expected_message) in rows {
        let (status, refused) = create_permission_version(&router, &request).await;
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
                "case {case:?}: wrong refusal message, got {refused:?}"
            );
        }
    }

    // Not one of those requests wrote a version: the permission still has only the one
    // it was created with.
    let (status, read) = get_permission(&router, &json!({"permissionArn": permission_arn})).await;
    assert_eq!(
        (status, field("refused writes change nothing", &detail_of("refused writes change nothing", &read), "version")),
        (200, FIRST_PERMISSION_VERSION.to_string()),
        "case \"refused writes change nothing\": the permission is still at its first version, \
         got {read:?}"
    );

    // The published cap: a permission carries at most MAX_PERMISSION_VERSIONS versions,
    // and the write past it is refused rather than stored.
    for version in (FIRST_PERMISSION_VERSION + 1)..=(MAX_PERMISSION_VERSIONS as i32) {
        let case = format!("version {version}");
        let (status, written) = create_permission_version(
            &router,
            &json!({
                "permissionArn": permission_arn,
                "policyTemplate": policy_template(&format!("imagebuilder:GetComponent{version}")),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {written:?}"
        );
        assert_eq!(
            field(&case, &detail_of(&case, &written), "version"),
            version.to_string(),
            "case {case:?}: the write reports the version it stored, got {written:?}"
        );
    }
    let case = format!("version {}", MAX_PERMISSION_VERSIONS as i32 + 1);
    let (status, refused) = create_permission_version(
        &router,
        &json!({
            "permissionArn": permission_arn,
            "policyTemplate": policy_template("imagebuilder:ListComponentBuildVersions"),
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 past the cap, got {status} with {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "PermissionVersionsLimitExceededException",
        "case {case:?}: expected PermissionVersionsLimitExceededException, got {refused:?}"
    );
    assert_eq!(
        error_message(&refused),
        format!(
            "Permission {permission_arn} already has the maximum of {MAX_PERMISSION_VERSIONS} \
             versions. Delete one of the existing versions before creating a new one."
        ),
        "case {case:?}: the refusal names the permission and the cap, got {refused:?}"
    );
}

#[tokio::test]
async fn an_attach_with_nothing_to_replace_is_served_whatever_replace_says() {
    let router = emulator();
    let component_reader = mint(
        &router,
        "mint a component permission",
        "ComponentReader",
        COMPONENT_TYPE,
        "imagebuilder:GetComponent",
    )
    .await;
    // The AWS managed permission the catalog publishes as `ec2:Subnet`'s default, which
    // is the type of LOCAL_RESOURCE: a share of one derives this permission without
    // attaching it by name.
    let subnet_default = managed("AWSRAMDefaultPermissionSubnet");

    // `replace` governs only the collision with ANOTHER permission of the same resource
    // type: "Use true to replace the current permissions. Use false to add the permission
    // to a resource share that currently doesn't have a permission"
    // (AssociateResourceSharePermission$replace). So an attach that collides with nothing
    // is served however the request sets it, and the share ends up carrying the attached
    // permission exactly once.
    let rows = [
        AttachRow {
            case: "replace true with no permission of that type attached",
            resource_arns: vec![],
            created_with: vec![],
            attached: vec![component_reader.clone()],
            replace: Some(true),
            expected: vec![component_reader.clone()],
        },
        AttachRow {
            case: "replace false with no permission of that type attached",
            resource_arns: vec![],
            created_with: vec![],
            attached: vec![component_reader.clone()],
            replace: Some(false),
            expected: vec![component_reader.clone()],
        },
        AttachRow {
            case: "the shared type's own default attached by name",
            resource_arns: vec![LOCAL_RESOURCE],
            created_with: vec![],
            attached: vec![subnet_default.clone()],
            replace: None,
            expected: vec![subnet_default.clone()],
        },
        AttachRow {
            case: "the shared type's own default attached by name with replace true",
            resource_arns: vec![LOCAL_RESOURCE],
            created_with: vec![],
            attached: vec![subnet_default.clone()],
            replace: Some(true),
            expected: vec![subnet_default.clone()],
        },
        AttachRow {
            case: "a replacing attach of the permission the create named",
            resource_arns: vec![],
            created_with: vec![subnet_default.clone()],
            attached: vec![subnet_default.clone()],
            replace: Some(true),
            expected: vec![subnet_default.clone()],
        },
        AttachRow {
            case: "a permission of each of two types, each attached with replace true",
            resource_arns: vec![LOCAL_RESOURCE],
            created_with: vec![],
            attached: vec![
                component_reader.clone(),
                managed("AWSRAMPermissionGlueDatabaseReadWrite"),
            ],
            replace: Some(true),
            expected: vec![
                subnet_default.clone(),
                component_reader.clone(),
                managed("AWSRAMPermissionGlueDatabaseReadWrite"),
            ],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        row.run(&router, &format!("no-collision-row-{index}")).await;
    }
}

#[tokio::test]
async fn an_attachment_is_the_state_of_the_one_share_it_was_made_to() {
    let router = emulator();
    let case = "an attach reaches one share only";
    let permission_arn = mint(
        &router,
        case,
        "ComponentReaderOnlyOne",
        COMPONENT_TYPE,
        "imagebuilder:GetComponent",
    )
    .await;
    // Two shares of the same resource, so the pair differs only in which one the attach
    // names.
    let attached_to = share_with(
        &router,
        case,
        &json!({"name": "attached-share", "resourceArns": [COMPONENT]}),
    )
    .await;
    let untouched = share_with(
        &router,
        case,
        &json!({"name": "untouched-share", "resourceArns": [COMPONENT]}),
    )
    .await;

    attach_ok(&router, case, &attached_to, &permission_arn).await;

    assert_eq!(
        listed_arns(&router, case, &attached_to).await,
        vec![permission_arn.clone()],
        "case {case:?}: the share named by the attach carries the permission"
    );
    assert_eq!(
        listed_arns(&router, case, &untouched).await,
        vec![managed(COMPONENT_TYPE_DEFAULT)],
        "case {case:?}: the share the attach did not name carries no attachment of its own, so \
         the published default of the type it shares is all it is governed by"
    );

    // The same permission attached to the second share too: each share's permission set
    // is its own, so both now carry it.
    attach_ok(&router, case, &untouched, &permission_arn).await;
    assert_eq!(
        (
            listed_arns(&router, case, &attached_to).await,
            listed_arns(&router, case, &untouched).await,
        ),
        (
            vec![permission_arn.clone()],
            vec![permission_arn.clone()]
        ),
        "case {case:?}: both shares carry the permission each attached"
    );
}

#[tokio::test]
async fn an_idempotency_token_is_spent_only_by_the_call_it_served() {
    let router = emulator();
    let case = "a token another operation already used";
    let permission_arn = mint(
        &router,
        case,
        "ComponentReaderToken",
        COMPONENT_TYPE,
        "imagebuilder:GetComponent",
    )
    .await;
    // "Passing the same value to a later call to an operation requires that you also
    // pass the same value for all other parameters" (clientToken's own documentation):
    // the token answers for a repeat of THAT operation, so one spent on a create is not
    // a repeat of an attach.
    let token = "ram-shared-token";
    let created = create_ok(
        &router,
        case,
        &json!({
            "name": "token-share",
            "resourceArns": [COMPONENT],
            "clientToken": token,
        }),
    )
    .await;
    let share_arn = arn_of(case, &created);

    let request = json!({
        "resourceShareArn": share_arn,
        "permissionArn": permission_arn,
        "clientToken": token,
    });
    let (status, answered) = attach(&router, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 for the first attach under this token, got {status} \
         with {answered:?}"
    );
    assert_eq!(
        answered.get("clientToken").and_then(Value::as_str),
        Some(token),
        "case {case:?}: the attach reports the token it was given, got {answered:?}"
    );
    assert_eq!(
        listed_arns(&router, case, &share_arn).await,
        vec![permission_arn.clone()],
        "case {case:?}: the attach was made"
    );

    // A refusal spends nothing: a token a refused attach carried still serves a later
    // attach that can be made.
    let case = "a token a refused attach carried";
    let other_share = share_with(
        &router,
        case,
        &json!({"name": "second-token-share", "resourceArns": [COMPONENT]}),
    )
    .await;
    let spent_on_a_refusal = "ram-refused-token";
    let (status, refused) = attach(
        &router,
        &json!({
            "resourceShareArn": other_share,
            "permissionArn": customer("NeverCreatedAtAll"),
            "clientToken": spent_on_a_refusal,
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 for a permission nothing answers to, got {status} \
         with {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "UnknownResourceException",
        "case {case:?}: expected UnknownResourceException, got {refused:?}"
    );
    let (status, answered) = attach(
        &router,
        &json!({
            "resourceShareArn": other_share,
            "permissionArn": permission_arn,
            "clientToken": spent_on_a_refusal,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 reusing the token of a refused attach, got {status} \
         with {answered:?}"
    );
    assert_eq!(
        listed_arns(&router, case, &other_share).await,
        vec![permission_arn],
        "case {case:?}: the served attach was made"
    );
}

/// Call `DisassociateResourceSharePermission`, the operation that takes a managed
/// permission back out of a share's permission set.
async fn remove(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(
        router,
        "POST",
        "/disassociateresourcesharepermission",
        Some(body),
    )
    .await
}

/// What a removal must answer.
enum Removal {
    /// It is served, and the share must then report exactly these permission ARNs.
    Served(Vec<String>),
    /// It is refused because the share still shares resources of this resource type
    /// and the permission named is the last one it holds for them.
    LastForType(&'static str),
    /// It is refused because the share is not governed by the permission named.
    NotAttached,
}

/// One row of the removal table: what the share holds and shares, which permission the
/// removal names, and what the removal must answer.
struct RemovalRow {
    case: &'static str,
    /// The resources the share is created with, so which resource types it shares — and
    /// so which defaults it derives — varies.
    resource_arns: Vec<&'static str>,
    /// The permissions attached after creation, each with `replace` true so a row may
    /// put its own permission in place of a shared type's default.
    attached: Vec<String>,
    /// Whether every resource association is torn down before the removal: that is what
    /// makes a share stop sharing the resources of a type it shared.
    drop_resources: bool,
    /// The permission the removal names.
    removed: String,
    outcome: Removal,
}

impl RemovalRow {
    /// Create this row's share, bring it to the state the row describes, remove the
    /// permission it names and assert what the removal answered — and what the share's
    /// permissions are afterwards.
    async fn run(&self, router: &MultiServiceRouter, name: &str) {
        let case = self.case;
        let share_arn = share_with(
            router,
            case,
            &json!({"name": name, "resourceArns": self.resource_arns}),
        )
        .await;
        for permission_arn in &self.attached {
            attach_served(router, case, &share_arn, permission_arn, Some(true)).await;
        }
        for resource_arn in self.resource_arns.iter().filter(|_| self.drop_resources) {
            let (status, answered) = disassociate(
                router,
                &json!({"resourceShareArn": share_arn, "resourceArns": [resource_arn]}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 tearing down {resource_arn}, got {status} with \
                 {answered:?}"
            );
        }

        let held_before = listed_arns(router, case, &share_arn).await;
        let (status, answered) = remove(
            router,
            &json!({"resourceShareArn": share_arn, "permissionArn": self.removed}),
        )
        .await;
        match &self.outcome {
            Removal::Served(expected) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200 removing {}, got {status} with {answered:?}",
                    self.removed
                );
                assert_eq!(
                    answered.get("returnValue").and_then(Value::as_bool),
                    Some(true),
                    "case {case:?}: a served removal reports returnValue true, got {answered:?}"
                );
                assert!(
                    answered
                        .get("clientToken")
                        .and_then(Value::as_str)
                        .is_some_and(|token| !token.is_empty()),
                    "case {case:?}: a removal reports the idempotency token it used, got \
                     {answered:?}"
                );
                let mut wanted = expected.clone();
                wanted.sort();
                assert_eq!(
                    listed_arns(router, case, &share_arn).await,
                    wanted,
                    "case {case:?}: removing {} from a share holding {held_before:?} must leave \
                     exactly {wanted:?}",
                    self.removed
                );
            }
            Removal::LastForType(resource_type) => {
                let message = last_permission_for_resource_type_message(&self.removed, resource_type);
                self.assert_refused(router, status, &answered, &share_arn, &held_before, message)
                    .await;
            }
            Removal::NotAttached => {
                let message = permission_not_attached_message(&share_arn, &self.removed);
                self.assert_refused(router, status, &answered, &share_arn, &held_before, message)
                    .await;
            }
        }
    }

    /// A refused removal answers HTTP 400 `InvalidParameterException` with `message`,
    /// and takes nothing away from the share.
    async fn assert_refused(
        &self,
        router: &MultiServiceRouter,
        status: u16,
        answered: &Value,
        share_arn: &str,
        held_before: &[String],
        message: String,
    ) {
        let case = self.case;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 removing {}, got {status} with {answered:?}",
            self.removed
        );
        assert_eq!(
            error_code(answered),
            "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {answered:?}"
        );
        assert_eq!(
            error_message(answered),
            message,
            "case {case:?}: unexpected refusal wording, got {answered:?}"
        );
        assert_eq!(
            listed_arns(router, case, share_arn).await,
            held_before,
            "case {case:?}: a refused removal leaves the share's permissions as they were"
        );
    }
}

/// The permission each removal row below removes, minted once per table.
async fn component_permission(router: &MultiServiceRouter, name: &str) -> String {
    mint(
        router,
        "mint a component permission",
        name,
        COMPONENT_TYPE,
        "imagebuilder:GetComponent",
    )
    .await
}

#[tokio::test]
async fn a_removal_is_refused_while_the_share_shares_resources_the_permission_governs() {
    let router = emulator();
    let component_reader = component_permission(&router, "ComponentReaderKept").await;
    // The resource types below are the ones the AWS managed catalog publishes these
    // permissions for (`ListPermissions`' `resourceType`), which is what the refusal
    // names: `ec2:Subnet` for the subnet default, `glue:Database` for both Glue
    // database permissions.
    let rows = [
        RemovalRow {
            case: "the derived default of a type the share shares",
            resource_arns: vec![LOCAL_RESOURCE],
            attached: vec![],
            drop_resources: false,
            removed: managed("AWSRAMDefaultPermissionSubnet"),
            outcome: Removal::LastForType("ec2:Subnet"),
        },
        RemovalRow {
            case: "the shared type's default attached by name",
            resource_arns: vec![LOCAL_RESOURCE],
            attached: vec![managed("AWSRAMDefaultPermissionSubnet")],
            drop_resources: false,
            removed: managed("AWSRAMDefaultPermissionSubnet"),
            outcome: Removal::LastForType("ec2:Subnet"),
        },
        RemovalRow {
            case: "an AWS managed permission attached in place of the shared type's default",
            resource_arns: vec![GLUE_DATABASE],
            attached: vec![managed("AWSRAMPermissionGlueDatabaseReadWrite")],
            drop_resources: false,
            removed: managed("AWSRAMPermissionGlueDatabaseReadWrite"),
            outcome: Removal::LastForType("glue:Database"),
        },
        RemovalRow {
            case: "a customer managed permission governing the only type the share shares",
            resource_arns: vec![COMPONENT],
            attached: vec![component_reader.clone()],
            drop_resources: false,
            removed: component_reader.clone(),
            outcome: Removal::LastForType(COMPONENT_TYPE),
        },
        RemovalRow {
            case: "the permission of one shared type beside another shared type",
            resource_arns: vec![LOCAL_RESOURCE, GLUE_DATABASE],
            attached: vec![],
            drop_resources: false,
            removed: managed("AWSRAMDefaultPermissionGlueDatabase"),
            outcome: Removal::LastForType("glue:Database"),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        row.run(&router, &format!("removal-refused-row-{index}"))
            .await;
    }

    // The sentence itself, not the helper that renders it: AWS refuses the removal of
    // the permission a share's resources of a type are being governed by with this
    // wording, its variable parts taken from the request and the share's state.
    let case = "the wording of the last-permission refusal";
    let subnet_default = managed("AWSRAMDefaultPermissionSubnet");
    let share_arn = share_with(
        &router,
        case,
        &json!({"name": "refusal-wording-share", "resourceArns": [LOCAL_RESOURCE]}),
    )
    .await;
    let (status, refused) = remove(
        &router,
        &json!({"resourceShareArn": share_arn, "permissionArn": subnet_default}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {refused:?}"
    );
    assert_eq!(
        error_message(&refused),
        format!(
            "The permission {subnet_default} cannot be disassociated because it is the last \
             permission being used by resources of type ec2:Subnet in the share."
        ),
        "case {case:?}: unexpected refusal wording, got {refused:?}"
    );
}

#[tokio::test]
async fn a_removal_is_served_once_the_share_shares_no_resource_of_the_permissions_type() {
    let router = emulator();
    let component_reader = component_permission(&router, "ComponentReaderRemoved").await;
    let rows = [
        RemovalRow {
            case: "a permission of a type the share shares nothing of",
            resource_arns: vec![],
            attached: vec![component_reader.clone()],
            drop_resources: false,
            removed: component_reader.clone(),
            outcome: Removal::Served(vec![]),
        },
        RemovalRow {
            case: "a permission removed beside the derived default of a type that is shared",
            resource_arns: vec![LOCAL_RESOURCE],
            attached: vec![managed("AWSRAMPermissionGlueDatabaseReadWrite")],
            drop_resources: false,
            removed: managed("AWSRAMPermissionGlueDatabaseReadWrite"),
            outcome: Removal::Served(vec![managed("AWSRAMDefaultPermissionSubnet")]),
        },
        RemovalRow {
            case: "the derived default of a type whose resources are no longer shared",
            resource_arns: vec![LOCAL_RESOURCE],
            attached: vec![],
            drop_resources: true,
            removed: managed("AWSRAMDefaultPermissionSubnet"),
            outcome: Removal::Served(vec![]),
        },
        RemovalRow {
            case: "a permission attached for a type whose resources are no longer shared",
            resource_arns: vec![COMPONENT],
            attached: vec![component_reader.clone()],
            drop_resources: true,
            removed: component_reader.clone(),
            // The removal takes away the attachment it names, not the published default
            // of the type the share shares, which governs it once nothing stands in for
            // it.
            outcome: Removal::Served(vec![managed(COMPONENT_TYPE_DEFAULT)]),
        },
        RemovalRow {
            case: "one of two permissions held for types the share shares nothing of",
            resource_arns: vec![LOCAL_RESOURCE],
            attached: vec![
                component_reader.clone(),
                managed("AWSRAMPermissionGlueDatabaseReadWrite"),
            ],
            drop_resources: false,
            removed: component_reader.clone(),
            outcome: Removal::Served(vec![
                managed("AWSRAMDefaultPermissionSubnet"),
                managed("AWSRAMPermissionGlueDatabaseReadWrite"),
            ]),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        row.run(&router, &format!("removal-served-row-{index}"))
            .await;
    }
}

#[tokio::test]
async fn a_removal_names_a_permission_the_share_is_governed_by() {
    let router = emulator();
    let component_reader = component_permission(&router, "ComponentReaderUnattached").await;
    let rows = [
        RemovalRow {
            case: "a permission the share never held",
            resource_arns: vec![LOCAL_RESOURCE],
            attached: vec![],
            drop_resources: false,
            removed: component_reader.clone(),
            outcome: Removal::NotAttached,
        },
        RemovalRow {
            case: "the shared type's default, which another permission stands in for",
            resource_arns: vec![GLUE_DATABASE],
            attached: vec![managed("AWSRAMPermissionGlueDatabaseReadWrite")],
            drop_resources: false,
            removed: managed("AWSRAMDefaultPermissionGlueDatabase"),
            outcome: Removal::NotAttached,
        },
        RemovalRow {
            case: "an AWS managed permission of a type the share does not share",
            resource_arns: vec![],
            attached: vec![],
            drop_resources: false,
            removed: managed("AWSRAMDefaultPermissionPrefixList"),
            outcome: Removal::NotAttached,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        row.run(&router, &format!("removal-unattached-row-{index}"))
            .await;
    }

    // A permission removed once is no longer one the share is governed by, so removing
    // it again names nothing to remove.
    let case = "a second removal of the same permission";
    let share_arn = share_with(
        &router,
        case,
        &json!({"name": "removed-twice-share", "resourceArns": []}),
    )
    .await;
    attach_ok(&router, case, &share_arn, &component_reader).await;
    let request = json!({"resourceShareArn": share_arn, "permissionArn": component_reader});
    let (status, answered) = remove(&router, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 for the first removal, got {status} with {answered:?}"
    );
    let (status, refused) = remove(&router, &request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 for the second removal, got {status} with {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidParameterException",
        "case {case:?}: expected InvalidParameterException, got {refused:?}"
    );
    assert_eq!(
        error_message(&refused),
        format!("Permission {component_reader} is not associated with resource share {share_arn}."),
        "case {case:?}: the refusal names the share and the permission, got {refused:?}"
    );
}

#[tokio::test]
async fn a_removal_answers_for_a_share_and_a_permission_that_exist() {
    let router = emulator();
    let case = "a share and permission that exist";
    let component_reader = component_permission(&router, "ComponentReaderRefusals").await;
    let share_arn = share_with(
        &router,
        case,
        &json!({"name": "refusal-share", "resourceArns": []}),
    )
    .await;
    let deleted_share = share_with(
        &router,
        case,
        &json!({"name": "deleted-refusal-share", "resourceArns": []}),
    )
    .await;
    let (status, _) = delete(&router, &deleted_share, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the share this row reads was deleted first"
    );
    // A share ARN of the right form that no share answers to, so "malformed" and
    // "unknown" are told apart.
    let unknown_share = format!(
        "arn:aws:ram:{REGION}:{ACCOUNT}:resource-share/00000000-0000-0000-0000-000000000000"
    );

    let rows = [
        (
            "a resourceShareArn that is not an ARN of a share",
            "not-an-arn".to_string(),
            component_reader.clone(),
            "MalformedArnException",
            share_arn_rejection("resourceShareArn", "not-an-arn").expect("a malformed share ARN"),
        ),
        (
            "a permissionArn that is not an ARN of a permission",
            share_arn.clone(),
            "arn:aws:ram:us-west-2:000000000000:resource-share/not-a-permission".to_string(),
            "MalformedArnException",
            permission_arn_rejection(
                "permissionArn",
                "arn:aws:ram:us-west-2:000000000000:resource-share/not-a-permission",
            )
            .expect("a malformed permission ARN"),
        ),
        (
            "a share nothing answers to",
            unknown_share.clone(),
            component_reader.clone(),
            "UnknownResourceException",
            format!("ResourceShare {unknown_share} could not be found."),
        ),
        (
            "a share a delete has removed",
            deleted_share.clone(),
            component_reader.clone(),
            "UnknownResourceException",
            unknown_share_message(&deleted_share),
        ),
        (
            "a permission nothing answers to",
            share_arn.clone(),
            customer("NeverCreatedAtAll"),
            "UnknownResourceException",
            format!("Permission {} could not be found.", customer("NeverCreatedAtAll")),
        ),
    ];

    for (case, resource_share_arn, permission_arn, code, message) in rows {
        let (status, refused) = remove(
            &router,
            &json!({"resourceShareArn": resource_share_arn, "permissionArn": permission_arn}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            code,
            "case {case:?}: expected {code}, got {refused:?}"
        );
        assert_eq!(
            error_message(&refused),
            message,
            "case {case:?}: unexpected refusal wording, got {refused:?}"
        );
    }
}

#[tokio::test]
async fn a_removals_client_token_answers_for_a_repeat_of_that_removal() {
    let router = emulator();
    let case = "a removal repeated under its own token";
    let component_reader = component_permission(&router, "ComponentReaderToken").await;
    let other_permission = managed("AWSRAMDefaultPermissionPrefixList");
    let share_arn = share_with(
        &router,
        case,
        &json!({"name": "token-removal-share", "resourceArns": []}),
    )
    .await;
    attach_ok(&router, case, &share_arn, &component_reader).await;
    attach_ok(&router, case, &share_arn, &other_permission).await;

    let token = "ram-removal-token";
    let request = json!({
        "resourceShareArn": share_arn,
        "permissionArn": component_reader,
        "clientToken": token,
    });
    let (status, answered) = remove(&router, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 for the first removal, got {status} with {answered:?}"
    );
    assert_eq!(
        answered.get("clientToken").and_then(Value::as_str),
        Some(token),
        "case {case:?}: the removal reports the token it was given, got {answered:?}"
    );
    assert_eq!(
        listed_arns(&router, case, &share_arn).await,
        vec![other_permission.clone()],
        "case {case:?}: the removal took away exactly the permission it named"
    );

    // The same request again: the token answers for the removal already made, so it is
    // served rather than refused as a removal of a permission the share no longer holds.
    let (status, repeated) = remove(&router, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 repeating the removal, got {status} with {repeated:?}"
    );
    assert_eq!(
        repeated.get("returnValue").and_then(Value::as_bool),
        Some(true),
        "case {case:?}: the repeat reports the removal that was made, got {repeated:?}"
    );
    assert_eq!(
        listed_arns(&router, case, &share_arn).await,
        vec![other_permission.clone()],
        "case {case:?}: the repeat took nothing further away"
    );

    // "Passing the same value to a later call to an operation requires that you also
    // pass the same value for all other parameters" (clientToken's own documentation):
    // the same token naming another permission is a different request.
    let case = "the same token for a different removal";
    let (status, refused) = remove(
        &router,
        &json!({
            "resourceShareArn": share_arn,
            "permissionArn": other_permission,
            "clientToken": token,
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidClientTokenException",
        "case {case:?}: expected InvalidClientTokenException, got {refused:?}"
    );
    assert_eq!(
        error_message(&refused),
        IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE,
        "case {case:?}: the refusal is worded for a token reused for another request, got \
         {refused:?}"
    );
    assert_eq!(
        listed_arns(&router, case, &share_arn).await,
        vec![other_permission],
        "case {case:?}: the refused removal took nothing away"
    );
}

#[tokio::test]
async fn a_resource_newly_shared_is_governed_by_its_resource_types_default_permission_again() {
    let router = emulator();
    let case = "a resource of a type whose default was removed, shared again";
    let subnet_default = managed("AWSRAMDefaultPermissionSubnet");
    let share_arn = share_with(
        &router,
        case,
        &json!({"name": "restored-default-share", "resourceArns": [LOCAL_RESOURCE]}),
    )
    .await;
    assert_eq!(
        listed_arns(&router, case, &share_arn).await,
        vec![subnet_default.clone()],
        "case {case:?}: the share derives the shared type's default permission"
    );

    // The removal is possible only once the share shares no resource of the type, so
    // the resource association is torn down first.
    let (status, torn_down) = disassociate(
        &router,
        &json!({"resourceShareArn": share_arn, "resourceArns": [LOCAL_RESOURCE]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 tearing the resource down, got {status} with \
         {torn_down:?}"
    );
    let (status, removed) = remove(
        &router,
        &json!({"resourceShareArn": share_arn, "permissionArn": subnet_default}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 removing the default, got {status} with {removed:?}"
    );
    assert_eq!(
        listed_arns(&router, case, &share_arn).await,
        Vec::<String>::new(),
        "case {case:?}: the share is governed by nothing once its only permission is removed"
    );

    // A resource of that type shared again is governed by the type's default
    // permission, which RAM attaches when the share holds none of its own for it.
    let (status, associated) = associate(
        &router,
        &json!({"resourceShareArn": share_arn, "resourceArns": [LOCAL_RESOURCE_TWO]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 sharing a second subnet, got {status} with \
         {associated:?}"
    );
    assert_eq!(
        listed_arns(&router, case, &share_arn).await,
        vec![subnet_default.clone()],
        "case {case:?}: the newly shared resource type is governed by its default permission"
    );

    // And with resources of the type shared again, that permission is the last one
    // governing them, so removing it is refused once more.
    let (status, refused) = remove(
        &router,
        &json!({"resourceShareArn": share_arn, "permissionArn": subnet_default}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 removing the last permission in use, got {status} with \
         {refused:?}"
    );
    assert_eq!(
        error_message(&refused),
        last_permission_for_resource_type_message(&subnet_default, "ec2:Subnet"),
        "case {case:?}: the refusal names the permission and the resource type, got {refused:?}"
    );
}

/// The names of the shares `GetResourceShares` reports for `permissionArn`: the other
/// read that answers which permissions govern a share, so a removal has to show in it
/// as well as in a read of the share's own permissions.
async fn shares_using(
    router: &MultiServiceRouter,
    case: &str,
    permission_arn: &str,
) -> Vec<String> {
    let (status, listed) = get_shares(
        router,
        &json!({"resourceOwner": "SELF", "permissionArn": permission_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 reading the shares governed by {permission_arn}, got \
         {status} with {listed:?}"
    );
    names_of(case, &listed)
}

/// One row of the removal-visibility table: the managed permission two shares are both
/// governed by, one of which the removal is made on.
struct RemovalVisibilityRow {
    case: &'static str,
    permission: String,
}

impl RemovalVisibilityRow {
    /// Govern two shares by this row's permission, remove it from one of them, and
    /// assert that both reads of a share's permission set stop reporting it for THAT
    /// share only — and that a later attach puts it back in both.
    async fn run(&self, router: &MultiServiceRouter, suffix: &str) {
        let case = self.case;
        let permission = self.permission.clone();
        let subject_name = format!("removal-visibility-subject-{suffix}");
        let bystander_name = format!("removal-visibility-bystander-{suffix}");
        let subject = share_with(
            router,
            case,
            &json!({"name": subject_name, "resourceArns": []}),
        )
        .await;
        let bystander = share_with(
            router,
            case,
            &json!({"name": bystander_name, "resourceArns": []}),
        )
        .await;
        attach_ok(router, case, &subject, &permission).await;
        attach_ok(router, case, &bystander, &permission).await;

        let mut both = vec![bystander_name.clone(), subject_name.clone()];
        both.sort();
        assert_eq!(
            listed_arns(router, case, &subject).await,
            vec![permission.clone()],
            "case {case:?}: the share is governed by the permission attached to it"
        );
        assert_eq!(
            shares_using(router, case, &permission).await,
            both,
            "case {case:?}: both shares are reported as governed by {permission}"
        );

        let (status, answered) = remove(
            router,
            &json!({"resourceShareArn": subject, "permissionArn": permission}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 removing {permission}, got {status} with \
             {answered:?}"
        );
        assert_eq!(
            answered.get("returnValue").and_then(Value::as_bool),
            Some(true),
            "case {case:?}: a served removal reports returnValue true, got {answered:?}"
        );

        assert_eq!(
            listed_arns(router, case, &subject).await,
            Vec::<String>::new(),
            "case {case:?}: the share the removal named is governed by nothing afterwards"
        );
        assert_eq!(
            shares_using(router, case, &permission).await,
            vec![bystander_name.clone()],
            "case {case:?}: only the other share is still reported as governed by {permission}"
        );
        assert_eq!(
            listed_arns(router, case, &bystander).await,
            vec![permission.clone()],
            "case {case:?}: a removal reaches the share it names and no other"
        );

        // "Adds or replaces the RAM permission for a resource type included in a
        // resource share": an attach of a permission a removal took away governs the
        // share by it again, so the removal is not permanent for the pair.
        attach_ok(router, case, &subject, &permission).await;
        assert_eq!(
            listed_arns(router, case, &subject).await,
            vec![permission.clone()],
            "case {case:?}: an attach after the removal governs the share by {permission} again"
        );
        assert_eq!(
            shares_using(router, case, &permission).await,
            both,
            "case {case:?}: the re-attached permission is reported by both reads again"
        );
    }
}

#[tokio::test]
async fn a_removal_leaves_every_read_of_that_shares_permission_set_without_it() {
    let router = emulator();
    let component_reader = component_permission(&router, "ComponentReaderVisibility").await;
    // Which permission the removal names: a resource type's default managed permission,
    // an AWS managed permission that is not its type's default, and a customer managed
    // one this account minted. None of the shares below shares a resource, so each
    // permission is held by name only and the removal is served.
    let rows = [
        RemovalVisibilityRow {
            case: "an AWS managed resource-type default",
            permission: managed("AWSRAMDefaultPermissionSubnet"),
        },
        RemovalVisibilityRow {
            case: "an AWS managed permission that is not its type's default",
            permission: managed("AWSRAMPermissionGlueDatabaseReadWrite"),
        },
        RemovalVisibilityRow {
            case: "a customer managed permission",
            permission: component_reader.clone(),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        row.run(&router, &format!("row-{index}")).await;
    }
}

#[tokio::test]
async fn a_removal_that_carries_no_client_token_mints_the_one_its_repeat_is_made_with() {
    let router = emulator();
    let case = "a removal that carries no client token";
    let component_reader = component_permission(&router, "ComponentReaderMintedToken").await;
    let other_permission = managed("AWSRAMDefaultPermissionPrefixList");
    let share_arn = share_with(
        &router,
        case,
        &json!({"name": "minted-token-share", "resourceArns": []}),
    )
    .await;
    attach_ok(&router, case, &share_arn, &component_reader).await;
    attach_ok(&router, case, &share_arn, &other_permission).await;

    let request = json!({"resourceShareArn": share_arn, "permissionArn": component_reader});
    let (status, answered) = remove(&router, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 for the removal, got {status} with {answered:?}"
    );
    // "If you don't provide this value, then Amazon Web Services generates a random one
    // for you" (`clientToken`'s own documentation), and the output reports "the
    // idempotency identifier associated with this request".
    let minted = answered
        .get("clientToken")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            panic!("case {case:?}: the removal reported no clientToken, got {answered:?}")
        });
    assert!(
        !minted.is_empty(),
        "case {case:?}: the removal reported an empty clientToken, got {answered:?}"
    );

    // "If you want to repeat the same operation in an idempotent manner then you must
    // include this value in the clientToken request parameter of that later call": the
    // minted token answers for the removal that was made.
    let (status, repeated) = remove(
        &router,
        &json!({
            "resourceShareArn": share_arn,
            "permissionArn": component_reader,
            "clientToken": minted,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 repeating the removal under the token it minted, got \
         {status} with {repeated:?}"
    );
    assert_eq!(
        repeated.get("returnValue").and_then(Value::as_bool),
        Some(true),
        "case {case:?}: the repeat reports the removal that was made, got {repeated:?}"
    );
    assert_eq!(
        listed_arns(&router, case, &share_arn).await,
        vec![other_permission.clone()],
        "case {case:?}: the repeat took nothing further away"
    );

    // The token is what makes the repeat idempotent: the same removal sent without one
    // names a permission the share is no longer governed by.
    let case = "the same removal again without a token";
    let (status, refused) = remove(&router, &request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidParameterException",
        "case {case:?}: expected InvalidParameterException, got {refused:?}"
    );
    assert_eq!(
        error_message(&refused),
        permission_not_attached_message(&share_arn, &component_reader),
        "case {case:?}: unexpected refusal wording, got {refused:?}"
    );

    // And the minted token answers for THAT removal only, so another permission under it
    // is the parameter clash it is.
    let case = "the minted token naming another permission";
    let (status, refused) = remove(
        &router,
        &json!({
            "resourceShareArn": share_arn,
            "permissionArn": other_permission,
            "clientToken": minted,
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidClientTokenException",
        "case {case:?}: expected InvalidClientTokenException, got {refused:?}"
    );
    assert_eq!(
        error_message(&refused),
        IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE,
        "case {case:?}: the refusal is worded for a token reused for another request, got \
         {refused:?}"
    );
    assert_eq!(
        listed_arns(&router, case, &share_arn).await,
        vec![other_permission],
        "case {case:?}: the refused removal took nothing away"
    );
}

#[tokio::test]
async fn a_removals_token_does_not_answer_for_a_share_that_is_gone() {
    let router = emulator();
    let case = "a token whose share was deleted after the removal";
    let component_reader = component_permission(&router, "ComponentReaderDeletedShare").await;
    let share_arn = share_with(
        &router,
        case,
        &json!({"name": "token-then-deleted-share", "resourceArns": []}),
    )
    .await;
    attach_ok(&router, case, &share_arn, &component_reader).await;

    let request = json!({
        "resourceShareArn": share_arn,
        "permissionArn": component_reader,
        "clientToken": "ram-removal-token-then-delete",
    });
    let (status, answered) = remove(&router, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 for the removal, got {status} with {answered:?}"
    );
    let (status, deleted) = delete(&router, &share_arn, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 deleting the share, got {status} with {deleted:?}"
    );

    // A share a delete has removed holds no permission for a call to remove from it, so
    // the token cannot answer for a removal on it either.
    let (status, refused) = remove(&router, &request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 repeating the removal on a deleted share, got {status} \
         with {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "UnknownResourceException",
        "case {case:?}: expected UnknownResourceException, got {refused:?}"
    );
    assert_eq!(
        error_message(&refused),
        unknown_share_message(&share_arn),
        "case {case:?}: unexpected refusal wording, got {refused:?}"
    );
}

// ---------------------------------------------------------------------------
// ReplacePermissionAssociations: the bulk half of the same rule — every share
// governed by one managed permission moved onto another one, by a background
// task the call reports and `ListReplacePermissionAssociationsWork` reads back.
// ---------------------------------------------------------------------------

/// How long a poll for a replacement task to complete keeps looking. Nothing sleeps
/// server-side: the move is one the clock owes and the first call at or after it falls
/// due is what materialises it, so the loop waits for the CONDITION and this is only the
/// point at which it gives up — deliberately far beyond the span itself, so a loaded host
/// still passes.
const REPLACE_POLL_DEADLINE: Duration =
    Duration::from_millis((REPLACE_PERMISSION_ASSOCIATIONS_COMPLETED_AFTER_MILLIS as u64) * 20);

/// How long the poll loop waits between looks.
const REPLACE_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// A Glue database permission the catalog publishes as that type's default, and one it
/// publishes at a later version that is NOT the default, so a replacement onto it must
/// report the version that permission's own default is.
const GLUE_DATABASE_DEFAULT: &str = "AWSRAMDefaultPermissionGlueDatabase";
const GLUE_DATABASE_READ_WRITE: &str = "AWSRAMPermissionGlueDatabaseReadWrite";
/// The version the catalog publishes [`GLUE_DATABASE_READ_WRITE`] at, which is therefore
/// its default version (`ListPermissions` reports it).
const GLUE_DATABASE_READ_WRITE_VERSION: i32 = 3;

async fn replace(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/replacepermissionassociations", Some(body)).await
}

async fn list_work(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(
        router,
        "POST",
        "/listreplacepermissionassociationswork",
        Some(body),
    )
    .await
}

/// The background tasks a read reported.
fn works_of(case: &str, body: &Value) -> Vec<Value> {
    body.get("replacePermissionAssociationsWorks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: response carried no replacePermissionAssociationsWorks, got \
                 {body:?}"
            )
        })
}

/// Start a replacement that must be served, and answer the task it reported — which a
/// call that has only just started it reports in flight.
async fn replace_ok(router: &MultiServiceRouter, case: &str, request: &Value) -> Value {
    let (status, answered) = replace(router, request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {request}, got {status} with {answered:?}"
    );
    assert!(
        answered
            .get("clientToken")
            .and_then(Value::as_str)
            .is_some_and(|token| !token.is_empty()),
        "case {case:?}: a replacement reports the idempotency token it used, got {answered:?}"
    );
    let work = answered
        .get("replacePermissionAssociationsWork")
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the call reported no background task, got {answered:?}")
        });
    assert_eq!(
        field(case, &work, "status"),
        REPLACE_WORK_STATUS_IN_PROGRESS,
        "case {case:?}: the task the call starts is still in flight, got {work:?}"
    );
    work
}

/// The one task `work_id` names, read back through `ListReplacePermissionAssociationsWork`.
async fn one_work(router: &MultiServiceRouter, case: &str, work_id: &str) -> Value {
    let (status, body) = list_work(router, &json!({"workIds": [work_id]})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 reading task {work_id}, got {status} with {body:?}"
    );
    let reported = works_of(case, &body);
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: a read naming one task id reports exactly that task, got {reported:?}"
    );
    assert_eq!(
        field(case, &reported[0], "id"),
        work_id,
        "case {case:?}: the task reported is the one asked for, got {reported:?}"
    );
    reported[0].clone()
}

/// Look at the task until it reports `COMPLETED`, or the deadline passes, and answer the
/// task that reported it.
async fn poll_until_completed(router: &MultiServiceRouter, case: &str, work_id: &str) -> Value {
    let give_up_at = Instant::now() + REPLACE_POLL_DEADLINE;
    loop {
        let reported = one_work(router, case, work_id).await;
        let status = field(case, &reported, "status");
        if status == REPLACE_WORK_STATUS_COMPLETED {
            return reported;
        }
        assert_eq!(
            status, REPLACE_WORK_STATUS_IN_PROGRESS,
            "case {case:?}: a task that has not completed is in flight, got {reported:?}"
        );
        assert!(
            Instant::now() < give_up_at,
            "case {case:?}: expected task {work_id} to report \
             {REPLACE_WORK_STATUS_COMPLETED}, still {status:?} after {REPLACE_POLL_DEADLINE:?}"
        );
        std::thread::sleep(REPLACE_POLL_INTERVAL);
    }
}

/// The version of `permission_arn` the share `share_arn` is governed by, and whether that
/// is the permission's default version.
async fn governed_at(
    router: &MultiServiceRouter,
    case: &str,
    share_arn: &str,
    permission_arn: &str,
) -> (String, bool) {
    let reported = listed(router, case, share_arn).await;
    let held = reported
        .iter()
        .find(|permission| field(case, permission, "arn") == permission_arn)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: share {share_arn} reports no permission {permission_arn}, got \
                 {reported:?}"
            )
        });
    (
        field(case, held, "version"),
        flag(case, held, "defaultVersion"),
    )
}

/// Write a further version of `permission_arn`, which becomes its default version, and
/// assert the write was served.
async fn write_version(
    router: &MultiServiceRouter,
    case: &str,
    permission_arn: &str,
    action: &str,
) {
    let (status, written) = create_permission_version(
        router,
        &json!({"permissionArn": permission_arn, "policyTemplate": policy_template(action)}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 writing a further version of {permission_arn}, got \
         {status} with {written:?}"
    );
}

/// One row of the replacement table: the permission a share is governed by, the one the
/// task moves it onto, and what the share must report once the task has run.
struct ReplaceRow {
    case: &'static str,
    /// The permission the subject share is governed by when the replacement is made.
    from: String,
    /// An action to write a further version of `from` with after the share attached it,
    /// so the permission's default version has moved past the one the share holds.
    revise_from: Option<&'static str>,
    /// The version the request restricts the task to, or `None` for every version of
    /// `from`.
    from_version: Option<i32>,
    /// The permission the task moves the share onto.
    to: String,
    /// The version of `to` the task must associate: that permission's default version.
    to_version: i32,
    /// A permission of ANOTHER resource type the share also holds, which the task must
    /// leave exactly where it is.
    kept: Vec<String>,
}

impl ReplaceRow {
    /// Govern a share by this row's permission, replace it, and assert that the share is
    /// moved onto the new permission at its default version — and not before the task
    /// the call reported has completed.
    async fn run(&self, router: &MultiServiceRouter, suffix: &str) {
        let case = self.case;
        let share_arn = share_with(
            router,
            case,
            &json!({"name": format!("replace-subject-{suffix}"), "resourceArns": []}),
        )
        .await;
        attach_ok(router, case, &share_arn, &self.from).await;
        for kept in &self.kept {
            attach_ok(router, case, &share_arn, kept).await;
        }
        let (held_version, _) = governed_at(router, case, &share_arn, &self.from).await;
        if let Some(action) = self.revise_from {
            write_version(router, case, &self.from, action).await;
        }

        let mut request = json!({"fromPermissionArn": self.from, "toPermissionArn": self.to});
        if let Some(version) = self.from_version {
            request["fromPermissionVersion"] = json!(version);
        }
        let work = replace_ok(router, case, &request).await;
        assert_eq!(
            (
                field(case, &work, "fromPermissionArn"),
                field(case, &work, "toPermissionArn"),
                field(case, &work, "toPermissionVersion"),
                work.get("fromPermissionVersion")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            ),
            (
                self.from.clone(),
                self.to.clone(),
                self.to_version.to_string(),
                self.from_version.map(|version| version.to_string()),
            ),
            "case {case:?}: the task reports the replacement it was asked for, at the target \
             permission's default version, got {work:?}"
        );

        // The move is the task's work, so a read taken before it has run still reports
        // the attachment the share made for itself.
        assert_eq!(
            (
                listed_arns(router, case, &share_arn).await,
                governed_at(router, case, &share_arn, &self.from).await.0,
            ),
            (
                {
                    let mut before = self.kept.clone();
                    before.push(self.from.clone());
                    before.sort();
                    before
                },
                held_version.clone()
            ),
            "case {case:?}: while the task is in flight the share is still governed by {} at \
             version {held_version}",
            self.from
        );

        let work_id = field(case, &work, "id");
        poll_until_completed(router, case, &work_id).await;

        let mut expected = self.kept.clone();
        expected.push(self.to.clone());
        expected.sort();
        assert_eq!(
            listed_arns(router, case, &share_arn).await,
            expected,
            "case {case:?}: once the task has run the share is governed by {} and keeps {:?}",
            self.to,
            self.kept
        );
        assert_eq!(
            governed_at(router, case, &share_arn, &self.to).await,
            (self.to_version.to_string(), true),
            "case {case:?}: the task associates the target permission's current default version"
        );
    }
}

#[tokio::test]
async fn a_replacement_moves_every_share_governed_by_the_old_permission() {
    let router = emulator();
    let case = "mint the permissions the replacement rows name";
    let propagated = mint(
        &router,
        case,
        "ComponentPropagated",
        COMPONENT_TYPE,
        "imagebuilder:GetComponent",
    )
    .await;
    let pinned = mint(
        &router,
        case,
        "ComponentPinned",
        COMPONENT_TYPE,
        "imagebuilder:GetComponent",
    )
    .await;
    let replaced = mint(
        &router,
        case,
        "ComponentReplaced",
        COMPONENT_TYPE,
        "imagebuilder:GetComponent",
    )
    .await;
    let target = mint(
        &router,
        case,
        "ComponentTarget",
        COMPONENT_TYPE,
        "imagebuilder:ListComponents",
    )
    .await;
    let promoted = mint(
        &router,
        case,
        "ComponentPromoted",
        COMPONENT_TYPE,
        "imagebuilder:GetComponent",
    )
    .await;
    let glue_reader = mint(
        &router,
        case,
        "GlueDatabaseReader",
        "glue:Database",
        "glue:GetDatabase",
    )
    .await;

    let rows = [
        ReplaceRow {
            case: "the same permission, propagating the default version it now has",
            from: propagated.clone(),
            revise_from: Some("imagebuilder:ListComponents"),
            from_version: None,
            to: propagated.clone(),
            to_version: FIRST_PERMISSION_VERSION + 1,
            // A permission for another resource type is another attachment entirely, so
            // the task must leave it alone.
            kept: vec![managed(GLUE_DATABASE_DEFAULT)],
        },
        ReplaceRow {
            case: "the same permission, restricted to the version the share holds",
            from: pinned.clone(),
            revise_from: Some("imagebuilder:ListComponents"),
            from_version: Some(FIRST_PERMISSION_VERSION),
            to: pinned.clone(),
            to_version: FIRST_PERMISSION_VERSION + 1,
            kept: vec![],
        },
        ReplaceRow {
            case: "another customer managed permission for the same resource type",
            from: replaced.clone(),
            revise_from: None,
            from_version: None,
            to: target.clone(),
            to_version: FIRST_PERMISSION_VERSION,
            kept: vec![],
        },
        ReplaceRow {
            case: "an AWS managed permission for the same resource type",
            from: glue_reader.clone(),
            revise_from: None,
            from_version: None,
            to: managed(GLUE_DATABASE_READ_WRITE),
            // The catalog publishes that permission at a later version, which is its
            // default — not the first version.
            to_version: GLUE_DATABASE_READ_WRITE_VERSION,
            kept: vec![],
        },
        ReplaceRow {
            case: "a customer managed permission replacing an AWS managed one",
            from: managed(COMPONENT_TYPE_DEFAULT),
            revise_from: None,
            from_version: None,
            to: promoted.clone(),
            to_version: FIRST_PERMISSION_VERSION,
            kept: vec![],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        row.run(&router, &index.to_string()).await;
    }
}

#[tokio::test]
async fn a_replacement_restricted_to_a_version_moves_only_the_shares_holding_it() {
    let router = emulator();
    let case = "a task restricted to one version of the permission";
    let permission = mint(
        &router,
        case,
        "ComponentVersionSelected",
        COMPONENT_TYPE,
        "imagebuilder:GetComponent",
    )
    .await;
    let other = mint(
        &router,
        case,
        "ComponentUntouched",
        COMPONENT_TYPE,
        "imagebuilder:GetComponent",
    )
    .await;

    // The first share attaches the permission's only version.
    let first = share_with(
        &router,
        case,
        &json!({"name": "replace-version-first", "resourceArns": []}),
    )
    .await;
    attach_ok(&router, case, &first, &permission).await;

    // A second version becomes the default, and the second share attaches that one.
    write_version(&router, case, &permission, "imagebuilder:ListComponents").await;
    let second = share_with(
        &router,
        case,
        &json!({"name": "replace-version-second", "resourceArns": []}),
    )
    .await;
    attach_ok(&router, case, &second, &permission).await;

    // A third share is governed by another permission entirely.
    let bystander = share_with(
        &router,
        case,
        &json!({"name": "replace-version-bystander", "resourceArns": []}),
    )
    .await;
    attach_ok(&router, case, &bystander, &other).await;

    // A third version becomes the default, so no share holds it yet.
    write_version(&router, case, &permission, "imagebuilder:GetComponentPolicy").await;
    let default_version = FIRST_PERMISSION_VERSION + 2;

    let work = replace_ok(
        &router,
        case,
        &json!({
            "fromPermissionArn": permission,
            "fromPermissionVersion": FIRST_PERMISSION_VERSION,
            "toPermissionArn": permission,
        }),
    )
    .await;
    let work_id = field(case, &work, "id");
    poll_until_completed(&router, case, &work_id).await;

    // The share holding the version the request named is moved to the permission's
    // current default version; the one holding another version keeps it, and the share
    // the permission does not govern is untouched.
    assert_eq!(
        governed_at(&router, case, &first, &permission).await,
        (default_version.to_string(), true),
        "case {case:?}: the share holding version {FIRST_PERMISSION_VERSION} is moved to the \
         default version {default_version}"
    );
    assert_eq!(
        governed_at(&router, case, &second, &permission).await,
        ((FIRST_PERMISSION_VERSION + 1).to_string(), false),
        "case {case:?}: the share holding another version of the permission keeps it"
    );
    assert_eq!(
        listed_arns(&router, case, &bystander).await,
        vec![other.clone()],
        "case {case:?}: a share the replaced permission does not govern is left alone"
    );
}

#[tokio::test]
async fn a_replacement_that_cannot_be_made_is_refused_and_starts_no_task() {
    let router = emulator();
    let case = "set up the permissions the refused rows name";
    let component = mint(
        &router,
        case,
        "ComponentRefusals",
        COMPONENT_TYPE,
        "imagebuilder:GetComponent",
    )
    .await;
    let share_arn = share_with(
        &router,
        case,
        &json!({"name": "replace-refusals", "resourceArns": []}),
    )
    .await;
    attach_ok(&router, case, &share_arn, &component).await;
    let missing = customer("ComponentNothingStored");
    let beyond = FIRST_PERMISSION_VERSION + 3;

    let rows = [
        (
            "a fromPermissionArn that is not a permission ARN",
            json!({"fromPermissionArn": "component-reader", "toPermissionArn": component}),
            "MalformedArnException",
            permission_arn_rejection("fromPermissionArn", "component-reader")
                .expect("a value that is not a permission ARN is refused"),
        ),
        (
            "a toPermissionArn naming another kind of resource",
            json!({"fromPermissionArn": component, "toPermissionArn": share_arn}),
            "MalformedArnException",
            permission_arn_rejection("toPermissionArn", &share_arn)
                .expect("a share ARN is not a permission ARN"),
        ),
        (
            "a version below the first one any permission carries",
            json!({
                "fromPermissionArn": component,
                "fromPermissionVersion": FIRST_PERMISSION_VERSION - 1,
                "toPermissionArn": component,
            }),
            "InvalidParameterException",
            permission_version_below_first_message("fromPermissionVersion"),
        ),
        (
            "a fromPermissionArn nothing is stored under",
            json!({"fromPermissionArn": missing, "toPermissionArn": component}),
            "UnknownResourceException",
            unknown_permission_message(&missing),
        ),
        (
            "a toPermissionArn nothing is stored under",
            json!({"fromPermissionArn": component, "toPermissionArn": missing}),
            "UnknownResourceException",
            unknown_permission_message(&missing),
        ),
        (
            "a version the permission never carried",
            json!({
                "fromPermissionArn": component,
                "fromPermissionVersion": beyond,
                "toPermissionArn": component,
            }),
            "UnknownResourceException",
            unknown_permission_version_message(&component, beyond),
        ),
        (
            "two permissions for different resource types",
            json!({
                "fromPermissionArn": component,
                "toPermissionArn": managed(GLUE_DATABASE_DEFAULT),
            }),
            "InvalidParameterException",
            permission_resource_type_mismatch_message(
                &component,
                COMPONENT_TYPE,
                &managed(GLUE_DATABASE_DEFAULT),
                "glue:Database",
            ),
        ),
    ];

    for (case, request, code, message) in rows {
        let (status, refused) = replace(&router, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {request}, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            code,
            "case {case:?}: expected {code}, got {refused:?}"
        );
        assert_eq!(
            error_message(&refused),
            message,
            "case {case:?}: unexpected refusal wording, got {refused:?}"
        );
    }

    // A refused request starts nothing: no task is held and the share is governed by
    // exactly what it attached.
    let (status, listed_tasks) = list_work(&router, &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 reading the tasks, got {status} with {listed_tasks:?}"
    );
    assert_eq!(
        works_of(case, &listed_tasks),
        Vec::<Value>::new(),
        "case {case:?}: every replacement above was refused, so no task was started"
    );
    assert_eq!(
        listed_arns(&router, case, &share_arn).await,
        vec![component.clone()],
        "case {case:?}: a refused replacement moved nothing"
    );
}

#[tokio::test]
async fn a_replacements_client_token_answers_for_a_repeat_of_that_request() {
    let router = emulator();
    let case = "a repeat of the same replacement";
    let component = mint(
        &router,
        case,
        "ComponentTokened",
        COMPONENT_TYPE,
        "imagebuilder:GetComponent",
    )
    .await;
    let target = mint(
        &router,
        case,
        "ComponentTokenedTarget",
        COMPONENT_TYPE,
        "imagebuilder:ListComponents",
    )
    .await;
    let token = "ram-replace-token";
    let request = json!({
        "fromPermissionArn": component,
        "toPermissionArn": target,
        "clientToken": token,
    });

    let first = replace_ok(&router, case, &request).await;
    let repeat = replace_ok(&router, case, &request).await;
    assert_eq!(
        field(case, &repeat, "id"),
        field(case, &first, "id"),
        "case {case:?}: a repeat reports the task the first call started, got {repeat:?}"
    );
    let (status, listed_tasks) = list_work(&router, &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 reading the tasks, got {status} with {listed_tasks:?}"
    );
    assert_eq!(
        works_of(case, &listed_tasks).len(),
        1,
        "case {case:?}: the repeat started no second task, got {listed_tasks:?}"
    );

    // The same token with any other parameter is a different request.
    let case = "the same token for a different replacement";
    let (status, refused) = replace(
        &router,
        &json!({
            "fromPermissionArn": target,
            "toPermissionArn": component,
            "clientToken": token,
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "IdempotentParameterMismatchException",
        "case {case:?}: expected IdempotentParameterMismatchException, got {refused:?}"
    );
    assert_eq!(
        error_message(&refused),
        IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE,
        "case {case:?}: unexpected refusal wording, got {refused:?}"
    );
}

#[tokio::test]
async fn the_task_read_selects_by_id_and_status_and_pages_in_id_order() {
    let router = emulator();
    let case = "three tasks to read back";
    let mut permissions = Vec::new();
    for name in ["ComponentListedOne", "ComponentListedTwo", "ComponentListedThree"] {
        permissions.push(
            mint(
                &router,
                case,
                name,
                COMPONENT_TYPE,
                "imagebuilder:GetComponent",
            )
            .await,
        );
    }
    let mut ids = Vec::new();
    for permission in &permissions {
        let work = replace_ok(
            &router,
            case,
            &json!({"fromPermissionArn": permission, "toPermissionArn": permission}),
        )
        .await;
        ids.push(field(case, &work, "id"));
    }
    let mut ordered = ids.clone();
    ordered.sort();

    let reported_ids = |body: &Value| -> Vec<String> {
        works_of(case, body)
            .iter()
            .map(|work| field(case, work, "id"))
            .collect()
    };

    // A read that names no filter reports every task, in id order.
    let (status, every) = list_work(&router, &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {every:?}"
    );
    assert_eq!(
        reported_ids(&every),
        ordered,
        "case {case:?}: every task is reported, in id order"
    );

    // `workIds` selects exactly the tasks it names, and an id nothing answers to selects
    // nothing.
    let wanted = vec![ids[0].clone(), ids[2].clone()];
    let mut wanted_order = wanted.clone();
    wanted_order.sort();
    let (_, selected) = list_work(&router, &json!({"workIds": wanted})).await;
    assert_eq!(
        reported_ids(&selected),
        wanted_order,
        "case {case:?}: a read naming ids reports those tasks only"
    );
    let (_, none) = list_work(
        &router,
        &json!({"workIds": ["00000000-0000-4000-8000-000000000000"]}),
    )
    .await;
    assert_eq!(
        reported_ids(&none),
        Vec::<String>::new(),
        "case {case:?}: an id no task answers to reports nothing"
    );

    // `status` selects by where a task has got to: in flight now, completed once the
    // clock has made each move due.
    let (_, in_flight) =
        list_work(&router, &json!({"status": REPLACE_WORK_STATUS_IN_PROGRESS})).await;
    assert_eq!(
        reported_ids(&in_flight),
        ordered,
        "case {case:?}: every task is in flight while its move is not due"
    );
    for id in &ordered {
        poll_until_completed(&router, case, id).await;
    }
    let (_, completed) =
        list_work(&router, &json!({"status": REPLACE_WORK_STATUS_COMPLETED})).await;
    assert_eq!(
        reported_ids(&completed),
        ordered,
        "case {case:?}: every task has completed once its move fell due"
    );
    let (_, still_in_flight) =
        list_work(&router, &json!({"status": REPLACE_WORK_STATUS_IN_PROGRESS})).await;
    assert_eq!(
        reported_ids(&still_in_flight),
        Vec::<String>::new(),
        "case {case:?}: no task is left in flight"
    );

    // One page at a time: a page holds at most `maxResults` tasks and the token walks
    // the rest, each task exactly once.
    for page_size in 1..=4 {
        let case = format!("pages of {page_size}");
        let mut walked = Vec::new();
        let mut next_token: Option<String> = None;
        for _ in 0..=ordered.len() {
            let mut request = json!({"maxResults": page_size});
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let (status, page) = list_work(&router, &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {page:?}"
            );
            let on_page = works_of(&case, &page);
            assert!(
                on_page.len() <= page_size,
                "case {case:?}: a page holds at most {page_size} tasks, got {on_page:?}"
            );
            walked.extend(on_page.iter().map(|work| field(&case, work, "id")));
            next_token = page
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            (walked, next_token),
            (ordered.clone(), None),
            "case {case:?}: following nextToken walks every task exactly once"
        );
    }

    // A token this read never issued is refused.
    let case = "a token this read never issued";
    let (status, refused) = list_work(&router, &json!({"nextToken": "not-a-token"})).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidNextTokenException",
        "case {case:?}: expected InvalidNextTokenException, got {refused:?}"
    );
}
