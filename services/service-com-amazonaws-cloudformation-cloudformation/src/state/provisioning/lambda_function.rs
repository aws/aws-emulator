// SPDX-License-Identifier: CC0-1.0

//! `AWS::Lambda::Function`: the function a stack declares, created in AWS Lambda itself.
//!
//! This is the per-type half of provisioning for one resource type, the half
//! [`super::super::provisioner`] holds for every other type this emulator creates. It lives
//! in its own module beside the algorithm that drives it because that file is far past the
//! length a change to this repository may touch, so a type added now brings its own module
//! rather than growing one that can no longer be edited. The seam that reaches it is
//! [`super::Owner`], which is where the algorithm asks who owns a type.
//!
//! The type's properties are `CreateFunction`'s own members, named identically on both
//! sides, with three exceptions, each the call that owns it:
//!
//! * `KmsKeyArn` is spelled `KMSKeyArn` by the API;
//! * `Code.ZipFile` is the function's SOURCE TEXT in a template ("you can specify the
//!   function code inline"), while the API's `ZipFile` is the bytes of a .zip deployment
//!   package — so the declared text is archived under the file name the runtime's handler
//!   resolves (`index` plus that language's extension) and the archive is what the create
//!   carries;
//! * `ReservedConcurrentExecutions`, `RecursiveLoop` and `RuntimeManagementConfig` are no
//!   members of the create at all: each is the Lambda call a client makes next on the
//!   function just created (`PutFunctionConcurrency`, `PutFunctionRecursionConfig`,
//!   `PutRuntimeManagementConfig`), so a function declared with one is configured with it
//!   rather than created without it.
//!
//! "Ref returns the function name", and that name is CloudFormation's to decide
//! ([`super::super::template`]), so the create carries the resource's physical id as
//! `FunctionName` and the function a client then calls `GetFunction` with is the one the
//! stack publishes.
//!
//! `CreateFunction` answers a function whose `State` is `Pending`, and in that window AWS
//! Lambda refuses every call made on it — "During this time, you can't invoke or modify the
//! function" — so the create does not answer until that service's own read reports the
//! function out of it ([`await_settled`]). A client that reads the stack as complete, and
//! another service that delivers to the function the moment it does, find a function AWS
//! Lambda takes calls on rather than one it refuses every call on.

use std::collections::BTreeMap;
use std::io::Write;
use std::time::Duration;

use serde_json::{Map, Value};

use super::{call_rest_json, percent_encoded, Created, Readiness, Refusal, Removal};
use crate::state::provisioner::tags_of;
use crate::state::StoredResource;

/// The resource type this module provisions.
pub(super) const LAMBDA_FUNCTION: &str = "AWS::Lambda::Function";

/// The SigV4 signing name of the service that owns it, which is what routes the in-process
/// call to AWS Lambda's own handlers.
const LAMBDA_SIGNING_NAME: &str = "lambda";

/// `CreateFunction`'s own URI: `POST /2015-03-31/functions`.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The error code Lambda answers for a function it does not hold
/// (`ResourceNotFoundException`, httpError 404), which is what says a delete has nothing
/// left to remove and a read has no function to report.
const FUNCTION_NOT_FOUND: &str = "ResourceNotFoundException";

/// The error code Lambda refuses a `CreateFunction` it cannot satisfy under
/// (`InvalidParameterValueException`, httpError 400, "One of the parameters in the request
/// is not valid"), which is the code a declaration naming an execution role that cannot be
/// handed to Lambda fails with ([`refuse_unpassable_role`]).
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValueException";

/// The property that names the role the function RUNS AS.
const ROLE_PROPERTY: &str = "Role";

/// The `State` member Lambda reports a function's lifecycle in: `Pending` while it is
/// provisioning the function, `Active` once it can be invoked. Read by [`await_settled`],
/// which is what holds a create and a change open while it is `Pending`, and by [`removal`],
/// which reports it as the state the function is still being removed in.
const STATE_MEMBER: &str = "State";

/// The member Lambda reports the progress of the LAST modification of a function in, beside
/// [`STATE_MEMBER`]: `InProgress` while it is still being applied, `Successful` once it is.
const LAST_UPDATE_STATUS_MEMBER: &str = "LastUpdateStatus";

/// The two values those members take while Lambda refuses a further modification of the
/// function: "When the code is Creating, you can't invoke or modify the function" for a
/// function still being provisioned, and "During this time, you can't modify the function"
/// for one whose last update is still being applied.
const STATE_PENDING: &str = "Pending";
const LAST_UPDATE_STATUS_IN_PROGRESS: &str = "InProgress";

/// How long [`await_settled`] waits between asking Lambda whether the function is out of
/// those windows, and how many times it asks.
///
/// The interval is short enough that a settled function is noticed about as soon as it
/// settles, and the budget — 50 ms apart, 180 times, so nine seconds — is a few times over
/// the window AWS Lambda itself holds a modification open for
/// (`service_com_amazonaws_lambda_awsgirapiservice::state::UPDATE_DELAY`, the span the
/// other waits in this provisioning layer are sized against), long enough to cover it on a
/// loaded machine. A wait that runs out is not a failure of its own: what fails then is the
/// call that needed the function modifiable, reported as Lambda's own refusal of it, so a
/// function genuinely stuck fails the resource rather than holding the deployment open.
const SETTLE_CHECK_INTERVAL: Duration = Duration::from_millis(50);
const SETTLE_CHECKS: usize = 180;

/// Every property `AWS::Lambda::Function` publishes. A declared property this list does not
/// carry is REFUSED rather than dropped, so a function is never reported created with less
/// than its declaration asked for.
///
/// Source: the type's own property set, which is `CreateFunction`'s member list
/// (`smithy_query(query="operation",operation="CreateFunction")`) plus the three properties
/// that are other calls' members.
const FUNCTION_PROPERTIES: &[&str] = &[
    "Architectures",
    "Code",
    "CodeSigningConfigArn",
    "DeadLetterConfig",
    "Description",
    "Environment",
    "EphemeralStorage",
    "FileSystemConfigs",
    "FunctionName",
    "Handler",
    "ImageConfig",
    "KmsKeyArn",
    "Layers",
    "LoggingConfig",
    "MemorySize",
    "PackageType",
    "RecursiveLoop",
    "ReservedConcurrentExecutions",
    "Role",
    "Runtime",
    "RuntimeManagementConfig",
    "SnapStart",
    "Tags",
    "Timeout",
    "TracingConfig",
    "VpcConfig",
];

/// The properties that ARE `CreateFunction`'s own members under the same name, so the
/// declared value travels into the request as written.
const CREATE_MEMBERS: &[&str] = &[
    "Architectures",
    "CodeSigningConfigArn",
    "DeadLetterConfig",
    "Description",
    "Environment",
    "EphemeralStorage",
    "FileSystemConfigs",
    "Handler",
    "ImageConfig",
    "Layers",
    "LoggingConfig",
    "MemorySize",
    "PackageType",
    "Role",
    "Runtime",
    "SnapStart",
    "Timeout",
    "TracingConfig",
    "VpcConfig",
];

/// The three properties no member of the create carries, each applied by the Lambda call
/// that owns it once the function exists ([`configure`]).
const RESERVED_CONCURRENCY_PROPERTY: &str = "ReservedConcurrentExecutions";
const RECURSIVE_LOOP_PROPERTY: &str = "RecursiveLoop";
const RUNTIME_MANAGEMENT_PROPERTY: &str = "RuntimeManagementConfig";

/// The property CloudFormation spells differently from the API member it is
/// (`KmsKeyArn` / `KMSKeyArn`).
const KMS_KEY_PROPERTY: &str = "KmsKeyArn";
const KMS_KEY_MEMBER: &str = "KMSKeyArn";

/// Create the function an `AWS::Lambda::Function` declares, in AWS Lambda itself — so a
/// client that deployed the stack and then calls `lambda get-function` with the id the
/// stack published reads the function back, rather than a `ResourceNotFoundException` for a
/// function nothing created.
///
/// `Err` is Lambda's own refusal of the create, which is what the resource fails with: a
/// role Lambda cannot assume, a runtime it does not publish, a name already taken. The
/// resource is never passed off as created on a call that did not answer 2xx.
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unmapped_properties(resource)?;
    refuse_unpassable_role(ctx, region, &resource.properties).await?;
    let name = resource.physical_resource_id.clone();
    let mut request = Map::new();
    request.insert("FunctionName".to_string(), Value::String(name.clone()));
    for member in CREATE_MEMBERS {
        if let Some(declared) = declared(resource, member) {
            request.insert((*member).to_string(), declared.clone());
        }
    }
    if let Some(declared) = declared(resource, KMS_KEY_PROPERTY) {
        request.insert(KMS_KEY_MEMBER.to_string(), declared.clone());
    }
    request.insert("Code".to_string(), function_code(resource)?);
    let tags = storable_tags(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert(
            "Tags".to_string(),
            Value::Object(
                tags.into_iter()
                    .map(|(key, value)| (key, Value::String(value)))
                    .collect(),
            ),
        );
    }

    let answered = call_rest_json(
        ctx,
        LAMBDA_SIGNING_NAME,
        region,
        "POST",
        FUNCTIONS_URI,
        Some(&Value::Object(request)),
    )
    .await?;

    // The three properties that are calls of their own, made on the function that now
    // exists. A refusal of one of them leaves the function configured differently from its
    // declaration, which is not a resource the stack may report as created — so the
    // function is removed again before the refusal is reported, exactly as a backup vault
    // whose declared lock was refused is.
    if let Err(refusal) = configure(ctx, region, resource, &name).await {
        let _ = delete_function(ctx, region, &name).await;
        return Err(refusal);
    }

    // `CreateFunction` answers the function `Pending`, and in that window AWS Lambda refuses
    // every call made on it: a resource reported created then is one its own service refuses
    // to invoke, so the create does not answer until that service reports the function out of
    // the window ([`await_settled`]).
    await_settled(ctx, region, &name).await;

    let mut attributes = BTreeMap::new();
    if let Some(arn) = reported_text(&answered, "FunctionArn") {
        attributes.insert("Arn".to_string(), arn);
    }
    Ok(Created::attributes(attributes))
}

/// Whether Lambda reports the function this create made ready to be invoked.
///
/// Nothing is asked here, because nothing is left to wait for: the create does not answer
/// until that service's own read reports the function out of the `Pending` window
/// ([`await_settled`]), so a resource this arm reports created is one AWS Lambda already
/// takes calls on.
pub(super) fn readiness() -> Readiness {
    Readiness::Ready
}

/// Apply a CHANGED declaration to the function the stack already has, through Lambda's own
/// update APIs: `UpdateFunctionCode` for the deployment package, and
/// `UpdateFunctionConfiguration` for every other member it takes — the pair a client makes
/// for the same change. The three properties that are calls of their own are re-applied
/// through those calls ([`configure`]).
///
/// The function's NAME is its identity, and a declaration that changes it is a different
/// resource: [`super::super::template`] computes a new physical id for it, so the deployment
/// replaces the function rather than reaching here.
pub(super) async fn update(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Created, Refusal> {
    let asked = StoredResource {
        properties: declared.clone(),
        ..resource.clone()
    };
    refuse_unmapped_properties(&asked)?;
    refuse_unpassable_role(ctx, region, declared).await?;
    let name = resource.physical_resource_id.clone();

    // Each of the calls below modifies the SAME function, and Lambda refuses a modification
    // while the one before it is still being applied, so each is made only once that service
    // reports the function out of that window ([`await_settled`]).
    await_settled(ctx, region, &name).await;

    let mut configuration = Map::new();
    for member in CREATE_MEMBERS {
        // `PackageType` and `Architectures` are no members of
        // `UpdateFunctionConfiguration`: the first cannot change on a function that exists,
        // and the second travels with the code below.
        if *member == "PackageType" || *member == "Architectures" {
            continue;
        }
        if let Some(value) = declared_in(declared, member) {
            configuration.insert((*member).to_string(), value.clone());
        }
    }
    if let Some(value) = declared_in(declared, KMS_KEY_PROPERTY) {
        configuration.insert(KMS_KEY_MEMBER.to_string(), value.clone());
    }
    let answered = call_rest_json(
        ctx,
        LAMBDA_SIGNING_NAME,
        region,
        "PUT",
        &configuration_uri(&name),
        Some(&Value::Object(configuration)),
    )
    .await?;

    // The code is a call of its own, and `UpdateFunctionCode` takes the package's members
    // flat rather than under a `Code` member.
    let mut code = match function_code(&asked)? {
        Value::Object(members) => members,
        _ => Map::new(),
    };
    if let Some(architectures) = declared_in(declared, "Architectures") {
        code.insert("Architectures".to_string(), architectures.clone());
    }
    await_settled(ctx, region, &name).await;
    call_rest_json(
        ctx,
        LAMBDA_SIGNING_NAME,
        region,
        "PUT",
        &format!("{}/code", function_uri(&name)),
        Some(&Value::Object(code)),
    )
    .await?;
    await_settled(ctx, region, &name).await;
    configure(ctx, region, &asked, &name).await?;

    // The modification AWS Lambda last accepted is still being applied when it answers, and
    // the same rule holds for a change as for a create: a resource the stack reports updated
    // is one that service takes the next call on ([`await_settled`]).
    await_settled(ctx, region, &name).await;

    let mut attributes = BTreeMap::new();
    if let Some(arn) = reported_text(&answered, "FunctionArn") {
        attributes.insert("Arn".to_string(), arn);
    }
    Ok(Created::attributes(attributes))
}

/// Bring the function's tag set back into step with the one CloudFormation gives it, through
/// Lambda's own `TagResource`/`UntagResource` — `CreateFunction`'s `Tags` member applies to
/// the function that call creates and to no function that already exists, so this is where
/// the set a function is to end up carrying is reconciled.
///
/// Idempotent: a function whose tags already agree with the set has nothing called about it.
pub(super) async fn retag(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
    propagated: &[(String, String)],
) -> Result<(), Refusal> {
    // The ARN Lambda's tagging calls address a function by: the one the record carries, or
    // the one that service reports for the function right now — never a value this builds,
    // so a resource whose attributes were not re-pointed still has its tags reconciled
    // rather than silently left as they were.
    let arn = match resource.attributes.get("Arn") {
        Some(arn) if !arn.is_empty() => arn.clone(),
        _ => {
            let answered = call_rest_json(
                ctx,
                LAMBDA_SIGNING_NAME,
                region,
                "GET",
                &configuration_uri(&resource.physical_resource_id),
                None,
            )
            .await?;
            match reported_text(&answered, "FunctionArn") {
                Some(arn) => arn,
                None => {
                    return Err(Refusal {
                        code: "InvalidRequest".to_string(),
                        message: format!(
                            "AWS Lambda reported no FunctionArn for {}, which its tagging \
                             calls address a function by",
                            resource.physical_resource_id
                        ),
                    })
                }
            }
        }
    };
    let wanted = storable_tags(declared, propagated);
    let uri = format!("/2017-03-31/tags/{}", percent_encoded(&arn));
    let held = call_rest_json(ctx, LAMBDA_SIGNING_NAME, region, "GET", &uri, None).await?;
    let held = held
        .get("Tags")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let removed: Vec<String> = held
        .keys()
        .filter(|key| !wanted.iter().any(|(wanted, _)| wanted == *key))
        .cloned()
        .collect();
    if !removed.is_empty() {
        let query = removed
            .iter()
            .map(|key| format!("tagKeys={}", percent_encoded(key)))
            .collect::<Vec<String>>()
            .join("&");
        call_rest_json(
            ctx,
            LAMBDA_SIGNING_NAME,
            region,
            "DELETE",
            &format!("{uri}?{query}"),
            None,
        )
        .await?;
    }

    let added: Map<String, Value> = wanted
        .into_iter()
        .filter(|(key, value)| held.get(key).and_then(Value::as_str) != Some(value.as_str()))
        .map(|(key, value)| (key, Value::String(value)))
        .collect();
    if !added.is_empty() {
        let request = Value::Object(
            [("Tags".to_string(), Value::Object(added))]
                .into_iter()
                .collect::<Map<String, Value>>(),
        );
        call_rest_json(
            ctx,
            LAMBDA_SIGNING_NAME,
            region,
            "POST",
            &uri,
            Some(&request),
        )
        .await?;
    }
    Ok(())
}

/// Delete the function a stack created, addressed by its name — this resource's physical id,
/// which is what `Ref` returns for the type.
///
/// A function Lambda no longer holds is already in the state the delete asks for, judged on
/// the CODE and never on a message.
pub(super) async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    delete_function(ctx, region, &resource.physical_resource_id).await
}

/// Whether Lambda has finished removing the function a delete was accepted for.
///
/// `DeleteFunction` takes the function away, so there is nothing to wait for; the read is
/// what says so rather than an assumption about it.
pub(super) async fn removal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Removal, Refusal> {
    match call_rest_json(
        ctx,
        LAMBDA_SIGNING_NAME,
        region,
        "GET",
        &configuration_uri(physical_resource_id),
        None,
    )
    .await
    {
        Ok(answered) => Ok(Removal::Pending(
            reported_text(&answered, STATE_MEMBER).unwrap_or_default(),
        )),
        Err(refusal) if refusal.code == FUNCTION_NOT_FOUND => Ok(Removal::Gone),
        Err(refusal) => Err(refusal),
    }
}

/// The function's configuration as Lambda reports it, for drift detection: the members
/// `GetFunctionConfiguration` answers under the same names the type publishes as properties.
/// A function that service no longer holds was deleted.
pub(super) async fn actual_configuration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    match call_rest_json(
        ctx,
        LAMBDA_SIGNING_NAME,
        region,
        "GET",
        &configuration_uri(physical_resource_id),
        None,
    )
    .await
    {
        Ok(answered) => {
            let mut reported = Map::new();
            for (property, member) in [
                ("FunctionName", "FunctionName"),
                ("Role", "Role"),
                ("Handler", "Handler"),
                ("Runtime", "Runtime"),
                ("Description", "Description"),
                ("Timeout", "Timeout"),
                ("MemorySize", "MemorySize"),
                ("PackageType", "PackageType"),
                (KMS_KEY_PROPERTY, KMS_KEY_MEMBER),
            ] {
                if let Some(value) = answered.get(member) {
                    if !value.is_null() {
                        reported.insert(property.to_string(), value.clone());
                    }
                }
            }
            Ok(Some(Value::Object(reported)))
        }
        Err(refusal) if refusal.code == FUNCTION_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// Apply the declared properties no member of the create carries — the three the module
/// documentation names — each through the Lambda call that owns it. A declaration that
/// states none of them makes no call.
async fn configure(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    name: &str,
) -> Result<(), Refusal> {
    // "The number of simultaneous executions to reserve for the function"
    // (`PutFunctionConcurrency`).
    if let Some(reserved) = declared(resource, RESERVED_CONCURRENCY_PROPERTY) {
        let request = Value::Object(
            [(RESERVED_CONCURRENCY_PROPERTY.to_string(), reserved.clone())]
                .into_iter()
                .collect::<Map<String, Value>>(),
        );
        call_rest_json(
            ctx,
            LAMBDA_SIGNING_NAME,
            region,
            "PUT",
            &format!("/2017-10-31/functions/{}/concurrency", percent_encoded(name)),
            Some(&request),
        )
        .await?;
    }
    // "Sets your function's recursive loop detection configuration"
    // (`PutFunctionRecursionConfig`).
    if let Some(recursive_loop) = declared(resource, RECURSIVE_LOOP_PROPERTY) {
        let request = Value::Object(
            [(RECURSIVE_LOOP_PROPERTY.to_string(), recursive_loop.clone())]
                .into_iter()
                .collect::<Map<String, Value>>(),
        );
        call_rest_json(
            ctx,
            LAMBDA_SIGNING_NAME,
            region,
            "PUT",
            &format!(
                "/2024-08-31/functions/{}/recursion-config",
                percent_encoded(name)
            ),
            Some(&request),
        )
        .await?;
    }
    // "Sets the runtime management configuration for a function's version"
    // (`PutRuntimeManagementConfig`), whose members the property states.
    if let Some(Value::Object(declared)) = declared(resource, RUNTIME_MANAGEMENT_PROPERTY) {
        let request = Value::Object(declared.clone());
        call_rest_json(
            ctx,
            LAMBDA_SIGNING_NAME,
            region,
            "PUT",
            &format!(
                "/2021-07-20/functions/{}/runtime-management-config",
                percent_encoded(name)
            ),
            Some(&request),
        )
        .await?;
    }
    Ok(())
}

/// Wait until Lambda reports the function SETTLED — neither still being provisioned
/// (`State` `Pending`) nor still applying the modification before this one
/// (`LastUpdateStatus` `InProgress`) — which is the state the function both takes the next
/// call of this deployment in and serves a client's own calls in.
///
/// Two things need it. Applying a changed declaration takes SEVERAL calls on one function
/// (`UpdateFunctionConfiguration`, then `UpdateFunctionCode`, then the properties that are
/// calls of their own), and Lambda refuses each of them inside the window the one before it
/// opened; a deployment that makes the next call regardless fails the resource on that
/// refusal, so the stack reports a resource it was perfectly able to update as failed. And a
/// CREATE answers the function `Pending`, a state in which "you can't invoke or modify the
/// function": a resource reported created then is one whose own service refuses every call
/// made on it, so the client that reads the stack as complete — and the service that delivers
/// to the function the moment it does — would be refused. Both are the same wait, exactly as
/// the proxy and stream arms of [`super::super::provisioner`] wait for the state the next
/// call needs.
///
/// Returns as soon as the function is settled, and gives up after [`SETTLE_CHECKS`]
/// checks. A refusal of the READ ends the wait the same way: it says nothing about whether
/// the function is settled, and the call that follows is what reports Lambda's answer.
async fn await_settled(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) {
    for _ in 0..SETTLE_CHECKS {
        let Ok(answered) = call_rest_json(
            ctx,
            LAMBDA_SIGNING_NAME,
            region,
            "GET",
            &configuration_uri(name),
            None,
        )
        .await
        else {
            return;
        };
        let provisioning = reported_text(&answered, STATE_MEMBER)
            .is_some_and(|state| state == STATE_PENDING);
        let updating = reported_text(&answered, LAST_UPDATE_STATUS_MEMBER)
            .is_some_and(|status| status == LAST_UPDATE_STATUS_IN_PROGRESS);
        if !provisioning && !updating {
            return;
        }
        ::tokio::time::sleep(SETTLE_CHECK_INTERVAL).await;
    }
}

/// `DeleteFunction` on one function, with a function Lambda no longer holds read as the
/// deletion this asked for.
async fn delete_function(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> Result<(), Refusal> {
    match call_rest_json(
        ctx,
        LAMBDA_SIGNING_NAME,
        region,
        "DELETE",
        &function_uri(name),
        None,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == FUNCTION_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// `CreateFunction`'s `Code` member, built from the declared `Code` property.
///
/// Every member but one is the same on both sides and travels as written. The one that is
/// not is `ZipFile`: a template states the function's SOURCE TEXT there, while the API's
/// member is the BYTES of a .zip deployment package — so the declared text is archived as
/// the one file in a package, under the name Lambda's handler resolves for the declared
/// runtime ([`inline_package`]), and the archive travels base64-encoded as the blob the wire
/// carries.
fn function_code(resource: &StoredResource) -> Result<Value, Refusal> {
    let Some(Value::Object(declared)) = declared(resource, "Code") else {
        return Err(Refusal {
            code: "InvalidRequest".to_string(),
            message: format!("{LAMBDA_FUNCTION} requires the Code property"),
        });
    };
    let mut code = Map::new();
    for (member, value) in declared {
        if member == "ZipFile" {
            continue;
        }
        code.insert(member.clone(), value.clone());
    }
    if let Some(source) = declared.get("ZipFile").and_then(Value::as_str) {
        let handler = declared_text(resource, "Handler").unwrap_or_default();
        let runtime = declared_text(resource, "Runtime").unwrap_or_default();
        code.insert(
            "ZipFile".to_string(),
            Value::String(inline_package(source, &handler, &runtime)?),
        );
    }
    Ok(Value::Object(code))
}

/// The deployment package CloudFormation builds for inline code: one file holding the
/// declared source, named after the handler's own file ("index" plus the declared runtime's
/// extension, which is what a `Handler` of `index.handler` names), in a .zip archive — then
/// base64-encoded, because the API's `ZipFile` is a blob and a JSON request carries a blob
/// encoded.
///
/// Beside the source, the package carries the RESPONSE MODULE CloudFormation supplies to
/// inline code ([`RESPONSE_MODULE_SOURCE`]): "the cfnresponse module is available only when
/// you use the `ZipFile` property to write your function's source code", which is what lets
/// a custom resource's inline provider answer the request it was sent without shipping an
/// HTTP client of its own. It is added for the runtime family the module is published for
/// and for no other ([`response_module_file`]).
///
/// Written with the maintained `zip` implementation rather than by hand, and held entirely
/// in memory: nothing is written to disk.
fn inline_package(source: &str, handler: &str, runtime: &str) -> Result<String, Refusal> {
    let stem = handler.split('.').next().unwrap_or("index");
    let stem = if stem.is_empty() { "index" } else { stem };
    let file = format!("{stem}{}", source_extension(runtime));
    let mut archive = ::zip::ZipWriter::new(::std::io::Cursor::new(Vec::new()));
    let options: ::zip::write::SimpleFileOptions = ::zip::write::SimpleFileOptions::default()
        .compression_method(::zip::CompressionMethod::Deflated);
    let mut written = archive
        .start_file(file, options)
        .and_then(|()| archive.write_all(source.as_bytes()).map_err(Into::into));
    if let Some(module) = response_module_file(runtime) {
        written = written
            .and_then(|()| archive.start_file(module, options))
            .and_then(|()| {
                archive
                    .write_all(RESPONSE_MODULE_SOURCE.as_bytes())
                    .map_err(Into::into)
            });
    }
    written
        .and_then(|()| archive.finish().map(|cursor| cursor.into_inner()))
        .map(|bytes| base64_encoded(&bytes))
        .map_err(|error| Refusal {
            code: "InternalFailure".to_string(),
            message: format!("the inline deployment package could not be archived: {error}"),
        })
}

/// The file the response module is supplied as, for the runtime family the declaration
/// names, or `None` for a runtime CloudFormation publishes no such module for.
///
/// Source: "the cfnresponse module is available only when you use the `ZipFile` property",
/// and the module's documented reference is the Python one, imported as `cfnresponse` — so
/// the file is that module's own name. Every other runtime family gets none: a module this
/// emulator supplied under a name CloudFormation does not publish would be a module no real
/// function can import.
fn response_module_file(runtime: &str) -> Option<&'static str> {
    runtime.starts_with("python").then_some("cfnresponse.py")
}

/// The response module an inline function imports to answer its custom resource request:
/// `cfnresponse.send(event, context, status, data)` PUTs the response document
/// CloudFormation documents to the request's own `ResponseURL`.
///
/// What the module DOES is the published contract — the members it exports, the document it
/// sends and the defaults it fills (the physical resource id and the reason a caller states
/// none for) — and how it sends that document is this emulator's: the request is made with
/// the standard library's own HTTP client, which reaches the `ResponseURL` this service
/// published ([`crate::state::provisioning::custom_resource::response_url`]) through the
/// function runtime's own seam, so nothing of a provider's answer is synthesised here.
const RESPONSE_MODULE_SOURCE: &str = r#"
"""The module CloudFormation supplies to inline function code, for answering a custom
resource request at its ResponseURL.
"""

import json
import urllib.request

SUCCESS = "SUCCESS"
FAILED = "FAILED"


def send(
    event,
    context,
    responseStatus,
    responseData,
    physicalResourceId=None,
    noEcho=False,
    reason=None,
):
    """Send one response document to the request's ResponseURL."""
    stream = getattr(context, "log_stream_name", "")
    body = {
        "Status": responseStatus,
        "Reason": reason
        or "See the details in CloudWatch Log Stream: {}".format(stream),
        "PhysicalResourceId": physicalResourceId or stream,
        "StackId": event["StackId"],
        "RequestId": event["RequestId"],
        "LogicalResourceId": event["LogicalResourceId"],
        "NoEcho": noEcho,
        "Data": responseData,
    }
    encoded = json.dumps(body).encode("utf-8")
    request = urllib.request.Request(
        event["ResponseURL"],
        data=encoded,
        headers={"content-type": "", "content-length": str(len(encoded))},
        method="PUT",
    )
    with urllib.request.urlopen(request) as answered:
        answered.read()
"#;

/// The file extension the source of an inline function carries, for the runtime family the
/// declaration names.
///
/// Source: the file each runtime loads a handler module from by DEFAULT, which is the one an
/// inline declaration's `Handler` (`index.handler`) resolves against — `index.py` for Python,
/// `index.js` for the Node.js runtimes, `index.rb` for Ruby. For Node that is the plain `.js`
/// source file, not the `.mjs` one: a Node runtime reads a `.mjs` entry as an ES module only,
/// so an inline handler written the documented way — `exports.handler` with `require` — would
/// not load at all, while a `.js` entry carries either form. A runtime whose inline form the
/// CloudFormation user guide does not publish (the compiled runtimes, which cannot be declared
/// inline at all) gets no extension, so the archive carries the handler's own file name.
fn source_extension(runtime: &str) -> &'static str {
    if runtime.starts_with("python") {
        ".py"
    } else if runtime.starts_with("nodejs") {
        ".js"
    } else if runtime.starts_with("ruby") {
        ".rb"
    } else {
        ""
    }
}

/// Standard base64, as a JSON request carries a blob member.
///
/// The encoder this crate already renders every blob with, so no second base64 path and no
/// alphabet is written here.
fn base64_encoded(bytes: &[u8]) -> String {
    ::aws_smithy_types::base64::encode(bytes)
}

/// The tags of a declared function that Lambda will STORE: the ones the declaration asks
/// for and the ones CloudFormation propagates ([`tags_of`]), less every key in the
/// namespace Lambda reserves that is not CloudFormation's own.
///
/// "Tag keys beginning with `aws:` are reserved" is Lambda's rule for a CALLER of its
/// tag-writing API — it refuses the whole request on one such key. The three records
/// CloudFormation keeps about a resource it provisioned (the stack's name, its id and the
/// resource's logical id) are AWS's own tags, set by the service that provisions the
/// resource rather than by anybody calling it, and Lambda admits them on the in-process
/// path this provider calls it over — so they TRAVEL, and a client's own `ListTags` on the
/// function the stack published reports them beside the declaration's tags. Every other
/// key in that namespace is a caller's to have supplied and is dropped here rather than
/// sent to a call that would refuse the function outright: a deployment must not fail over
/// tags a template merely spelled.
///
/// The namespace and CloudFormation's own three keys are each the reading this crate states
/// once ([`crate::state::stack_tags::is_reserved_tag_key`],
/// [`super::is_stack_provenance_tag`]), since Lambda reserves the same prefix every other
/// tag-writing API does.
fn storable_tags(properties: &Value, propagated: &[(String, String)]) -> Vec<(String, String)> {
    tags_of(properties, propagated)
        .into_iter()
        .filter(|(key, _)| {
            super::is_stack_provenance_tag(key)
                || !crate::state::stack_tags::is_reserved_tag_key(key)
        })
        .collect()
}

/// The URI that addresses ONE function: the `@httpLabel` every function operation binds the
/// name to, percent-encoded as a non-greedy label is sent.
fn function_uri(name: &str) -> String {
    format!("{FUNCTIONS_URI}/{}", percent_encoded(name))
}

/// The URI `GetFunctionConfiguration` and `UpdateFunctionConfiguration` bind.
fn configuration_uri(name: &str) -> String {
    format!("{}/configuration", function_uri(name))
}

/// One declared property of the resource, as the template evaluated it.
fn declared<'a>(resource: &'a StoredResource, name: &str) -> Option<&'a Value> {
    declared_in(&resource.properties, name)
}

/// The same reading against a property document that is not the record's own — the
/// declaration a deployment states for a resource the stack already has.
fn declared_in<'a>(properties: &'a Value, name: &str) -> Option<&'a Value> {
    properties
        .get(name)
        .filter(|value| !value.is_null())
}

/// One declared property read as TEXT, for the two the package's file name is built from.
fn declared_text(resource: &StoredResource, name: &str) -> Option<String> {
    declared(resource, name)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// One member of an answer read as TEXT: the ARN and the state Lambda reports.
fn reported_text(answered: &Value, member: &str) -> Option<String> {
    answered
        .get(member)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// The refusal for a declared execution role CloudFormation cannot hand to Lambda: this
/// account's IAM holds no role under the name the ARN carries, or the trust policy the role
/// it does hold carries does not let Lambda assume it.
///
/// A function RUNS AS the role its declaration names, and it is CloudFormation that hands
/// that role over when it creates the function, so the role is resolved against this
/// account's own IAM before the create is made. Both answers are grounded in what IAM said:
/// a role it holds whose trust document withholds the service principal, and a name it holds
/// no role under at all — which inside a stack is evidence, because CloudFormation creates
/// every role the template it is deploying declares, and a `Role` naming a role of this
/// account that does not exist is a declaration no create can satisfy. (A DIRECT
/// `CreateFunction` caller is served in that case: an execution role is routinely
/// provisioned outside the client that creates the function, so an absent record says
/// nothing about the role it named.) Nothing is refused where nothing is known — another
/// account's role, an answer this cannot read, an emulator with no IAM mounted — exactly as
/// the shared reading answers it.
///
/// The code and the sentence are the ones AWS Lambda refuses such a create with, because
/// that is the refusal this condition is: `InvalidParameterValueException` with that
/// service's own message, which the resource's failure reason then quotes.
async fn refuse_unpassable_role(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    properties: &Value,
) -> Result<(), Refusal> {
    let Some(role) = declared_in(properties, ROLE_PROPERTY).and_then(Value::as_str) else {
        return Ok(());
    };
    if ::service_common::iam_role::role_is_unassumable(
        ctx,
        region,
        role,
        ::service_com_amazonaws_lambda_awsgirapiservice::state::EXECUTION_ROLE_PRINCIPAL,
    )
    .await
    {
        return Err(Refusal {
            code: INVALID_PARAMETER_VALUE.to_string(),
            message:
                ::service_com_amazonaws_lambda_awsgirapiservice::state::ROLE_NOT_ASSUMABLE_MESSAGE
                    .to_string(),
        });
    }
    Ok(())
}

/// The refusal for a declared property this module does not apply, naming which property it
/// was: refusing is what keeps a function from being reported as created with less than its
/// declaration asked for.
fn refuse_unmapped_properties(resource: &StoredResource) -> Result<(), Refusal> {
    let Value::Object(properties) = &resource.properties else {
        return Ok(());
    };
    for property in properties.keys() {
        if FUNCTION_PROPERTIES.contains(&property.as_str()) {
            continue;
        }
        return Err(Refusal {
            code: "NotImplemented".to_string(),
            message: format!(
                "this emulator does not apply the {property} property of {}",
                resource.resource_type
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The properties that are other calls' members rather than the create's.
    const CONFIGURED_AFTER_CREATE: &[&str] = &[
        RESERVED_CONCURRENCY_PROPERTY,
        RECURSIVE_LOOP_PROPERTY,
        RUNTIME_MANAGEMENT_PROPERTY,
    ];

    /// The three properties that are other calls' members are not sent as members of the
    /// create, and every other published property is either a create member or one of
    /// those three — so no declared property can be silently dropped.
    #[test]
    fn every_published_property_has_a_place() {
        for property in FUNCTION_PROPERTIES {
            let placed = CREATE_MEMBERS.contains(property)
                || CONFIGURED_AFTER_CREATE.contains(property)
                || ["Code", "FunctionName", "Tags", KMS_KEY_PROPERTY].contains(property);
            assert!(
                placed,
                "case {property:?}: expected the property to be a create member, a \
                 follow-up call's member or one of the four the arm maps itself, got none"
            );
        }
        for member in CREATE_MEMBERS {
            assert!(
                FUNCTION_PROPERTIES.contains(member),
                "case {member:?}: expected the create member to be a published property of \
                 {LAMBDA_FUNCTION}, got none"
            );
        }
    }

    /// The inline package is a .zip archive holding the declared source, named after the
    /// handler's own file with the extension the declared runtime's handler format names —
    /// and, for the runtime family CloudFormation publishes it for, the response module an
    /// inline provider answers its custom resource request through.
    #[test]
    fn inline_code_is_archived_under_the_handler_file() {
        // (case, handler, runtime, the source's file, the module file the package also
        // carries)
        let cases: &[(&str, &str, &str, &str, Option<&str>)] = &[
            ("python", "index.handler", "python3.12", "index.py", Some("cfnresponse.py")),
            (
                "python under another file",
                "app.handler",
                "python3.13",
                "app.py",
                Some("cfnresponse.py"),
            ),
            ("node", "index.handler", "nodejs20.x", "index.js", None),
            ("node under another file", "app.handler", "nodejs22.x", "app.js", None),
            ("ruby", "index.handler", "ruby3.3", "index.rb", None),
            (
                "a runtime with no inline form",
                "bootstrap",
                "provided.al2023",
                "bootstrap",
                None,
            ),
        ];
        for (case, handler, runtime, expected, module) in cases {
            let encoded = inline_package("def handler(event, context):\n    return 1\n", handler, runtime)
                .unwrap_or_else(|error| panic!("case {case:?}: the package could not be built: {error}"));
            let bytes = ::aws_smithy_types::base64::decode(&encoded)
                .unwrap_or_else(|error| panic!("case {case:?}: the package is not base64: {error}"));
            let mut archive = ::zip::ZipArchive::new(::std::io::Cursor::new(bytes))
                .unwrap_or_else(|error| panic!("case {case:?}: the package is not a zip: {error}"));
            let names: Vec<String> = archive
                .file_names()
                .map(|name| {
                    name.unwrap_or_else(|error| {
                        panic!("case {case:?}: an entry name could not be read: {error}")
                    })
                    .to_string()
                })
                .collect();
            let mut held_names = vec![(*expected).to_string()];
            held_names.extend(module.map(str::to_string));
            assert_eq!(
                names, held_names,
                "case {case:?}: expected the archive to hold {held_names:?}, got {names:?}"
            );
            let mut held = String::new();
            ::std::io::Read::read_to_string(
                &mut archive
                    .by_name(expected)
                    .unwrap_or_else(|error| panic!("case {case:?}: {expected} is absent: {error}")),
                &mut held,
            )
            .unwrap_or_else(|error| panic!("case {case:?}: {expected} could not be read: {error}"));
            assert_eq!(
                held, "def handler(event, context):\n    return 1\n",
                "case {case:?}: expected the declared source, got {held:?}"
            );
            // The module the package carries is the one a handler imports by that name, and
            // what it exports is the published protocol: the two statuses and the call that
            // sends the response document to the request's own `ResponseURL`.
            if let Some(module) = module {
                let mut source = String::new();
                ::std::io::Read::read_to_string(
                    &mut archive.by_name(module).unwrap_or_else(|error| {
                        panic!("case {case:?}: {module} is absent: {error}")
                    }),
                    &mut source,
                )
                .unwrap_or_else(|error| {
                    panic!("case {case:?}: {module} could not be read: {error}")
                });
                for published in ["SUCCESS", "FAILED", "def send(", "ResponseURL"] {
                    assert!(
                        source.contains(published),
                        "case {case:?}: expected {module} to carry {published:?}, got {source}"
                    );
                }
            }
        }
    }
}
