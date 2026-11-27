// SPDX-License-Identifier: CC0-1.0

//! The FLOW SNAPSHOT one execution runs against: the flow metadata and definition as they
//! stood at the instant the execution was started.
//!
//! A flow, its versions and its aliases are the Bedrock Agent build-time plane's, and that
//! plane keeps changing them: a draft is edited, a role or an encryption key is replaced, an
//! alias is repointed. An EXECUTION, though, runs the flow it resolved when it started, and
//! "even if the flow is edited after an execution starts, the snapshot connected to the
//! execution remains unchanged"
//! (`smithy_query(query="doc",shape="GetExecutionFlowSnapshotResponse")`). So what the start
//! resolved is COPIED here, beside the execution's own record, rather than read back off the
//! build-time plane when somebody asks for it — a later read of the build-time plane would
//! report whatever that plane holds now, which is not what the execution ran.
//!
//! The copy is the definition the version the alias routed to packages and the two members
//! that travel with it ([`FlowSnapshot`]): the role the execution assumes and the customer
//! managed key the flow is encrypted with, read from the thing that PACKAGES the definition
//! — the flow record itself for the DRAFT version, the version's own snapshot for a numbered
//! one — so an execution on a published version reports that version's metadata and not the
//! draft's.
//!
//! [`definition_text`] renders the copied definition as the `definition` member carries it:
//! the model types that member as a plain `String` whose contents "follow the structure of a
//! FlowDefinition object", so it is the stored structure serialized against its OWN schema,
//! through the one JSON serializer this emulator's wire already uses. Nothing is re-worded
//! on the way out: the member names, the nesting and the values are the stored definition's.

use ::aws_smithy_schema::serde::ShapeSerializer;
use ::aws_smithy_schema_server::json::JsonShapeSerializer;

use ::service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::ops::types::FlowDefinition;

/// The flow metadata and definition one execution was started against.
#[derive(Debug, Clone)]
pub struct FlowSnapshot {
    /// The definition the version the alias routed to packaged at that instant, held as the
    /// structure it is so that what a reader reports is this definition rendered rather than
    /// text somebody composed.
    pub definition: FlowDefinition,
    /// The IAM service role the execution runs with, as the thing packaging the definition
    /// declared it.
    pub execution_role_arn: String,
    /// The customer managed KMS key that thing is encrypted with, for a flow that names one.
    pub customer_encryption_key_arn: Option<String>,
}

/// `definition` as the snapshot reports it: the stored `FlowDefinition` serialized against
/// its own schema, which is the "structure of a FlowDefinition object" the member's text
/// follows.
///
/// `None` only when the serializer refuses the structure, which is the one case a reader has
/// no definition to report and answers a server error for rather than a definition it made
/// up. The member names are the wire's — `@jsonName` where the model declares one, the member
/// name otherwise — because this is the same rendering the restJson1 wire gives that shape
/// anywhere else it travels.
pub fn definition_text(definition: &FlowDefinition) -> Option<String> {
    let mut serializer = JsonShapeSerializer::new_with_config(true);
    serializer
        .write_struct(&FlowDefinition::SCHEMA, definition)
        .ok()?;
    Some(serializer.into_string())
}
