// SPDX-License-Identifier: CC0-1.0

//! Which members of its request an integration's own API REQUIRES, so a definition that
//! builds a request without one is refused where it is written rather than where it runs.
//!
//! A `Task` state's service integration performs one API operation, and the document the
//! state's payload template builds IS that operation's request ([`super`]). A request
//! that does not carry a member the operation's model marks `required` is a request that
//! service cannot serve — whatever the execution's data turns out to be — so the
//! definition is refused at create time ([`super::super::asl::diagnose`]) instead of
//! deploying a workflow whose task can only fail.
//!
//! One entry per integration, because which members an operation requires is the invoked
//! model's own fact: each entry below names the `smithy_query` it was read from, exactly
//! as the entries stating where an integration's request names its target do
//! ([`super::TARGET_RESOURCES`]). An integration with no entry states nothing, so nothing
//! is checked for it and its definition is created as before — an entry is added from the
//! invoked service's model rather than guessed at.
//!
//! An entry serves BOTH integration families: the AWS SDK family (`aws-sdk:<service>:
//! <action>`) names the same service and the same operation as the optimized one, and the
//! parameters of either are written in the integration's own convention — the member
//! names of the invoked model ([`super::MemberCase`]) — so the same members are required
//! either way. The integration PATTERN after the action (`.sync`, `.waitForTaskToken`)
//! says what finishes the task rather than what the call carries, so it changes nothing
//! here.

use super::*;

/// The members one integration's call cannot be made without.
#[derive(Debug)]
pub struct RequiredParameters {
    /// The ARN namespace of the service the integration calls, as [`Api::namespace`]
    /// spells it.
    pub namespace: &'static str,
    /// The operation the integration's action names, as the invoked model spells it
    /// ([`Call::operation`]).
    pub operation: &'static str,
    /// Every member of that operation's input the model marks `required`, in the order
    /// the model declares them — which is the order a refusal lists the missing ones in.
    pub members: &'static [&'static str],
}

/// The integrations whose invoked operation's required members this service states.
///
/// DynamoDB's item-level operations are stated from
/// `smithy_query(query="operation",operation="PutItem,GetItem,UpdateItem,DeleteItem",
/// model_path="services/service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/
/// com-amazonaws-dynamodb-dynamodb-20120810.json")`: each of the four requires
/// `TableName`, the write of an item requires the `Item` it writes, and the three that
/// address one item by its key require that `Key`.
pub const REQUIRED_PARAMETERS: &[RequiredParameters] = &[
    RequiredParameters {
        namespace: "dynamodb",
        operation: "PutItem",
        members: &["TableName", "Item"],
    },
    RequiredParameters {
        namespace: "dynamodb",
        operation: "GetItem",
        members: &["TableName", "Key"],
    },
    RequiredParameters {
        namespace: "dynamodb",
        operation: "UpdateItem",
        members: &["TableName", "Key"],
    },
    RequiredParameters {
        namespace: "dynamodb",
        operation: "DeleteItem",
        members: &["TableName", "Key"],
    },
];

/// The members the call `resource` names cannot be made without, and `None` for a
/// `Resource` that names no call at all or one no entry is stated for.
///
/// The `Resource` is read by the ONE reading of it this service has ([`read`]), so a
/// definition is judged against the same call its execution would make: the service, the
/// operation and the family are that reading's, and a `Resource` it cannot read names no
/// operation whose model could require anything.
pub fn required_parameters(resource: &str) -> Option<&'static RequiredParameters> {
    let call = read(resource).ok()?;
    // A `Resource` whose work is an HTTP REQUEST (this service's own HTTP Task, the API
    // Gateway invoke) or the invocation of a Lambda FUNCTION by its ARN calls no
    // operation whose input model states anything about it.
    if call.endpoint.is_some() || call.invokes.is_some() {
        return None;
    }
    REQUIRED_PARAMETERS.iter().find(|entry| {
        entry.namespace == call.api.namespace && entry.operation == call.operation
    })
}
