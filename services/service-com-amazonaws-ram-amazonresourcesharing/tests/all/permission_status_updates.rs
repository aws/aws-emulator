// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdatePermission`: the status it writes onto a stored
//! customer managed permission, and the one thing a deprecated permission may no longer
//! do.
//!
//! The rules the rows below pin, each asserted over a table rather than one case:
//!
//! * The status is state the WHOLE permission carries. `UpdatePermission` "updates the
//!   status of the specified customer managed permission", so the call answers with the
//!   status it wrote and every later read of the permission — of any version, through
//!   any operation that reports one — reports it, until another call changes it. Asking
//!   for the status the permission already carries is the same success, and reading the
//!   permission never moves it.
//! * Deprecation stops exactly one thing: a NEW association of the permission with a
//!   resource share. "Setting the status to DEPRECATED prevents new resource share
//!   associations … existing resource shares that use the permission continue to
//!   function", so every operation that would make such an association is refused as
//!   `OperationNotPermittedException` while the deprecation stands, the shares already
//!   governed by the permission go on reporting it, and `ATTACHABLE` lets the
//!   association be made again.
//! * A call the service cannot apply is refused with the modelled error RAM words that
//!   refusal with, and changes nothing.
//!
//! The rows vary along what the rules generalise over: which status is asked for and in
//! what sequence (including the one the permission already carries), how many versions
//! the permission carries when its status changes, WHICH operation would make the
//! association a deprecation stops, and WHY a call cannot be applied.

use serde_json::{json, Value};
use service_com_amazonaws_ram_amazonresourcesharing::state::{
    aws_managed_permission_arn, AWS_MANAGED_PERMISSIONS, IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

// Every suite in this crate calls the service the one way: through `tests/all/harness.rs`.
use super::harness::*;

/// The two values `UpdatePermissionStatus` admits, from
/// `smithy_query(query="shape",shape="UpdatePermissionStatus")`.
const DEPRECATED: &str = "DEPRECATED";
const ATTACHABLE: &str = "ATTACHABLE";

/// A resource type a customer managed permission can be created for, and the actions
/// its templates allow (`CreatePermission$resourceType`, `RESOURCE_TYPES`).
const RESOURCE_TYPE: &str = "codebuild:Project";
const TEMPLATE: &str = r#"{"Effect":"ALLOW","Action":["codebuild:ListProjects"]}"#;
const OTHER_TEMPLATE: &str = r#"{"Effect":"ALLOW","Action":["codebuild:BatchGetProjects"]}"#;

async fn create_permission(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/createpermission", Some(body)).await
}

async fn create_permission_version(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/createpermissionversion", Some(body)).await
}

async fn update_permission(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/updatepermission", Some(body)).await
}

async fn get_permission(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/getpermission", Some(body)).await
}

async fn list_permissions(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/listpermissions", Some(body)).await
}

async fn list_permission_versions(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/listpermissionversions", Some(body)).await
}

async fn list_share_permissions(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/listresourcesharepermissions", Some(body)).await
}

async fn attach_permission(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(
        router,
        "POST",
        "/associateresourcesharepermission",
        Some(body),
    )
    .await
}

async fn replace_permission_associations(
    router: &MultiServiceRouter,
    body: &Value,
) -> (u16, Value) {
    call(router, "POST", "/replacepermissionassociations", Some(body)).await
}

/// `DeletePermission`, whose members are query parameters
/// (`smithy_query(query="operation",operation="DeletePermission")`).
async fn delete_permission(router: &MultiServiceRouter, permission_arn: &str) -> (u16, Value) {
    let uri = format!(
        "/deletepermission?permissionArn={}",
        query_escape(permission_arn)
    );
    call(router, "DELETE", &uri, None).await
}

/// The ARN a customer managed permission of this name is minted under.
fn customer(name: &str) -> String {
    format!("arn:aws:ram:{REGION}:{ACCOUNT}:permission/{name}")
}

/// Mint a customer managed permission named `name`, with `extra_versions` further
/// versions written for it, and answer its ARN.
async fn mint(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    extra_versions: usize,
) -> String {
    let (status, minted) = create_permission(
        router,
        &json!({
            "name": name,
            "resourceType": RESOURCE_TYPE,
            "policyTemplate": TEMPLATE,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: minting {name} expected 200, got {status} with {minted:?}"
    );
    let arn = customer(name);
    for index in 0..extra_versions {
        let (status, written) = create_permission_version(
            router,
            &json!({"permissionArn": arn, "policyTemplate": OTHER_TEMPLATE}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: writing version {} expected 200, got {status} with {written:?}",
            index + 2
        );
    }
    arn
}

/// A string member of a reported permission.
fn field(case: &str, permission: &Value, member: &str) -> String {
    permission
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} member, got {permission:?}"))
        .to_string()
}

/// The permissions a read reported.
fn permissions_of(case: &str, body: &Value) -> Vec<Value> {
    body.get("permissions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: response carried no permissions, got {body:?}"))
}

/// The `status` every read of `permission_arn` reports, one entry per read: the
/// permission's own read at its default version and at each version it carries, the
/// summary the whole-store listing projects, and the summaries the per-version listing
/// projects.
async fn reported_statuses(
    router: &MultiServiceRouter,
    case: &str,
    permission_arn: &str,
    versions: usize,
) -> Vec<(String, String)> {
    let mut statuses = Vec::new();

    let (status, read) = get_permission(router, &json!({"permissionArn": permission_arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetPermission expected 200, got {status} with {read:?}"
    );
    let detail = read
        .get("permission")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the read reported no permission, got {read:?}"));
    statuses.push((
        "GetPermission, the default version".to_string(),
        field(case, &detail, "status"),
    ));

    for version in 1..=versions {
        let (status, read) = get_permission(
            router,
            &json!({"permissionArn": permission_arn, "permissionVersion": version}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetPermission of version {version} expected 200, got {status} with \
             {read:?}"
        );
        let detail = read.get("permission").cloned().unwrap_or_else(|| {
            panic!("case {case:?}: the read reported no permission, got {read:?}")
        });
        statuses.push((
            format!("GetPermission, version {version}"),
            field(case, &detail, "status"),
        ));
    }

    let (status, listed) =
        list_permissions(router, &json!({"permissionType": "CUSTOMER_MANAGED"})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListPermissions expected 200, got {status} with {listed:?}"
    );
    for summary in permissions_of(case, &listed)
        .iter()
        .filter(|summary| field(case, summary, "arn") == permission_arn)
    {
        statuses.push((
            format!("ListPermissions, version {}", field(case, summary, "version")),
            field(case, summary, "status"),
        ));
    }

    let (status, listed) =
        list_permission_versions(router, &json!({"permissionArn": permission_arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListPermissionVersions expected 200, got {status} with {listed:?}"
    );
    let reported = permissions_of(case, &listed);
    assert_eq!(
        reported.len(),
        versions,
        "case {case:?}: expected one summary per stored version, got {listed:?}"
    );
    for summary in &reported {
        statuses.push((
            format!(
                "ListPermissionVersions, version {}",
                field(case, summary, "version")
            ),
            field(case, summary, "status"),
        ));
    }

    statuses
}

/// A row of the status table: how many versions the permission carries, and the
/// statuses the call writes onto it in order.
struct StatusCase {
    case: &'static str,
    name: &'static str,
    extra_versions: usize,
    steps: Vec<&'static str>,
}

#[tokio::test]
async fn the_status_a_call_writes_is_what_every_later_read_reports() {
    let router = emulator();

    // The rows differ along what the rule generalises over: which status is asked for,
    // the sequence it is asked for in (including one that asks for the status the
    // permission already carries, and one that asks the same thing twice), and how many
    // versions the permission carries while its status moves.
    let cases = vec![
        StatusCase {
            case: "deprecate a permission carrying one version",
            name: "StatusOneVersion",
            extra_versions: 0,
            steps: vec![DEPRECATED],
        },
        StatusCase {
            case: "deprecate and then restore",
            name: "StatusRestored",
            extra_versions: 0,
            steps: vec![DEPRECATED, ATTACHABLE],
        },
        StatusCase {
            case: "deprecate twice",
            name: "StatusDeprecatedTwice",
            extra_versions: 0,
            steps: vec![DEPRECATED, DEPRECATED],
        },
        StatusCase {
            case: "restore a permission nobody deprecated",
            name: "StatusAlreadyAttachable",
            extra_versions: 0,
            steps: vec![ATTACHABLE],
        },
        StatusCase {
            case: "deprecate a permission carrying three versions",
            name: "StatusThreeVersions",
            extra_versions: 2,
            steps: vec![DEPRECATED],
        },
        StatusCase {
            case: "move the status back and forth over two versions",
            name: "StatusBackAndForth",
            extra_versions: 1,
            steps: vec![ATTACHABLE, DEPRECATED, ATTACHABLE, DEPRECATED],
        },
    ];

    for StatusCase {
        case,
        name,
        extra_versions,
        steps,
    } in cases
    {
        let arn = mint(&router, case, name, extra_versions).await;
        let versions = extra_versions + 1;

        // Before any call, the permission is attachable: that is the status a create
        // leaves behind, and it is what the steps below are read against.
        for (read, reported) in reported_statuses(&router, case, &arn, versions).await {
            assert_eq!(
                reported, ATTACHABLE,
                "case {case:?}: before the update, {read} expected {ATTACHABLE}, got {reported}"
            );
        }

        for (index, step) in steps.iter().enumerate() {
            let label = format!("{case}/step {index} -> {step}");
            let (status, body) =
                update_permission(&router, &json!({"permissionArn": arn, "status": step})).await;
            assert_eq!(
                status, 200,
                "case {label:?}: UpdatePermission expected 200, got {status} with {body:?}"
            );
            assert_eq!(
                body.get("permissionStatus").and_then(Value::as_str),
                Some(*step),
                "case {label:?}: expected the status it wrote, got {body:?}"
            );
            assert!(
                body.get("clientToken")
                    .and_then(Value::as_str)
                    .is_some_and(|token| !token.is_empty()),
                "case {label:?}: a call that names no clientToken is answered with the one the \
                 service generated, got {body:?}"
            );

            // Every read of the permission, at every version it carries, reports the
            // status the call wrote — and reading it twice does not move it.
            for round in 0..2 {
                for (read, reported) in reported_statuses(&router, case, &arn, versions).await {
                    assert_eq!(
                        reported, *step,
                        "case {label:?}: read {round}, {read} expected {step}, got {reported}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn a_deprecated_permission_cannot_be_newly_associated_with_a_resource_share() {
    let router = emulator();
    let case = "a deprecated permission";

    let permission = mint(&router, case, "DeprecationProbe", 0).await;
    // A second permission of the same resource type, so a replacement onto the
    // deprecated one has a live `from` end of the type a replacement holds between.
    let other = mint(&router, case, "DeprecationProbeOther", 0).await;

    // A share governed by the permission BEFORE it is deprecated, so the rule can be
    // read against an association that already exists.
    let governed = arn_of(
        case,
        &create_ok(
            &router,
            case,
            &json!({"name": "deprecation-governed", "permissionArns": [permission]}),
        )
        .await,
    );
    // A share governed by the OTHER permission, which the replacement would move.
    let moved = arn_of(
        case,
        &create_ok(
            &router,
            case,
            &json!({"name": "deprecation-moved", "permissionArns": [other]}),
        )
        .await,
    );
    // A share holding no permission of this resource type, so an attach of one is a NEW
    // association rather than a replacement of an existing attachment.
    let bare = arn_of(
        case,
        &create_ok(&router, case, &json!({"name": "deprecation-bare"})).await,
    );

    let (status, body) =
        update_permission(&router, &json!({"permissionArn": permission, "status": DEPRECATED}))
            .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the deprecation expected 200, got {status} with {body:?}"
    );

    let expected_message = format!(
        "Permission {permission} is DEPRECATED, so it can't be associated with new resource \
         shares. Set its status to {ATTACHABLE} to associate it again."
    );

    // One row per operation that would make a new association with the permission: the
    // attach, a create naming it, and a replacement moving shares onto it.
    let rows: Vec<(&str, (u16, Value))> = vec![
        (
            "AssociateResourceSharePermission",
            attach_permission(
                &router,
                &json!({"resourceShareArn": bare, "permissionArn": permission}),
            )
            .await,
        ),
        (
            "CreateResourceShare naming the permission",
            create(
                &router,
                &json!({"name": "deprecation-refused", "permissionArns": [permission]}),
            )
            .await,
        ),
        (
            "ReplacePermissionAssociations onto the permission",
            replace_permission_associations(
                &router,
                &json!({"fromPermissionArn": other, "toPermissionArn": permission}),
            )
            .await,
        ),
    ];
    for (row, (status, body)) in rows {
        assert_eq!(
            status, 400,
            "case {row:?}: expected 400 while the permission is deprecated, got {status} with \
             {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "OperationNotPermittedException",
            "case {row:?}: wrong error shape in {body:?}"
        );
        assert_eq!(
            error_message(&body),
            expected_message,
            "case {row:?}: wrong message in {body:?}"
        );
    }

    // The share created with no permission of the type took none of the refused
    // attaches, and the share already governed by the permission still is: "existing
    // resource shares that use the permission continue to function".
    for (row, share, expected) in [
        ("the share already governed", &governed, true),
        ("the share the attach was refused for", &bare, false),
    ] {
        let (status, listed) =
            list_share_permissions(&router, &json!({"resourceShareArn": share})).await;
        assert_eq!(
            status, 200,
            "case {row:?}: ListResourceSharePermissions expected 200, got {status} with {listed:?}"
        );
        let holds = permissions_of(row, &listed)
            .iter()
            .any(|summary| field(row, summary, "arn") == permission);
        assert_eq!(
            holds, expected,
            "case {row:?}: expected the share to hold the permission: {expected}, got {listed:?}"
        );
    }
    // And the replacement moved nothing: the share it would have moved still holds the
    // permission it was created with.
    let (status, listed) =
        list_share_permissions(&router, &json!({"resourceShareArn": moved})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListResourceSharePermissions expected 200, got {status} with {listed:?}"
    );
    assert!(
        permissions_of(case, &listed)
            .iter()
            .any(|summary| field(case, summary, "arn") == other),
        "case {case:?}: the refused replacement must leave the share on its own permission, got \
         {listed:?}"
    );

    // `ATTACHABLE` "restores a previously deprecated permission": the very attach that
    // was refused now succeeds.
    let (status, body) =
        update_permission(&router, &json!({"permissionArn": permission, "status": ATTACHABLE}))
            .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the restore expected 200, got {status} with {body:?}"
    );
    let (status, attached) = attach_permission(
        &router,
        &json!({"resourceShareArn": bare, "permissionArn": permission}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the attach after the restore expected 200, got {status} with {attached:?}"
    );
    let (status, listed) =
        list_share_permissions(&router, &json!({"resourceShareArn": bare})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListResourceSharePermissions expected 200, got {status} with {listed:?}"
    );
    assert!(
        permissions_of(case, &listed)
            .iter()
            .any(|summary| field(case, summary, "arn") == permission),
        "case {case:?}: the restored permission must govern the share it was attached to, got \
         {listed:?}"
    );
}

#[tokio::test]
async fn an_update_the_service_cannot_apply_is_refused_as_ram_words_it() {
    let router = emulator();
    let case = "refusals";

    let permission = mint(&router, case, "RefusalProbe", 0).await;
    // A permission on the delete path, which is no longer one a status describes.
    let deleted = mint(&router, case, "RefusalDeleted", 0).await;
    let (status, body) = delete_permission(&router, &deleted).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the delete expected 200, got {status} with {body:?}"
    );
    // A permission Amazon Web Services manages, taken from the library this service
    // publishes rather than spelled out.
    let aws_managed = aws_managed_permission_arn(AWS_MANAGED_PERMISSIONS[0].name);
    let unknown = customer("RefusalNothingMintedThis");
    // A short ARN that is not a managed-permission ARN: the refusal's message carries
    // the value it was given, and a long one is clipped before it is reported.
    let share = format!("arn:aws:ram:{REGION}:{ACCOUNT}:resource-share/abc");

    // A token this call used once, so a second call reusing it for another status is the
    // mismatch case.
    let (status, body) = update_permission(
        &router,
        &json!({"permissionArn": permission, "status": DEPRECATED, "clientToken": "reused"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the tokened call expected 200, got {status} with {body:?}"
    );

    // One row per reason a call cannot be applied, each with the modelled error shape and
    // the message RAM words that refusal with.
    let rows: Vec<(&str, Value, &str, String)> = vec![
        (
            "a permissionArn that is not an ARN",
            json!({"permissionArn": "not-an-arn", "status": DEPRECATED}),
            "MalformedArnException",
            "permissionArn 'not-an-arn' is not a managed permission ARN of the form \
             arn:<partition>:ram:<region>:<account>:permission/<name>"
                .to_string(),
        ),
        (
            "a permissionArn that names a resource share",
            json!({"permissionArn": share, "status": DEPRECATED}),
            "MalformedArnException",
            format!(
                "permissionArn '{share}' is not a managed permission ARN of the form \
                 arn:<partition>:ram:<region>:<account>:permission/<name>"
            ),
        ),
        (
            "a permission nothing was created under",
            json!({"permissionArn": unknown, "status": DEPRECATED}),
            "UnknownResourceException",
            format!("Permission {unknown} could not be found."),
        ),
        (
            "a permission on the delete path",
            json!({"permissionArn": deleted, "status": DEPRECATED}),
            "UnknownResourceException",
            format!("Permission {deleted} could not be found."),
        ),
        (
            "a permission Amazon Web Services manages",
            json!({"permissionArn": aws_managed, "status": DEPRECATED}),
            "OperationNotPermittedException",
            format!(
                "Permission {aws_managed} is an Amazon Web Services managed permission, which \
                 Amazon Web Services creates and manages. You can't modify it."
            ),
        ),
        (
            "an empty clientToken",
            json!({"permissionArn": permission, "status": DEPRECATED, "clientToken": ""}),
            "InvalidClientTokenException",
            "clientToken must not be empty".to_string(),
        ),
        (
            "a clientToken reused for another status",
            json!({"permissionArn": permission, "status": ATTACHABLE, "clientToken": "reused"}),
            "IdempotentParameterMismatchException",
            IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE.to_string(),
        ),
    ];

    for (row, request, expected_code, expected_message) in rows {
        let (status, body) = update_permission(&router, &request).await;
        assert_eq!(
            status, 400,
            "case {row:?}: expected 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {row:?}: wrong error shape in {body:?}"
        );
        assert_eq!(
            error_message(&body),
            expected_message,
            "case {row:?}: wrong message in {body:?}"
        );
    }

    // Nothing above changed a stored status: the AWS managed permission is still
    // attachable, and the one the refused calls named still carries the status the one
    // call that succeeded wrote.
    for (row, arn, expected) in [
        ("the permission the refusals named", &permission, DEPRECATED),
        ("the permission Amazon Web Services manages", &aws_managed, ATTACHABLE),
    ] {
        for (read, reported) in reported_statuses(&router, row, arn, 1).await {
            assert_eq!(
                reported, expected,
                "case {row:?}: {read} expected {expected}, got {reported}"
            );
        }
    }
}

#[tokio::test]
async fn repeating_a_call_under_the_same_token_writes_nothing_further() {
    let router = emulator();
    let case = "an idempotent repeat";
    let arn = mint(&router, case, "IdempotentProbe", 0).await;

    let request = json!({"permissionArn": arn, "status": DEPRECATED, "clientToken": "once"});
    let (status, first) = update_permission(&router, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the first call expected 200, got {status} with {first:?}"
    );
    assert_eq!(
        first.get("clientToken").and_then(Value::as_str),
        Some("once"),
        "case {case:?}: the call reports the token it was given, got {first:?}"
    );

    // What the permission looks like after the first call, so the repeat can be held to
    // leaving it exactly there.
    let (_, after_first) = get_permission(&router, &json!({"permissionArn": arn})).await;

    let (status, repeated) = update_permission(&router, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the repeat expected 200, got {status} with {repeated:?}"
    );
    assert_eq!(
        repeated, first,
        "case {case:?}: the repeat must report the same success, got {repeated:?}"
    );
    let (_, after_repeat) = get_permission(&router, &json!({"permissionArn": arn})).await;
    assert_eq!(
        after_repeat, after_first,
        "case {case:?}: the repeat must leave the permission exactly as the first call did, got \
         {after_repeat:?}"
    );

    // And the token cannot answer for a permission that is gone: once the delete has
    // taken it off the live population, the very same call is refused as the unknown
    // resource it now is.
    let (status, deleted) = delete_permission(&router, &arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the delete expected 200, got {status} with {deleted:?}"
    );
    let (status, refused) = update_permission(&router, &request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: the repeat after the delete expected 400, got {status} with {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "UnknownResourceException",
        "case {case:?}: wrong error shape in {refused:?}"
    );
    assert_eq!(
        error_message(&refused),
        format!("Permission {arn} could not be found."),
        "case {case:?}: wrong message in {refused:?}"
    );
}

/// Every value `PermissionStatus` admits, from
/// `smithy_query(query="shape",shape="PermissionStatus")`, which is the shape
/// `ListPermissions$permissionStatus` filters on.
const PERMISSION_STATUSES: [&str; 5] = [
    "ATTACHABLE",
    "UNATTACHABLE",
    "DELETING",
    "DELETED",
    "DEPRECATED",
];

/// The ARNs one `ListPermissions` page reports for `request`, paired with the status
/// each entry carries. The page is asked for at the published maximum of `maxResults`,
/// so the whole population fits in it and the read reports no `nextToken`.
async fn listed_with(
    router: &MultiServiceRouter,
    case: &str,
    request: &Value,
) -> Vec<(String, String)> {
    let mut body = request.clone();
    body["maxResults"] = json!(500);
    let (status, listed) = list_permissions(router, &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListPermissions expected 200, got {status} with {listed:?}"
    );
    assert!(
        listed.get("nextToken").is_none(),
        "case {case:?}: the whole population must fit in one page, got {listed:?}"
    );
    permissions_of(case, &listed)
        .iter()
        .map(|summary| (field(case, summary, "arn"), field(case, summary, "status")))
        .collect()
}

#[tokio::test]
async fn listing_permissions_by_status_keeps_the_permissions_reporting_it() {
    let router = emulator();
    let case = "the status filter";

    // Three stored permissions, one per status this store can put a permission in: one
    // left as its create wrote it, one `UpdatePermission` deprecated, and one on the
    // delete path.
    let attachable = mint(&router, case, "FilterAttachable", 0).await;
    let deprecated = mint(&router, case, "FilterDeprecated", 0).await;
    let deleting = mint(&router, case, "FilterDeleting", 0).await;
    let (status, body) = update_permission(
        &router,
        &json!({"permissionArn": deprecated, "status": DEPRECATED}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the deprecation expected 200, got {status} with {body:?}"
    );
    let (status, body) = delete_permission(&router, &deleting).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the delete expected 200, got {status} with {body:?}"
    );

    // A read that names no filter keeps every permission, whatever its status.
    let unfiltered = listed_with(&router, case, &json!({})).await;
    for arn in [&attachable, &deprecated, &deleting] {
        assert!(
            unfiltered.iter().any(|(listed, _)| listed == arn),
            "case {case:?}: an unfiltered read must report {arn}, got {} entries",
            unfiltered.len()
        );
    }

    // One row per value the filter admits: every entry a filtered read reports carries
    // exactly that status — the invariant, whatever the store happens to hold — and the
    // three probes are selected by their own status alone.
    for wanted in PERMISSION_STATUSES {
        let filtered = listed_with(&router, wanted, &json!({"permissionStatus": wanted})).await;
        for (arn, status) in &filtered {
            assert_eq!(
                status, wanted,
                "case {wanted:?}: the read must report only permissions with that status, got \
                 {status} for {arn}"
            );
        }
        for (probe, probe_status) in [
            (&attachable, "ATTACHABLE"),
            (&deprecated, "DEPRECATED"),
            (&deleting, "DELETING"),
        ] {
            let reported = filtered.iter().any(|(arn, _)| arn == probe);
            assert_eq!(
                reported,
                probe_status == wanted,
                "case {wanted:?}: expected {probe} (status {probe_status}) to be reported: {}, \
                 got {reported}",
                probe_status == wanted
            );
        }
        // The AWS managed library is attachable, so it is kept by that status alone.
        let published = aws_managed_permission_arn(AWS_MANAGED_PERMISSIONS[0].name);
        assert_eq!(
            filtered.iter().any(|(arn, _)| *arn == published),
            wanted == "ATTACHABLE",
            "case {wanted:?}: a published permission is ATTACHABLE, got {filtered:?}"
        );
    }

    // A value the shape does not admit names no status to keep: `permissionStatus`
    // targets the `PermissionStatus` enum, so the modelled-constraint walk refuses it
    // before the read runs rather than answering a page.
    let (status, refused) =
        list_permissions(&router, &json!({"permissionStatus": "NOT-A-STATUS"})).await;
    assert_eq!(
        status, 400,
        "case {case:?}: an unmodelled filter expected 400, got {status} with {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "case {case:?}: wrong error shape in {refused:?}"
    );
    for value in PERMISSION_STATUSES {
        assert!(
            error_message(&refused).contains(value),
            "case {case:?}: the refusal must name the values the enum admits, got {refused:?}"
        );
    }
}

#[tokio::test]
async fn listing_permissions_by_owner_keeps_the_permissions_that_account_owns() {
    let router = emulator();
    let case = "the owner filter";

    // A permission this account created, beside the library Amazon Web Services
    // publishes, so both populations the store holds are in range of the filter.
    let own = mint(&router, case, "OwnerProbe", 0).await;
    let published = aws_managed_permission_arn(AWS_MANAGED_PERMISSIONS[0].name);

    // One row per value `ResourceOwner` admits, plus the read that names none: the owner
    // of a permission is the account its ARN names, so `SELF` keeps the ones this
    // account created and `OTHER-ACCOUNTS` keeps the library Amazon Web Services shares
    // with it. The member declares no default, so an unfiltered read keeps both.
    for (row, request, expect_own, expect_published) in [
        ("no owner named", json!({}), true, true),
        ("SELF", json!({"resourceOwner": "SELF"}), true, false),
        (
            "OTHER-ACCOUNTS",
            json!({"resourceOwner": "OTHER-ACCOUNTS"}),
            false,
            true,
        ),
    ] {
        let listed = listed_with(&router, row, &request).await;
        for (probe, expected) in [(&own, expect_own), (&published, expect_published)] {
            let reported = listed.iter().any(|(arn, _)| arn == probe);
            assert_eq!(
                reported, expected,
                "case {row:?}: expected {probe} to be reported: {expected}, got {reported}"
            );
        }
        // And the rule holds over the WHOLE page rather than the two probes: every entry
        // a filtered read reports is on the side of the account the filter asked for.
        if let Some(owner) = request.get("resourceOwner").and_then(Value::as_str) {
            for (arn, _) in &listed {
                let owned_here = arn.starts_with(&format!("arn:aws:ram:{REGION}:{ACCOUNT}:"));
                assert_eq!(
                    owned_here,
                    owner == "SELF",
                    "case {row:?}: {arn} is on the wrong side of the owner filter"
                );
            }
        }
    }

    // The filter is one of several, and they compose: this account owns no AWS managed
    // permission, so asking for both selects nothing.
    let listed = listed_with(
        &router,
        "SELF and AWS_MANAGED together",
        &json!({"resourceOwner": "SELF", "permissionType": "AWS_MANAGED"}),
    )
    .await;
    assert!(
        listed.is_empty(),
        "case \"SELF and AWS_MANAGED together\": expected no permission, got {listed:?}"
    );
}
