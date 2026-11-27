// SPDX-License-Identifier: CC0-1.0

//! `AWS::Lambda::Function`, as it lives in the AWS Lambda service itself.
//!
//! A function is not a record of this service: `lambda:CreateFunction` creates it,
//! `lambda:GetFunction` reports it, `lambda:UpdateFunctionCode` and
//! `lambda:UpdateFunctionConfiguration` change it and `lambda:DeleteFunction` removes it.
//! A create of this type therefore does work of its own in another service — it deploys a
//! package, provisions the function and reports a lifecycle of its own
//! ([`progress`]) — which is why every answer here is a call on THAT service's own API,
//! dispatched in process ([`in_process_lambda`]) rather than a write into a store of this
//! one: a function any API created is one this service resolves, and a function that
//! service no longer holds is one it reports as not found.
//!
//! ## The resource model
//!
//! Catalog source: the `AWS::Lambda::Function` resource provider schema's `properties`,
//! whose `primaryIdentifier` is `/properties/FunctionName` — the name the caller gives the
//! function, which is therefore also its one create-only property — and whose
//! `readOnlyProperties` are the `Arn` AWS Lambda assigns and the `SnapStartResponse` it
//! reports ([`crate::state::read_only_properties`] holds them).
//!
//! The model a read reports is what `GetFunction` answers under those property names: the
//! function's name and ARN, the role it runs as, its runtime and handler, its description,
//! its memory and timeout, its architectures and layers, its environment, its tracing,
//! dead-letter, ephemeral-storage, file-system, VPC, image and logging configuration, its
//! package type, its reserved concurrency, the snap-start state that service reports and
//! its tags. The deployment package is part of that model as the type's `Code`, and EMPTY:
//! every member the schema declares for it is one a caller SETS — the inline source, the
//! Amazon S3 object, the image, the key the package is encrypted under — and that read
//! answers none of them, which is what the type's `writeOnlyProperties` state
//! ([`crate::state::write_only_properties`] holds them, pointing at those members rather
//! than at the property). So no model this service reports carries a deployment package, and
//! a function whose code has since been replaced still reads back under its own identifier
//! with the same model.
//!
//! `Code.ZipFile` is the function's SOURCE TEXT ("you can specify your function code
//! inline"), while the API's `ZipFile` member is the BYTES of a .zip deployment package, so
//! the stated text is archived as the one file of a package under the name the handler
//! resolves ([`inline_package`]) and the archive is what the call carries.
//!
//! ## What a refusal carries
//!
//! A call AWS Lambda refused is reported as that service's own verdict ([`refusal_of`]): the
//! error code it named decides the `HandlerErrorCode` the resource operation request fails
//! with, and the message it worded is the diagnostic. Nothing here re-words it.

use ::std::collections::HashMap;
use ::std::io::Write;
use ::std::time::Duration;

use ::serde_json::{Map, Value};

use super::{
    answered_error, count, flag, tag_entries, text, text_list, Progress, Refusal,
    MAX_OWNING_PAGES, TAGS, TAG_KEY, TAG_VALUE,
};

/// The `TypeName` this module stands behind.
pub const TYPE_NAME: &str = "AWS::Lambda::Function";

/// The property that carries the type's primary identifier, which its schema's
/// `primaryIdentifier` points at (`/properties/FunctionName`): the function's own name.
pub const IDENTIFIER_PROPERTY: &str = "FunctionName";

/// The ARN AWS Lambda assigns the function, the type's other read-only property beside the
/// snap-start state ([`SNAP_START_RESPONSE`]).
const ARN: &str = "Arn";

/// The deployment package the function is created from, whose every member the type's schema
/// declares write-only.
const CODE: &str = "Code";

/// The remaining properties of the type's schema, each a member of the AWS Lambda calls that
/// create and change a function.
const ARCHITECTURES: &str = "Architectures";
const DEAD_LETTER_CONFIG: &str = "DeadLetterConfig";
const DESCRIPTION: &str = "Description";
const ENVIRONMENT: &str = "Environment";
const EPHEMERAL_STORAGE: &str = "EphemeralStorage";
const FILE_SYSTEM_CONFIGS: &str = "FileSystemConfigs";
const HANDLER: &str = "Handler";
const IMAGE_CONFIG: &str = "ImageConfig";
const KMS_KEY_ARN: &str = "KmsKeyArn";
const LAYERS: &str = "Layers";
const LOGGING_CONFIG: &str = "LoggingConfig";
const MEMORY_SIZE: &str = "MemorySize";
const PACKAGE_TYPE: &str = "PackageType";
const ROLE: &str = "Role";
const RUNTIME: &str = "Runtime";
const SNAP_START: &str = "SnapStart";
const SNAP_START_RESPONSE: &str = "SnapStartResponse";
const TIMEOUT: &str = "Timeout";
const TRACING_CONFIG: &str = "TracingConfig";
const VPC_CONFIG: &str = "VpcConfig";

/// The properties of the type's schema that are each a LATER call on the function rather
/// than a member of the two this module makes: the reserved concurrency
/// (`PutFunctionConcurrency`), the recursive-loop detection
/// (`PutFunctionRecursionConfig`), the runtime management configuration
/// (`PutRuntimeManagementConfig`) and the code-signing configuration
/// (`PutFunctionCodeSigningConfig`).
///
/// A read reports the first of them, which AWS Lambda answers beside the function
/// (`GetFunction$Concurrency`), so a model a client round-trips carries it; the others no
/// read of a function answers. Either way a model may only carry what the read REPORTS for
/// them ([`DesiredFunction::read`]): any other value asks for work this handler does not do,
/// which is refused rather than accepted and dropped.
const RESERVED_CONCURRENT_EXECUTIONS: &str = "ReservedConcurrentExecutions";
const RECURSIVE_LOOP: &str = "RecursiveLoop";
const RUNTIME_MANAGEMENT_CONFIG: &str = "RuntimeManagementConfig";
const CODE_SIGNING_CONFIG_ARN: &str = "CodeSigningConfigArn";

/// EVERY property the type's schema declares. A model naming anything else describes no
/// function, so it is refused rather than carried into a call that would drop it.
const DECLARED_PROPERTIES: &[&str] = &[
    ARCHITECTURES,
    ARN,
    CODE,
    CODE_SIGNING_CONFIG_ARN,
    DEAD_LETTER_CONFIG,
    DESCRIPTION,
    ENVIRONMENT,
    EPHEMERAL_STORAGE,
    FILE_SYSTEM_CONFIGS,
    HANDLER,
    IDENTIFIER_PROPERTY,
    IMAGE_CONFIG,
    KMS_KEY_ARN,
    LAYERS,
    LOGGING_CONFIG,
    MEMORY_SIZE,
    PACKAGE_TYPE,
    RECURSIVE_LOOP,
    RESERVED_CONCURRENT_EXECUTIONS,
    ROLE,
    RUNTIME,
    RUNTIME_MANAGEMENT_CONFIG,
    SNAP_START,
    SNAP_START_RESPONSE,
    TAGS,
    TIMEOUT,
    TRACING_CONFIG,
    VPC_CONFIG,
];

/// The properties whose value `UpdateFunctionConfiguration` carries, which is what decides
/// whether an update has that call to make at all: a patch that changed none of them asks
/// for no change to the function's configuration, and making the call regardless would open
/// AWS Lambda's own modification window on a function nothing modified.
const CONFIGURED_PROPERTIES: &[&str] = &[
    DEAD_LETTER_CONFIG,
    DESCRIPTION,
    ENVIRONMENT,
    EPHEMERAL_STORAGE,
    FILE_SYSTEM_CONFIGS,
    HANDLER,
    IMAGE_CONFIG,
    KMS_KEY_ARN,
    LAYERS,
    LOGGING_CONFIG,
    MEMORY_SIZE,
    ROLE,
    RUNTIME,
    SNAP_START,
    TIMEOUT,
    TRACING_CONFIG,
    VPC_CONFIG,
];

/// The members of the type's `Code` property, as the schema spells them.
const CODE_ZIP_FILE: &str = "ZipFile";
const CODE_S3_BUCKET: &str = "S3Bucket";
const CODE_S3_KEY: &str = "S3Key";
const CODE_S3_OBJECT_VERSION: &str = "S3ObjectVersion";
const CODE_IMAGE_URI: &str = "ImageUri";
const CODE_SOURCE_KMS_KEY_ARN: &str = "SourceKMSKeyArn";

/// The members of the structured properties, as the schema spells them.
const ENVIRONMENT_VARIABLES: &str = "Variables";
const DEAD_LETTER_TARGET_ARN: &str = "TargetArn";
const TRACING_MODE: &str = "Mode";
const EPHEMERAL_SIZE: &str = "Size";
const FILE_SYSTEM_ARN: &str = "Arn";
const FILE_SYSTEM_LOCAL_MOUNT_PATH: &str = "LocalMountPath";
const VPC_SUBNET_IDS: &str = "SubnetIds";
const VPC_SECURITY_GROUP_IDS: &str = "SecurityGroupIds";
const VPC_IPV6_ALLOWED: &str = "Ipv6AllowedForDualStack";
const LOGGING_APPLICATION_LOG_LEVEL: &str = "ApplicationLogLevel";
const LOGGING_LOG_FORMAT: &str = "LogFormat";
const LOGGING_LOG_GROUP: &str = "LogGroup";
const LOGGING_SYSTEM_LOG_LEVEL: &str = "SystemLogLevel";
const SNAP_START_APPLY_ON: &str = "ApplyOn";
const SNAP_START_OPTIMIZATION_STATUS: &str = "OptimizationStatus";
const IMAGE_COMMAND: &str = "Command";
const IMAGE_ENTRY_POINT: &str = "EntryPoint";
const IMAGE_WORKING_DIRECTORY: &str = "WorkingDirectory";

/// The two values AWS Lambda reports a function's lifecycle in while it still refuses a
/// further modification of it: `State` `Pending` while it is provisioning the function
/// ("you can't invoke or modify the function") and `LastUpdateStatus` `InProgress` while the
/// last change is still being applied ("during this time, you can't modify the function").
///
/// Source: the `State` and `LastUpdateStatus` enums of that service's own model
/// (`smithy_query(query="shape",shape="State,LastUpdateStatus")`).
const STATE_PENDING: &str = "Pending";
const STATE_FAILED: &str = "Failed";
const LAST_UPDATE_STATUS_IN_PROGRESS: &str = "InProgress";
const LAST_UPDATE_STATUS_FAILED: &str = "Failed";

/// How long [`await_modifiable`] waits between asking AWS Lambda whether the function is out
/// of those two windows, and how many times it asks.
///
/// An update that changes both the function's CODE and its CONFIGURATION is two calls on
/// that service, and the first opens a window in which it refuses the second — so the second
/// waits for the window the first opened to close. The interval is short enough that the
/// call is made about as soon as it can be, and the budget — 50 ms apart, 180 times, so nine
/// seconds — is a few times the span that service holds a modification open for, long enough
/// to cover it on a loaded machine, and well inside the lease the claim on this work holds.
/// A wait that runs out is not a failure of its own: what fails then is the call that needed
/// the function modifiable, reported as that service's own refusal of it.
const MODIFIABLE_CHECK_INTERVAL: Duration = Duration::from_millis(50);
const MODIFIABLE_CHECKS: usize = 180;

/// Whether `property` is one of this type's the OWNING service COMPUTES.
///
/// The schema's `readOnlyProperties` are held in this crate's one catalogue of them
/// ([`crate::state::read_only_properties`]), so this module restates none of them.
fn read_only_properties() -> Vec<String> {
    crate::state::read_only_properties(TYPE_NAME, IDENTIFIER_PROPERTY)
}

/// The function `identifier` names in `region`, as AWS Lambda's own read reports it, or
/// `None` when that service holds no function under it.
pub async fn read(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    identifier: &str,
) -> Option<Value> {
    held(&in_process_lambda(ctx, region), identifier).await
}

/// The function `identifier` names, as the resource model this service reports — or `None`
/// for one that service does not hold.
async fn held(client: &::aws_sdk_lambda::Client, identifier: &str) -> Option<Value> {
    let reported = client
        .get_function()
        .function_name(identifier)
        .send()
        .await
        .ok()?;
    let configuration = reported.configuration.as_ref()?;
    Some(model(
        configuration,
        reported.tags.as_ref(),
        reported
            .concurrency
            .as_ref()
            .and_then(|concurrency| concurrency.reserved_concurrent_executions),
    ))
}

/// The functions AWS Lambda holds in `region`, as `(primary identifier, resource model)` in
/// identifier order: at most `limit` of them, and only those whose identifier sorts AFTER
/// `after`.
///
/// `ListFunctions` reports them in its own order and one bounded page at a time, while a
/// resource listing of this service pages in IDENTIFIER order, so the pages are walked and
/// the window kept as they go: what is held is never more than the page this listing is
/// about to report. Each function of that window is then read back through the same read
/// `GetResource` uses, so the two cannot report different models for one function — the
/// listing of that service answers a function's configuration alone, and the model carries
/// what a read of the function answers. A page that service did not answer ends the walk, so
/// what a listing reports is the window it could read of that service's own inventory.
pub async fn list(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    after: Option<&str>,
    limit: usize,
) -> Vec<(String, Value)> {
    let client = in_process_lambda(ctx, region);
    let mut identifiers: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    for _ in 0..MAX_OWNING_PAGES {
        let Ok(page) = client
            .list_functions()
            .set_marker(marker.clone())
            .send()
            .await
        else {
            break;
        };
        for configuration in page.functions.clone().unwrap_or_default() {
            let Some(identifier) = configuration.function_name.clone() else {
                continue;
            };
            if after.is_none_or(|after| identifier.as_str() > after) {
                identifiers.push(identifier);
            }
        }
        identifiers.sort();
        identifiers.truncate(limit);
        marker = page.next_marker;
        if marker.is_none() {
            break;
        }
    }
    let mut window = Vec::with_capacity(identifiers.len());
    for identifier in identifiers {
        if let Some(model) = held(&client, &identifier).await {
            window.push((identifier, model));
        }
    }
    window
}

/// Create the function the resource model `properties` describes, in `region`, and answer the
/// `FunctionName` it answers to — the primary identifier the resource then has.
///
/// "To create a function, you need a deployment package and an execution role"
/// (`CreateFunction`), so a model carrying neither describes no function that could be
/// created. A model naming no function NAME is created under one this service picks, exactly
/// as a resource whose schema makes its name optional is.
pub async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    properties: &Value,
) -> Result<String, Refusal> {
    let desired = DesiredFunction::read(properties, None)?;
    let (Some(code), Some(role)) = (desired.deployed_code(), desired.role.clone()) else {
        return Err(Refusal::invalid(format!(
            "{CODE} and {ROLE} are required to create a resource of type {TYPE_NAME}"
        )));
    };
    let name = desired
        .function_name
        .clone()
        .unwrap_or_else(crate::state::mint_token);
    let client = in_process_lambda(ctx, region);
    let mut request = client
        .create_function()
        .function_name(&name)
        .role(role)
        .code(code)
        .set_runtime(desired.runtime())
        .set_handler(desired.handler.clone())
        .set_description(desired.description.clone())
        .set_memory_size(desired.memory_size)
        .set_timeout(desired.timeout)
        .set_kms_key_arn(desired.kms_key_arn.clone())
        .set_package_type(desired.package_type())
        .set_architectures(desired.architectures())
        .set_layers(desired.layers.clone())
        .set_environment(desired.environment.clone())
        .set_tracing_config(desired.tracing_config.clone())
        .set_dead_letter_config(desired.dead_letter_config.clone())
        .set_ephemeral_storage(desired.ephemeral_storage.clone())
        .set_file_system_configs(desired.file_system_configs.clone())
        .set_vpc_config(desired.vpc_config.clone())
        .set_logging_config(desired.logging_config.clone())
        .set_snap_start(desired.snap_start.clone())
        .set_image_config(desired.image_config.clone());
    for (key, value) in desired.tags.clone().unwrap_or_default() {
        request = request.tags(key, value);
    }
    request
        .send()
        .await
        .map_err(|error| refusal_of(error, ConflictAs::AlreadyExisting))?
        .function_name
        .ok_or_else(|| Refusal::rejected(NO_FUNCTION_NAME.to_string()))
}

/// What a create answers with when AWS Lambda reported no name for the function it made:
/// nothing names the resource, so the request fails rather than reporting a success with no
/// identifier.
const NO_FUNCTION_NAME: &str = "AWS Lambda reported no FunctionName for the function it created";

/// Apply the resource model `properties` to the function `identifier` names in `region`.
///
/// Two calls change a function that exists, each owning part of it: `UpdateFunctionCode`
/// deploys a package and `UpdateFunctionConfiguration` applies everything else. Only the one
/// whose part the model CHANGES is made — the function's own read says what it holds now, and
/// a `Code` that states no package at all (the empty one every read reports) asks for no
/// deployment — because each opens a window in which that service refuses a further
/// modification, so a call that asks for nothing would hold the function closed for nothing.
/// Where both are made, the second waits for the window the first opened to close
/// ([`await_modifiable`]).
pub async fn update(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    identifier: &str,
    properties: &Value,
) -> Result<(), Refusal> {
    let client = in_process_lambda(ctx, region);
    let held = held(&client, identifier).await.ok_or(Refusal::NotFound)?;
    let desired = DesiredFunction::read(properties, Some(&held))?;
    let configured = CONFIGURED_PROPERTIES
        .iter()
        .any(|property| properties.get(*property) != held.get(*property));
    let mut modified = false;
    if let Some(code) = desired.deployed_code() {
        client
            .update_function_code()
            .function_name(identifier)
            .set_zip_file(code.zip_file.clone())
            .set_s3_bucket(code.s3_bucket.clone())
            .set_s3_key(code.s3_key.clone())
            .set_s3_object_version(code.s3_object_version.clone())
            .set_image_uri(code.image_uri.clone())
            .set_source_kms_key_arn(code.source_kms_key_arn.clone())
            .set_architectures(desired.architectures())
            .send()
            .await
            .map_err(|error| refusal_of(error, ConflictAs::Rejected))?;
        modified = true;
    }
    if configured {
        // The code deployed just above opened AWS Lambda's own modification window, which
        // that service refuses this call inside of: the function has to be modifiable again
        // before the rest of the model can be applied to it.
        if modified {
            await_modifiable(&client, identifier).await;
        }
        client
            .update_function_configuration()
            .function_name(identifier)
            .set_role(desired.role.clone())
            .set_runtime(desired.runtime())
            .set_handler(desired.handler.clone())
            .set_description(desired.description.clone())
            .set_memory_size(desired.memory_size)
            .set_timeout(desired.timeout)
            .set_kms_key_arn(desired.kms_key_arn.clone())
            .set_layers(desired.layers.clone())
            .set_environment(desired.environment.clone())
            .set_tracing_config(desired.tracing_config.clone())
            .set_dead_letter_config(desired.dead_letter_config.clone())
            .set_ephemeral_storage(desired.ephemeral_storage.clone())
            .set_file_system_configs(desired.file_system_configs.clone())
            .set_vpc_config(desired.vpc_config.clone())
            .set_logging_config(desired.logging_config.clone())
            .set_snap_start(desired.snap_start.clone())
            .set_image_config(desired.image_config.clone())
            .send()
            .await
            .map_err(|error| refusal_of(error, ConflictAs::Rejected))?;
    }
    apply_tags(&client, &held, desired.tags.as_deref()).await
}

/// Bring the function's tag set to what the model asks for: the tags it names are set on the
/// function and the ones it no longer names are removed, each through the call that owns it
/// (`TagResource` and `UntagResource`). A model that names no tag set at all asks nothing of
/// the function's tags.
///
/// The resource those calls name is the function's own ARN, which is what the read reports
/// for it; a function whose read reported none is one no tagging call can address, so the
/// tags are left exactly as that service holds them.
async fn apply_tags(
    client: &::aws_sdk_lambda::Client,
    held: &Value,
    desired: Option<&[(String, String)]>,
) -> Result<(), Refusal> {
    let (Some(desired), Some(arn)) = (desired, held.get(ARN).and_then(Value::as_str)) else {
        return Ok(());
    };
    let current: HashMap<&str, &str> = held
        .get(TAGS)
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    let key = entry.get(TAG_KEY).and_then(Value::as_str)?;
                    let value = entry.get(TAG_VALUE).and_then(Value::as_str)?;
                    Some((key, value))
                })
                .collect()
        })
        .unwrap_or_default();
    let removed: Vec<String> = current
        .keys()
        .filter(|key| !desired.iter().any(|(name, _)| name == *key))
        .map(|key| (*key).to_string())
        .collect();
    if !removed.is_empty() {
        client
            .untag_resource()
            .resource(arn)
            .set_tag_keys(Some(removed))
            .send()
            .await
            .map_err(|error| refusal_of(error, ConflictAs::Rejected))?;
    }
    let set: Vec<&(String, String)> = desired
        .iter()
        .filter(|(key, value)| current.get(key.as_str()) != Some(&value.as_str()))
        .collect();
    if !set.is_empty() {
        let mut request = client.tag_resource().resource(arn);
        for (key, value) in set {
            request = request.tags(key, value);
        }
        request
            .send()
            .await
            .map_err(|error| refusal_of(error, ConflictAs::Rejected))?;
    }
    Ok(())
}

/// Wait until AWS Lambda reports the function modifiable again: out of the window it
/// provisions a function in and out of the window it applies a change in. Returns as soon as
/// it is, and gives up after [`MODIFIABLE_CHECKS`] checks — see that constant for why a wait
/// that runs out is not a failure of its own.
async fn await_modifiable(client: &::aws_sdk_lambda::Client, identifier: &str) {
    for _ in 0..MODIFIABLE_CHECKS {
        let Ok(reported) = client
            .get_function_configuration()
            .function_name(identifier)
            .send()
            .await
        else {
            return;
        };
        let provisioning = reported
            .state
            .as_ref()
            .is_some_and(|state| state.as_str() == STATE_PENDING);
        let updating = reported
            .last_update_status
            .as_ref()
            .is_some_and(|status| status.as_str() == LAST_UPDATE_STATUS_IN_PROGRESS);
        if !provisioning && !updating {
            return;
        }
        ::tokio::time::sleep(MODIFIABLE_CHECK_INTERVAL).await;
    }
}

/// Delete the function `identifier` names in `region`.
pub async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    identifier: &str,
) -> Result<(), Refusal> {
    in_process_lambda(ctx, region)
        .delete_function()
        .function_name(identifier)
        .send()
        .await
        .map_err(|error| refusal_of(error, ConflictAs::Rejected))
        .map(|_| ())
}

/// Where AWS Lambda's own lifecycle for the function `identifier` names has got to, for a
/// request that is following it.
///
/// That service publishes the lifecycle itself: a function it is still provisioning is
/// `State` `Pending` and one whose last change is still being applied is `LastUpdateStatus`
/// `InProgress` — in both of which it refuses calls made on the function — so a request
/// reaches `SUCCESS` only once the function is one that service takes calls on, and `FAILED`
/// where that lifecycle reports the state or the change having failed, in its own words. A
/// delete is confirmed by the function being gone.
pub async fn progress(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    identifier: &str,
    operation: &str,
) -> Progress {
    let client = in_process_lambda(ctx, region);
    if operation == crate::state::OPERATION_DELETE {
        return match held(&client, identifier).await {
            Some(_) => Progress::Running,
            None => Progress::Settled(None),
        };
    }
    let Ok(reported) = client
        .get_function()
        .function_name(identifier)
        .send()
        .await
    else {
        return Progress::Failed(Refusal::NotFound);
    };
    let Some(configuration) = reported.configuration.as_ref() else {
        return Progress::Failed(Refusal::NotFound);
    };
    let state = configuration
        .state
        .as_ref()
        .map(|state| state.as_str().to_string())
        .unwrap_or_default();
    let last_update = configuration
        .last_update_status
        .as_ref()
        .map(|status| status.as_str().to_string())
        .unwrap_or_default();
    if state == STATE_PENDING || last_update == LAST_UPDATE_STATUS_IN_PROGRESS {
        return Progress::Running;
    }
    if state == STATE_FAILED {
        return Progress::Failed(Refusal::rejected(
            configuration
                .state_reason
                .clone()
                .unwrap_or_else(|| unreported_failure(STATE_FAILED)),
        ));
    }
    if last_update == LAST_UPDATE_STATUS_FAILED {
        return Progress::Failed(Refusal::rejected(
            configuration
                .last_update_status_reason
                .clone()
                .unwrap_or_else(|| unreported_failure(LAST_UPDATE_STATUS_FAILED)),
        ));
    }
    Progress::Settled(Some(model(
        configuration,
        reported.tags.as_ref(),
        reported
            .concurrency
            .as_ref()
            .and_then(|concurrency| concurrency.reserved_concurrent_executions),
    )))
}

/// What a failed lifecycle is reported as when the owning service stated no reason for it:
/// the state it reached, so a client is told what that service reports rather than nothing.
fn unreported_failure(state: &str) -> String {
    format!("AWS Lambda reports the function {state}")
}

/// One function as the resource model this service reports: the schema properties the read
/// ANSWERS, and only those the function HAS — a property the read carries nothing for is
/// absent rather than reported empty.
///
/// The deployment package is the one property reported with no member: the read answers none
/// of the members the schema declares for it (see this module's header), and the property
/// itself is part of the model, which is what makes a patch that `add`s a member of it one
/// the document reader can apply.
fn model(
    configuration: &::aws_sdk_lambda::types::FunctionConfiguration,
    tags: Option<&HashMap<String, String>>,
    reserved_concurrent_executions: Option<i32>,
) -> Value {
    let mut model = Map::new();
    for (property, value) in [
        (IDENTIFIER_PROPERTY, configuration.function_name.clone()),
        (ARN, configuration.function_arn.clone()),
        (ROLE, configuration.role.clone()),
        (HANDLER, configuration.handler.clone()),
        (DESCRIPTION, configuration.description.clone()),
        (KMS_KEY_ARN, configuration.kms_key_arn.clone()),
        (
            RUNTIME,
            configuration
                .runtime
                .as_ref()
                .map(|runtime| runtime.as_str().to_string()),
        ),
        (
            PACKAGE_TYPE,
            configuration
                .package_type
                .as_ref()
                .map(|package| package.as_str().to_string()),
        ),
    ] {
        if let Some(value) = value {
            model.insert(property.to_string(), Value::from(value));
        }
    }
    for (property, value) in [
        (MEMORY_SIZE, configuration.memory_size),
        (TIMEOUT, configuration.timeout),
        (
            RESERVED_CONCURRENT_EXECUTIONS,
            reserved_concurrent_executions,
        ),
    ] {
        if let Some(value) = value {
            model.insert(property.to_string(), Value::from(value));
        }
    }
    if let Some(architectures) = &configuration.architectures {
        model.insert(
            ARCHITECTURES.to_string(),
            Value::Array(
                architectures
                    .iter()
                    .map(|architecture| Value::from(architecture.as_str()))
                    .collect(),
            ),
        );
    }
    if let Some(layers) = &configuration.layers {
        model.insert(
            LAYERS.to_string(),
            Value::Array(
                layers
                    .iter()
                    .filter_map(|layer| layer.arn.clone().map(Value::from))
                    .collect(),
            ),
        );
    }
    if let Some(environment) = configuration.environment.as_ref() {
        // The property is reported for the environment the FUNCTION CARRIES, which is what
        // `GetFunction` answers the member as: AWS Lambda reports the member exactly while
        // the function holds at least one variable, so the model carries the property for
        // such a function and no property at all for one holding none — this projection
        // takes the owning service's own answer rather than deciding it.
        model.insert(
            ENVIRONMENT.to_string(),
            Value::Object(Map::from_iter([(
                ENVIRONMENT_VARIABLES.to_string(),
                Value::Object(
                    environment
                        .variables
                        .as_ref()
                        .map(|variables| {
                            variables
                                .iter()
                                .map(|(name, value)| (name.clone(), Value::from(value.clone())))
                                .collect()
                        })
                        .unwrap_or_default(),
                ),
            )])),
        );
    }
    if let Some(mode) = configuration
        .tracing_config
        .as_ref()
        .and_then(|tracing| tracing.mode.as_ref())
    {
        model.insert(
            TRACING_CONFIG.to_string(),
            Value::Object(Map::from_iter([(
                TRACING_MODE.to_string(),
                Value::from(mode.as_str()),
            )])),
        );
    }
    if let Some(target) = configuration
        .dead_letter_config
        .as_ref()
        .and_then(|dead_letter| dead_letter.target_arn.clone())
    {
        model.insert(
            DEAD_LETTER_CONFIG.to_string(),
            Value::Object(Map::from_iter([(
                DEAD_LETTER_TARGET_ARN.to_string(),
                Value::from(target),
            )])),
        );
    }
    if let Some(storage) = &configuration.ephemeral_storage {
        model.insert(
            EPHEMERAL_STORAGE.to_string(),
            Value::Object(Map::from_iter([(
                EPHEMERAL_SIZE.to_string(),
                Value::from(storage.size),
            )])),
        );
    }
    if let Some(mounts) = &configuration.file_system_configs {
        model.insert(
            FILE_SYSTEM_CONFIGS.to_string(),
            Value::Array(
                mounts
                    .iter()
                    .map(|mount| {
                        Value::Object(Map::from_iter([
                            (FILE_SYSTEM_ARN.to_string(), Value::from(mount.arn.clone())),
                            (
                                FILE_SYSTEM_LOCAL_MOUNT_PATH.to_string(),
                                Value::from(mount.local_mount_path.clone()),
                            ),
                        ]))
                    })
                    .collect(),
            ),
        );
    }
    if let Some(vpc) = &configuration.vpc_config {
        let mut rendered = Map::new();
        for (member, values) in [
            (VPC_SUBNET_IDS, vpc.subnet_ids.as_ref()),
            (VPC_SECURITY_GROUP_IDS, vpc.security_group_ids.as_ref()),
        ] {
            if let Some(values) = values {
                rendered.insert(
                    member.to_string(),
                    Value::Array(values.iter().map(|value| Value::from(value.clone())).collect()),
                );
            }
        }
        if let Some(allowed) = vpc.ipv6_allowed_for_dual_stack {
            rendered.insert(VPC_IPV6_ALLOWED.to_string(), Value::from(allowed));
        }
        if !rendered.is_empty() {
            model.insert(VPC_CONFIG.to_string(), Value::Object(rendered));
        }
    }
    if let Some(logging) = &configuration.logging_config {
        let mut rendered = Map::new();
        for (member, value) in [
            (
                LOGGING_APPLICATION_LOG_LEVEL,
                logging
                    .application_log_level
                    .as_ref()
                    .map(|level| level.as_str().to_string()),
            ),
            (
                LOGGING_LOG_FORMAT,
                logging
                    .log_format
                    .as_ref()
                    .map(|format| format.as_str().to_string()),
            ),
            (LOGGING_LOG_GROUP, logging.log_group.clone()),
            (
                LOGGING_SYSTEM_LOG_LEVEL,
                logging
                    .system_log_level
                    .as_ref()
                    .map(|level| level.as_str().to_string()),
            ),
        ] {
            if let Some(value) = value {
                rendered.insert(member.to_string(), Value::from(value));
            }
        }
        if !rendered.is_empty() {
            model.insert(LOGGING_CONFIG.to_string(), Value::Object(rendered));
        }
    }
    if let Some(image) = configuration
        .image_config_response
        .as_ref()
        .and_then(|reported| reported.image_config.as_ref())
    {
        let mut rendered = Map::new();
        for (member, values) in [
            (IMAGE_COMMAND, image.command.as_ref()),
            (IMAGE_ENTRY_POINT, image.entry_point.as_ref()),
        ] {
            if let Some(values) = values {
                rendered.insert(
                    member.to_string(),
                    Value::Array(values.iter().map(|value| Value::from(value.clone())).collect()),
                );
            }
        }
        if let Some(directory) = image.working_directory.clone() {
            rendered.insert(IMAGE_WORKING_DIRECTORY.to_string(), Value::from(directory));
        }
        if !rendered.is_empty() {
            model.insert(IMAGE_CONFIG.to_string(), Value::Object(rendered));
        }
    }
    if let Some(snap_start) = &configuration.snap_start {
        let mut rendered = Map::new();
        if let Some(apply_on) = snap_start.apply_on.as_ref() {
            rendered.insert(
                SNAP_START_APPLY_ON.to_string(),
                Value::from(apply_on.as_str()),
            );
        }
        if let Some(status) = snap_start.optimization_status.as_ref() {
            rendered.insert(
                SNAP_START_OPTIMIZATION_STATUS.to_string(),
                Value::from(status.as_str()),
            );
        }
        if !rendered.is_empty() {
            model.insert(SNAP_START_RESPONSE.to_string(), Value::Object(rendered));
        }
    }
    if let Some(tags) = tags.filter(|tags| !tags.is_empty()) {
        let ordered: ::std::collections::BTreeMap<&String, &String> = tags.iter().collect();
        model.insert(
            TAGS.to_string(),
            Value::Array(
                ordered
                    .into_iter()
                    .map(|(key, value)| {
                        Value::Object(Map::from_iter([
                            (TAG_KEY.to_string(), Value::from(key.clone())),
                            (TAG_VALUE.to_string(), Value::from(value.clone())),
                        ]))
                    })
                    .collect(),
            ),
        );
    }
    // The deployment package, whose every member the read answers nothing for: the property
    // is part of the model and carries none of them.
    model.insert(CODE.to_string(), Value::Object(Map::new()));
    Value::Object(model)
}

/// What a resource model asks a function to be: the members AWS Lambda's own calls take,
/// each read off the model's properties.
#[derive(Debug, Default)]
struct DesiredFunction {
    function_name: Option<String>,
    role: Option<String>,
    runtime: Option<String>,
    handler: Option<String>,
    description: Option<String>,
    memory_size: Option<i32>,
    timeout: Option<i32>,
    kms_key_arn: Option<String>,
    package_type: Option<String>,
    architectures: Option<Vec<String>>,
    layers: Option<Vec<String>>,
    environment: Option<::aws_sdk_lambda::types::Environment>,
    tracing_config: Option<::aws_sdk_lambda::types::TracingConfig>,
    dead_letter_config: Option<::aws_sdk_lambda::types::DeadLetterConfig>,
    ephemeral_storage: Option<::aws_sdk_lambda::types::EphemeralStorage>,
    file_system_configs: Option<Vec<::aws_sdk_lambda::types::FileSystemConfig>>,
    vpc_config: Option<::aws_sdk_lambda::types::VpcConfig>,
    logging_config: Option<::aws_sdk_lambda::types::LoggingConfig>,
    snap_start: Option<::aws_sdk_lambda::types::SnapStart>,
    image_config: Option<::aws_sdk_lambda::types::ImageConfig>,
    code: Option<::aws_sdk_lambda::types::FunctionCode>,
    tags: Option<Vec<(String, String)>>,
}

impl DesiredFunction {
    /// Read `properties` as the model of a function — or state why AWS Lambda would not take
    /// it.
    ///
    /// `held` is the model the function ALREADY reports, for a model applied to a function
    /// that exists, and `None` for one that describes a function to create. Two classes of
    /// property are checked against it rather than applied: the type's read-only properties,
    /// which the owning service computes, and the properties that are a LATER CALL on the
    /// function ([`RESERVED_CONCURRENT_EXECUTIONS`] and the three beside it). A model may
    /// carry either — a read reports them, so a model a client round-trips carries them —
    /// but only with the value the function already answers with, and a create, where there
    /// is no function to hold one, may carry none at all.
    fn read(properties: &Value, held: Option<&Value>) -> Result<Self, Refusal> {
        let Some(model) = properties.as_object() else {
            return Err(Refusal::invalid(format!(
                "{TYPE_NAME} requires a resource model of its properties"
            )));
        };
        let mut desired = Self::default();
        let computed = read_only_properties();
        for (property, value) in model {
            if !DECLARED_PROPERTIES.contains(&property.as_str()) {
                return Err(Refusal::invalid(format!(
                    "{property} is not a property of {TYPE_NAME}"
                )));
            }
            if computed.contains(property) {
                if held.and_then(|held| held.get(property)) != Some(value) {
                    return Err(Refusal::invalid(format!(
                        "{property} is a read-only property of {TYPE_NAME}"
                    )));
                }
                continue;
            }
            if [
                RESERVED_CONCURRENT_EXECUTIONS,
                RECURSIVE_LOOP,
                RUNTIME_MANAGEMENT_CONFIG,
                CODE_SIGNING_CONFIG_ARN,
            ]
            .contains(&property.as_str())
            {
                if held.and_then(|held| held.get(property)) != Some(value) {
                    return Err(Refusal::invalid(format!(
                        "{property} is a property of {TYPE_NAME} this emulator's handler does \
                         not apply"
                    )));
                }
                continue;
            }
            match property.as_str() {
                IDENTIFIER_PROPERTY => desired.function_name = Some(text(property, value)?),
                ROLE => desired.role = Some(text(property, value)?),
                RUNTIME => desired.runtime = Some(text(property, value)?),
                HANDLER => desired.handler = Some(text(property, value)?),
                DESCRIPTION => desired.description = Some(text(property, value)?),
                KMS_KEY_ARN => desired.kms_key_arn = Some(text(property, value)?),
                PACKAGE_TYPE => desired.package_type = Some(text(property, value)?),
                MEMORY_SIZE => desired.memory_size = Some(count(property, value)?),
                TIMEOUT => desired.timeout = Some(count(property, value)?),
                ARCHITECTURES => desired.architectures = Some(text_list(property, value)?),
                LAYERS => desired.layers = Some(text_list(property, value)?),
                ENVIRONMENT => desired.environment = Some(environment_of(value)?),
                TRACING_CONFIG => desired.tracing_config = Some(tracing_config_of(value)?),
                DEAD_LETTER_CONFIG => {
                    desired.dead_letter_config = Some(dead_letter_config_of(value)?);
                }
                EPHEMERAL_STORAGE => desired.ephemeral_storage = Some(ephemeral_storage_of(value)?),
                FILE_SYSTEM_CONFIGS => {
                    desired.file_system_configs = Some(file_system_configs_of(value)?);
                }
                VPC_CONFIG => desired.vpc_config = Some(vpc_config_of(value)?),
                LOGGING_CONFIG => desired.logging_config = Some(logging_config_of(value)?),
                SNAP_START => desired.snap_start = Some(snap_start_of(value)?),
                IMAGE_CONFIG => desired.image_config = Some(image_config_of(value)?),
                TAGS => desired.tags = Some(tag_entries(value)?),
                // `Code` is the deployment package, whose inline form is archived under the
                // file name the function's own handler and runtime resolve — so it is read
                // last, with both of them already in hand.
                CODE => {}
                other => {
                    return Err(Refusal::invalid(format!(
                        "{other} is not a property of {TYPE_NAME}"
                    )))
                }
            }
        }
        if let Some(code) = model.get(CODE) {
            desired.code = Some(code_of(
                code,
                desired.handler.as_deref(),
                desired.runtime.as_deref(),
            )?);
        }
        Ok(desired)
    }

    /// The runtime the model names, as that service's own member carries it.
    fn runtime(&self) -> Option<::aws_sdk_lambda::types::Runtime> {
        self.runtime
            .as_deref()
            .map(::aws_sdk_lambda::types::Runtime::from)
    }

    /// The package type the model names, as that service's own member carries it.
    fn package_type(&self) -> Option<::aws_sdk_lambda::types::PackageType> {
        self.package_type
            .as_deref()
            .map(::aws_sdk_lambda::types::PackageType::from)
    }

    /// The instruction set architectures the model names, as that service's own member
    /// carries them.
    fn architectures(&self) -> Option<Vec<::aws_sdk_lambda::types::Architecture>> {
        self.architectures.as_ref().map(|architectures| {
            architectures
                .iter()
                .map(|architecture| {
                    ::aws_sdk_lambda::types::Architecture::from(architecture.as_str())
                })
                .collect()
        })
    }

    /// The package the model asks to be DEPLOYED, and `None` where it asks for none: a
    /// `Code` that states no source at all is what every read of a function reports (the
    /// type's schema declares every member of that property write-only), so a model carrying
    /// it asks for no deployment — while a create, which has to deploy one, is refused.
    fn deployed_code(&self) -> Option<::aws_sdk_lambda::types::FunctionCode> {
        self.code.clone().filter(|code| {
            code.zip_file.is_some()
                || code.s3_bucket.is_some()
                || code.s3_key.is_some()
                || code.image_uri.is_some()
        })
    }
}

/// The members `value` carries, or why it is not the structure `property` takes: a member the
/// schema does not declare for it asks for something no call applies, so it is refused rather
/// than accepted and dropped.
fn members<'a>(
    property: &str,
    value: &'a Value,
    declared: &[&str],
) -> Result<&'a Map<String, Value>, Refusal> {
    let Some(members) = value.as_object() else {
        return Err(Refusal::invalid(format!("{property} must be an object")));
    };
    if let Some(unknown) = members
        .keys()
        .find(|member| !declared.contains(&member.as_str()))
    {
        return Err(Refusal::invalid(format!(
            "{unknown} is not a member of {property} of {TYPE_NAME}"
        )));
    }
    Ok(members)
}

/// The function's environment, as `CreateFunction$Environment` carries it.
fn environment_of(value: &Value) -> Result<::aws_sdk_lambda::types::Environment, Refusal> {
    let members = members(ENVIRONMENT, value, &[ENVIRONMENT_VARIABLES])?;
    let mut environment = ::aws_sdk_lambda::types::Environment::builder();
    if let Some(variables) = members.get(ENVIRONMENT_VARIABLES) {
        let Some(variables) = variables.as_object() else {
            return Err(Refusal::invalid(format!(
                "{ENVIRONMENT_VARIABLES} of {ENVIRONMENT} must be an object of variables"
            )));
        };
        for (name, value) in variables {
            environment = environment.variables(name, text(name, value)?);
        }
    }
    Ok(environment.build())
}

/// The function's tracing configuration, as `CreateFunction$TracingConfig` carries it.
fn tracing_config_of(value: &Value) -> Result<::aws_sdk_lambda::types::TracingConfig, Refusal> {
    let members = members(TRACING_CONFIG, value, &[TRACING_MODE])?;
    let mut tracing = ::aws_sdk_lambda::types::TracingConfig::builder();
    if let Some(mode) = members.get(TRACING_MODE) {
        tracing = tracing.mode(::aws_sdk_lambda::types::TracingMode::from(
            text(TRACING_MODE, mode)?.as_str(),
        ));
    }
    Ok(tracing.build())
}

/// The queue or topic a failed asynchronous invocation is sent to, as
/// `CreateFunction$DeadLetterConfig` carries it.
fn dead_letter_config_of(
    value: &Value,
) -> Result<::aws_sdk_lambda::types::DeadLetterConfig, Refusal> {
    let members = members(DEAD_LETTER_CONFIG, value, &[DEAD_LETTER_TARGET_ARN])?;
    let mut dead_letter = ::aws_sdk_lambda::types::DeadLetterConfig::builder();
    if let Some(target) = members.get(DEAD_LETTER_TARGET_ARN) {
        dead_letter = dead_letter.target_arn(text(DEAD_LETTER_TARGET_ARN, target)?);
    }
    Ok(dead_letter.build())
}

/// The function's `/tmp` allocation, as `CreateFunction$EphemeralStorage` carries it: its one
/// member is the size, which that service requires.
fn ephemeral_storage_of(
    value: &Value,
) -> Result<::aws_sdk_lambda::types::EphemeralStorage, Refusal> {
    let members = members(EPHEMERAL_STORAGE, value, &[EPHEMERAL_SIZE])?;
    let Some(size) = members.get(EPHEMERAL_SIZE) else {
        return Err(Refusal::invalid(format!(
            "{EPHEMERAL_STORAGE} requires {EPHEMERAL_SIZE}"
        )));
    };
    ::aws_sdk_lambda::types::EphemeralStorage::builder()
        .size(count(EPHEMERAL_SIZE, size)?)
        .build()
        .map_err(|error| Refusal::invalid(format!("{EPHEMERAL_STORAGE}: {error}")))
}

/// The file systems the function mounts, as `CreateFunction$FileSystemConfigs` carries them:
/// each entry names the access point and where it is mounted, both of which that service
/// requires.
fn file_system_configs_of(
    value: &Value,
) -> Result<Vec<::aws_sdk_lambda::types::FileSystemConfig>, Refusal> {
    let Some(entries) = value.as_array() else {
        return Err(Refusal::invalid(format!(
            "{FILE_SYSTEM_CONFIGS} must be a list of mounts"
        )));
    };
    let mut mounts = Vec::with_capacity(entries.len());
    for entry in entries {
        let members = members(
            FILE_SYSTEM_CONFIGS,
            entry,
            &[FILE_SYSTEM_ARN, FILE_SYSTEM_LOCAL_MOUNT_PATH],
        )?;
        let (Some(arn), Some(path)) = (
            members.get(FILE_SYSTEM_ARN),
            members.get(FILE_SYSTEM_LOCAL_MOUNT_PATH),
        ) else {
            return Err(Refusal::invalid(format!(
                "every {FILE_SYSTEM_CONFIGS} entry must carry {FILE_SYSTEM_ARN} and \
                 {FILE_SYSTEM_LOCAL_MOUNT_PATH}"
            )));
        };
        mounts.push(
            ::aws_sdk_lambda::types::FileSystemConfig::builder()
                .arn(text(FILE_SYSTEM_ARN, arn)?)
                .local_mount_path(text(FILE_SYSTEM_LOCAL_MOUNT_PATH, path)?)
                .build()
                .map_err(|error| Refusal::invalid(format!("{FILE_SYSTEM_CONFIGS}: {error}")))?,
        );
    }
    Ok(mounts)
}

/// The VPC the function joins, as `CreateFunction$VpcConfig` carries it.
fn vpc_config_of(value: &Value) -> Result<::aws_sdk_lambda::types::VpcConfig, Refusal> {
    let members = members(
        VPC_CONFIG,
        value,
        &[VPC_SUBNET_IDS, VPC_SECURITY_GROUP_IDS, VPC_IPV6_ALLOWED],
    )?;
    let mut vpc = ::aws_sdk_lambda::types::VpcConfig::builder();
    if let Some(subnets) = members.get(VPC_SUBNET_IDS) {
        vpc = vpc.set_subnet_ids(Some(text_list(VPC_SUBNET_IDS, subnets)?));
    }
    if let Some(groups) = members.get(VPC_SECURITY_GROUP_IDS) {
        vpc = vpc.set_security_group_ids(Some(text_list(VPC_SECURITY_GROUP_IDS, groups)?));
    }
    if let Some(allowed) = members.get(VPC_IPV6_ALLOWED) {
        vpc = vpc.ipv6_allowed_for_dual_stack(flag(VPC_IPV6_ALLOWED, allowed)?);
    }
    Ok(vpc.build())
}

/// How the function's logs are delivered, as `CreateFunction$LoggingConfig` carries it.
fn logging_config_of(value: &Value) -> Result<::aws_sdk_lambda::types::LoggingConfig, Refusal> {
    let members = members(
        LOGGING_CONFIG,
        value,
        &[
            LOGGING_APPLICATION_LOG_LEVEL,
            LOGGING_LOG_FORMAT,
            LOGGING_LOG_GROUP,
            LOGGING_SYSTEM_LOG_LEVEL,
        ],
    )?;
    let mut logging = ::aws_sdk_lambda::types::LoggingConfig::builder();
    if let Some(level) = members.get(LOGGING_APPLICATION_LOG_LEVEL) {
        logging = logging.application_log_level(
            ::aws_sdk_lambda::types::ApplicationLogLevel::from(
                text(LOGGING_APPLICATION_LOG_LEVEL, level)?.as_str(),
            ),
        );
    }
    if let Some(format) = members.get(LOGGING_LOG_FORMAT) {
        logging = logging.log_format(::aws_sdk_lambda::types::LogFormat::from(
            text(LOGGING_LOG_FORMAT, format)?.as_str(),
        ));
    }
    if let Some(group) = members.get(LOGGING_LOG_GROUP) {
        logging = logging.log_group(text(LOGGING_LOG_GROUP, group)?);
    }
    if let Some(level) = members.get(LOGGING_SYSTEM_LOG_LEVEL) {
        logging = logging.system_log_level(::aws_sdk_lambda::types::SystemLogLevel::from(
            text(LOGGING_SYSTEM_LOG_LEVEL, level)?.as_str(),
        ));
    }
    Ok(logging.build())
}

/// The function's SnapStart setting, as `CreateFunction$SnapStart` carries it.
fn snap_start_of(value: &Value) -> Result<::aws_sdk_lambda::types::SnapStart, Refusal> {
    let members = members(SNAP_START, value, &[SNAP_START_APPLY_ON])?;
    let mut snap_start = ::aws_sdk_lambda::types::SnapStart::builder();
    if let Some(apply_on) = members.get(SNAP_START_APPLY_ON) {
        snap_start = snap_start.apply_on(::aws_sdk_lambda::types::SnapStartApplyOn::from(
            text(SNAP_START_APPLY_ON, apply_on)?.as_str(),
        ));
    }
    Ok(snap_start.build())
}

/// The container image overrides, as `CreateFunction$ImageConfig` carries them.
fn image_config_of(value: &Value) -> Result<::aws_sdk_lambda::types::ImageConfig, Refusal> {
    let members = members(
        IMAGE_CONFIG,
        value,
        &[IMAGE_COMMAND, IMAGE_ENTRY_POINT, IMAGE_WORKING_DIRECTORY],
    )?;
    let mut image = ::aws_sdk_lambda::types::ImageConfig::builder();
    if let Some(command) = members.get(IMAGE_COMMAND) {
        image = image.set_command(Some(text_list(IMAGE_COMMAND, command)?));
    }
    if let Some(entry_point) = members.get(IMAGE_ENTRY_POINT) {
        image = image.set_entry_point(Some(text_list(IMAGE_ENTRY_POINT, entry_point)?));
    }
    if let Some(directory) = members.get(IMAGE_WORKING_DIRECTORY) {
        image = image.working_directory(text(IMAGE_WORKING_DIRECTORY, directory)?);
    }
    Ok(image.build())
}

/// The deployment package, as `CreateFunction$Code` carries it.
///
/// Every member but one is the same on both sides and travels as written. The one that is not
/// is `ZipFile`: the schema states the function's SOURCE TEXT there, while that service's own
/// member is the BYTES of a .zip deployment package, so the stated text is archived as the
/// one file of a package under the name the function's handler resolves
/// ([`inline_package`]).
fn code_of(
    value: &Value,
    handler: Option<&str>,
    runtime: Option<&str>,
) -> Result<::aws_sdk_lambda::types::FunctionCode, Refusal> {
    let members = members(
        CODE,
        value,
        &[
            CODE_IMAGE_URI,
            CODE_S3_BUCKET,
            CODE_S3_KEY,
            CODE_S3_OBJECT_VERSION,
            CODE_SOURCE_KMS_KEY_ARN,
            CODE_ZIP_FILE,
        ],
    )?;
    let mut code = ::aws_sdk_lambda::types::FunctionCode::builder();
    if let Some(value) = members.get(CODE_IMAGE_URI) {
        code = code.image_uri(text(CODE_IMAGE_URI, value)?);
    }
    if let Some(value) = members.get(CODE_S3_BUCKET) {
        code = code.s3_bucket(text(CODE_S3_BUCKET, value)?);
    }
    if let Some(value) = members.get(CODE_S3_KEY) {
        code = code.s3_key(text(CODE_S3_KEY, value)?);
    }
    if let Some(value) = members.get(CODE_S3_OBJECT_VERSION) {
        code = code.s3_object_version(text(CODE_S3_OBJECT_VERSION, value)?);
    }
    if let Some(value) = members.get(CODE_SOURCE_KMS_KEY_ARN) {
        code = code.source_kms_key_arn(text(CODE_SOURCE_KMS_KEY_ARN, value)?);
    }
    if let Some(source) = members.get(CODE_ZIP_FILE) {
        code = code.zip_file(::aws_sdk_lambda::primitives::Blob::new(inline_package(
            &text(CODE_ZIP_FILE, source)?,
            handler.unwrap_or_default(),
            runtime.unwrap_or_default(),
        )?));
    }
    Ok(code.build())
}

/// The deployment package built for inline code: one file holding the stated source, named
/// after the handler's own file ("index" plus the runtime's extension, which is what a
/// `Handler` of `index.handler` names), in a .zip archive.
///
/// Written with the maintained `zip` implementation rather than by hand, and held entirely in
/// memory: nothing is written to disk.
fn inline_package(source: &str, handler: &str, runtime: &str) -> Result<Vec<u8>, Refusal> {
    let stem = handler.split('.').next().unwrap_or_default();
    let stem = if stem.is_empty() { "index" } else { stem };
    let file = format!("{stem}{}", source_extension(runtime));
    let mut archive = ::zip::ZipWriter::new(::std::io::Cursor::new(Vec::new()));
    let options: ::zip::write::SimpleFileOptions =
        ::zip::write::SimpleFileOptions::default().compression_method(::zip::CompressionMethod::Deflated);
    archive
        .start_file(file, options)
        .and_then(|()| archive.write_all(source.as_bytes()).map_err(Into::into))
        .and_then(|()| archive.finish().map(::std::io::Cursor::into_inner))
        .map_err(|error| {
            Refusal::invalid(format!(
                "the inline deployment package could not be archived: {error}"
            ))
        })
}

/// The file extension the source of an inline function carries, for the runtime family the
/// model names.
///
/// Source: the file each runtime loads a handler module from by DEFAULT, which is the one an
/// inline `Handler` (`index.handler`) resolves against — `index.py` for Python, `index.js`
/// for the Node.js runtimes, `index.rb` for Ruby. For Node that is the plain `.js` source
/// file, not the `.mjs` one: a Node runtime reads a `.mjs` entry as an ES module only, while
/// a `.js` entry carries either form. A runtime that cannot be stated inline at all gets no
/// extension, so the archive carries the handler's own file name.
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

/// What a `ResourceConflictException` means for the call that was refused.
///
/// That service answers it both for a function a create would duplicate and for a function
/// another operation is still being applied to, and the two are different verdicts: only the
/// create's is the resource already being there.
#[derive(Debug, Clone, Copy)]
enum ConflictAs {
    /// The function the call would create is already there.
    AlreadyExisting,
    /// That service would not take the call in the function's current state.
    Rejected,
}

/// A call AWS Lambda refused, as the refusal this service reports: the error code that
/// service NAMED decides which handler verdict the resource operation request fails with, and
/// the message it worded is the diagnostic, passed on rather than re-worded.
fn refusal_of<E>(
    error: ::aws_smithy_runtime_api::client::result::SdkError<
        E,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
    conflict: ConflictAs,
) -> Refusal
where
    ::aws_smithy_runtime_api::client::result::SdkError<
        E,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >: ::aws_smithy_types::error::metadata::ProvideErrorMetadata + ::std::fmt::Display,
{
    let (code, detail) = answered_error(error);
    match code.as_str() {
        // The modelled 404 of every read, update and delete of a function: that service
        // holds no such function.
        "ResourceNotFoundException" => Refusal::NotFound,
        // "The resource already exists, or another operation is in progress" — which of the
        // two it is follows from the call that was refused.
        "ResourceConflictException" => match conflict {
            ConflictAs::AlreadyExisting => Refusal::already_exists(detail),
            ConflictAs::Rejected => Refusal::rejected(detail),
        },
        // "The request throttling limit was exceeded", and the account's code storage quota:
        // that service has no room for the work.
        "TooManyRequestsException" | "CodeStorageExceededException" => Refusal::at_capacity(detail),
        // "One of the parameters in the request is not valid", which for this type is a fault
        // in the resource model the request carried.
        "InvalidParameterValueException" => Refusal::invalid(detail),
        _ => Refusal::rejected(detail),
    }
}

/// An AWS Lambda client whose transport is the framework's IN-PROCESS callback into this same
/// emulator — the one way this crate reaches the service that owns this type.
///
/// The callback is the framework's documented seam for calling another emulated service: it
/// dispatches to the service mounted beside this one INSIDE this process. No socket is
/// opened, no host name is resolved, nothing outside this process is reachable through it,
/// and nothing a request carries can steer it elsewhere — the endpoint URL exists because the
/// SDK requires one, and the only part of it a caller contributes is the Region token its own
/// credential scope named, which `crate::state::request_region` has already narrowed to the
/// characters a Region name is made of. The credentials are the emulator's own; the caller's
/// `Authorization` header is never forwarded.
fn in_process_lambda(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_lambda::Client {
    let config = ::aws_sdk_lambda::Config::builder()
        .behavior_version(::aws_sdk_lambda::config::BehaviorVersion::latest())
        .region(::aws_sdk_lambda::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://lambda.{region}.amazonaws.com"))
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        // The transport is in-process: nothing to retry, time out or watch for a stalled
        // stream, and each would need a sleep implementation this client is not given.
        .retry_config(::aws_sdk_lambda::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_lambda::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_lambda::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_lambda::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_lambda::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A model of a function that this reader takes, as the rows below vary it.
    fn function_model() -> Value {
        ::serde_json::json!({
            IDENTIFIER_PROPERTY: "a-function",
            ROLE: "arn:aws:iam::000000000000:role/a-role",
            RUNTIME: "python3.12",
            HANDLER: "index.handler",
            CODE: { CODE_ZIP_FILE: "def handler(event, context):\n    return {}\n" },
        })
    }

    /// The type's read-only properties are the ones no desired state may ask for: a create
    /// that names any of them describes a function AWS Lambda would not make.
    #[test]
    fn a_create_may_not_name_a_read_only_property() {
        let computed = read_only_properties();
        assert!(
            computed.contains(&ARN.to_string()),
            "expected the ARN the owning service assigns to be one it computes, got {computed:?}"
        );
        for property in &computed {
            let mut model = function_model();
            model[property.as_str()] = ::serde_json::json!("anything");
            let refusal = DesiredFunction::read(&model, None).err();
            assert_eq!(
                refusal,
                Some(Refusal::invalid(format!(
                    "{property} is a read-only property of {TYPE_NAME}"
                ))),
                "property {property:?}: expected a create naming it to be refused, got \
                 {refusal:?}"
            );
        }
    }

    /// ...and a model a client round-trips carries them with the values the function reports,
    /// which asks for nothing: such a model is taken, and what it asks of AWS Lambda is the
    /// writable members it carries.
    #[test]
    fn an_update_takes_the_read_only_values_the_read_reported() {
        let held = ::serde_json::json!({
            IDENTIFIER_PROPERTY: "a-function",
            ARN: "arn:aws:lambda:us-east-1:000000000000:function:a-function",
            ROLE: "arn:aws:iam::000000000000:role/a-role",
            TIMEOUT: 3,
        });
        let mut desired = held.clone();
        desired[TIMEOUT] = ::serde_json::json!(30);
        let read = DesiredFunction::read(&desired, Some(&held))
            .expect("a model carrying the values the read reported");
        assert_eq!(
            read.timeout,
            Some(30),
            "expected the patched timeout to be what the update asks for, got {read:?}"
        );
    }

    /// A property the type's schema does not declare asks for something no handler applies,
    /// which is refused rather than accepted and dropped.
    #[test]
    fn a_property_the_type_does_not_declare_is_refused() {
        let mut model = function_model();
        model["ThisPropertyDoesNotExist"] = ::serde_json::json!(1);
        let refusal = DesiredFunction::read(&model, None).err();
        assert_eq!(
            refusal,
            Some(Refusal::invalid(format!(
                "ThisPropertyDoesNotExist is not a property of {TYPE_NAME}"
            ))),
            "expected an undeclared property to be refused, got {refusal:?}"
        );
    }

    /// ...and so does a member the schema does not declare for one of the type's structured
    /// properties.
    #[test]
    fn a_member_a_structured_property_does_not_declare_is_refused() {
        let mut model = function_model();
        model[ENVIRONMENT] = ::serde_json::json!({ "Secrets": { "A": "b" } });
        let refusal = DesiredFunction::read(&model, None).err();
        assert_eq!(
            refusal,
            Some(Refusal::invalid(format!(
                "Secrets is not a member of {ENVIRONMENT} of {TYPE_NAME}"
            ))),
            "expected an undeclared member to be refused, got {refusal:?}"
        );
    }

    /// A property that is a LATER CALL on the function is applied by nothing here, so a model
    /// may carry it only with the value a read of the function reports — and a create, where
    /// there is no function to report one, may carry none at all.
    #[test]
    fn a_property_this_handler_does_not_apply_may_only_carry_what_the_read_reports() {
        for property in [
            RESERVED_CONCURRENT_EXECUTIONS,
            RECURSIVE_LOOP,
            RUNTIME_MANAGEMENT_CONFIG,
            CODE_SIGNING_CONFIG_ARN,
        ] {
            let mut model = function_model();
            model[property] = ::serde_json::json!(4);
            let refusal = DesiredFunction::read(&model, None).err();
            assert_eq!(
                refusal,
                Some(Refusal::invalid(format!(
                    "{property} is a property of {TYPE_NAME} this emulator's handler does not \
                     apply"
                ))),
                "property {property:?}: expected a model asking for it to be refused, got \
                 {refusal:?}"
            );
            let held = ::serde_json::json!({ property: 4 });
            assert!(
                DesiredFunction::read(&::serde_json::json!({ property: 4 }), Some(&held)).is_ok(),
                "property {property:?}: expected the value the read reports to ask for nothing"
            );
        }
    }

    /// Inline source text is archived as the ONE file of a .zip deployment package, under the
    /// name the function's own handler and runtime resolve.
    #[test]
    fn inline_source_is_archived_under_the_handlers_own_file_name() {
        for (handler, runtime, expected) in [
            ("index.handler", "python3.12", "index.py"),
            ("handler.run", "nodejs20.x", "handler.js"),
            ("index.handler", "ruby3.3", "index.rb"),
            ("", "python3.13", "index.py"),
            ("bootstrap.handler", "provided.al2023", "bootstrap"),
        ] {
            let case = format!("{handler:?} on {runtime:?}");
            let package = inline_package("the source", handler, runtime)
                .unwrap_or_else(|error| panic!("case {case}: the package must archive: {error:?}"));
            let mut archive = ::zip::ZipArchive::new(::std::io::Cursor::new(package))
                .unwrap_or_else(|error| panic!("case {case}: a .zip archive: {error:?}"));
            let names: Vec<String> = archive
                .file_names()
                .flatten()
                .map(|name| name.to_string())
                .collect();
            assert_eq!(
                names,
                vec![expected.to_string()],
                "case {case}: unexpected archive contents"
            );
            let mut entry = archive
                .by_name(expected)
                .unwrap_or_else(|error| panic!("case {case}: the entry must be there: {error:?}"));
            let mut held = String::new();
            ::std::io::Read::read_to_string(&mut entry, &mut held)
                .unwrap_or_else(|error| panic!("case {case}: the entry must read: {error:?}"));
            assert_eq!(
                held, "the source",
                "case {case}: the archive must carry the stated source"
            );
        }
    }
}
