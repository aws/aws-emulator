// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the operations a Lambda client reaches after the function
//! itself exists: reading a version's configuration, publishing a version, writing and
//! reading a resource-based policy, and the code-signing-configuration and
//! capacity-provider resources.
//!
//! Every suite drives real HTTP requests through the router (the harness in
//! `function_lifecycle.rs`), so the assertions are about wire behaviour — status, body
//! members and bound query parameters — rather than about any handler's internals. The
//! clock-driven half (`Pending` -> `Active`, for a function and for a capacity provider)
//! is exercised by moving the stored due instant into the past, so nothing here sleeps.
//!
//! Each suite is a TABLE over the class its rule generalises over — every documented
//! principal form, every package type, every code-signing policy value — rather than a
//! replay of one recorded request.

use std::sync::Arc;

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::function_lifecycle::{
    create_active_function, create_function, function_arn, json_call, make_activation_due,
    package_bytes, zip_file, Answer, ACCOUNT, REGION, ROLE,
};

/// `GetFunctionConfiguration` on a function reference, optionally qualified.
async fn get_function_configuration(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: Option<&str>,
) -> Answer {
    let uri = match qualifier {
        Some(qualifier) => {
            format!("/2015-03-31/functions/{reference}/configuration?Qualifier={qualifier}")
        }
        None => format!("/2015-03-31/functions/{reference}/configuration"),
    };
    json_call(router, "GET", &uri, &serde_json::Value::Null).await
}

/// `PublishVersion` with the given request body.
async fn publish_version(
    router: &MultiServiceRouter,
    reference: &str,
    body: &serde_json::Value,
) -> Answer {
    let uri = format!("/2015-03-31/functions/{reference}/versions");
    json_call(router, "POST", &uri, body).await
}

/// `AddPermission` with the given request body, optionally qualified.
async fn add_permission(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: Option<&str>,
    body: &serde_json::Value,
) -> Answer {
    let uri = match qualifier {
        Some(qualifier) => format!("/2015-03-31/functions/{reference}/policy?Qualifier={qualifier}"),
        None => format!("/2015-03-31/functions/{reference}/policy"),
    };
    json_call(router, "POST", &uri, body).await
}

/// `GetPolicy`, optionally qualified.
async fn get_policy(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: Option<&str>,
) -> Answer {
    let uri = match qualifier {
        Some(qualifier) => format!("/2015-03-31/functions/{reference}/policy?Qualifier={qualifier}"),
        None => format!("/2015-03-31/functions/{reference}/policy"),
    };
    json_call(router, "GET", &uri, &serde_json::Value::Null).await
}

/// `RemovePermission`, optionally carrying a `RevisionId` precondition.
async fn remove_permission(
    router: &MultiServiceRouter,
    reference: &str,
    statement_id: &str,
    revision_id: Option<&str>,
) -> Answer {
    remove_permission_on(router, reference, statement_id, None, revision_id).await
}

/// `RemovePermission` on the policy of one qualifier, optionally carrying a `RevisionId`
/// precondition: `Qualifier` and `RevisionId` are both query parameters of the bound
/// `DELETE /2015-03-31/functions/{FunctionName}/policy/{StatementId}`.
async fn remove_permission_on(
    router: &MultiServiceRouter,
    reference: &str,
    statement_id: &str,
    qualifier: Option<&str>,
    revision_id: Option<&str>,
) -> Answer {
    let mut query: Vec<String> = Vec::new();
    if let Some(qualifier) = qualifier {
        query.push(format!("Qualifier={qualifier}"));
    }
    if let Some(revision_id) = revision_id {
        query.push(format!("RevisionId={revision_id}"));
    }
    let suffix = if query.is_empty() {
        String::new()
    } else {
        format!("?{}", query.join("&"))
    };
    let uri = format!("/2015-03-31/functions/{reference}/policy/{statement_id}{suffix}");
    json_call(router, "DELETE", &uri, &serde_json::Value::Null).await
}

/// One row of the `Timeout` bound table: the value asked for, and whether AWS's
/// published quota admits it.
struct TimeoutCase {
    name: &'static str,
    timeout: i64,
    accepted: bool,
}

/// `CreateFunction` admits a `Timeout` up to AWS's published Lambda quota — "Function
/// timeout: 900 seconds (15 minutes)" — and refuses one past it with the modelled 400,
/// naming the member and the bound it failed.
///
/// The model's own `Timeout` range is wider (1..5400), so the boundary validator does not
/// enforce the published bound and the handler is the only place that can. The table
/// walks the published bounds and one value past each.
#[tokio::test]
async fn create_function_refuses_a_timeout_past_the_published_quota() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = crate::function_lifecycle::router_on(&engine);
    let code = package_bytes(32);

    let cases = [
        TimeoutCase {
            name: "the lowest timeout the model admits",
            timeout: 1,
            accepted: true,
        },
        TimeoutCase {
            name: "a mid-range timeout",
            timeout: 300,
            accepted: true,
        },
        TimeoutCase {
            name: "the published maximum",
            timeout: 900,
            accepted: true,
        },
        TimeoutCase {
            name: "one second past the published maximum",
            timeout: 901,
            accepted: false,
        },
        TimeoutCase {
            name: "the maximum the model's own range admits",
            timeout: 5400,
            accepted: false,
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let name = format!("timeout-{index}");
        let answer = create_function(
            &router,
            &json!({
                "FunctionName": name,
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Timeout": case.timeout,
                "Code": { "ZipFile": zip_file(&code) },
            }),
        )
        .await;

        if case.accepted {
            assert_eq!(
                answer.status,
                201,
                "case {}: Timeout {} is inside the published quota, expected 201, got {} {}",
                case.name,
                case.timeout,
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.json()["Timeout"],
                json!(case.timeout),
                "case {}: expected Timeout {} to be stored, got {:?}",
                case.name,
                case.timeout,
                answer.json()["Timeout"]
            );
        } else {
            assert_eq!(
                answer.status,
                400,
                "case {}: Timeout {} is past the published quota, expected 400, got {} {}",
                case.name,
                case.timeout,
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            // The clause states a bound on the VALUE of a member, so it is answered the
            // way the framework's own constraint walk answers one
            // (`function_configuration_quotas.rs` pins both quota channels).
            assert!(
                answer.error_code().contains("ValidationException"),
                "case {}: expected ValidationException, got {}",
                case.name,
                answer.error_code()
            );
            let message = answer
                .string("Message")
                .or_else(|| answer.string("message"))
                .unwrap_or_default();
            assert!(
                message.contains("timeout") && message.contains("900"),
                "case {}: the refusal must name the member and the bound it failed, got \
                 {message:?}",
                case.name
            );
            // The refused function was not created.
            let read = get_function_configuration(&router, &name, None).await;
            assert_eq!(
                read.status,
                404,
                "case {}: a refused CreateFunction must store nothing, got {} {}",
                case.name,
                read.status,
                String::from_utf8_lossy(&read.body)
            );
        }
    }
}

/// One row of the `GetFunctionConfiguration` table: how the function was created, and
/// what the version-specific read must therefore report.
struct ConfigurationCase {
    name: &'static str,
    request: serde_json::Value,
    runtime: Option<&'static str>,
    package_type: &'static str,
    description: Option<&'static str>,
    log_format: &'static str,
    log_group: String,
    snap_start_apply_on: &'static str,
    memory_size: i64,
    timeout: i64,
}

/// `GetFunctionConfiguration` answers every version-specific member out of the stored
/// record: the values the request named, the documented default for the ones it did not,
/// and the lifecycle state the clock has already made due.
#[tokio::test]
async fn get_function_configuration_reports_the_stored_configuration() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = crate::function_lifecycle::router_on(&engine);
    let code = package_bytes(128);
    // The container-image row resolves its reference against the registry at create.
    crate::function_lifecycle::seed_image(
        &engine,
        "000000000000.dkr.ecr.us-east-1.amazonaws.com/fn:latest",
    )
    .await;

    let cases = vec![
        // Nothing but the required members: every reported value must be a documented
        // default.
        ConfigurationCase {
            name: "defaults",
            request: json!({
                "FunctionName": "defaults",
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": { "ZipFile": zip_file(&code) },
            }),
            runtime: Some("python3.12"),
            package_type: "Zip",
            // EMPTY, not absent: `Description` is a member of every
            // `FunctionConfiguration` AWS reports, so a function created without one
            // reports it as the empty string.
            description: Some(""),
            log_format: "Text",
            log_group: "/aws/lambda/defaults".to_string(),
            snap_start_apply_on: "None",
            memory_size: 128,
            timeout: 3,
        },
        // Every optional member named, so none of the defaults can be what is reported.
        ConfigurationCase {
            name: "configured",
            request: json!({
                "FunctionName": "configured",
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "app.main",
                "Description": "a configured function",
                "MemorySize": 2048,
                "Timeout": 300,
                "LoggingConfig": {
                    "LogFormat": "JSON",
                    "ApplicationLogLevel": "DEBUG",
                    "SystemLogLevel": "WARN",
                    "LogGroup": "/custom/group",
                },
                "SnapStart": { "ApplyOn": "PublishedVersions" },
                "Code": { "ZipFile": zip_file(&code) },
            }),
            // SnapStart is applied here, so the runtime is one of the families that
            // supports it (`crate::state::runtime_supports_snap_start`).
            runtime: Some("python3.12"),
            package_type: "Zip",
            description: Some("a configured function"),
            log_format: "JSON",
            log_group: "/custom/group".to_string(),
            snap_start_apply_on: "PublishedVersions",
            memory_size: 2048,
            timeout: 300,
        },
        // A container image carries no managed runtime, so there is no runtime and no
        // runtime version to report for it.
        ConfigurationCase {
            name: "from-image",
            request: json!({
                "FunctionName": "from-image",
                "Role": ROLE,
                "PackageType": "Image",
                "Code": { "ImageUri": "000000000000.dkr.ecr.us-east-1.amazonaws.com/fn:latest" },
            }),
            runtime: None,
            package_type: "Image",
            description: Some(""),
            log_format: "Text",
            log_group: "/aws/lambda/from-image".to_string(),
            snap_start_apply_on: "None",
            memory_size: 128,
            timeout: 3,
        },
    ];

    for case in &cases {
        let created = create_function(&router, &case.request).await;
        assert_eq!(
            created.status,
            201,
            "case {}: expected 201 from CreateFunction, got {} {}",
            case.name,
            created.status,
            String::from_utf8_lossy(&created.body)
        );
    }
    make_activation_due(&engine);

    for case in &cases {
        let answer = get_function_configuration(&router, case.name, None).await;
        assert_eq!(
            answer.status,
            200,
            "case {}: expected 200, got {} {}",
            case.name,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let read = answer.json();

        assert_eq!(
            read["FunctionArn"],
            json!(function_arn(case.name)),
            "case {}: expected FunctionArn {}, got {:?}",
            case.name,
            function_arn(case.name),
            read["FunctionArn"]
        );
        assert_eq!(
            read["FunctionName"],
            json!(case.name),
            "case {}: expected FunctionName {}, got {:?}",
            case.name,
            case.name,
            read["FunctionName"]
        );
        assert_eq!(
            read["Version"],
            json!("$LATEST"),
            "case {}: expected Version $LATEST, got {:?}",
            case.name,
            read["Version"]
        );
        assert_eq!(
            read["Role"],
            json!(ROLE),
            "case {}: expected Role {ROLE}, got {:?}",
            case.name,
            read["Role"]
        );
        assert_eq!(
            read["Runtime"].as_str(),
            case.runtime,
            "case {}: expected Runtime {:?}, got {:?}",
            case.name,
            case.runtime,
            read["Runtime"]
        );
        assert_eq!(
            read["PackageType"],
            json!(case.package_type),
            "case {}: expected PackageType {}, got {:?}",
            case.name,
            case.package_type,
            read["PackageType"]
        );
        assert_eq!(
            read["Description"].as_str(),
            case.description,
            "case {}: expected Description {:?}, got {:?}",
            case.name,
            case.description,
            read["Description"]
        );
        assert_eq!(
            read["MemorySize"],
            json!(case.memory_size),
            "case {}: expected MemorySize {}, got {:?}",
            case.name,
            case.memory_size,
            read["MemorySize"]
        );
        assert_eq!(
            read["Timeout"],
            json!(case.timeout),
            "case {}: expected Timeout {}, got {:?}",
            case.name,
            case.timeout,
            read["Timeout"]
        );
        assert_eq!(
            read["LoggingConfig"]["LogFormat"],
            json!(case.log_format),
            "case {}: expected LoggingConfig.LogFormat {}, got {:?}",
            case.name,
            case.log_format,
            read["LoggingConfig"]
        );
        assert_eq!(
            read["LoggingConfig"]["LogGroup"],
            json!(case.log_group),
            "case {}: expected LoggingConfig.LogGroup {}, got {:?}",
            case.name,
            case.log_group,
            read["LoggingConfig"]
        );
        assert_eq!(
            read["SnapStart"]["ApplyOn"],
            json!(case.snap_start_apply_on),
            "case {}: expected SnapStart.ApplyOn {}, got {:?}",
            case.name,
            case.snap_start_apply_on,
            read["SnapStart"]
        );
        // The unpublished version is never snapshotted, whatever ApplyOn says.
        assert_eq!(
            read["SnapStart"]["OptimizationStatus"],
            json!("Off"),
            "case {}: expected SnapStart.OptimizationStatus Off on $LATEST, got {:?}",
            case.name,
            read["SnapStart"]
        );
        // A managed runtime pins a runtime version; a container image has none.
        match case.runtime {
            Some(_) => {
                let arn = read["RuntimeVersionConfig"]["RuntimeVersionArn"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    arn.starts_with(&format!("arn:aws:lambda:{REGION}::runtime:")),
                    "case {}: expected a runtime version ARN, got {:?}",
                    case.name,
                    read["RuntimeVersionConfig"]
                );
            }
            None => assert!(
                read["RuntimeVersionConfig"].is_null(),
                "case {}: a container image has no runtime version, got {:?}",
                case.name,
                read["RuntimeVersionConfig"]
            ),
        }
        assert_eq!(
            read["State"],
            json!("Active"),
            "case {}: expected State Active once activation is due, got {:?}",
            case.name,
            read["State"]
        );
        assert_eq!(
            read["LastUpdateStatus"],
            json!("Successful"),
            "case {}: expected LastUpdateStatus Successful, got {:?}",
            case.name,
            read["LastUpdateStatus"]
        );
        assert!(
            read["RevisionId"].as_str().is_some_and(|id| !id.is_empty()),
            "case {}: expected a RevisionId, got {:?}",
            case.name,
            read["RevisionId"]
        );
        assert!(
            read["LastModified"]
                .as_str()
                .is_some_and(|at| at.contains('T')),
            "case {}: expected an ISO-8601 LastModified, got {:?}",
            case.name,
            read["LastModified"]
        );
    }

    // Two reads of an unchanged function report the same configuration: the read owes
    // no change of its own.
    let first = get_function_configuration(&router, "defaults", None)
        .await
        .json();
    let second = get_function_configuration(&router, "defaults", None)
        .await
        .json();
    assert_eq!(
        first, second,
        "a second read of an unchanged function must report the same configuration, \
         got {first:?} then {second:?}"
    );

    // A name or qualifier that names no record is the modelled 404, for every form.
    for (name, reference, qualifier) in [
        ("absent function", "no-such-function", None),
        ("absent version", "defaults", Some("7")),
    ] {
        let answer = get_function_configuration(&router, reference, qualifier).await;
        assert_eq!(
            answer.status,
            404,
            "case {name}: expected 404, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            answer.error_code().contains("ResourceNotFoundException"),
            "case {name}: expected ResourceNotFoundException, got {}",
            answer.error_code()
        );
    }
}

/// `PublishVersion` snapshots the unpublished configuration, reports the numbered
/// version it created, and refuses the requests its preconditions cover.
#[tokio::test]
async fn publish_version_snapshots_the_unpublished_configuration() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = crate::function_lifecycle::router_on(&engine);

    // Two functions, so the version numbering is per function rather than global.
    for (name, size) in [("alpha", 64_usize), ("beta", 200_usize)] {
        create_active_function(&router, &engine, name, &package_bytes(size)).await;
    }

    for (name, description) in [("alpha", "first cut"), ("beta", "")] {
        let body = if description.is_empty() {
            json!({})
        } else {
            json!({ "Description": description })
        };
        let answer = publish_version(&router, name, &body).await;
        assert_eq!(
            answer.status,
            201,
            "case {name}: expected 201 from PublishVersion, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let published = answer.json();
        assert_eq!(
            published["Version"],
            json!("1"),
            "case {name}: the first version of a function is 1, got {:?}",
            published["Version"]
        );
        let expected_arn = format!("{}:1", function_arn(name));
        assert_eq!(
            published["FunctionArn"],
            json!(expected_arn),
            "case {name}: expected FunctionArn {expected_arn}, got {:?}",
            published["FunctionArn"]
        );
        if !description.is_empty() {
            assert_eq!(
                published["Description"],
                json!(description),
                "case {name}: the version's Description overrides the function's, got {:?}",
                published["Description"]
            );
        }

        // Round trip: the version is readable back through the version-specific read.
        let read = get_function_configuration(&router, name, Some("1")).await;
        assert_eq!(
            read.status,
            200,
            "case {name}: the published version must be readable, got {} {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            read.json()["FunctionArn"],
            json!(expected_arn),
            "case {name}: expected the version ARN on read-back, got {:?}",
            read.json()["FunctionArn"]
        );
    }

    // "Lambda doesn't publish a version if the function's configuration and code
    // haven't changed since the last version": the same version is reported again. The
    // row is `beta`, published WITHOUT a `Description` override, so a republish freezes
    // exactly the configuration the last version froze; `alpha`'s version carries an
    // override, which a republish without one would not reproduce.
    let again = publish_version(&router, "beta", &json!({})).await;
    assert_eq!(
        again.status,
        201,
        "case republish: expected 201, got {} {}",
        again.status,
        String::from_utf8_lossy(&again.body)
    );
    assert_eq!(
        again.json()["Version"],
        json!("1"),
        "case republish: an unchanged function publishes no second version, got {:?}",
        again.json()["Version"]
    );

    // The refusals, one row per precondition the model states.
    let revision = get_function_configuration(&router, "alpha", None)
        .await
        .string("RevisionId")
        .unwrap_or_default();
    let cases: Vec<(&str, &str, serde_json::Value, u16, &str)> = vec![
        (
            "stale revision",
            "alpha",
            json!({ "RevisionId": "00000000-0000-4000-8000-000000000000" }),
            412,
            "PreconditionFailedException",
        ),
        (
            "matching revision",
            "alpha",
            json!({ "RevisionId": revision }),
            201,
            "",
        ),
        (
            "code digest mismatch",
            "alpha",
            json!({ "CodeSha256": "not-the-stored-digest" }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "absent function",
            "no-such-function",
            json!({}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "unimplemented publish channel",
            "alpha",
            json!({ "PublishTo": "LATEST_PUBLISHED" }),
            501,
            "NotImplemented",
        ),
    ];
    for (case, reference, body, status, code) in cases {
        let answer = publish_version(&router, reference, &body).await;
        assert_eq!(
            answer.status,
            status,
            "case {case}: expected {status}, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        if !code.is_empty() {
            assert!(
                answer.error_code().contains(code),
                "case {case}: expected {code}, got {}",
                answer.error_code()
            );
        }
    }

    // The `Pending` -> `Active` window is NOT asserted here: `PublishVersion` as this
    // crate implements it (`src/ops/publish_version.rs`) snapshots the stored
    // configuration whatever lifecycle state the function is in, and whether it should
    // instead answer the modelled `ResourceConflictException` while the function
    // provisions ("During this time, you can't invoke or modify the function") is that
    // operation's own fidelity question rather than this suite's.
}

/// One row of the permission table: the principal form asked for, and the IAM
/// `Principal` element the stored statement must therefore carry.
struct PermissionCase {
    name: &'static str,
    statement_id: &'static str,
    principal: &'static str,
    extra: serde_json::Value,
    expected_principal: serde_json::Value,
    expected_condition: Option<serde_json::Value>,
}

/// `AddPermission` stores one statement per `StatementId`, `GetPolicy` reports the
/// document they make up, and `RemovePermission` takes one back out — over every
/// documented principal form.
#[tokio::test]
async fn function_policy_round_trips_every_principal_form() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = crate::function_lifecycle::router_on(&engine);
    create_active_function(&router, &engine, "guarded", &package_bytes(96)).await;

    let cases = vec![
        // A service principal is a DNS name.
        PermissionCase {
            name: "service principal",
            statement_id: "s3-invoke",
            principal: "s3.amazonaws.com",
            extra: json!({}),
            expected_principal: json!({ "Service": "s3.amazonaws.com" }),
            expected_condition: None,
        },
        // An account is a bare 12-digit id, which IAM writes as that account's root ARN.
        PermissionCase {
            name: "account principal",
            statement_id: "account-invoke",
            principal: ACCOUNT,
            extra: json!({}),
            expected_principal: json!({ "AWS": format!("arn:aws:iam::{ACCOUNT}:root") }),
            expected_condition: None,
        },
        // A role or user is already an ARN.
        PermissionCase {
            name: "role principal",
            statement_id: "role-invoke",
            principal: "arn:aws:iam::000000000000:role/caller",
            extra: json!({}),
            expected_principal: json!({ "AWS": "arn:aws:iam::000000000000:role/caller" }),
            expected_condition: None,
        },
        // The wildcard principal is a bare string.
        PermissionCase {
            name: "wildcard principal",
            statement_id: "anyone-invoke",
            principal: "*",
            extra: json!({}),
            expected_principal: json!("*"),
            expected_condition: None,
        },
        // A service principal is told by its SHAPE (a dotted DNS name, no `:`), not by
        // the `amazonaws.com` suffix, so another partition's suffix is one too.
        PermissionCase {
            name: "service principal in another partition",
            statement_id: "cn-service-invoke",
            principal: "s3.amazonaws.com.cn",
            extra: json!({}),
            expected_principal: json!({ "Service": "s3.amazonaws.com.cn" }),
            expected_condition: None,
        },
        // An assumed-role session is an ARN, so it goes under `AWS` as given.
        PermissionCase {
            name: "assumed role principal",
            statement_id: "session-invoke",
            principal: "arn:aws:sts::000000000000:assumed-role/caller/session",
            extra: json!({}),
            expected_principal: json!({
                "AWS": "arn:aws:sts::000000000000:assumed-role/caller/session"
            }),
            expected_condition: None,
        },
        // The scoping members become conditions on the global condition keys they name.
        PermissionCase {
            name: "scoped by source",
            statement_id: "bucket-invoke",
            principal: "s3.amazonaws.com",
            extra: json!({
                "SourceArn": "arn:aws:s3:::uploads",
                "SourceAccount": ACCOUNT,
            }),
            expected_principal: json!({ "Service": "s3.amazonaws.com" }),
            expected_condition: Some(json!({
                "ArnLike": { "AWS:SourceArn": "arn:aws:s3:::uploads" },
                "StringEquals": { "AWS:SourceAccount": ACCOUNT },
            })),
        },
    ];

    for case in &cases {
        let mut body = json!({
            "StatementId": case.statement_id,
            "Action": "lambda:InvokeFunction",
            "Principal": case.principal,
        });
        if let (Some(target), Some(extra)) = (body.as_object_mut(), case.extra.as_object()) {
            for (key, value) in extra {
                target.insert(key.clone(), value.clone());
            }
        }

        let answer = add_permission(&router, "guarded", None, &body).await;
        assert_eq!(
            answer.status,
            201,
            "case {}: expected 201 from AddPermission, got {} {}",
            case.name,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let statement: serde_json::Value = serde_json::from_str(
            &answer
                .string("Statement")
                .unwrap_or_else(|| panic!("case {}: expected a Statement member", case.name)),
        )
        .unwrap_or_else(|error| panic!("case {}: Statement must be JSON: {error}", case.name));

        assert_eq!(
            statement["Sid"],
            json!(case.statement_id),
            "case {}: expected Sid {}, got {:?}",
            case.name,
            case.statement_id,
            statement["Sid"]
        );
        assert_eq!(
            statement["Effect"],
            json!("Allow"),
            "case {}: expected Effect Allow, got {:?}",
            case.name,
            statement["Effect"]
        );
        assert_eq!(
            statement["Principal"], case.expected_principal,
            "case {}: expected Principal {:?}, got {:?}",
            case.name, case.expected_principal, statement["Principal"]
        );
        assert_eq!(
            statement["Resource"],
            json!(function_arn("guarded")),
            "case {}: expected the function ARN as Resource, got {:?}",
            case.name,
            statement["Resource"]
        );
        match &case.expected_condition {
            Some(expected) => assert_eq!(
                &statement["Condition"], expected,
                "case {}: expected Condition {expected:?}, got {:?}",
                case.name, statement["Condition"]
            ),
            None => assert!(
                statement["Condition"].is_null(),
                "case {}: a request that scopes nothing carries no Condition, got {:?}",
                case.name,
                statement["Condition"]
            ),
        }
    }

    // `GetPolicy` reports one document holding every statement added, at a revision.
    let policy_answer = get_policy(&router, "guarded", None).await;
    assert_eq!(
        policy_answer.status,
        200,
        "case read policy: expected 200, got {} {}",
        policy_answer.status,
        String::from_utf8_lossy(&policy_answer.body)
    );
    let revision = policy_answer
        .string("RevisionId")
        .unwrap_or_else(|| panic!("case read policy: expected a RevisionId"));
    let document: serde_json::Value = serde_json::from_str(
        &policy_answer
            .string("Policy")
            .unwrap_or_else(|| panic!("case read policy: expected a Policy member")),
    )
    .unwrap_or_else(|error| panic!("case read policy: Policy must be JSON: {error}"));
    assert_eq!(
        document["Version"],
        json!("2012-10-17"),
        "case read policy: expected the IAM policy version, got {:?}",
        document["Version"]
    );
    let statements = document["Statement"]
        .as_array()
        .unwrap_or_else(|| panic!("case read policy: Statement must be an array"))
        .clone();
    assert_eq!(
        statements.len(),
        cases.len(),
        "case read policy: expected {} statements, got {}",
        cases.len(),
        statements.len()
    );
    for case in &cases {
        assert!(
            statements
                .iter()
                .any(|statement| statement["Sid"] == json!(case.statement_id)),
            "case {}: expected its statement in the document, got {statements:?}",
            case.name
        );
    }

    // One `StatementId` names at most one statement of a policy.
    let duplicate = add_permission(
        &router,
        "guarded",
        None,
        &json!({
            "StatementId": "s3-invoke",
            "Action": "lambda:InvokeFunction",
            "Principal": "s3.amazonaws.com",
        }),
    )
    .await;
    assert_eq!(
        duplicate.status,
        409,
        "case duplicate statement id: expected 409, got {} {}",
        duplicate.status,
        String::from_utf8_lossy(&duplicate.body)
    );

    // A stale revision fails the precondition; the current one passes it.
    let stale = remove_permission(
        &router,
        "guarded",
        "s3-invoke",
        Some("00000000-0000-4000-8000-000000000000"),
    )
    .await;
    assert_eq!(
        stale.status,
        412,
        "case stale revision: expected 412, got {} {}",
        stale.status,
        String::from_utf8_lossy(&stale.body)
    );
    let removed = remove_permission(&router, "guarded", "s3-invoke", Some(&revision)).await;
    assert_eq!(
        removed.status,
        204,
        "case remove with the current revision: expected 204, got {} {}",
        removed.status,
        String::from_utf8_lossy(&removed.body)
    );

    // Removing a statement that is not there is the modelled 404.
    let absent = remove_permission(&router, "guarded", "s3-invoke", None).await;
    assert_eq!(
        absent.status,
        404,
        "case remove twice: expected 404, got {} {}",
        absent.status,
        String::from_utf8_lossy(&absent.body)
    );

    // Taking the last statement out takes the policy with it.
    for case in &cases {
        if case.statement_id == "s3-invoke" {
            continue;
        }
        let answer = remove_permission(&router, "guarded", case.statement_id, None).await;
        assert_eq!(
            answer.status,
            204,
            "case remove {}: expected 204, got {} {}",
            case.name,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }
    let gone = get_policy(&router, "guarded", None).await;
    assert_eq!(
        gone.status,
        404,
        "case empty policy: a resource with no statements holds no policy, got {} {}",
        gone.status,
        String::from_utf8_lossy(&gone.body)
    );
}

/// A policy is attached per qualified function, so the unpublished function and a
/// published version of it never see each other's statements — and the document is
/// refused once it passes the published quota.
#[tokio::test]
async fn function_policy_is_per_qualifier_and_bounded() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = crate::function_lifecycle::router_on(&engine);
    create_active_function(&router, &engine, "scoped", &package_bytes(48)).await;
    let published = publish_version(&router, "scoped", &json!({})).await;
    assert_eq!(
        published.status,
        201,
        "setup: expected 201 from PublishVersion, got {} {}",
        published.status,
        String::from_utf8_lossy(&published.body)
    );

    for (case, qualifier, statement_id) in [
        ("unpublished", None, "latest-only"),
        ("version 1", Some("1"), "version-only"),
    ] {
        let answer = add_permission(
            &router,
            "scoped",
            qualifier,
            &json!({
                "StatementId": statement_id,
                "Action": "lambda:InvokeFunction",
                "Principal": "events.amazonaws.com",
            }),
        )
        .await;
        assert_eq!(
            answer.status,
            201,
            "case {case}: expected 201, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }

    for (case, qualifier, expected_sid, unexpected_sid) in [
        ("unpublished", None, "latest-only", "version-only"),
        ("version 1", Some("1"), "version-only", "latest-only"),
    ] {
        let answer = get_policy(&router, "scoped", qualifier).await;
        let document = answer.string("Policy").unwrap_or_default();
        assert!(
            document.contains(expected_sid),
            "case {case}: expected {expected_sid} in its own policy, got {document}"
        );
        assert!(
            !document.contains(unexpected_sid),
            "case {case}: another qualifier's statement leaked in, got {document}"
        );
    }

    // AWS's published quota bounds the whole document, not one statement: each of these
    // statements carries a ~4 KB SourceArn — which keeps every request itself inside the
    // operation's own 5 KB request quota — so the document crosses the 20 KB limit after
    // four of them.
    let long_source_arn = format!("arn:aws:s3:::{}", "a".repeat(4_000));
    let mut refused = 0_usize;
    for index in 0..6 {
        let answer = add_permission(
            &router,
            "scoped",
            None,
            &json!({
                "StatementId": format!("bulk-{index}"),
                "Action": "lambda:InvokeFunction",
                "Principal": "s3.amazonaws.com",
                "SourceArn": long_source_arn,
            }),
        )
        .await;
        if answer.status == 400 {
            refused += 1;
            assert!(
                answer.error_code().contains("PolicyLengthExceededException"),
                "case bulk-{index}: expected PolicyLengthExceededException, got {}",
                answer.error_code()
            );
        } else {
            assert_eq!(
                answer.status,
                201,
                "case bulk-{index}: expected 201 or 400, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
        }
    }
    assert!(
        refused > 0,
        "case policy quota: a document past the published 20 KB limit must be refused, \
         got {refused} refusals"
    );
}

/// One row of the code-signing table.
struct CodeSigningCase {
    name: &'static str,
    request: serde_json::Value,
    policy: &'static str,
    description: Option<&'static str>,
    publishers: usize,
}

/// `CreateCodeSigningConfig` mints a configuration and `GetCodeSigningConfig` reads it
/// back unchanged, over every value of the policy the model's enum carries.
#[tokio::test]
async fn code_signing_configurations_round_trip() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = crate::function_lifecycle::router_on(&engine);

    let profile_one = "arn:aws:signer:us-east-1:000000000000:/signing-profiles/one/1";
    let profile_two = "arn:aws:signer:us-east-1:000000000000:/signing-profiles/two/2";

    let cases = vec![
        // No policy named: the documented default, and no `Description` either: the
        // create ECHOES the configuration it was asked for, so the member is absent from
        // its answer (a later READ of the same record renders it as the empty string,
        // which the read-back below normalises for).
        CodeSigningCase {
            name: "default policy",
            request: json!({
                "AllowedPublishers": { "SigningProfileVersionArns": [profile_one] },
            }),
            policy: "Warn",
            description: None,
            publishers: 1,
        },
        // Both values the model's `CodeSigningPolicy` enum carries.
        CodeSigningCase {
            name: "warn",
            request: json!({
                "Description": "warn only",
                "AllowedPublishers": { "SigningProfileVersionArns": [profile_one] },
                "CodeSigningPolicies": { "UntrustedArtifactOnDeployment": "Warn" },
            }),
            policy: "Warn",
            description: Some("warn only"),
            publishers: 1,
        },
        CodeSigningCase {
            name: "enforce with two publishers",
            request: json!({
                "Description": "enforced",
                "AllowedPublishers": {
                    "SigningProfileVersionArns": [profile_one, profile_two],
                },
                "CodeSigningPolicies": { "UntrustedArtifactOnDeployment": "Enforce" },
            }),
            policy: "Enforce",
            description: Some("enforced"),
            publishers: 2,
        },
    ];

    let mut minted = Vec::new();
    for case in &cases {
        let answer = json_call(
            &router,
            "POST",
            "/2020-04-22/code-signing-configs",
            &case.request,
        )
        .await;
        assert_eq!(
            answer.status,
            201,
            "case {}: expected 201, got {} {}",
            case.name,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let config = answer.json()["CodeSigningConfig"].clone();

        let id = config["CodeSigningConfigId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        // The model's `CodeSigningConfigArn` pattern narrows the id to
        // `csc-[a-z0-9]{17}`.
        assert!(
            id.len() == 21
                && id.starts_with("csc-")
                && id[4..]
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit()),
            "case {}: expected a csc-[a-z0-9]{{17}} id, got {id:?}",
            case.name
        );
        let arn = config["CodeSigningConfigArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let expected_arn =
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:code-signing-config:{id}");
        assert_eq!(
            arn, expected_arn,
            "case {}: expected ARN {expected_arn}, got {arn}",
            case.name
        );
        assert_eq!(
            config["CodeSigningPolicies"]["UntrustedArtifactOnDeployment"],
            json!(case.policy),
            "case {}: expected policy {}, got {:?}",
            case.name,
            case.policy,
            config["CodeSigningPolicies"]
        );
        assert_eq!(
            config["Description"].as_str(),
            case.description,
            "case {}: expected Description {:?}, got {:?}",
            case.name,
            case.description,
            config["Description"]
        );
        assert_eq!(
            config["AllowedPublishers"]["SigningProfileVersionArns"]
                .as_array()
                .map(Vec::len),
            Some(case.publishers),
            "case {}: expected {} publishers, got {:?}",
            case.name,
            case.publishers,
            config["AllowedPublishers"]
        );
        minted.push((case.name, arn, config));
    }

    // Every minted configuration reads back exactly as it was stored.
    for (name, arn, created) in &minted {
        let uri = format!("/2020-04-22/code-signing-configs/{arn}");
        let answer = json_call(&router, "GET", &uri, &serde_json::Value::Null).await;
        assert_eq!(
            answer.status,
            200,
            "case {name}: expected 200 on read-back, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        // The read reports the STORED configuration, which renders `Description` on every
        // read — as the empty string for a row that described none — while the create
        // echoes the members its own request carried. That one member is normalised here,
        // and everything else must be identical.
        let mut expected = created.clone();
        expected["Description"] = json!(created["Description"].as_str().unwrap_or_default());
        assert_eq!(
            &answer.json()["CodeSigningConfig"],
            &expected,
            "case {name}: the read-back configuration must match the created one, got {:?}",
            answer.json()["CodeSigningConfig"]
        );
    }

    // Two configurations never share an id.
    let ids: std::collections::BTreeSet<String> = minted
        .iter()
        .map(|(_, arn, _)| arn.clone())
        .collect();
    assert_eq!(
        ids.len(),
        minted.len(),
        "expected {} distinct configuration ARNs, got {ids:?}",
        minted.len()
    );

    let unknown = format!(
        "arn:aws:lambda:{REGION}:{ACCOUNT}:code-signing-config:csc-00000000000000000"
    );
    let answer = json_call(
        &router,
        "GET",
        &format!("/2020-04-22/code-signing-configs/{unknown}"),
        &serde_json::Value::Null,
    )
    .await;
    assert_eq!(
        answer.status,
        404,
        "case unknown ARN: expected 404, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}

/// `CreateCapacityProvider` persists a provider in the model's `Pending` state, the
/// clock owes its move to `Active`, and `GetCapacityProvider` reports it under either
/// form of its name.
#[tokio::test]
async fn capacity_providers_round_trip_and_become_active_when_due() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = crate::function_lifecycle::router_on(&engine);

    let cases = vec![
        (
            "minimal",
            json!({
                "CapacityProviderName": "minimal",
                "VpcConfig": {
                    "SubnetIds": ["subnet-0123456789abcdef0"],
                    "SecurityGroupIds": [],
                },
                "PermissionsConfig": { "CapacityProviderOperatorRoleArn": ROLE },
            }),
        ),
        (
            "fully configured",
            json!({
                "CapacityProviderName": "fully-configured",
                "VpcConfig": {
                    "SubnetIds": ["subnet-0123456789abcdef0", "subnet-00000000000000001"],
                    "SecurityGroupIds": ["sg-0123456789abcdef0"],
                },
                "PermissionsConfig": { "CapacityProviderOperatorRoleArn": ROLE },
                "InstanceRequirements": { "Architectures": ["arm64"] },
                "CapacityProviderScalingConfig": {
                    "ScalingMode": "Auto",
                    "MaxVCpuCount": 64,
                },
                "TelemetryConfig": {
                    "LoggingConfig": { "SystemLogLevel": "INFO", "LogGroup": "/cp/logs" },
                },
            }),
        ),
    ];

    let mut created = Vec::new();
    for (case, request) in &cases {
        let answer = json_call(&router, "POST", "/2025-11-30/capacity-providers", request).await;
        assert_eq!(
            answer.status,
            202,
            "case {case}: expected 202, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let provider = answer.json()["CapacityProvider"].clone();
        let name = request["CapacityProviderName"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let expected_arn =
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:capacity-provider:{name}");
        assert_eq!(
            provider["CapacityProviderArn"],
            json!(expected_arn),
            "case {case}: expected ARN {expected_arn}, got {:?}",
            provider["CapacityProviderArn"]
        );
        // The activation span is never zero, so a caller acting straight after the
        // create still sees the transient state.
        assert_eq!(
            provider["State"],
            json!("Pending"),
            "case {case}: a freshly created provider is Pending, got {:?}",
            provider["State"]
        );
        assert_eq!(
            provider["VpcConfig"], request["VpcConfig"],
            "case {case}: expected the VpcConfig that was asked for, got {:?}",
            provider["VpcConfig"]
        );
        assert_eq!(
            provider["PermissionsConfig"], request["PermissionsConfig"],
            "case {case}: expected the PermissionsConfig that was asked for, got {:?}",
            provider["PermissionsConfig"]
        );
        for member in ["InstanceRequirements", "CapacityProviderScalingConfig"] {
            if !request[member].is_null() {
                assert_eq!(
                    provider[member], request[member],
                    "case {case}: expected {member} {:?}, got {:?}",
                    request[member], provider[member]
                );
            }
        }
        created.push((*case, name, expected_arn));
    }

    // A duplicate create names a provider that already exists.
    let duplicate =
        json_call(&router, "POST", "/2025-11-30/capacity-providers", &cases[0].1).await;
    assert_eq!(
        duplicate.status,
        409,
        "case duplicate create: expected 409, got {} {}",
        duplicate.status,
        String::from_utf8_lossy(&duplicate.body)
    );

    // Still Pending before the clock owes the transition.
    for (case, name, _) in &created {
        let answer = json_call(
            &router,
            "GET",
            &format!("/2025-11-30/capacity-providers/{name}"),
            &serde_json::Value::Null,
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "case {case}: expected 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.json()["CapacityProvider"]["State"],
            json!("Pending"),
            "case {case}: still Pending before activation is due, got {:?}",
            answer.json()["CapacityProvider"]["State"]
        );
    }

    make_activation_due(&engine);

    // Once due, every read reports Active — by bare name and by ARN alike.
    for (case, name, arn) in &created {
        for (form, reference) in [("name", name.as_str()), ("arn", arn.as_str())] {
            let answer = json_call(
                &router,
                "GET",
                &format!("/2025-11-30/capacity-providers/{reference}"),
                &serde_json::Value::Null,
            )
            .await;
            assert_eq!(
                answer.status,
                200,
                "case {case} by {form}: expected 200, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.json()["CapacityProvider"]["State"],
                json!("Active"),
                "case {case} by {form}: expected Active once due, got {:?}",
                answer.json()["CapacityProvider"]["State"]
            );
            assert_eq!(
                answer.json()["CapacityProvider"]["CapacityProviderArn"],
                json!(arn),
                "case {case} by {form}: expected ARN {arn}, got {:?}",
                answer.json()["CapacityProvider"]["CapacityProviderArn"]
            );
        }
    }

    let unknown = json_call(
        &router,
        "GET",
        "/2025-11-30/capacity-providers/no-such-provider",
        &serde_json::Value::Null,
    )
    .await;
    assert_eq!(
        unknown.status,
        404,
        "case unknown provider: expected 404, got {} {}",
        unknown.status,
        String::from_utf8_lossy(&unknown.body)
    );
}

/// One row of the revocation table: the statements to grant, which one `RemovePermission`
/// names, and which policy they live on.
struct RevocationCase {
    name: &'static str,
    function: &'static str,
    /// The published version the policy is attached to, or `None` for the function level.
    qualifier: Option<&'static str>,
    /// Whether that version is named by appending it to the `FunctionName` rather than by
    /// the `Qualifier` parameter — "You can append a version number or alias to any of the
    /// formats", so both name one resource.
    qualifier_in_name: bool,
    /// The `StatementId`s granted, in order, before anything is revoked.
    granted: &'static [&'static str],
    /// The one the revocation names.
    revoke: &'static str,
}

/// `RemovePermission` takes out exactly the statement its `StatementId` names, leaves the
/// rest of that policy in the order they were granted at a new revision, and touches no
/// other qualifier's policy.
///
/// The rows differ along what the rule generalises over: how many statements the policy
/// holds, whether the revoked one is the first, the middle, the last or the only one,
/// whether the policy is the function's or a published version's, and which documented
/// form names that version.
#[tokio::test]
async fn remove_permission_takes_out_one_statement_of_the_policy_it_names() {
    let cases = vec![
        RevocationCase {
            name: "the first of three function-level statements",
            function: "revoke-first",
            qualifier: None,
            qualifier_in_name: false,
            granted: &["s3-invoke", "sns-invoke", "events-invoke"],
            revoke: "s3-invoke",
        },
        RevocationCase {
            name: "the middle of three function-level statements",
            function: "revoke-middle",
            qualifier: None,
            qualifier_in_name: false,
            granted: &["s3-invoke", "sns-invoke", "events-invoke"],
            revoke: "sns-invoke",
        },
        RevocationCase {
            name: "the last of three function-level statements",
            function: "revoke-last",
            qualifier: None,
            qualifier_in_name: false,
            granted: &["s3-invoke", "sns-invoke", "events-invoke"],
            revoke: "events-invoke",
        },
        RevocationCase {
            name: "the only statement of a function-level policy",
            function: "revoke-only",
            qualifier: None,
            qualifier_in_name: false,
            granted: &["only-invoke"],
            revoke: "only-invoke",
        },
        RevocationCase {
            name: "one of a published version's two statements",
            function: "revoke-version",
            qualifier: Some("1"),
            qualifier_in_name: false,
            granted: &["v1-first", "v1-second"],
            revoke: "v1-first",
        },
        RevocationCase {
            name: "a published version's only statement, named by appending the version",
            function: "revoke-label",
            qualifier: Some("1"),
            qualifier_in_name: true,
            granted: &["v1-only"],
            revoke: "v1-only",
        },
    ];

    for case in &cases {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = crate::function_lifecycle::router_on(&engine);
        create_active_function(&router, &engine, case.function, &package_bytes(64)).await;

        // A version to attach a policy to, for the rows that name one.
        if case.qualifier.is_some() {
            let published = publish_version(&router, case.function, &json!({})).await;
            assert_eq!(
                published.status,
                201,
                "case {}: expected 201 from PublishVersion, got {} {}",
                case.name,
                published.status,
                String::from_utf8_lossy(&published.body)
            );
        }

        // How this row names the resource: the plain name plus a `Qualifier` parameter, or
        // the version appended to the `FunctionName`.
        let (reference, query_qualifier) = match (case.qualifier, case.qualifier_in_name) {
            (Some(qualifier), true) => (format!("{}:{qualifier}", case.function), None),
            (Some(qualifier), false) => (case.function.to_string(), Some(qualifier)),
            (None, _) => (case.function.to_string(), None),
        };
        let resource = case.qualifier.map_or_else(
            || function_arn(case.function),
            |qualifier| format!("{}:{qualifier}", function_arn(case.function)),
        );

        // A statement on the function level, for the rows that edit a version's policy:
        // one policy's revocation must not reach another's.
        let sibling = "sibling-invoke";
        if case.qualifier.is_some() {
            let kept = add_permission(
                &router,
                case.function,
                None,
                &json!({
                    "StatementId": sibling,
                    "Action": "lambda:InvokeFunction",
                    "Principal": "events.amazonaws.com",
                }),
            )
            .await;
            assert_eq!(
                kept.status,
                201,
                "case {}: granting the function-level statement must succeed, got {} {}",
                case.name,
                kept.status,
                String::from_utf8_lossy(&kept.body)
            );
        }

        for statement_id in case.granted {
            let granted = add_permission(
                &router,
                &reference,
                query_qualifier,
                &json!({
                    "StatementId": statement_id,
                    "Action": "lambda:InvokeFunction",
                    "Principal": "s3.amazonaws.com",
                }),
            )
            .await;
            assert_eq!(
                granted.status,
                201,
                "case {}: granting {statement_id:?} must succeed, got {} {}",
                case.name,
                granted.status,
                String::from_utf8_lossy(&granted.body)
            );
        }

        let before = get_policy(&router, &reference, query_qualifier).await;
        let before_revision = before.string("RevisionId").unwrap_or_else(|| {
            panic!(
                "case {}: expected a RevisionId, got {}",
                case.name,
                String::from_utf8_lossy(&before.body)
            )
        });

        let revoked =
            remove_permission_on(&router, &reference, case.revoke, query_qualifier, None).await;
        assert_eq!(
            revoked.status,
            204,
            "case {}: expected 204 from RemovePermission, got {} {}",
            case.name,
            revoked.status,
            String::from_utf8_lossy(&revoked.body)
        );
        assert!(
            revoked.body.is_empty(),
            "case {}: expected an empty body with the 204, got {}",
            case.name,
            String::from_utf8_lossy(&revoked.body)
        );

        let remaining: Vec<&str> = case
            .granted
            .iter()
            .copied()
            .filter(|statement_id| *statement_id != case.revoke)
            .collect();

        let after = get_policy(&router, &reference, query_qualifier).await;
        if remaining.is_empty() {
            // The last statement is gone, so the resource holds no policy to report.
            assert_eq!(
                after.status,
                404,
                "case {}: a policy with no statements left is no policy, got {} {}",
                case.name,
                after.status,
                String::from_utf8_lossy(&after.body)
            );
        } else {
            assert_eq!(
                after.status,
                200,
                "case {}: expected 200 from GetPolicy, got {} {}",
                case.name,
                after.status,
                String::from_utf8_lossy(&after.body)
            );
            let document: serde_json::Value = serde_json::from_str(
                &after
                    .string("Policy")
                    .unwrap_or_else(|| panic!("case {}: expected a Policy member", case.name)),
            )
            .unwrap_or_else(|error| panic!("case {}: Policy must be JSON: {error}", case.name));
            let statements = document["Statement"]
                .as_array()
                .unwrap_or_else(|| panic!("case {}: Statement must be an array", case.name))
                .clone();
            let sids: Vec<String> = statements
                .iter()
                .map(|statement| statement["Sid"].as_str().unwrap_or_default().to_string())
                .collect();
            assert_eq!(
                sids,
                remaining
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<String>>(),
                "case {}: expected the statements granted but not revoked, in order, got {sids:?}",
                case.name
            );
            for statement in &statements {
                assert_eq!(
                    statement["Resource"],
                    json!(resource),
                    "case {}: expected the ARN the grant was made on as Resource, got {:?}",
                    case.name,
                    statement["Resource"]
                );
            }
            let after_revision = after.string("RevisionId").unwrap_or_default();
            assert_ne!(
                after_revision, before_revision,
                "case {}: a changed policy is at a new revision, got the earlier \
                 {after_revision:?}",
                case.name
            );
        }

        // Revoking the same statement twice finds nothing to revoke the second time.
        let again =
            remove_permission_on(&router, &reference, case.revoke, query_qualifier, None).await;
        assert_eq!(
            again.status,
            404,
            "case {}: expected 404 revoking the same statement twice, got {} {}",
            case.name,
            again.status,
            String::from_utf8_lossy(&again.body)
        );

        // The other qualifier's policy is untouched by all of it.
        if case.qualifier.is_some() {
            let function_level = get_policy(&router, case.function, None).await;
            assert_eq!(
                function_level.status,
                200,
                "case {}: the function-level policy must survive, got {} {}",
                case.name,
                function_level.status,
                String::from_utf8_lossy(&function_level.body)
            );
            let document: serde_json::Value = serde_json::from_str(
                &function_level
                    .string("Policy")
                    .unwrap_or_else(|| panic!("case {}: expected a Policy member", case.name)),
            )
            .unwrap_or_else(|error| panic!("case {}: Policy must be JSON: {error}", case.name));
            assert_eq!(
                document["Statement"],
                json!([{
                    "Sid": sibling,
                    "Effect": "Allow",
                    "Principal": { "Service": "events.amazonaws.com" },
                    "Action": "lambda:InvokeFunction",
                    "Resource": function_arn(case.function),
                }]),
                "case {}: expected the function-level statement alone, got {:?}",
                case.name,
                document["Statement"]
            );
        }
    }
}
