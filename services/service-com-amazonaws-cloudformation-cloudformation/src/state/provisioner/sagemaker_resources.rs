// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of Amazon SageMaker AI's own control plane: the MODEL an
//! `AWS::SageMaker::Model` declares, the NOTEBOOK INSTANCE an
//! `AWS::SageMaker::NotebookInstance` declares and the LIFECYCLE CONFIGURATION an
//! `AWS::SageMaker::NotebookInstanceLifecycleConfig` declares. One file of [`super`]'s
//! per-type arms, dispatched to by its `Provisioner`, beside the model-registry arm of the
//! same service ([`super::application_services::create_model_package_group`]).
//!
//! ## Why these resources are really created
//!
//! Each of the three is addressed by its NAME in every SageMaker call that reaches it:
//! `DescribeModel`/`DeleteModel` take a `ModelName`,
//! `DescribeNotebookInstance`/`StopNotebookInstance`/`CreatePresignedNotebookInstanceUrl`
//! take a `NotebookInstanceName`, and a notebook instance's own `LifecycleConfigName` names
//! a configuration that service has to HOLD. None of those operations models a not-found
//! error: SageMaker validates the name and answers `ValidationException` for one it holds
//! nothing under. So a stack that reported one of these resources created while nothing
//! created it publishes — through `Ref`, through an output, through another resource's
//! property — a name every SageMaker call refuses, and the first call a client makes with
//! the deployed stack fails for a reason the stack never reported. Creating each resource in
//! the service that owns it is what makes the name the stack publishes one that service
//! answers for.
//!
//! ## What the declarations are
//!
//! Every property these three types publish is a member of the owning create under the SAME
//! name, so the resolved property document IS the request with the one member CloudFormation
//! decides — the resource's name, which is its physical id — set on the way out. Source: AWS's
//! CloudFormation resource reference for each type, read against that create's own member list
//! (`smithy_query(query="operation",operation="CreateModel,CreateNotebookInstance,\
//! CreateNotebookInstanceLifecycleConfig")` on SageMaker's model). The catalogues below name
//! those properties, and a declared property outside them is REFUSED
//! ([`refuse_unapplied_properties`]) rather than dropped: a resource reported created with
//! less than its declaration asked for is the shallow success these arms exist to avoid.
//!
//! Two members are not carried verbatim, because a template may state either as the TEXT a
//! parameter reference resolves to while the model types them otherwise: `VolumeSizeInGB` is
//! an integer ([`declared_number`]) and `EnableNetworkIsolation` a boolean, read by the one
//! reading this crate gives a template boolean ([`declared_flag`]). `Tags` is the third: each
//! of the three creates takes a `Tags` list, so every one of these resources carries the tags
//! CloudFormation gives it beside the declared ones ([`tags_of`]), as the `[{Key, Value}]`
//! list SageMaker's own `Tag` shape spells.
//!
//! ## What the stack waits for, and what it deletes
//!
//! `CreateNotebookInstance` answers the instance `Pending` — "SageMaker AI launches the
//! instance" after the create has answered, and the instance serves no Jupyter session and
//! mints no presigned URL in that status — so the resource's creation is held open until that
//! service reports it `InService`, and an instance it reports `Failed` fails the resource with
//! the `FailureReason` SageMaker gave rather than waiting the window out
//! ([`notebook_instance_readiness`]). A model and a lifecycle configuration exist the moment
//! their own create answers.
//!
//! Deleting a notebook instance is the mirror of that wait: "before you can delete a notebook
//! instance, you must call the StopNotebookInstance API", so the delete STOPS an instance that
//! is still running and the removal is finished once SageMaker no longer holds it
//! ([`notebook_instance_removal`], which carries out the delete the stop made possible — the
//! step a resource provider's own delete handler takes when it is re-invoked).

use super::*;

/// The `AWS::<cloudFormationName>::<Resource>` type of a SageMaker model: the inference model
/// an endpoint configuration points at. The middle segment is the SageMaker model's own
/// `awsService.cloudFormationName` (`smithy_query(query="service_info")` reports `SageMaker`),
/// the same namespace [`super::application_services`]'s model-registry type is spelled with.
pub(crate) const SAGEMAKER_MODEL: &str = "AWS::SageMaker::Model";

/// The type of a SageMaker notebook instance: the managed Jupyter instance
/// `CreateNotebookInstance` launches.
pub(crate) const SAGEMAKER_NOTEBOOK_INSTANCE: &str = "AWS::SageMaker::NotebookInstance";

/// The type of a notebook instance LIFECYCLE CONFIGURATION: the `OnCreate`/`OnStart` shell
/// scripts a notebook instance's `LifecycleConfigName` names.
pub(crate) const SAGEMAKER_NOTEBOOK_INSTANCE_LIFECYCLE_CONFIG: &str =
    "AWS::SageMaker::NotebookInstanceLifecycleConfig";

/// Every property `AWS::SageMaker::Model` publishes, each a `CreateModel` member of the same
/// name (its containers, the role the model's containers run as, how the inference pipeline
/// executes, the network-isolation switch, the model's name, its VPC configuration and its
/// tags).
const MODEL_PROPERTIES: &[&str] = &[
    "Containers",
    "EnableNetworkIsolation",
    "ExecutionRoleArn",
    "InferenceExecutionConfig",
    "ModelName",
    "PrimaryContainer",
    "Tags",
    "VpcConfig",
];

/// Every property `AWS::SageMaker::NotebookInstance` publishes, each a
/// `CreateNotebookInstance` member of the same name.
const NOTEBOOK_INSTANCE_PROPERTIES: &[&str] = &[
    "AcceleratorTypes",
    "AdditionalCodeRepositories",
    "DefaultCodeRepository",
    "DirectInternetAccess",
    "InstanceMetadataServiceConfiguration",
    "InstanceType",
    "IpAddressType",
    "KmsKeyId",
    "LifecycleConfigName",
    "NotebookInstanceName",
    "PlatformIdentifier",
    "RoleArn",
    "RootAccess",
    "SecurityGroupIds",
    "SubnetId",
    "Tags",
    "VolumeSizeInGB",
];

/// Every property `AWS::SageMaker::NotebookInstanceLifecycleConfig` publishes: its name,
/// the two hook lists and its tags, each a `CreateNotebookInstanceLifecycleConfig` member of
/// the same name.
const NOTEBOOK_INSTANCE_LIFECYCLE_CONFIG_PROPERTIES: &[&str] = &[
    "NotebookInstanceLifecycleConfigName",
    "OnCreate",
    "OnStart",
    "Tags",
];

/// The member each type is created under, which carries the name CloudFormation decided —
/// the declaration's own, or the generated one — rather than whatever the property document
/// happens to hold.
const MODEL_NAME: &str = "ModelName";
const NOTEBOOK_INSTANCE_NAME: &str = "NotebookInstanceName";
const NOTEBOOK_INSTANCE_LIFECYCLE_CONFIG_NAME: &str = "NotebookInstanceLifecycleConfigName";

/// The member a declared tag list reaches each taggable create as.
const TAGS: &str = "Tags";

/// The `NotebookInstanceStatus` an instance serves in: "InService", the status
/// `CreateNotebookInstance` leaves the instance coming up towards and the one a Jupyter
/// session and a presigned URL need (`smithy_query(query="shape",
/// shape="NotebookInstanceStatus")`).
const NOTEBOOK_INSTANCE_IN_SERVICE: &str = "InService";

/// The `NotebookInstanceStatus` a created instance is answered in: "Pending", while SageMaker
/// launches the ML compute instance for it.
const NOTEBOOK_INSTANCE_PENDING: &str = "Pending";

/// The `NotebookInstanceStatus` an instance whose provisioning could not be completed rests
/// in: "Failed", which no wait brings an instance out of.
const NOTEBOOK_INSTANCE_FAILED: &str = "Failed";

/// The `NotebookInstanceStatus` values from which an instance can be DELETED — the ones in
/// which no ML compute instance is running for it — and the one a stop is still in flight in.
const NOTEBOOK_INSTANCE_STOPPED: &str = "Stopped";
const NOTEBOOK_INSTANCE_STOPPING: &str = "Stopping";

/// The code a refusal of this module's own carries for a declaration it cannot act on: a
/// property no call it makes carries. The same code the catalogue's own arms use.
const SAGEMAKER_NOT_IMPLEMENTED: &str = "NotImplemented";

/// The code a refusal carries for a declaration whose property cannot be sent as the member
/// the owning create models.
const SAGEMAKER_INVALID_REQUEST: &str = "InvalidRequest";

/// Call one SageMaker operation on the awsJson1_1 wire its model declares.
async fn call_sagemaker(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_json_rpc(
        ctx,
        SAGEMAKER_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{SAGEMAKER_TARGET_PREFIX}.{operation}"),
        request,
    )
    .await
}

/// Create the model an `AWS::SageMaker::Model` declares, so a client that deployed the stack
/// and calls `DescribeModel` with the name the stack published reads the model back — and so
/// every endpoint configuration that names it addresses a model that exists.
///
/// The declared properties ARE `CreateModel`'s own members ([`MODEL_PROPERTIES`]), with the
/// model's NAME — this resource's physical id — set on the way out and the tag list carrying
/// the tags CloudFormation gives the resource beside the declared ones.
pub(super) async fn create_sagemaker_model(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unapplied_properties(&resource.properties, resource, MODEL_PROPERTIES)?;
    let mut request = declared_document(resource);
    request.insert(
        MODEL_NAME.to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    // "If True, no inbound or outbound network calls can be made to or from the model
    // container": the member is a boolean, so a declaration spelling it as text is sent as
    // the boolean it spells, read by the ONE reading this crate gives a template boolean
    // ([`declared_flag`]).
    if let Some(declared) = request.get("EnableNetworkIsolation").cloned() {
        let isolated = declared_flag(&request, "EnableNetworkIsolation")
            .ok_or_else(|| not_a(resource, "EnableNetworkIsolation", "boolean", &declared))?;
        request.insert(
            "EnableNetworkIsolation".to_string(),
            Value::Bool(isolated),
        );
    }
    tagged(&mut request, &resource.properties, propagated);
    call_sagemaker(ctx, region, "CreateModel", &Value::Object(request)).await?;
    // The model's own name is what `Ref` and `Fn::GetAtt ModelName` report, and the naming
    // rules already computed it: the create answers the ARN SageMaker built around that name,
    // which this type publishes no return value for.
    Ok(Created::attributes(BTreeMap::new()))
}

/// Delete the model a stack created.
///
/// `DeleteModel` models no not-found error — SageMaker validates the name and answers
/// `ValidationException`, which is also what it answers for a request it refuses for another
/// reason — so "already gone" is decided by READING the model back rather than by matching the
/// refusal's text, exactly as the model-registry arm beside this one decides it.
pub(super) async fn delete_sagemaker_model(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ MODEL_NAME: physical_resource_id });
    let refusal = match call_sagemaker(ctx, region, "DeleteModel", &request).await {
        Ok(_) => return Ok(()),
        Err(refusal) => refusal,
    };
    if absent(call_sagemaker(ctx, region, "DescribeModel", &request).await) {
        // SageMaker does not have the model, which is the state the delete asked for.
        return Ok(());
    }
    Err(refusal)
}

/// The model's configuration as SageMaker reports it, for drift detection.
///
/// `DescribeModel` answers the declaration's own members under the same names, so the read IS
/// the configuration; a model that service no longer holds answers `ValidationException`,
/// which is the `DELETED` drift status. A property the read does not report is not evidence
/// of drift ([`super::super::drift`]).
pub(super) async fn describe_sagemaker_model(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ MODEL_NAME: physical_resource_id });
    let described = match call_sagemaker(ctx, region, "DescribeModel", &request).await {
        Ok(described) => described,
        Err(refusal) if refusal.code == SAGEMAKER_VALIDATION_EXCEPTION => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    Ok(Some(reported_properties(
        &described,
        &[
            ("Containers", "Containers"),
            ("EnableNetworkIsolation", "EnableNetworkIsolation"),
            ("ExecutionRoleArn", "ExecutionRoleArn"),
            ("InferenceExecutionConfig", "InferenceExecutionConfig"),
            (MODEL_NAME, MODEL_NAME),
            ("PrimaryContainer", "PrimaryContainer"),
            ("VpcConfig", "VpcConfig"),
        ],
    )))
}

/// Create the notebook instance an `AWS::SageMaker::NotebookInstance` declares, so a client
/// that deployed the stack reads it back with `DescribeNotebookInstance` and can mint a
/// presigned URL against it — and so a declaration SageMaker will not accept (a role it
/// cannot assume, a placement it refuses) fails THAT resource with that service's own refusal
/// instead of being reported deployed.
///
/// The declared properties ARE `CreateNotebookInstance`'s own members
/// ([`NOTEBOOK_INSTANCE_PROPERTIES`]), with the instance's NAME — this resource's physical id
/// — set on the way out, the volume size sent as the integer the member models, and the tag
/// list carrying the tags CloudFormation gives the resource beside the declared ones.
pub(super) async fn create_sagemaker_notebook_instance(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unapplied_properties(&resource.properties, resource, NOTEBOOK_INSTANCE_PROPERTIES)?;
    let mut request = declared_document(resource);
    request.insert(
        NOTEBOOK_INSTANCE_NAME.to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    // "The size, in GB, of the ML storage volume to attach to the notebook instance" is an
    // integer member (`NotebookInstanceVolumeSizeInGB`, range 5..16384), so a declaration
    // spelling it as text is sent as the number it spells.
    if let Some(volume) = request.get("VolumeSizeInGB").cloned() {
        let volume = declared_number(&volume)
            .ok_or_else(|| not_a(resource, "VolumeSizeInGB", "number", &volume))?;
        request.insert("VolumeSizeInGB".to_string(), volume);
    }
    tagged(&mut request, &resource.properties, propagated);
    call_sagemaker(
        ctx,
        region,
        "CreateNotebookInstance",
        &Value::Object(request),
    )
    .await?;
    // The instance's own name is what `Ref` and `Fn::GetAtt NotebookInstanceName` report, and
    // the naming rules already computed it: the create answers the ARN SageMaker built around
    // that name, which this type publishes no return value for.
    Ok(Created::attributes(BTreeMap::new()))
}

/// Whether SageMaker reports the created notebook instance ready for a client to use.
///
/// `CreateNotebookInstance` answers the instance [`NOTEBOOK_INSTANCE_PENDING`] — SageMaker
/// creates the network interface and launches the ML compute instance after it has answered,
/// and the instance serves no Jupyter session and mints no presigned URL until it is
/// [`NOTEBOOK_INSTANCE_IN_SERVICE`] — so the stack holds the resource's creation open until
/// that service reports it there. An instance it reports [`NOTEBOOK_INSTANCE_FAILED`] is one
/// nothing is going to bring up, so the resource fails with the `FailureReason` SageMaker
/// gave; so is any other status a NEW instance does not come up from, and so is an instance
/// that service holds none of.
pub(super) async fn notebook_instance_readiness(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Readiness, Refusal> {
    let Some(described) = read_notebook_instance(ctx, region, physical_resource_id).await? else {
        return Ok(Readiness::Failed(format!(
            "Amazon SageMaker holds no notebook instance {physical_resource_id}"
        )));
    };
    let status = notebook_instance_status(&described);
    match status.as_deref() {
        Some(NOTEBOOK_INSTANCE_IN_SERVICE) => Ok(Readiness::Ready),
        Some(NOTEBOOK_INSTANCE_PENDING) => {
            Ok(Readiness::Pending(NOTEBOOK_INSTANCE_PENDING.to_string()))
        }
        Some(other) => {
            // `DescribeNotebookInstance$FailureReason` carries "if status is Failed, the
            // reason it failed"; the status speaks for itself where that service reported no
            // reason.
            let reason = described.get("FailureReason").and_then(Value::as_str);
            Ok(Readiness::Failed(match reason {
                Some(reason) => format!(
                    "Amazon SageMaker reports notebook instance {physical_resource_id} as \
                     {other}: {reason}"
                ),
                None => format!(
                    "Amazon SageMaker reports notebook instance {physical_resource_id} as \
                     {other}"
                ),
            }))
        }
        None => Ok(Readiness::Failed(format!(
            "Amazon SageMaker reports no status for notebook instance {physical_resource_id}"
        ))),
    }
}

/// Delete the notebook instance a stack created.
///
/// "Before you can delete a notebook instance, you must call the StopNotebookInstance API"
/// (`DeleteNotebookInstance`), so an instance that is serving or still coming up is STOPPED
/// here and taken away by [`notebook_instance_removal`] once that service reports it stopped
/// — the step a resource provider's delete handler takes when CloudFormation re-invokes it.
/// An instance in a status with nothing running for it is deleted straight away, and one
/// SageMaker no longer holds is already in the state the delete asks for.
pub(super) async fn delete_sagemaker_notebook_instance(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let Some(described) = read_notebook_instance(ctx, region, physical_resource_id).await? else {
        return Ok(());
    };
    match notebook_instance_status(&described).as_deref() {
        Some(NOTEBOOK_INSTANCE_IN_SERVICE | NOTEBOOK_INSTANCE_PENDING) => {
            stop_notebook_instance(ctx, region, physical_resource_id).await
        }
        Some(NOTEBOOK_INSTANCE_STOPPED | NOTEBOOK_INSTANCE_FAILED) => {
            remove_notebook_instance(ctx, region, physical_resource_id).await
        }
        // A stop is already in flight (or SageMaker is already removing the instance):
        // nothing more is asked of that service, and the removal below finishes it.
        _ => Ok(()),
    }
}

/// Whether SageMaker has finished removing a deleted notebook instance.
///
/// An instance that service no longer holds is gone. One it reports stopped — or one whose
/// provisioning failed — is the instance the delete above could not take away yet, so the
/// delete is carried out here, which is what a resource provider's delete handler does when it
/// is re-invoked; any other status means the stop it asked for is still in flight, and the
/// stack's operation stays open on it.
pub(super) async fn notebook_instance_removal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Removal, Refusal> {
    let Some(described) = read_notebook_instance(ctx, region, physical_resource_id).await? else {
        return Ok(Removal::Gone);
    };
    match notebook_instance_status(&described) {
        Some(status)
            if status == NOTEBOOK_INSTANCE_STOPPED || status == NOTEBOOK_INSTANCE_FAILED =>
        {
            remove_notebook_instance(ctx, region, physical_resource_id).await?;
            Ok(Removal::Gone)
        }
        Some(status) => Ok(Removal::Pending(status)),
        None => Ok(Removal::Pending(NOTEBOOK_INSTANCE_STOPPING.to_string())),
    }
}

/// The notebook instance's configuration as SageMaker reports it, for drift detection.
///
/// The read answers the declaration's own members under the same names, but for the
/// lifecycle configuration it names — `NotebookInstanceLifecycleConfigName` on the read,
/// `LifecycleConfigName` on the type — so the pairs are mapped one by one. An instance that
/// service no longer holds answers `ValidationException`, which is the `DELETED` drift status.
pub(super) async fn describe_sagemaker_notebook_instance(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let Some(described) = read_notebook_instance(ctx, region, physical_resource_id).await? else {
        return Ok(None);
    };
    Ok(Some(reported_properties(
        &described,
        &[
            ("DefaultCodeRepository", "DefaultCodeRepository"),
            ("DirectInternetAccess", "DirectInternetAccess"),
            (
                "InstanceMetadataServiceConfiguration",
                "InstanceMetadataServiceConfiguration",
            ),
            ("InstanceType", "InstanceType"),
            ("IpAddressType", "IpAddressType"),
            ("KmsKeyId", "KmsKeyId"),
            ("LifecycleConfigName", NOTEBOOK_INSTANCE_LIFECYCLE_CONFIG_NAME),
            (NOTEBOOK_INSTANCE_NAME, NOTEBOOK_INSTANCE_NAME),
            ("PlatformIdentifier", "PlatformIdentifier"),
            ("RoleArn", "RoleArn"),
            ("RootAccess", "RootAccess"),
            ("SubnetId", "SubnetId"),
            ("VolumeSizeInGB", "VolumeSizeInGB"),
        ],
    )))
}

/// Create the lifecycle configuration an `AWS::SageMaker::NotebookInstanceLifecycleConfig`
/// declares, so the `LifecycleConfigName` a notebook instance of the same stack names is one
/// SageMaker holds — and so the hooks it declares really run on that instance.
///
/// The declared properties ARE `CreateNotebookInstanceLifecycleConfig`'s own members
/// ([`NOTEBOOK_INSTANCE_LIFECYCLE_CONFIG_PROPERTIES`]), with the configuration's NAME — this
/// resource's physical id — set on the way out.
pub(super) async fn create_sagemaker_notebook_instance_lifecycle_config(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unapplied_properties(
        &resource.properties,
        resource,
        NOTEBOOK_INSTANCE_LIFECYCLE_CONFIG_PROPERTIES,
    )?;
    let mut request = declared_document(resource);
    request.insert(
        NOTEBOOK_INSTANCE_LIFECYCLE_CONFIG_NAME.to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    tagged(&mut request, &resource.properties, propagated);
    call_sagemaker(
        ctx,
        region,
        "CreateNotebookInstanceLifecycleConfig",
        &Value::Object(request),
    )
    .await?;
    Ok(Created::attributes(BTreeMap::new()))
}

/// Delete the lifecycle configuration a stack created. Decided the way the model's deletion
/// is: the operation models no not-found error, so "already gone" is settled by READING the
/// configuration back.
pub(super) async fn delete_sagemaker_notebook_instance_lifecycle_config(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request =
        serde_json::json!({ NOTEBOOK_INSTANCE_LIFECYCLE_CONFIG_NAME: physical_resource_id });
    let refusal = match call_sagemaker(
        ctx,
        region,
        "DeleteNotebookInstanceLifecycleConfig",
        &request,
    )
    .await
    {
        Ok(_) => return Ok(()),
        Err(refusal) => refusal,
    };
    if absent(
        call_sagemaker(
            ctx,
            region,
            "DescribeNotebookInstanceLifecycleConfig",
            &request,
        )
        .await,
    ) {
        return Ok(());
    }
    Err(refusal)
}

/// The lifecycle configuration as SageMaker reports it, for drift detection: the two hook
/// lists, which are the whole of what a declaration states besides the name. A configuration
/// that service no longer holds answers `ValidationException`, which is the `DELETED` drift
/// status.
pub(super) async fn describe_sagemaker_notebook_instance_lifecycle_config(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request =
        serde_json::json!({ NOTEBOOK_INSTANCE_LIFECYCLE_CONFIG_NAME: physical_resource_id });
    let described = match call_sagemaker(
        ctx,
        region,
        "DescribeNotebookInstanceLifecycleConfig",
        &request,
    )
    .await
    {
        Ok(described) => described,
        Err(refusal) if refusal.code == SAGEMAKER_VALIDATION_EXCEPTION => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    Ok(Some(reported_properties(
        &described,
        &[
            (
                NOTEBOOK_INSTANCE_LIFECYCLE_CONFIG_NAME,
                NOTEBOOK_INSTANCE_LIFECYCLE_CONFIG_NAME,
            ),
            ("OnCreate", "OnCreate"),
            ("OnStart", "OnStart"),
        ],
    )))
}

/// What SageMaker reports about the notebook instance `physical_resource_id`, or `None` for an
/// instance it holds none of.
///
/// The ONE read behind [`notebook_instance_readiness`],
/// [`delete_sagemaker_notebook_instance`], [`notebook_instance_removal`] and
/// [`describe_sagemaker_notebook_instance`], so all four ask that service the same question
/// and read its answer the same way. `DescribeNotebookInstance` models no not-found error, so
/// absence is its `ValidationException` — judged on the CODE, never on the message, which
/// carries the request's own name.
async fn read_notebook_instance(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ NOTEBOOK_INSTANCE_NAME: physical_resource_id });
    match call_sagemaker(ctx, region, "DescribeNotebookInstance", &request).await {
        Ok(described) => Ok(Some(described)),
        Err(refusal) if refusal.code == SAGEMAKER_VALIDATION_EXCEPTION => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The `NotebookInstanceStatus` a read reports the instance in.
fn notebook_instance_status(described: &Value) -> Option<String> {
    described
        .get("NotebookInstanceStatus")
        .and_then(Value::as_str)
        .filter(|status| !status.is_empty())
        .map(str::to_string)
}

/// Terminate the ML compute instance of a notebook instance a stack is deleting. A stop
/// SageMaker refuses because there is nothing to terminate leaves the instance in the status
/// it is in, which the removal reads: nothing about the deletion is decided from the refusal's
/// text.
async fn stop_notebook_instance(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ NOTEBOOK_INSTANCE_NAME: physical_resource_id });
    match call_sagemaker(ctx, region, "StopNotebookInstance", &request).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == SAGEMAKER_VALIDATION_EXCEPTION => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// Take the notebook instance away, now that nothing is running for it. An instance SageMaker
/// no longer holds is already in the state the delete asks for, settled by READING it back
/// because the operation models no not-found error.
async fn remove_notebook_instance(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ NOTEBOOK_INSTANCE_NAME: physical_resource_id });
    let refusal = match call_sagemaker(ctx, region, "DeleteNotebookInstance", &request).await {
        Ok(_) => return Ok(()),
        Err(refusal) => refusal,
    };
    if matches!(
        read_notebook_instance(ctx, region, physical_resource_id).await,
        Ok(None)
    ) {
        return Ok(());
    }
    Err(refusal)
}

/// Whether a read answered the `ValidationException` SageMaker refuses a name it holds nothing
/// under with, which is what says a resource is ABSENT for a type whose operations model no
/// not-found error. Any other answer — a transport fault, an internal failure — is a read that
/// did not happen and says nothing about the resource.
fn absent(read: Result<Value, Refusal>) -> bool {
    matches!(read, Err(refusal) if refusal.code == SAGEMAKER_VALIDATION_EXCEPTION)
}

/// The resolved property document of one declaration, as the request each create is built on.
fn declared_document(resource: &StoredResource) -> Map<String, Value> {
    match &resource.properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    }
}

/// Put the tags CloudFormation gives the resource on the request, as the `[{Key, Value}]` list
/// SageMaker's own `Tag` shape spells — the declared ones plus the stack's
/// ([`tags_of`]). A resource with no tag at all carries no member, so that service's own
/// validation judges the request it would judge for a client's call.
fn tagged(
    request: &mut Map<String, Value>,
    properties: &Value,
    propagated: &[(String, String)],
) {
    let tags = tags_of(properties, propagated);
    if tags.is_empty() {
        request.remove(TAGS);
    } else {
        request.insert(TAGS.to_string(), tags_as_list(&tags, UPPER_CASED_TAG));
    }
}

/// The members of a read that a declaration also states, under the PROPERTY names the type
/// spells them with, as `(property, member)` pairs. A member the read does not carry is left
/// out rather than reported empty.
fn reported_properties(described: &Value, members: &[(&str, &str)]) -> Value {
    let mut reported = Map::new();
    for (property, member) in members {
        if let Some(value) = described.get(*member).filter(|value| !value.is_null()) {
            reported.insert((*property).to_string(), value.clone());
        }
    }
    Value::Object(reported)
}

/// A declared number in either spelling a template gives one: JSON's own, or the text a
/// template resolves a parameter reference to. An integral value is carried as an integer, so
/// a member modelled as one is not handed a fractional rendering of it.
fn declared_number(value: &Value) -> Option<Value> {
    match value {
        Value::Number(_) => Some(value.clone()),
        Value::String(text) => {
            let text = text.trim();
            if let Ok(whole) = text.parse::<i64>() {
                return Some(Value::Number(whole.into()));
            }
            text.parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number)
        }
        _ => None,
    }
}

/// The refusal for a declared property whose value cannot be sent as the member the owning
/// create models, naming the property and what the member is.
fn not_a(resource: &StoredResource, property: &str, kind: &str, value: &Value) -> Refusal {
    Refusal {
        code: SAGEMAKER_INVALID_REQUEST.to_string(),
        message: format!(
            "the {property} property of {} is a {kind}, and {value} is not one",
            resource.resource_type
        ),
    }
}

/// The refusal for a declared property no call this module makes carries, naming which
/// property it was: refusing is what keeps a resource from being reported as created with
/// less than its declaration asked for.
fn refuse_unapplied_properties(
    properties: &Value,
    resource: &StoredResource,
    published: &[&str],
) -> Result<(), Refusal> {
    let Value::Object(properties) = properties else {
        return Ok(());
    };
    for property in properties.keys() {
        if published.contains(&property.as_str()) {
            continue;
        }
        return Err(Refusal {
            code: SAGEMAKER_NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not apply the {property} property of {}",
                resource.resource_type
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod sagemaker_resource_tests {
    use super::*;

    /// A `StoredResource` of one of these types, as a deployment hands it to the arms above.
    fn declared(resource_type: &str, properties: Value) -> StoredResource {
        StoredResource {
            logical_id: "Subject".to_string(),
            resource_type: resource_type.to_string(),
            physical_resource_id: "a-name".to_string(),
            attributes: BTreeMap::new(),
            metadata: None,
            properties,
            materialised: false,
            deletion_policy: None,
            update_replace_policy: None,
        }
    }

    /// Every property each type publishes is applied, and one no call here carries is
    /// refused naming it — so a declaration is never reported created with less than it
    /// asked for.
    #[test]
    fn an_unapplied_property_is_refused() {
        // (case, the type, its published properties, a declaration of all of them, a
        // property no call carries).
        let rows: &[(&str, &str, &[&str], Value, &str)] = &[
            (
                "a model",
                SAGEMAKER_MODEL,
                MODEL_PROPERTIES,
                serde_json::json!({
                    "Containers": [],
                    "EnableNetworkIsolation": false,
                    "ExecutionRoleArn": "arn:aws:iam::000000000000:role/a-role",
                    "InferenceExecutionConfig": { "Mode": "Serial" },
                    "ModelName": "a-model",
                    "PrimaryContainer": { "Image": "000000000000.dkr.ecr.us-east-1.amazonaws.com/an-image:1" },
                    "Tags": [],
                    "VpcConfig": { "SecurityGroupIds": [], "Subnets": [] },
                }),
                "ModelPackageName",
            ),
            (
                "a notebook instance",
                SAGEMAKER_NOTEBOOK_INSTANCE,
                NOTEBOOK_INSTANCE_PROPERTIES,
                serde_json::json!({
                    "AcceleratorTypes": [],
                    "AdditionalCodeRepositories": [],
                    "DefaultCodeRepository": "a-repository",
                    "DirectInternetAccess": "Enabled",
                    "InstanceMetadataServiceConfiguration": {
                        "MinimumInstanceMetadataServiceVersion": "2"
                    },
                    "InstanceType": "ml.t3.medium",
                    "IpAddressType": "ipv4",
                    "KmsKeyId": "a-key",
                    "LifecycleConfigName": "a-config",
                    "NotebookInstanceName": "a-notebook",
                    "PlatformIdentifier": "notebook-al2-v2",
                    "RoleArn": "arn:aws:iam::000000000000:role/a-role",
                    "RootAccess": "Enabled",
                    "SecurityGroupIds": [],
                    "SubnetId": "subnet-01234567",
                    "Tags": [],
                    "VolumeSizeInGB": 5,
                }),
                "InstanceMetadataServiceConfigurationVersion",
            ),
            (
                "a lifecycle configuration",
                SAGEMAKER_NOTEBOOK_INSTANCE_LIFECYCLE_CONFIG,
                NOTEBOOK_INSTANCE_LIFECYCLE_CONFIG_PROPERTIES,
                serde_json::json!({
                    "NotebookInstanceLifecycleConfigName": "a-config",
                    "OnCreate": [{ "Content": "ZWNobyBoaQ==" }],
                    "OnStart": [{ "Content": "ZWNobyBoaQ==" }],
                    "Tags": [],
                }),
                "OnStop",
            ),
        ];
        for (case, resource_type, published, declaration, unapplied) in rows {
            let resource = declared(resource_type, declaration.clone());
            assert!(
                refuse_unapplied_properties(declaration, &resource, published).is_ok(),
                "case {case:?}: expected every published property to be applied, got a refusal"
            );
            let extra = serde_json::json!({ *unapplied: "a value" });
            let refusal = refuse_unapplied_properties(&extra, &resource, published)
                .expect_err(&format!("case {case:?}: expected a refusal"));
            assert!(
                refusal.message.contains(unapplied) && refusal.message.contains(resource_type),
                "case {case:?}: expected the refusal to name the property and the type, got \
                 {:?}",
                refusal.message
            );
        }
    }

    /// A number a template writes as TEXT is the number its member models, and a value that
    /// is no number at all is none — so a declaration resolved from a parameter reaches
    /// SageMaker as the request its model takes.
    #[test]
    fn a_number_is_read_in_either_spelling() {
        let numbers: &[(&str, Value, Option<Value>)] = &[
            ("a JSON integer", serde_json::json!(5), Some(serde_json::json!(5))),
            ("the text of one", serde_json::json!("16384"), Some(serde_json::json!(16384))),
            ("text with spaces", serde_json::json!(" 5 "), Some(serde_json::json!(5))),
            ("a word", serde_json::json!("five"), None),
            ("a boolean", serde_json::json!(true), None),
        ];
        for (case, value, expected) in numbers {
            let actual = declared_number(value);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected declared_number {expected:?}, got {actual:?}"
            );
        }
    }

    /// The members a read reports reach the configuration under the PROPERTY names the type
    /// spells them with — including the lifecycle configuration, which the read and the type
    /// name differently — and a member the read does not carry is left out.
    #[test]
    fn a_read_is_reported_under_the_types_own_property_names() {
        let described = serde_json::json!({
            "NotebookInstanceName": "a-notebook",
            "InstanceType": "ml.t3.medium",
            "NotebookInstanceLifecycleConfigName": "a-config",
            "RoleArn": "arn:aws:iam::000000000000:role/a-role",
            "VolumeSizeInGB": 5,
            "SubnetId": Value::Null,
        });
        let reported = reported_properties(
            &described,
            &[
                ("InstanceType", "InstanceType"),
                ("LifecycleConfigName", NOTEBOOK_INSTANCE_LIFECYCLE_CONFIG_NAME),
                ("RoleArn", "RoleArn"),
                ("SubnetId", "SubnetId"),
                ("VolumeSizeInGB", "VolumeSizeInGB"),
            ],
        );
        let expected = serde_json::json!({
            "InstanceType": "ml.t3.medium",
            "LifecycleConfigName": "a-config",
            "RoleArn": "arn:aws:iam::000000000000:role/a-role",
            "VolumeSizeInGB": 5,
        });
        assert_eq!(
            reported, expected,
            "case \"a described notebook instance\": expected {expected:?}, got {reported:?}"
        );
    }
}
