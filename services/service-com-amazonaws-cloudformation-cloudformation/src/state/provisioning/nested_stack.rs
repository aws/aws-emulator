// SPDX-License-Identifier: CC0-1.0

//! `AWS::CloudFormation::Stack`: the NESTED STACK a stack declares, created in
//! CloudFormation itself.
//!
//! ## The rule this module implements
//!
//! A nested stack is not a record on its parent — it is a STACK. "When you create a stack
//! from a template that declares an `AWS::CloudFormation::Stack` resource, CloudFormation
//! creates a stack for it", named `<parent name>-<logical id>-<unique id>`, with its own
//! stack id, its own resources, its own events and its own status. So this provider creates
//! it the way every other provider creates its resource: through the OWNING service's own
//! create API, which for this type is CloudFormation's own `CreateStack`, called in-process
//! over the same signed callback every other provisioning call rides
//! ([`super::call_query`]). Everything that follows from a stack existing follows from that
//! one call — the child is in the stack store, so `ListStacks` and `DescribeStacks` report
//! it, its own template's resources are really created in the services that own them, and
//! its parent and root are derivable from the resource record that created it
//! ([`crate::state::stack_nesting`]).
//!
//! "Ref returns the stack ID" for this type, and `DescribeStackResource` reports that same
//! stack id as the resource's `PhysicalResourceId` — so the identity is the one
//! CloudFormation ANSWERED rather than the name the naming rules computed
//! ([`crate::state::template::identity_is_service_assigned`] names the type), and the name
//! is only what the create asked for. A caller can therefore follow the parent's resource to
//! the child stack, which is the documented way to inspect a nested stack.
//!
//! Every property the type publishes is SENT to that call ([`STACK_PROPERTIES`]): the
//! template reference, the parameters passed down, the tag set the child must carry (the
//! declaration's own plus the ones CloudFormation propagates, through the shared reading of
//! that set), the topics it notifies, and the create's own `TimeoutInMinutes`. The
//! capabilities the call is made with are the ones the PARENT's operation acknowledged, which
//! is what covers a nested stack — so a child template needing one the parent did not give is
//! refused by `CreateStack`'s own gate rather than deployed quietly.
//!
//! ## What the parent waits for
//!
//! `CreateStack` answers as soon as the stack's creation has STARTED, so the child is still
//! `CREATE_IN_PROGRESS` when the call returns: the parent's resource is held open until
//! CloudFormation reports the child settled ([`readiness`], through
//! [`super::super::stabilising`]), which is what makes a parent complete only once every
//! stack it provisions is itself complete. A child that did not come up fails the parent's
//! resource carrying what the child reported ([`embedded_failure`]).
//!
//! ## What one request may deploy
//!
//! A nested stack is the one resource whose deployment RE-ENTERS this walk, so the work one
//! caller's request can set off is bounded here, by the template's own shape rather than by
//! what the machine happens to survive:
//!
//! * the DEPTH of nesting is capped at [`MAX_NESTING_DEPTH`] and a child past it is refused,
//!   which is also what stops a template that nests ITSELF;
//! * a child whose template reference is already being deployed in the chain above it is
//!   refused as the cycle it is ([`CHAIN_HEADER`] carries that chain), so a cycle is reported
//!   rather than walked to the depth cap;
//! * each call is held to a wall-clock ceiling ([`CALL_CEILING`]), so one level cannot hold a
//!   request open indefinitely and the whole chain is bounded by that ceiling times the depth
//!   cap;
//! * the TOTAL a chain may create is bounded by the gate the re-entered handler already runs:
//!   `CreateStack` refuses past [`crate::state::MAX_LIVE_STACKS`] stacks and
//!   [`crate::state::MAX_CONCURRENT_RESOURCES`] resources across the store, which is the
//!   bound that dominates breadth as well as depth — every level's create passes through it.
//!
//! It lives in its own module beside the algorithm that drives it, reached through
//! [`super::Owner`], for the reason [`super::lambda_function`] records: the catalogue file is
//! far past the length a change to this repository may touch.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::Value;

use super::{
    call_query_carrying, xml_element, xml_member_structures, Created, Readiness, Refusal,
    Removal,
};
use crate::state::StoredResource;

/// The resource type this module provisions.
pub(super) const CLOUDFORMATION_STACK: &str = "AWS::CloudFormation::Stack";

/// The SigV4 signing name of the service that owns it — CloudFormation's own, so the
/// in-process call reaches this same emulator's `CreateStack` handler.
const CLOUDFORMATION_SIGNING_NAME: &str = "cloudformation";

/// The API version an awsQuery request of that service names beside its action, which is the
/// one this service's own model declares.
const API_VERSION: &str = "2010-05-15";

/// The actions this provider calls on the owning service.
const CREATE_STACK: &str = "CreateStack";
const UPDATE_STACK: &str = "UpdateStack";
const DELETE_STACK: &str = "DeleteStack";
const DESCRIBE_STACKS: &str = "DescribeStacks";

/// How deep a chain of nested stacks one deployment may reach: CloudFormation's own
/// documented maximum nesting depth. A child past it is refused, so the work one request sets
/// off is bounded by a stated number rather than by what the machine survives — and a template
/// that nests itself stops here.
const MAX_NESTING_DEPTH: usize = 5;

/// The header the in-process call carries the CHAIN of template references a nested deployment
/// has already reached through, outermost first: one short digest per reference
/// ([`reference_digest`]).
///
/// The chain travels with the CALL because that is where the relation is — each level is a
/// request of its own — and it carries digests rather than the references themselves, so no
/// caller-supplied text reaches a header value. A request that arrives with a chain of its own
/// can only make this provider refuse EARLIER (a longer chain, or a reference it claims is
/// already in flight): there is no value of the header that raises the depth this provider
/// will go to, so nothing a caller sends can widen what one request deploys.
const CHAIN_HEADER: &str = "x-emulator-cloudformation-nested-stack-chain";

/// How long one call on the child's behalf may take before the resource is failed for it.
///
/// The ceiling is the span this service holds any wait open for
/// ([`crate::state::MAX_HELD_WAIT`]): a nested create deploys a whole stack, which is the same
/// class of work a stabilising wait covers, and a chain is then bounded by this ceiling times
/// [`MAX_NESTING_DEPTH`] rather than by nothing at all.
const CALL_CEILING: Duration = crate::state::MAX_HELD_WAIT;

/// The property that says WHERE the child's template is: "the URL of a template that
/// specifies the stack that you want to create as a resource", which is required for the
/// type.
const TEMPLATE_URL: &str = "TemplateURL";

/// The property carrying the values the child's own parameters take: "the set of parameters
/// passed to CloudFormation when this nested stack is created", a map of parameter key to
/// value.
const PARAMETERS: &str = "Parameters";

/// The declaration's own tag set, which the child stack carries beside the ones
/// CloudFormation propagates to it. It is also the member of the child's own `DescribeStacks`
/// answer the set IN FORCE is read from, and the two members of each of its entries.
const TAGS: &str = "Tags";
const TAG_KEY: &str = "Key";
const TAG_VALUE: &str = "Value";

/// The topics the child stack publishes its own events to.
const NOTIFICATION_ARNS: &str = "NotificationARNs";

/// "The length of time, in minutes, that CloudFormation waits for the nested stack to reach
/// the CREATE_COMPLETE state" — a member of `CreateStack` and of no other stack call.
const TIMEOUT_IN_MINUTES: &str = "TimeoutInMinutes";

/// Every property `AWS::CloudFormation::Stack` publishes, from the type's own property set.
/// Each one is SENT: the reference and the parameters on every call, the tag set and the
/// notification topics on every call that takes them, and `TimeoutInMinutes` on the create
/// (the one call that has such a member).
const STACK_PROPERTIES: &[&str] = &[
    NOTIFICATION_ARNS,
    PARAMETERS,
    TAGS,
    TEMPLATE_URL,
    TIMEOUT_IN_MINUTES,
];

/// The error code CloudFormation refuses a declaration it cannot act on under, the one the
/// catalogue reports a missing or malformed property with.
const INVALID_PROPERTY_CODE: &str = "ValidationError";

// AWS's reserved tag-key namespace, which a stack operation's own `Tags` may not write into
// (`crate::state::stack_tags`): the three `aws:cloudformation:*` tags CloudFormation gives
// every resource are set from inside AWS and no public call takes them, so the child's create
// carries the stack's OWN tags and not those. The namespace itself, and the predicate over it,
// live beside this module (`super::is_reserved_tag_key`).

/// Create the nested stack this resource declares, and report the stack id CloudFormation
/// answered with as the resource's identity.
///
/// `acknowledged` are the capabilities the PARENT's own operation carried: a nested stack is
/// covered by the parent's acknowledgement — "if you have IAM resources in nested stacks, you
/// must specify `CAPABILITY_IAM`" on the parent operation — and CloudFormation does not ask a
/// second time, so the child's create is made with exactly that set and a child template
/// needing one the parent did not give is refused by `CreateStack`'s own gate.
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
    acknowledged: &[String],
) -> Result<Created, Refusal> {
    if let Some(refusal) = undeclared_property(resource) {
        return Err(refusal);
    }
    let template_url = template_url(resource)?;
    // How much nesting this deployment is already inside, and what it has already deployed
    // through: a child past the depth cap, or one whose reference the chain above it is
    // already deploying, is refused before anything is asked of anybody.
    let chain = chain_in_flight(ctx);
    if let Some(refusal) = beyond_the_bounds(&chain, &resource.logical_id, template_url) {
        return Err(refusal);
    }
    // The child is created under the name the naming rules computed for the resource —
    // `<parent name>-<logical id>-<unique id>`, which is the name CloudFormation gives a
    // nested stack — and that name is all the create asks for: the stack id is
    // CloudFormation's to mint.
    let mut parameters = vec![
        ("StackName".to_string(), resource.physical_resource_id.clone()),
        // The reference is passed THROUGH: it says where the template is, and the operation
        // that takes it is the one that reads the object and words a refusal of that read.
        (TEMPLATE_URL.to_string(), template_url.to_string()),
    ];
    parameters.extend(declared_parameters(&resource.properties));
    parameters.extend(stack_tags(&resource.properties, propagated));
    parameters.extend(notification_arns(&resource.properties));
    // The one member of the create no later call takes, so it is given where it can be: the
    // wait the declaration asks the child's creation to be held to.
    if let Some(timeout) = scalar_text(&resource.properties, TIMEOUT_IN_MINUTES) {
        parameters.push((TIMEOUT_IN_MINUTES.to_string(), timeout));
    }
    parameters.extend(capabilities(acknowledged));
    let answer = called(ctx, region, CREATE_STACK, &parameters, &chain, template_url).await?;
    let stack_id = xml_element(&answer, "StackId").ok_or_else(|| Refusal {
        code: INVALID_PROPERTY_CODE.to_string(),
        message: format!("{CREATE_STACK} identified no nested stack"),
    })?;
    Ok(Created {
        // "Ref returns the stack ID": the identity is the one CloudFormation minted, which
        // is also the handle every call on the child stack takes.
        physical_resource_id: Some(stack_id),
        attributes: BTreeMap::new(),
    })
}

/// The chain of template references the deployment that reached THIS request has already
/// deployed through, outermost first ([`CHAIN_HEADER`]), or empty for a request no nested
/// deployment led to.
fn chain_in_flight(ctx: &smithy_server_runtime::OperationContext<'_>) -> Vec<String> {
    ctx.parts()
        .headers
        .get(CHAIN_HEADER)
        .and_then(|carried| carried.to_str().ok())
        .map(|carried| {
            carried
                .split(',')
                .filter(|entry| !entry.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// `Some(refusal)` for a child the bounds of this module do not allow to be deployed: one past
/// [`MAX_NESTING_DEPTH`], and one whose own reference the chain above it is already deploying
/// (a cycle, which a template that nests itself is).
///
/// Each refusal names the DECLARATION it is about — the logical id the nested stack is declared
/// under — which is what an author has to go and look at.
fn beyond_the_bounds(
    chain: &[String],
    logical_id: &str,
    template_url: &str,
) -> Option<Refusal> {
    let digest = reference_digest(template_url);
    if chain.iter().any(|carried| carried == &digest) {
        return Some(Refusal {
            code: INVALID_PROPERTY_CODE.to_string(),
            message: format!(
                "The {TEMPLATE_URL} of resource {logical_id} is already being deployed by a \
                 stack above this one: a nested stack cannot deploy itself"
            ),
        });
    }
    (chain.len() >= MAX_NESTING_DEPTH).then(|| Refusal {
        code: INVALID_PROPERTY_CODE.to_string(),
        message: format!(
            "Nested stacks are limited to a depth of {MAX_NESTING_DEPTH}, and resource \
             {logical_id} would be at depth {}",
            chain.len() + 1
        ),
    })
}

/// One template reference as the chain carries it: a short hexadecimal digest, so the chain is
/// a fixed-shape header value whatever a template's own references spell.
fn reference_digest(template_url: &str) -> String {
    use ::sha2::{Digest, Sha256};

    /// Enough to tell this service's own references apart within one chain, which is at most
    /// [`MAX_NESTING_DEPTH`] long.
    const DIGEST_BYTES: usize = 8;

    let digest = Sha256::digest(template_url.as_bytes());
    ::hex::encode(&digest[..DIGEST_BYTES])
}

/// One call on the child's behalf: the chain this deployment is inside, extended by the
/// reference the call is about, and a wall-clock ceiling on the answer ([`CALL_CEILING`]).
///
/// A call that does not answer inside the ceiling is reported as the failure of this
/// resource — nothing waits on it further — so neither a chain of deployments nor one slow
/// level can hold the request that started it open indefinitely.
async fn called(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    action: &str,
    parameters: &[(String, String)],
    chain: &[String],
    template_url: &str,
) -> Result<String, Refusal> {
    let mut carried: Vec<String> = chain.to_vec();
    carried.push(reference_digest(template_url));
    called_carrying(ctx, region, action, parameters, &carried).await
}

/// [`called`] for a call that introduces no new template reference — a read of the child, its
/// deletion, or the tags-only update that deploys the template it already carries: the chain
/// in flight travels on unchanged, so anything the call deploys below it is held to the same
/// bounds, and the ceiling applies the same way.
async fn called_carrying(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    action: &str,
    parameters: &[(String, String)],
    chain: &[String],
) -> Result<String, Refusal> {
    let headers = vec![(CHAIN_HEADER, chain.join(","))];
    match ::tokio::time::timeout(
        CALL_CEILING,
        call_query_carrying(
            ctx,
            CLOUDFORMATION_SIGNING_NAME,
            region,
            API_VERSION,
            action,
            parameters,
            headers,
        ),
    )
    .await
    {
        Ok(answered) => answered,
        Err(_) => Err(Refusal {
            code: INVALID_PROPERTY_CODE.to_string(),
            message: format!(
                "{action} on the nested stack did not answer within {} seconds",
                CALL_CEILING.as_secs()
            ),
        }),
    }
}

/// Apply a CHANGED declaration to the nested stack that already exists, through the owning
/// service's own `UpdateStack` — the call that changes a stack that exists, under the id
/// CloudFormation knows it by, so the child keeps its identity and its resources. The members
/// that call takes are all sent: the template reference, the parameters passed down, the tag
/// set the child must carry and the topics it notifies.
///
/// A revision the child reads as no change at all is an update CloudFormation refuses ("No
/// updates are to be performed"), and that is not a failure of this resource: the child is
/// already in the state the declaration asks for, so the change is reported as applied. Every
/// other refusal is the owning service's own and fails the resource.
///
/// `Ok(None)` — the change reported as NOT applied — for a declaration that changes
/// `TimeoutInMinutes`: it is a member of `CreateStack` and of no call that changes a stack
/// that exists, so nothing here can apply it and the deployment says so rather than recording
/// a wait nothing was asked for.
pub(super) async fn update(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
    propagated: &[(String, String)],
    acknowledged: &[String],
) -> Result<Option<Created>, Refusal> {
    let asked = StoredResource {
        properties: declared.clone(),
        ..resource.clone()
    };
    if let Some(refusal) = undeclared_property(&asked) {
        return Err(refusal);
    }
    if scalar_text(&resource.properties, TIMEOUT_IN_MINUTES)
        != scalar_text(declared, TIMEOUT_IN_MINUTES)
    {
        return Ok(None);
    }
    let template_url = template_url(&asked)?;
    let chain = chain_in_flight(ctx);
    if let Some(refusal) = beyond_the_bounds(&chain, &resource.logical_id, template_url) {
        return Err(refusal);
    }
    let mut parameters = vec![
        ("StackName".to_string(), resource.physical_resource_id.clone()),
        (TEMPLATE_URL.to_string(), template_url.to_string()),
    ];
    parameters.extend(declared_parameters(declared));
    parameters.extend(stack_tags(declared, propagated));
    parameters.extend(notification_arns(declared));
    parameters.extend(capabilities(acknowledged));
    match called(ctx, region, UPDATE_STACK, &parameters, &chain, template_url).await {
        Ok(_) => Ok(Some(Created::default())),
        Err(refusal) if refusal.message == NO_UPDATES => Ok(Some(Created::default())),
        Err(refusal) => Err(refusal),
    }
}

/// Bring the tags of the nested stack back into step with the set it must carry: the
/// declaration's own `Tags` plus the ones CloudFormation propagates to it.
///
/// A stack operation's `Tags` are propagated to the resources in the stack, and a deployment
/// can state them differently with no resource's declaration changing at all — so this is
/// asked of a carried-forward nested stack as it is of every other carried-forward resource.
/// The call that writes them is the child's own `UpdateStack`, over the template it already
/// carries (`UsePreviousTemplate`), which is how a tags-only update of any stack is made.
///
/// IDEMPOTENT: the set in force is read first (`DescribeStacks`), entry by entry so a tag the
/// answer carries without a value cannot pair a later value with the wrong key
/// ([`xml_member_structures`]) — so a child whose tags already agree with it has
/// nothing written, which also keeps a deployment that changed nothing from re-deploying the
/// child's template.
///
/// BOUNDED like every other re-entry: `UsePreviousTemplate` re-deploys the template the child
/// already carries, so the walk enters every nested stack declared inside it again. The call
/// is therefore held to the SAME two bounds the create and the update are
/// ([`beyond_the_bounds`]) and made through [`called`], which extends the chain by the
/// reference it is about — so a tags-only deployment grows the chain one entry per level it
/// re-enters, is refused past [`MAX_NESTING_DEPTH`], and is refused as the cycle it is where
/// the chain above it is already deploying the same reference.
pub(super) async fn retag(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
    propagated: &[(String, String)],
) -> Result<(), Refusal> {
    let wanted = tag_set(declared, propagated);
    let template_url = template_url(resource)?;
    let chain = chain_in_flight(ctx);
    if let Some(refusal) = beyond_the_bounds(&chain, &resource.logical_id, template_url) {
        return Err(refusal);
    }
    let answer = called_carrying(
        ctx,
        region,
        DESCRIBE_STACKS,
        &[(
            "StackName".to_string(),
            resource.physical_resource_id.clone(),
        )],
        &chain,
    )
    .await?;
    let in_force: Vec<(String, String)> = xml_member_structures(&answer, TAGS)
        .into_iter()
        .filter_map(|tag| {
            let key = tag.get(TAG_KEY).and_then(Value::as_str)?.to_string();
            let value = tag.get(TAG_VALUE).and_then(Value::as_str).unwrap_or_default();
            Some((key, value.to_string()))
        })
        .collect();
    if tag_sets_agree(&in_force, &wanted) {
        return Ok(());
    }
    let mut parameters = vec![
        ("StackName".to_string(), resource.physical_resource_id.clone()),
        // The template the child already carries: a tag set is all this call changes.
        ("UsePreviousTemplate".to_string(), "true".to_string()),
    ];
    for (position, (key, value)) in wanted.iter().enumerate() {
        let position = position + 1;
        parameters.push((format!("Tags.member.{position}.Key"), key.clone()));
        parameters.push((format!("Tags.member.{position}.Value"), value.clone()));
    }
    match called(ctx, region, UPDATE_STACK, &parameters, &chain, template_url).await {
        Ok(_) => Ok(()),
        // The child already carries them after all, which is nothing to do.
        Err(refusal) if refusal.message == NO_UPDATES => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// Whether two tag sets are the same SET: read through the ONE comparison the module beside
/// this one holds ([`super::tag_sets_agree`]).
use super::tag_sets_agree;

/// CloudFormation's own refusal of an update that would change nothing, which about a NESTED
/// stack says the child is already as the declaration asks.
const NO_UPDATES: &str = "No updates are to be performed.";

/// Delete the nested stack, through the owning service's own `DeleteStack`.
///
/// A stack CloudFormation no longer holds is already in the state the delete asks for: that
/// call is answered for a stack name nothing resolves with the refusal that names it, which
/// leaves nothing to delete rather than failing the teardown.
pub(super) async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let parameters = vec![(
        "StackName".to_string(),
        resource.physical_resource_id.clone(),
    )];
    match called_carrying(ctx, region, DELETE_STACK, &parameters, &chain_in_flight(ctx)).await
    {
        Ok(_) => Ok(()),
        Err(refusal) if is_absent(&refusal) => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// Whether a refusal says CloudFormation holds no such stack, which is what makes a delete
/// nothing to do and a removal finished.
///
/// Read off the SENTENCE CloudFormation words for it, since the code every request refusal of
/// that surface carries is the same one: a stack this account and Region does not hold is
/// `Stack with id <name> does not exist`.
fn is_absent(refusal: &Refusal) -> bool {
    refusal.message.contains("does not exist")
}

/// Whether CloudFormation reports the nested stack SETTLED — the question the parent's
/// resource is held open on.
///
/// A stack's own status is what answers it: a status that is still `*_IN_PROGRESS` is a stack
/// still being worked on, a `*_COMPLETE` that is not a rollback is the stack as the
/// declaration asked for it, and a `*_FAILED` or a rollback is a child that did not come up —
/// which fails the parent's resource rather than holding it until the window runs out.
pub(super) async fn readiness(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Readiness, Refusal> {
    let (status, reason) = reported_status(ctx, region, physical_resource_id).await?;
    if status.ends_with(IN_PROGRESS_SUFFIX) {
        return Ok(Readiness::Pending(status));
    }
    if settled_as_asked(&status) {
        return Ok(Readiness::Ready);
    }
    Ok(Readiness::Failed(embedded_failure(
        physical_resource_id,
        &status,
        reason.as_deref(),
    )))
}

/// The attributes this type publishes once the nested stack is up: ONE per output the child
/// stack published, under the name `Fn::GetAtt` addresses it by.
///
/// "Fn::GetAtt … `Outputs.<NestedStackOutputName>`: returns the value of the specified output
/// of the nested stack" — so the attributes of an `AWS::CloudFormation::Stack` resource are
/// not a fixed roster of the TYPE at all: they are whatever its child template declares as
/// outputs, read under the [`OUTPUTS_ATTRIBUTE`] prefix in the dotted notation `Fn::GetAtt`
/// reads a path with. That is what lets a parent expose a child's resources — its own output
/// reading `Outputs.<Key>` of the child, or one of its resources' properties doing so.
///
/// The values are the child's OWN outputs, read through CloudFormation's own
/// `DescribeStacks` exactly as its status is ([`reported_status`]) and never decided here, so
/// what the parent publishes is what the child published. They are read only once the child
/// is settled, which is where this is asked: a stack still being worked on has not published
/// its outputs yet.
///
/// An output the child could not evaluate a value for carries no `OutputValue`, and the
/// parent publishes no attribute for it rather than an empty one — an absent value is not a
/// blank, and a declaration reading it is answered as reading an attribute nothing publishes.
pub(super) async fn ready_attributes(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<BTreeMap<String, String>, Refusal> {
    let parameters = vec![("StackName".to_string(), physical_resource_id.to_string())];
    let answer =
        called_carrying(ctx, region, DESCRIBE_STACKS, &parameters, &chain_in_flight(ctx))
            .await?;
    let mut attributes = BTreeMap::new();
    for output in xml_member_structures(&answer, OUTPUTS) {
        let Some(key) = output
            .get(OUTPUT_KEY)
            .and_then(Value::as_str)
            .filter(|key| !key.is_empty())
        else {
            continue;
        };
        let Some(value) = output.get(OUTPUT_VALUE).and_then(Value::as_str) else {
            continue;
        };
        attributes.insert(
            format!("{OUTPUTS_ATTRIBUTE}.{key}"),
            value.to_string(),
        );
    }
    Ok(attributes)
}

/// The member a stack's `DescribeStacks` answer carries its outputs under, and the two
/// members each entry of that list carries: "Output has members OutputKey … OutputValue".
const OUTPUTS: &str = "Outputs";
const OUTPUT_KEY: &str = "OutputKey";
const OUTPUT_VALUE: &str = "OutputValue";

/// The prefix the attributes of this type are addressed under —
/// `Outputs.<NestedStackOutputName>`. See [`ready_attributes`].
const OUTPUTS_ATTRIBUTE: &str = "Outputs";

/// Whether CloudFormation still holds the nested stack: the question the parent's resource
/// removal is held open on.
///
/// A stack whose deletion has completed is gone, and so is one CloudFormation no longer
/// reports at all; every other status is a deletion still under way.
pub(super) async fn removal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Removal, Refusal> {
    match reported_status(ctx, region, physical_resource_id).await {
        Ok((status, _)) if status == DELETE_COMPLETE => Ok(Removal::Gone),
        Ok((status, _)) => Ok(Removal::Pending(status)),
        Err(refusal) if is_absent(&refusal) => Ok(Removal::Gone),
        Err(refusal) => Err(refusal),
    }
}

/// The `StackStatus` and `StackStatusReason` CloudFormation reports for the nested stack,
/// read through its own `DescribeStacks` — never decided here.
async fn reported_status(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(String, Option<String>), Refusal> {
    let parameters = vec![("StackName".to_string(), physical_resource_id.to_string())];
    let answer =
        called_carrying(ctx, region, DESCRIBE_STACKS, &parameters, &chain_in_flight(ctx))
            .await?;
    let status = xml_element(&answer, "StackStatus").ok_or_else(|| Refusal {
        code: INVALID_PROPERTY_CODE.to_string(),
        message: format!("{DESCRIBE_STACKS} reported no status for {physical_resource_id}"),
    })?;
    Ok((status, xml_element(&answer, "StackStatusReason")))
}

/// The suffix of every status a stack is still being worked on in.
const IN_PROGRESS_SUFFIX: &str = "_IN_PROGRESS";

/// The status a stack whose deletion has completed reports.
const DELETE_COMPLETE: &str = "DELETE_COMPLETE";

/// Whether a settled status is the stack as the declaration asked for it — its creation, its
/// update or its import completed. Every other settled status is a stack that did not come up
/// — a failure, or a rollback that undid what was asked.
///
/// The one reading of that, shared with the change set whose execution ends in what the stack
/// operation it started ended in ([`crate::state::operation_carried_through`]), so the two
/// cannot drift when a status is added.
fn settled_as_asked(status: &str) -> bool {
    crate::state::operation_carried_through(status)
}

/// What the PARENT's resource reports for a nested stack that did not come up: the child's
/// own stack id, what CloudFormation left it in, and the reason the child itself gave.
///
/// CloudFormation reports the EMBEDDED stack's own account of the failure, because that is
/// where the detail is: the parent's resource failed because the stack it provisions did, and
/// a client reading the parent is sent to the child's own status and reason. A create that
/// did not go through is reported as the stack not having been created, and a change to a
/// stack that exists as the stack not having been updated — which is what the status it is
/// left in says.
fn embedded_failure(stack_id: &str, status: &str, reason: Option<&str>) -> String {
    let action = if status.starts_with("UPDATE") {
        "updated"
    } else {
        "created"
    };
    let stated = reason.unwrap_or_default();
    if stated.is_empty() {
        return format!("Embedded stack {stack_id} was not successfully {action}. Currently in {status}");
    }
    format!(
        "Embedded stack {stack_id} was not successfully {action}. Currently in {status} with \
         reason: {stated}"
    )
}

/// The `TemplateURL` the declaration states, or the refusal for a declaration that states
/// none: the member is required for the type, and there is no nested stack to create without
/// it.
fn template_url(resource: &StoredResource) -> Result<&str, Refusal> {
    resource
        .properties
        .get(TEMPLATE_URL)
        .and_then(Value::as_str)
        .filter(|url| !url.is_empty())
        .ok_or_else(|| Refusal {
            code: INVALID_PROPERTY_CODE.to_string(),
            message: format!(
                "Property validation failure: [Value for property {{/{TEMPLATE_URL}}} is \
                 required]"
            ),
        })
}

/// The refusal for a declaration carrying a property this type does not publish, named so the
/// author can find it — a property is never dropped, since a stack must not be reported
/// created with less than its declaration asked for.
fn undeclared_property(resource: &StoredResource) -> Option<Refusal> {
    let members = resource.properties.as_object()?;
    let undeclared = members
        .keys()
        .find(|property| !STACK_PROPERTIES.contains(&property.as_str()))?;
    Some(Refusal {
        code: INVALID_PROPERTY_CODE.to_string(),
        message: format!(
            "Property validation failure: [Encountered unsupported property {undeclared}]"
        ),
    })
}

/// The child's own parameter values as `CreateStack` takes them: one
/// `Parameters.member.<n>.ParameterKey`/`ParameterValue` pair per entry the declaration's
/// `Parameters` map carries, in KEY order so the same declaration builds the same request —
/// `serde_json`'s own map order depends on a feature any crate in the build may enable, and a
/// request this service sends must not depend on how the workspace was compiled (the same
/// reason `super::super::template::properties_blob` writes a document in key order).
///
/// A value the template left as a number or a boolean is sent as the text a parameter value
/// is — every CloudFormation parameter value is a string on the wire.
fn declared_parameters(properties: &Value) -> Vec<(String, String)> {
    let Some(declared) = properties.get(PARAMETERS).and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut keyed: Vec<(&String, &Value)> = declared.iter().collect();
    keyed.sort_by_key(|(key, _)| *key);
    let mut parameters = Vec::new();
    for (position, (key, value)) in keyed.into_iter().enumerate() {
        let position = position + 1;
        parameters.push((
            format!("{PARAMETERS}.member.{position}.ParameterKey"),
            key.clone(),
        ));
        parameters.push((
            format!("{PARAMETERS}.member.{position}.ParameterValue"),
            parameter_text(value),
        ));
    }
    parameters
}

/// The topics the child stack notifies, as the stack calls take them: one
/// `NotificationARNs.member.<n>` per entry the declaration lists, in the order it lists them.
fn notification_arns(properties: &Value) -> Vec<(String, String)> {
    let Some(declared) = properties.get(NOTIFICATION_ARNS).and_then(Value::as_array) else {
        return Vec::new();
    };
    declared
        .iter()
        .filter_map(Value::as_str)
        .enumerate()
        .map(|(position, arn)| {
            (
                format!("{NOTIFICATION_ARNS}.member.{}", position + 1),
                arn.to_string(),
            )
        })
        .collect()
}

/// The capabilities a stack call is made with: the set the PARENT's own operation carried, in
/// the order it carried them.
fn capabilities(acknowledged: &[String]) -> Vec<(String, String)> {
    acknowledged
        .iter()
        .enumerate()
        .map(|(position, capability)| {
            (
                format!("Capabilities.member.{}", position + 1),
                capability.clone(),
            )
        })
        .collect()
}

/// One scalar property as the text the wire carries, or `None` where the declaration states
/// none: a `TimeoutInMinutes` a template wrote as a number is a number in the document and a
/// text member on the wire.
fn scalar_text(properties: &Value, property: &str) -> Option<String> {
    match properties.get(property)? {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        other => Some(other.to_string()),
    }
}

/// One declared parameter value as the text the wire carries: a string as itself, every other
/// scalar as its own rendering, and a composite as the JSON it is (which is what a template
/// passing a list through to a nested stack's `CommaDelimitedList` parameter states).
fn parameter_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The tag set the child stack must carry: the ones the declaration itself asks for plus the
/// ones CloudFormation propagates to the resources it creates, through the ONE reading of that
/// set this crate holds ([`crate::state::provisioner::tags_of`]).
///
/// A key in AWS's reserved namespace is left off through the predicate the module beside this
/// one holds ([`super::is_reserved_tag_key`]): a stack operation's own `Tags` may not write
/// into it (`crate::state::stack_tags`), and the three `aws:cloudformation:*` tags AWS sets
/// from inside itself have no call here to set them through — so they are left off rather than
/// sent to be refused, since a deployment must not fail over tags CloudFormation adds of its
/// own accord.
fn tag_set(properties: &Value, propagated: &[(String, String)]) -> Vec<(String, String)> {
    let applicable = super::applicable_tags(propagated);
    crate::state::provisioner::tags_of(properties, &applicable)
        .into_iter()
        .filter(|(key, _)| !super::is_reserved_tag_key(key))
        .collect()
}

/// The tags a stack call carries, as it takes them: one `Tags.member.<n>.Key`/`Value` pair per
/// tag of [`tag_set`], in that set's own order.
fn stack_tags(properties: &Value, propagated: &[(String, String)]) -> Vec<(String, String)> {
    let mut tags = Vec::new();
    for (position, (key, value)) in tag_set(properties, propagated).into_iter().enumerate() {
        let position = position + 1;
        tags.push((format!("Tags.member.{position}.Key"), key));
        tags.push((format!("Tags.member.{position}.Value"), value));
    }
    tags
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One resource record as a declaration leaves it.
    fn declared(properties: Value) -> StoredResource {
        StoredResource {
            logical_id: "Child".to_string(),
            resource_type: CLOUDFORMATION_STACK.to_string(),
            physical_resource_id: "parent-Child-ABCDEF123456".to_string(),
            attributes: BTreeMap::new(),
            metadata: None,
            properties,
            materialised: false,
            deletion_policy: None,
            update_replace_policy: None,
        }
    }

    /// RULE: the create carries the reference the declaration states and the child's own
    /// parameter values; a declaration stating no reference, and one stating a property the
    /// type does not publish, are refused rather than sent.
    #[test]
    fn the_create_carries_what_the_declaration_states() {
        let resource = declared(::serde_json::json!({
            "TemplateURL": "https://s3.us-east-1.amazonaws.com/assets/child.json",
            "Parameters": {"Retention": 7, "Name": "data", "Enabled": true}
        }));
        assert_eq!(
            template_url(&resource).ok(),
            Some("https://s3.us-east-1.amazonaws.com/assets/child.json"),
            "case \"a stated reference\": expected the declaration's own URL"
        );
        let parameters = declared_parameters(&resource.properties);
        assert_eq!(
            parameters,
            vec![
                ("Parameters.member.1.ParameterKey".to_string(), "Enabled".to_string()),
                ("Parameters.member.1.ParameterValue".to_string(), "true".to_string()),
                ("Parameters.member.2.ParameterKey".to_string(), "Name".to_string()),
                ("Parameters.member.2.ParameterValue".to_string(), "data".to_string()),
                ("Parameters.member.3.ParameterKey".to_string(), "Retention".to_string()),
                ("Parameters.member.3.ParameterValue".to_string(), "7".to_string()),
            ],
            "case \"declared parameters\": expected one pair per entry in key order, got \
             {parameters:?}"
        );

        let no_reference = declared(::serde_json::json!({"Parameters": {}}));
        assert!(
            template_url(&no_reference).is_err(),
            "case \"no reference\": expected the declaration to be refused"
        );
        let unsupported = declared(::serde_json::json!({
            "TemplateURL": "https://s3.us-east-1.amazonaws.com/assets/child.json",
            "StackName": "chosen-by-the-author"
        }));
        assert!(
            undeclared_property(&unsupported).is_some(),
            "case \"a property the type does not publish\": expected the declaration to be \
             refused"
        );
        assert!(
            undeclared_property(&resource).is_none(),
            "case \"only published properties\": expected no refusal"
        );
    }

    /// RULE: every property the type publishes reaches the call that takes it — the
    /// declaration's own tags beside the ones CloudFormation propagates, the notification
    /// topics and the create's own timeout — and the three `aws:cloudformation:*` tags AWS
    /// sets from inside itself are left off, since no call here may carry them.
    #[test]
    fn the_create_carries_every_property_the_declaration_states() {
        let propagated = vec![
            ("owner".to_string(), "platform".to_string()),
            (
                "aws:cloudformation:stack-name".to_string(),
                "parent".to_string(),
            ),
            ("tier".to_string(), "gold".to_string()),
        ];
        // The declaration's own tag set, one key of which the stack also states (the
        // declaration is the more specific statement about this resource).
        let properties = ::serde_json::json!({
            "TemplateURL": "https://s3.us-east-1.amazonaws.com/assets/child.json",
            "Tags": [{"Key": "tier", "Value": "silver"}, {"Key": "team", "Value": "data"}],
            "NotificationARNs": ["arn:aws:sns:us-east-1:000000000000:deployments"],
            "TimeoutInMinutes": 30
        });
        let tags = stack_tags(&properties, &propagated);
        assert_eq!(
            tags,
            vec![
                ("Tags.member.1.Key".to_string(), "tier".to_string()),
                ("Tags.member.1.Value".to_string(), "silver".to_string()),
                ("Tags.member.2.Key".to_string(), "team".to_string()),
                ("Tags.member.2.Value".to_string(), "data".to_string()),
                ("Tags.member.3.Key".to_string(), "owner".to_string()),
                ("Tags.member.3.Value".to_string(), "platform".to_string()),
            ],
            "case \"a declared set beside the propagated one\": expected the declaration's own \
             tags first, the propagated keys it does not state after them, and no reserved \
             key, got {tags:?}"
        );
        let topics = notification_arns(&properties);
        assert_eq!(
            topics,
            vec![(
                "NotificationARNs.member.1".to_string(),
                "arn:aws:sns:us-east-1:000000000000:deployments".to_string()
            )],
            "case \"a declared topic\": expected it sent as the call takes it, got {topics:?}"
        );
        assert_eq!(
            scalar_text(&properties, TIMEOUT_IN_MINUTES),
            Some("30".to_string()),
            "case \"a declared timeout\": expected the number as the text the wire carries"
        );
        assert_eq!(
            scalar_text(&::serde_json::json!({}), TIMEOUT_IN_MINUTES),
            None,
            "case \"no timeout\": expected nothing sent for it"
        );
        // ...and the capabilities are the parent operation's own, in its own order.
        let acknowledged = vec!["CAPABILITY_IAM".to_string()];
        assert_eq!(
            capabilities(&acknowledged),
            vec![(
                "Capabilities.member.1".to_string(),
                "CAPABILITY_IAM".to_string()
            )],
            "case \"the parent's acknowledgement\": expected exactly the set it carried"
        );
        assert!(
            capabilities(&[]).is_empty(),
            "case \"a parent that acknowledged nothing\": expected no capability sent"
        );
    }

    /// RULE: the parent's resource is held open while the child is being worked on, ready
    /// once the child settled as asked, and failed carrying the child's own account
    /// otherwise.
    #[test]
    fn the_child_status_decides_what_the_parent_reports() {
        let rows: &[(&str, &str, bool, bool)] = &[
            ("a creation still running", "CREATE_IN_PROGRESS", false, false),
            ("a creation that completed", crate::state::status::CREATE_COMPLETE, true, false),
            ("an update that completed", crate::state::status::UPDATE_COMPLETE, true, false),
            ("an import that completed", crate::state::status::IMPORT_COMPLETE, true, false),
            ("a creation that failed", "CREATE_FAILED", false, true),
            ("a creation that rolled back", "ROLLBACK_COMPLETE", false, true),
            (
                "an update that rolled back",
                "UPDATE_ROLLBACK_COMPLETE",
                false,
                true,
            ),
        ];
        for (case, status, ready, failed) in rows {
            let pending = status.ends_with(IN_PROGRESS_SUFFIX);
            let settled = settled_as_asked(status);
            assert_eq!(
                settled, *ready,
                "case {case:?}: expected ready={ready}, got {settled}"
            );
            assert_eq!(
                !pending && !settled,
                *failed,
                "case {case:?}: expected failed={failed}"
            );
        }

        let reported = embedded_failure(
            "arn:aws:cloudformation:us-east-1:000000000000:stack/parent-Child-AB12/0-1",
            "UPDATE_ROLLBACK_COMPLETE",
            Some("The following resource(s) failed to update: [Data]."),
        );
        assert_eq!(
            reported,
            "Embedded stack arn:aws:cloudformation:us-east-1:000000000000:stack/\
             parent-Child-AB12/0-1 was not successfully updated. Currently in \
             UPDATE_ROLLBACK_COMPLETE with reason: The following resource(s) failed to \
             update: [Data].",
            "case \"a child whose update rolled back\": expected the child's own account, got \
             {reported:?}"
        );
    }
}
