// SPDX-License-Identifier: CC0-1.0

//! `AWS::DynamoDB::Table`: the table a stack declares, created in Amazon DynamoDB itself.
//!
//! This is the per-type half of provisioning for one resource type, the half
//! [`super::super::provisioner`] holds for every other type this emulator creates; it lives
//! in its own module beside [`super::lambda_function`] for the reason that module records —
//! that file is far past the length a change to this repository may touch — and is reached
//! through the same seam ([`super::Owner`]).
//!
//! ## Why the table is really created
//!
//! A stack's output is the hand-off between a deployment and the code that consumes it, and
//! what it hands over are the resource's own identifiers: "Ref returns the table name", and
//! `Fn::GetAtt` reads the table's `Arn` and its `StreamArn`. The stream ARN is the one
//! Amazon DynamoDB MINTED when the table's `StreamSpecification` enabled a stream — no
//! naming rule computes it, and it is the handle every DynamoDB Streams consumer
//! (`DescribeStream`, `GetShardIterator`, `GetRecords`) is called with — so the only way an
//! output over it can carry a value a client can use is for the table to exist in DynamoDB
//! and for its own answer to be what the attribute reports. That is what this arm does: the
//! create is `CreateTable`, and the attributes published for the resource are read off the
//! `TableDescription` it answered.
//!
//! ## The properties
//!
//! The type's properties are `CreateTable`'s own members, named identically on both sides,
//! with three exceptions:
//!
//! * `TableName` ADDRESSES the call rather than travelling beside the others: "Ref returns
//!   the table name", and that name is CloudFormation's to decide
//!   ([`super::super::template`]), so the create carries the resource's physical id as
//!   `TableName` and the table a client then calls `DescribeTable` with is the one the stack
//!   publishes;
//! * `SSESpecification` spells its flag `SSEEnabled`, while the API's own member is
//!   `Enabled` ([`sse_specification`]);
//! * `ResourcePolicy` is an object carrying a `PolicyDocument` in a template, while the
//!   API's `ResourcePolicy` is that document as TEXT ([`resource_policy`]).
//!
//! Every other property of the type is a DIFFERENT DynamoDB call made on the table after it
//! exists (`UpdateTimeToLive`, `UpdateContinuousBackups`, `UpdateContributorInsights`, the
//! Kinesis destination calls) or an import rather than a create
//! (`ImportSourceSpecification`). The calls are MADE, by [`settings`], on the table this
//! arm has just created, and the resource is reported created only once DynamoDB's own read
//! reports each of those settings in force ([`readiness`]) — a table configured with less
//! than its declaration asked for is the shallow success this arm exists to avoid, and a
//! declaration of one of them that this service refuses fails the resource rather than
//! leaving the property dropped. `ImportSourceSpecification` is not a property applied to a
//! created table at all but a different create, so a declaration carrying THAT is still
//! refused ([`refuse_unapplied_properties`]).
//!
//! `CreateTable` answers a table whose `TableStatus` is `CREATING` — "You can perform read
//! and write operations only on an ACTIVE table" — so the resource's creation IS held open on
//! that window ([`readiness`]): a table in it takes none of the calls the stack published it
//! for, and a client acting on the stack's output the instant the stack completes is answered
//! as if the table were not there. What ALSO holds the resource open is a declared SETTING
//! that service has not put into force yet: the declaration is not applied while it is still
//! being applied, so a client that reads the setting back the instant the stack completes is
//! answered what the template declared.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::{
    call_json_rpc, key_value_tag_list, Created, Readiness, Refusal, Removal, AWS_JSON_1_0,
};
use crate::state::provisioner::tags_of;
use crate::state::StoredResource;

/// The properties DynamoDB applies through a call of its own on the table that already
/// exists, and reports through a read of its own.
mod settings;

/// The resource type this module provisions.
pub(crate) const DYNAMODB_TABLE: &str = "AWS::DynamoDB::Table";

/// The SigV4 signing name of the service that owns it, which is what routes the in-process
/// call to Amazon DynamoDB's own handlers (`awsService.endpointPrefix`/`sigv4Name`).
const DYNAMODB_SIGNING_NAME: &str = "dynamodb";

/// The service shape an awsJson `X-Amz-Target` names the operation under
/// (`com.amazonaws.dynamodb#DynamoDB_20120810`).
const DYNAMODB_TARGET_PREFIX: &str = "DynamoDB_20120810";

/// The error code DynamoDB answers for a table it does not hold
/// (`ResourceNotFoundException`), which is what says a delete has nothing left to remove and
/// a read has no table to report.
const TABLE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The property that names the table, which addresses every call this arm makes.
const TABLE_NAME_PROPERTY: &str = "TableName";

/// The property carrying the table's own tag set, merged with the ones CloudFormation
/// propagates ([`tags_of`]).
const TAGS_PROPERTY: &str = "Tags";

/// The property carrying a resource-based policy, whose spelling differs between the
/// template and the API ([`resource_policy`]).
const RESOURCE_POLICY_PROPERTY: &str = "ResourcePolicy";

/// The property carrying the server-side-encryption settings, whose flag is spelled
/// differently on the two sides ([`sse_specification`]).
const SSE_SPECIFICATION_PROPERTY: &str = "SSESpecification";

/// The flag `AWS::DynamoDB::Table`'s `SSESpecification` spells, and the one the API's own
/// `SSESpecification` spells for the same thing.
const SSE_ENABLED_PROPERTY: &str = "SSEEnabled";
const SSE_ENABLED_MEMBER: &str = "Enabled";

/// The member a template's `ResourcePolicy` carries the policy document under.
const POLICY_DOCUMENT_PROPERTY: &str = "PolicyDocument";

/// The tag-key namespace DynamoDB reserves, compared in lower case: a tag write carrying
/// one is refused outright, whoever makes it ([`storable_tags`]).
const RESERVED_TAG_PREFIX: &str = "aws:";

/// The properties that ARE `CreateTable`'s own body members under the same name, so the
/// declared value travels into the request as written.
///
/// Source: `CreateTable`'s member list
/// (`smithy_query(query="operation",operation="CreateTable")`), intersected with the
/// properties `AWS::DynamoDB::Table` publishes.
const CREATE_MEMBERS: &[&str] = &[
    "AttributeDefinitions",
    "BillingMode",
    "DeletionProtectionEnabled",
    "GlobalSecondaryIndexes",
    "KeySchema",
    "LocalSecondaryIndexes",
    "OnDemandThroughput",
    "ProvisionedThroughput",
    "TableClass",
    "WarmThroughput",
];

/// The members `UpdateTable` takes that a declaration can change on a table that already
/// exists, under the same name on both sides.
///
/// Source: `UpdateTable`'s member list
/// (`smithy_query(query="operation",operation="UpdateTable")`). A property outside this set
/// has no in-place path — `KeySchema` and the index lists are not members of `UpdateTable`
/// at all — so a deployment that changes one is reported as not applied rather than as a
/// success that dropped the caller's change ([`update`]).
const UPDATE_MEMBERS: &[&str] = &[
    "BillingMode",
    "DeletionProtectionEnabled",
    "OnDemandThroughput",
    "ProvisionedThroughput",
    "TableClass",
    "WarmThroughput",
];

/// The property carrying the table's stream configuration, whose required flag the type does
/// not publish at all ([`stream_specification`]). A member of both `CreateTable` and
/// `UpdateTable` under this same name, which is why it is applied beside
/// [`CREATE_MEMBERS`]/[`UPDATE_MEMBERS`] rather than inside them.
const STREAM_SPECIFICATION_PROPERTY: &str = "StreamSpecification";

/// The flag the API's own `StreamSpecification` requires, which no property of the type
/// carries: "Indicates whether DynamoDB Streams is enabled (true) or disabled (false) on the
/// table" (`StreamSpecification$StreamEnabled`, `@required`).
const STREAM_ENABLED_MEMBER: &str = "StreamEnabled";

/// The member both sides spell the same way: what is written to the stream for each modified
/// item.
const STREAM_VIEW_TYPE_MEMBER: &str = "StreamViewType";

/// The member `CreateTable` and `UpdateTable` answer the table's description under.
const TABLE_DESCRIPTION_MEMBER: &str = "TableDescription";

/// The member a read answers the table's description under (`DescribeTable$Table`).
const TABLE_MEMBER: &str = "Table";

/// The `TableDescription` members the table's published `Fn::GetAtt` attributes are read
/// off, each paired with the attribute name the type publishes it as.
///
/// Source: the type's own "Return values" section — `Fn::GetAtt` reads `Arn` and
/// `StreamArn` off an `AWS::DynamoDB::Table` — and `TableDescription`'s own members
/// (`smithy_query(query="shape",shape="TableDescription")`), which is where each value
/// comes from. `StreamArn` is reported only for a table that HAS a stream: a table whose
/// declaration enabled none has no stream ARN, and an attribute nothing minted is one a
/// client must never be handed.
const PUBLISHED_ATTRIBUTES: &[(&str, &str)] = &[
    ("Arn", "TableArn"),
    ("StreamArn", "LatestStreamArn"),
];

/// The member a description reports the table's lifecycle state in, read by
/// [`await_settled`] and [`creation_window`].
const TABLE_STATUS_MEMBER: &str = "TableStatus";

/// Create the table an `AWS::DynamoDB::Table` declares, in Amazon DynamoDB itself — so a
/// client that deployed the stack reads it back with `dynamodb describe-table`, writes items
/// to it, and reaches the stream its `StreamSpecification` enabled through the DynamoDB
/// Streams endpoint.
///
/// `Err` is DynamoDB's own refusal of the create, which is what the resource fails with: a
/// declaration that names no key schema, a key schema whose attributes are not defined, a
/// name already taken, a member this emulator does not store yet.
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unapplied_properties(&resource.properties, &resource.resource_type)?;
    let mut request = Map::new();
    // The table is created under the identifier CloudFormation decided for the resource,
    // which is what `Ref` resolves to and what a client calls DynamoDB with.
    request.insert(
        TABLE_NAME_PROPERTY.to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    for member in CREATE_MEMBERS {
        if let Some(value) = declared_in(&resource.properties, member) {
            request.insert((*member).to_string(), value.clone());
        }
    }
    if let Some(specification) = stream_specification(&resource.properties) {
        request.insert(STREAM_SPECIFICATION_PROPERTY.to_string(), specification);
    }
    if let Some(specification) = sse_specification(&resource.properties) {
        request.insert(SSE_SPECIFICATION_PROPERTY.to_string(), specification);
    }
    if let Some(document) = resource_policy(&resource.properties) {
        request.insert(RESOURCE_POLICY_PROPERTY.to_string(), Value::String(document));
    }
    let tags = storable_tags(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert(TAGS_PROPERTY.to_string(), tag_list(&tags));
    }
    // Nothing is in force on a table that does not exist yet, so every declared setting is
    // applied from off — which is the state `CreateTable` leaves each of them in, since that
    // call carries none of them. No such change has to be reported as having no in-place
    // path, so the answer here is the calls themselves ([`settings::calls`]).
    let setting_calls = settings::calls(None, &resource.properties).unwrap_or_default();
    let answered = call_json_rpc(
        ctx,
        DYNAMODB_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{DYNAMODB_TARGET_PREFIX}.CreateTable"),
        &Value::Object(request),
    )
    .await?;
    if !setting_calls.is_empty() {
        // Each of these calls changes the CONFIGURATION of the table, which DynamoDB refuses
        // while the table is still being created, so they are made once that service reports
        // it settled ([`await_settled`]) — exactly as a change to the table itself is.
        await_settled(ctx, region, &resource.physical_resource_id).await;
        if let Err(refusal) =
            settings::apply(ctx, region, &resource.physical_resource_id, &setting_calls).await
        {
            // A table configured with less than its declaration asked for is not a resource
            // the stack may report as created, so the table is removed again before the
            // refusal is reported — exactly as a function whose own configuration calls were
            // refused is ([`super::lambda_function::create`]).
            let _ = delete_table(ctx, region, &resource.physical_resource_id).await;
            return Err(refusal);
        }
    }
    Ok(Created::attributes(published_attributes(
        answered.get(TABLE_DESCRIPTION_MEMBER),
    )))
}

/// Whether DynamoDB reports the table this create or change made ready.
///
/// The TABLE itself is waited for: `CreateTable` answers a table whose `TableStatus` is
/// `CREATING`, and "you can perform read and write operations only on an ACTIVE table", so a
/// table in that window takes none of the calls its declaring stack published it for — a
/// client handed the table's name or ARN by a completed stack is answered as if the table
/// were not there. That is the same case Amazon EC2's `pending` is for an instance
/// ([`super::ec2_instance::readiness`]): the stack holds the resource's creation open until
/// the owning service reports the resource out of its own creation window, so a client that
/// reads the stack as complete finds a table it can really read and write. `UPDATING` is
/// that window for a CHANGE: the declaration is not in force while that service is still
/// putting it there.
///
/// Also waited for is a SETTING the DECLARATION asked for that this arm applied through a
/// call of its own ([`settings`]): until that service's own read reports the setting in
/// force, the configuration the declaration asked for is not the configuration the table
/// has, and a resource reported created is one whose whole declared configuration a client
/// reads back. A declaration that asked for none of those settings is nothing to wait for,
/// and nothing is asked about them. `declared` is `None` where the records do not say what
/// the resource was declared with, and every setting of the type is then asked about, since
/// nothing says which of them this resource's creation owes.
///
/// A read DynamoDB refuses, and a table it reports none of, say nothing about either
/// question — one unanswered read is no evidence about the table — so neither holds the
/// resource open, exactly as [`settings::settling`] treats a refused read.
pub(super) async fn readiness(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
    declared: Option<&Value>,
) -> Readiness {
    if let Some(status) = creation_window(ctx, region, physical_resource_id).await {
        return Readiness::Pending(status);
    }
    match settings::settling(ctx, region, physical_resource_id, declared).await {
        Some(status) => Readiness::Pending(status),
        None => Readiness::Ready,
    }
}

/// The `TableStatus` DynamoDB reports for a table it has not finished creating or changing
/// ([`in_change_window`]), or `None` where it reports the table out of both windows — or
/// reports nothing this can read.
async fn creation_window(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Option<String> {
    let description = describe(ctx, region, physical_resource_id).await.ok()??;
    let status = description.get(TABLE_STATUS_MEMBER)?.as_str()?;
    in_change_window(status).then(|| status.to_string())
}

/// Apply a CHANGED declaration to the table the stack already has, through DynamoDB's own
/// `UpdateTable`.
///
/// Only the members that really CHANGED are sent: `UpdateTable` refuses a
/// `StreamSpecification` that enables a stream the table already has, so re-stating an
/// unchanged member would turn a change to one property into a refusal of the whole
/// deployment.
///
/// `Ok(None)` where the change is to a property `UpdateTable` does not take
/// ([`UPDATE_MEMBERS`]): the table keeps the configuration in force and the deployment
/// reports the change as not applied, rather than as a success that dropped it.
///
/// The table's NAME is its identity, and a declaration that changes it is a different
/// resource: [`super::super::template`] computes a new physical id for it, so the deployment
/// replaces the table rather than reaching here.
pub(super) async fn update(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    refuse_unapplied_properties(declared, &resource.resource_type)?;
    // The settings that are calls of their own: what takes each from what the table has to
    // what this declaration asks for, or `None` where one of those changes has no call of
    // this service to express it — which is reported as not applied, exactly as a change to a
    // property `UpdateTable` does not take is.
    let Some(setting_calls) = settings::calls(Some(&resource.properties), declared) else {
        return Ok(None);
    };
    let mut request = Map::new();
    for member in UPDATE_MEMBERS {
        let asked = declared_in(declared, member);
        if asked == declared_in(&resource.properties, member) {
            continue;
        }
        let Some(value) = asked else {
            // A member the declaration DROPPED is not a member `UpdateTable` can take back
            // to its default: there is no "unset" on that call, so the change has no
            // in-place path.
            return Ok(None);
        };
        request.insert((*member).to_string(), value.clone());
    }
    // The stream configuration travels as the API's own `StreamSpecification`, whose required
    // flag the type does not publish ([`stream_specification`]). It is a member `UpdateTable`
    // takes, so a declaration that changes it has an in-place path; a declaration that DROPPED
    // it has none here, for the reason the loop above gives for every other dropped member.
    let stream_in_force = declared_in(&resource.properties, STREAM_SPECIFICATION_PROPERTY);
    let stream_declared = declared_in(declared, STREAM_SPECIFICATION_PROPERTY);
    if stream_declared != stream_in_force {
        match stream_specification(declared) {
            Some(specification) => {
                request.insert(STREAM_SPECIFICATION_PROPERTY.to_string(), specification);
            }
            None => return Ok(None),
        }
    }
    if changed_outside_update_members(&resource.properties, declared) {
        return Ok(None);
    }
    if request.is_empty() && setting_calls.is_empty() {
        // Nothing `UpdateTable` takes changed, no setting of its own changed, and nothing
        // outside either did, so the table already has the declared configuration.
        return Ok(Some(Created::default()));
    }
    // Every change below is made once DynamoDB reports the table out of the window it
    // refuses one in ([`await_settled`]), exactly as the delete is.
    await_settled(ctx, region, &resource.physical_resource_id).await;
    // The settings first: each of their calls leaves the TABLE itself untouched, while
    // `UpdateTable` leaves it in the window this service refuses a further change in.
    settings::apply(ctx, region, &resource.physical_resource_id, &setting_calls).await?;
    if request.is_empty() {
        // Only settings changed, and they are applied: the table itself keeps the
        // configuration in force.
        return Ok(Some(Created::default()));
    }
    request.insert(
        TABLE_NAME_PROPERTY.to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    let answered = call_json_rpc(
        ctx,
        DYNAMODB_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{DYNAMODB_TARGET_PREFIX}.UpdateTable"),
        &Value::Object(request),
    )
    .await?;
    Ok(Some(Created::attributes(published_attributes(
        answered.get(TABLE_DESCRIPTION_MEMBER),
    ))))
}

/// Delete the table a stack created, addressed by the name its physical id carries.
///
/// The delete is made once DynamoDB reports the table out of the window it refuses a delete
/// in ([`await_settled`]): "tried to delete a table currently in the CREATING state" is a
/// `ResourceInUseException`, so a deployment deleting a table its own create had just made
/// would fail a resource it was perfectly able to delete.
///
/// A table DynamoDB no longer holds is already in the state the delete asks for, judged on
/// the CODE and never on a message, which carries the request's own table name.
pub(super) async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    delete_table(ctx, region, &resource.physical_resource_id).await
}

/// Delete the table `table_name`: what the stack's own deletion owes, and what a create
/// whose declared settings this service refused undoes before it reports that refusal.
async fn delete_table(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    table_name: &str,
) -> Result<(), Refusal> {
    await_settled(ctx, region, table_name).await;
    let request = addressed(table_name);
    match call_json_rpc(
        ctx,
        DYNAMODB_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{DYNAMODB_TARGET_PREFIX}.DeleteTable"),
        &request,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == TABLE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// Whether DynamoDB has finished removing the table a stack deleted.
///
/// Nothing is asked, and nothing waits: `DeleteTable` takes the table away as it answers —
/// "DynamoDB immediately returns a response with a TableStatus of DELETING", and from that
/// moment the table takes no further call and the name is free again, so the stack's
/// resource is already gone. That is the OPPOSITE of the `CREATING` window [`readiness`]
/// holds a create open on: there the table exists and serves nothing yet, here it serves
/// nothing and is no longer there to wait for.
pub(super) fn removal() -> Removal {
    Removal::Gone
}

/// The `TableStatus` values DynamoDB refuses a further change of the table in: "attempt to
/// recreate an existing table, or tried to delete a table currently in the CREATING state"
/// is its `ResourceInUseException`, and the same holds while an earlier change is still
/// being applied. They are the same windows a table serves no item call in ("You can perform
/// read and write operations only on an ACTIVE table"), which is why [`readiness`] holds the
/// resource open on them as well.
const TABLE_STATUS_CREATING: &str = "CREATING";
const TABLE_STATUS_UPDATING: &str = "UPDATING";

/// Whether `status` is one of those windows — the ONE reading of a `TableStatus` in this
/// module, so the wait before a further call ([`await_settled`]) and the readiness of a
/// created or changed resource ([`creation_window`]) cannot come to disagree about which
/// statuses they are.
fn in_change_window(status: &str) -> bool {
    status == TABLE_STATUS_CREATING || status == TABLE_STATUS_UPDATING
}

/// How long [`await_settled`] waits between asking DynamoDB whether the table is out of
/// those windows, and how many times it asks.
///
/// The interval is short enough that a settled table is noticed about as soon as it settles,
/// and the budget covers the span DynamoDB itself holds a new table `CREATING` for several
/// times over, so a loaded machine does not turn the wait into a failure. A wait that runs
/// out is not a failure of its own: what fails then is the call that needed the table
/// settled, reported as DynamoDB's own refusal of it.
const SETTLED_CHECK_INTERVAL: ::std::time::Duration = ::std::time::Duration::from_millis(25);
const SETTLED_CHECKS: usize = 200;

/// Wait until DynamoDB reports the table out of the windows it refuses a change in
/// ([`TABLE_STATUS_CREATING`], [`TABLE_STATUS_UPDATING`]), which is what the next call on it
/// needs.
///
/// Returns as soon as the table is settled, and gives up after [`SETTLED_CHECKS`] checks. A
/// refusal of the READ ends the wait the same way: it says nothing about whether the table
/// is settled, and the call that follows is what reports DynamoDB's answer.
async fn await_settled(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) {
    for _ in 0..SETTLED_CHECKS {
        let Ok(described) = describe(ctx, region, physical_resource_id).await else {
            return;
        };
        let Some(description) = described else {
            // DynamoDB holds no such table, so there is nothing to wait for.
            return;
        };
        let status = description
            .get(TABLE_STATUS_MEMBER)
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !in_change_window(status) {
            return;
        }
        ::tokio::time::sleep(SETTLED_CHECK_INTERVAL).await;
    }
}

/// The table's description as DynamoDB reports it, or `None` where that service holds no
/// such table.
async fn describe(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = addressed(physical_resource_id);
    match call_json_rpc(
        ctx,
        DYNAMODB_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{DYNAMODB_TARGET_PREFIX}.DescribeTable"),
        &request,
    )
    .await
    {
        Ok(answered) => Ok(answered.get(TABLE_MEMBER).cloned()),
        Err(refusal) if refusal.code == TABLE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// A request that only ADDRESSES one table: the single `TableName` member `DeleteTable` and
/// `DescribeTable` take.
fn addressed(physical_resource_id: &str) -> Value {
    let mut request = Map::new();
    request.insert(
        TABLE_NAME_PROPERTY.to_string(),
        Value::String(physical_resource_id.to_string()),
    );
    Value::Object(request)
}

/// The `Fn::GetAtt` attributes the type publishes, read off the `TableDescription` DynamoDB
/// answered ([`PUBLISHED_ATTRIBUTES`]).
///
/// An attribute the description does not carry is LEFT OUT rather than reported empty: a
/// table whose declaration enabled no stream has no stream ARN, and the resource keeps
/// whatever the naming rules computed for the names it does publish.
fn published_attributes(description: Option<&Value>) -> BTreeMap<String, String> {
    let mut attributes = BTreeMap::new();
    let Some(description) = description else {
        return attributes;
    };
    for (attribute, member) in PUBLISHED_ATTRIBUTES {
        if let Some(value) = description
            .get(*member)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            attributes.insert((*attribute).to_string(), value.to_string());
        }
    }
    attributes
}

/// One declared property, or `None` where the declaration states none or states it null.
fn declared_in<'p>(properties: &'p Value, property: &str) -> Option<&'p Value> {
    properties
        .get(property)
        .filter(|value| !value.is_null())
}

/// The API's `StreamSpecification` for the property the template declares, or `None` where it
/// declares none.
///
/// The type publishes `StreamViewType` and a `ResourcePolicy` for the stream, and no flag of
/// its own: declaring the property IS asking for a stream, while the API's own structure
/// REQUIRES `StreamEnabled` ("Member must not be null" is how `CreateTable` refuses a request
/// that leaves it out). So the flag the declaration means — enabled — is what CloudFormation
/// supplies, a declaration that states the flag itself keeps the value it states, and the view
/// type travels as written for DynamoDB to judge against its own enum.
fn stream_specification(properties: &Value) -> Option<Value> {
    let declared = declared_in(properties, STREAM_SPECIFICATION_PROPERTY)?;
    let Value::Object(members) = declared else {
        // Not an object: handed over as written, for DynamoDB to judge.
        return Some(declared.clone());
    };
    let mut specification = Map::new();
    specification.insert(
        STREAM_ENABLED_MEMBER.to_string(),
        members
            .get(STREAM_ENABLED_MEMBER)
            .cloned()
            .unwrap_or(Value::Bool(true)),
    );
    if let Some(view_type) = members.get(STREAM_VIEW_TYPE_MEMBER) {
        specification.insert(STREAM_VIEW_TYPE_MEMBER.to_string(), view_type.clone());
    }
    Some(Value::Object(specification))
}

/// The API's `SSESpecification` for the property the template declares, or `None` where it
/// declares none.
///
/// The two sides spell the flag differently — the template's property is `SSEEnabled` and
/// the API's member is `Enabled` — so it is renamed; `SSEType` and `KMSMasterKeyId` are
/// spelled the same way on both and travel as written.
fn sse_specification(properties: &Value) -> Option<Value> {
    let declared = declared_in(properties, SSE_SPECIFICATION_PROPERTY)?;
    let Value::Object(members) = declared else {
        // Not an object: handed over as written, for DynamoDB to judge.
        return Some(declared.clone());
    };
    let mut specification = Map::new();
    for (property, value) in members {
        let member = if property == SSE_ENABLED_PROPERTY {
            SSE_ENABLED_MEMBER
        } else {
            property.as_str()
        };
        specification.insert(member.to_string(), value.clone());
    }
    Some(Value::Object(specification))
}

/// The policy document a declared `ResourcePolicy` carries, as the TEXT `CreateTable`'s own
/// `ResourcePolicy` member takes.
///
/// "ResourcePolicy … PolicyDocument" is an object in a template, while the API member is
/// "an Amazon Web Services resource-based policy document in JSON format" — a string. A
/// document already written as text travels as written, so a template that states the policy
/// as a JSON string is not re-encoded.
fn resource_policy(properties: &Value) -> Option<String> {
    let declared = declared_in(properties, RESOURCE_POLICY_PROPERTY)?;
    let document = declared.get(POLICY_DOCUMENT_PROPERTY).unwrap_or(declared);
    match document {
        Value::String(text) => Some(text.clone()),
        other => Some(other.to_string()),
    }
}

/// The tags of a declared table that DynamoDB will STORE: the ones the declaration asks for
/// and the ones CloudFormation propagates ([`tags_of`]), less every key in the namespace
/// DynamoDB reserves.
///
/// A key in the reserved `aws:` namespace "cannot be assigned by a caller" is DynamoDB's own
/// rule for every caller of its tag-taking calls — it refuses the whole request on one such
/// key — so the three records CloudFormation keeps about a resource it provisioned (the
/// stack's name, its id and the resource's logical id) are left in CloudFormation's own
/// records rather than sent to a call that would refuse the table outright. Every other tag
/// travels, which is what a stack's tags being "propagated to the resources that
/// CloudFormation creates" means here. It is the same split
/// [`super::lambda_function`] records for AWS Lambda, which reserves the same namespace.
fn storable_tags(properties: &Value, propagated: &[(String, String)]) -> Vec<(String, String)> {
    tags_of(properties, propagated)
        .into_iter()
        .filter(|(key, _)| !key.to_ascii_lowercase().starts_with(RESERVED_TAG_PREFIX))
        .collect()
}

/// The tag set as `CreateTable`'s own `Tags` list: "an array of key-value pairs", each a
/// `Tag` with a `Key` and a `Value` — the form [`super::key_value_tag_list`] renders for
/// every provider whose owning model spells it that way.
fn tag_list(tags: &[(String, String)]) -> Value {
    key_value_tag_list(tags)
}

/// Whether the declaration changes a property that `UpdateTable` does not take, which is
/// what [`update`] reports as having no in-place path.
fn changed_outside_update_members(in_force: &Value, declared: &Value) -> bool {
    let mut names: Vec<&str> = Vec::new();
    for properties in [in_force, declared] {
        if let Value::Object(members) = properties {
            for property in members.keys() {
                if !UPDATE_MEMBERS.contains(&property.as_str())
                    && !names.contains(&property.as_str())
                {
                    names.push(property.as_str());
                }
            }
        }
    }
    names
        .into_iter()
        // The tags of a resource already created are reconciled by `retag`, not by an
        // update, so a change to them alone is not a configuration change with no path.
        .filter(|property| *property != TAGS_PROPERTY)
        // A setting DynamoDB applies through a call of its own has an in-place path of its
        // own ([`settings`]), so a change to one is not a change `UpdateTable` had to carry.
        .filter(|property| !settings::is_a_setting(property))
        // The stream configuration IS a member `UpdateTable` takes — under the API's own
        // spelling of it ([`stream_specification`]) — so [`update`] carries a change to it.
        .filter(|property| *property != STREAM_SPECIFICATION_PROPERTY)
        .any(|property| declared_in(declared, property) != declared_in(in_force, property))
}

/// The refusal for a declared property this module does not apply, naming which property it
/// was: refusing is what keeps a table from being reported created with less than its
/// declaration asked for.
///
/// What is left is `ImportSourceSpecification`: it is not a property applied to a table this
/// arm created but a different create — `ImportTable` rather than `CreateTable` — so a
/// declaration carrying it asks for a table nothing here makes, filed as its own gap rather
/// than dropped from a create that answered success. Every other property of the type is
/// either a member of the create or a call of this service's own on the table that exists,
/// which [`settings`] makes.
fn refuse_unapplied_properties(properties: &Value, resource_type: &str) -> Result<(), Refusal> {
    let Value::Object(members) = properties else {
        return Ok(());
    };
    for property in members.keys() {
        if applies(property) {
            continue;
        }
        return Err(Refusal {
            code: "NotImplemented".to_string(),
            message: format!(
                "this emulator does not apply the {property} property of {resource_type}"
            ),
        });
    }
    // The stream's own resource-based policy is a sub-resource of the STREAM rather than a
    // member of either table call, so a declaration asking for one asks for something this
    // arm does not put: refused, for the reason every unapplied property above is. The two
    // members the API's own structure carries are applied ([`stream_specification`]).
    if let Some(Value::Object(stream)) = declared_in(properties, STREAM_SPECIFICATION_PROPERTY) {
        for member in stream.keys() {
            if member == STREAM_VIEW_TYPE_MEMBER || member == STREAM_ENABLED_MEMBER {
                continue;
            }
            return Err(Refusal {
                code: "NotImplemented".to_string(),
                message: format!(
                    "this emulator does not apply the \
                     {STREAM_SPECIFICATION_PROPERTY}.{member} property of {resource_type}"
                ),
            });
        }
    }
    Ok(())
}

/// Whether this arm applies the property `name`: a body member of the create, the one that
/// addresses it, the tag set, one of the two whose spelling differs between the two sides, or
/// one of the settings DynamoDB applies through a call of its own ([`settings`]).
fn applies(name: &str) -> bool {
    CREATE_MEMBERS.contains(&name)
        || name == STREAM_SPECIFICATION_PROPERTY
        || settings::is_a_setting(name)
        || [
            TABLE_NAME_PROPERTY,
            TAGS_PROPERTY,
            RESOURCE_POLICY_PROPERTY,
            SSE_SPECIFICATION_PROPERTY,
        ]
        .contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A resource record of this type, as the naming rules leave one.
    fn declared(properties: Value) -> StoredResource {
        StoredResource {
            logical_id: "Table".to_string(),
            resource_type: DYNAMODB_TABLE.to_string(),
            physical_resource_id: "stack-Table-ABC123".to_string(),
            attributes: BTreeMap::new(),
            metadata: None,
            properties,
            materialised: false,
            deletion_policy: None,
            update_replace_policy: None,
        }
    }

    /// Every member `UpdateTable` is sent is also a property the create applies, so one
    /// declaration cannot mean two different things on the two calls.
    #[test]
    fn every_updated_member_is_an_applied_property() {
        for member in UPDATE_MEMBERS {
            assert!(
                applies(member),
                "case {member:?}: expected the updated member to be an applied property of \
                 {DYNAMODB_TABLE}, got none"
            );
        }
    }

    /// One row of [`the_published_attributes_are_the_services_own_answer`]: the case's
    /// name, the `TableDescription` DynamoDB answered, and the attributes it publishes.
    type AttributeRow = (&'static str, Value, &'static [(&'static str, &'static str)]);

    /// The attributes the type publishes are read off the description the create answered,
    /// and an attribute the description does not carry is left out rather than reported
    /// empty.
    #[test]
    fn the_published_attributes_are_the_services_own_answer() {
        let cases: &[AttributeRow] = &[
            (
                "a table with a stream",
                serde_json::json!({
                    "TableArn": "arn:aws:dynamodb:us-east-1:000000000000:table/orders",
                    "LatestStreamArn":
                        "arn:aws:dynamodb:us-east-1:000000000000:table/orders/stream/\
                         2026-10-23T11:08:02.000",
                }),
                &[
                    (
                        "Arn",
                        "arn:aws:dynamodb:us-east-1:000000000000:table/orders",
                    ),
                    (
                        "StreamArn",
                        "arn:aws:dynamodb:us-east-1:000000000000:table/orders/stream/\
                         2026-10-23T11:08:02.000",
                    ),
                ],
            ),
            (
                "a table with no stream: no StreamArn at all",
                serde_json::json!({
                    "TableArn": "arn:aws:dynamodb:us-east-1:000000000000:table/plain",
                }),
                &[(
                    "Arn",
                    "arn:aws:dynamodb:us-east-1:000000000000:table/plain",
                )],
            ),
            (
                "an empty stream ARN is no ARN",
                serde_json::json!({
                    "TableArn": "arn:aws:dynamodb:us-east-1:000000000000:table/plain",
                    "LatestStreamArn": "",
                }),
                &[(
                    "Arn",
                    "arn:aws:dynamodb:us-east-1:000000000000:table/plain",
                )],
            ),
        ];
        for (case, description, expected) in cases {
            let published = published_attributes(Some(description));
            let wanted: BTreeMap<String, String> = expected
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect();
            assert_eq!(
                published, wanted,
                "case {case:?}: expected the published attributes {wanted:?}, got {published:?}"
            );
        }
    }

    /// A property the arm does not apply is refused rather than dropped, and every property
    /// it does apply is admitted.
    #[test]
    fn an_unapplied_property_is_refused() {
        // An import is not a property applied to a table this arm created but a different
        // create, so a declaration carrying it is refused.
        let refused: &[&str] = &["ImportSourceSpecification"];
        for property in refused {
            let mut properties = Map::new();
            properties.insert(
                "KeySchema".to_string(),
                serde_json::json!([{ "AttributeName": "pk", "KeyType": "HASH" }]),
            );
            properties.insert((*property).to_string(), Value::Object(Map::new()));
            let resource = declared(Value::Object(properties));
            let answer =
                refuse_unapplied_properties(&resource.properties, &resource.resource_type);
            assert!(
                answer.is_err(),
                "case {property:?}: expected a refusal naming the unapplied property, got \
                 {answer:?}"
            );
        }
        let applied = declared(serde_json::json!({
            "TableName": "orders",
            "AttributeDefinitions": [{ "AttributeName": "pk", "AttributeType": "S" }],
            "KeySchema": [{ "AttributeName": "pk", "KeyType": "HASH" }],
            "BillingMode": "PAY_PER_REQUEST",
            "StreamSpecification": { "StreamEnabled": true,
                                     "StreamViewType": "NEW_AND_OLD_IMAGES" },
            "TimeToLiveSpecification": { "AttributeName": "expiresAt", "Enabled": true },
            "PointInTimeRecoverySpecification": { "PointInTimeRecoveryEnabled": true },
            "ContributorInsightsSpecification": { "Enabled": true },
            "KinesisStreamSpecification": {
                "StreamArn": "arn:aws:kinesis:us-east-1:000000000000:stream/changes" },
            "Tags": [{ "Key": "Tier", "Value": "gold" }],
        }));
        let answer = refuse_unapplied_properties(&applied.properties, &applied.resource_type);
        assert!(
            answer.is_ok(),
            "case {:?}: expected every applied property to be admitted, got {answer:?}",
            "a fully declared table"
        );
    }

    /// The template's `SSEEnabled` reaches the API as its own `Enabled` member, and the
    /// other two members travel as written.
    #[test]
    fn the_encryption_flag_is_renamed_for_the_api() {
        let specification = sse_specification(&serde_json::json!({
            "SSESpecification": { "SSEEnabled": true, "SSEType": "KMS",
                                  "KMSMasterKeyId": "alias/aws/dynamodb" },
        }));
        assert_eq!(
            specification,
            Some(serde_json::json!({
                "Enabled": true, "SSEType": "KMS", "KMSMasterKeyId": "alias/aws/dynamodb",
            })),
            "case {:?}: expected the API's own member names, got {specification:?}",
            "a declared SSESpecification"
        );
    }

    /// A declared `ResourcePolicy` reaches the API as the policy document's TEXT, whichever
    /// of the two spellings the template used.
    #[test]
    fn the_resource_policy_reaches_the_api_as_text() {
        let cases: &[(&str, Value)] = &[
            (
                "a PolicyDocument object",
                serde_json::json!({
                    "ResourcePolicy": { "PolicyDocument": { "Version": "2012-10-17" } },
                }),
            ),
            (
                "a PolicyDocument already written as text",
                serde_json::json!({
                    "ResourcePolicy": { "PolicyDocument": "{\"Version\":\"2012-10-17\"}" },
                }),
            ),
        ];
        for (case, properties) in cases {
            let document = resource_policy(properties);
            assert_eq!(
                document.as_deref(),
                Some("{\"Version\":\"2012-10-17\"}"),
                "case {case:?}: expected the policy document as text, got {document:?}"
            );
        }
    }

    /// A change to a property `UpdateTable` does not take is reported as having no in-place
    /// path, while a change to one it does take is not.
    #[test]
    fn a_change_outside_the_update_members_has_no_in_place_path() {
        let cases: &[(&str, Value, Value, bool)] = &[
            (
                "the capacity mode changed: UpdateTable takes it",
                serde_json::json!({ "BillingMode": "PROVISIONED" }),
                serde_json::json!({ "BillingMode": "PAY_PER_REQUEST" }),
                false,
            ),
            (
                "the key schema changed: no member of UpdateTable",
                serde_json::json!({ "KeySchema": [{ "AttributeName": "pk",
                                                    "KeyType": "HASH" }] }),
                serde_json::json!({ "KeySchema": [{ "AttributeName": "id",
                                                    "KeyType": "HASH" }] }),
                true,
            ),
            (
                "only the tags changed: reconciled by the re-tagging pass",
                serde_json::json!({ "Tags": [{ "Key": "Tier", "Value": "gold" }] }),
                serde_json::json!({ "Tags": [{ "Key": "Tier", "Value": "silver" }] }),
                false,
            ),
            (
                "a property was dropped altogether",
                serde_json::json!({ "LocalSecondaryIndexes": [] }),
                serde_json::json!({}),
                true,
            ),
        ];
        for (case, in_force, declared_now, expected) in cases {
            let outside = changed_outside_update_members(in_force, declared_now);
            assert_eq!(
                outside, *expected,
                "case {case:?}: expected a change outside UpdateTable's members to be \
                 {expected:?}, got {outside:?}"
            );
        }
    }
}
