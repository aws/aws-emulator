// SPDX-License-Identifier: CC0-1.0

//! What a FLOW INVOCATION reads: the flow an alias routes to, and the path the request's
//! input really travels through that flow's own definition.
//!
//! A flow, its versions and its aliases belong to the Bedrock Agent build-time plane, so
//! every one of them is read THERE and never assumed from the request:
//!
//! * the `flowIdentifier` names a stored flow, in either form the modelled shape admits
//!   (a flow ARN or the bare id), through that plane's own reader;
//! * the `flowAliasIdentifier` names an alias STORED ON THAT FLOW — an alias of another
//!   flow is a resource the addressed flow does not hold;
//! * the alias's `routingConfiguration` names the VERSION the invocation runs, and the
//!   definition that version packages is the one that is walked: the DRAFT version is the
//!   flow record itself, a numbered version its own snapshot. So a flow whose draft has
//!   moved since the version was cut still runs what the version packages;
//! * a flow that is not PREPARED runs nothing: preparation is what turns a definition into
//!   something invocable, and its status is the build-time plane's.
//!
//! NO LANGUAGE MODEL, LAMBDA FUNCTION OR OTHER EXTERNAL WORKER RUNS IN THIS EMULATOR, so
//! the walk computes exactly the nodes it can and REFUSES the rest rather than inventing a
//! value for them: the request's document enters at the flow's input node and reaches the
//! output nodes that node is connected to. A path through any other node kind — a prompt, a
//! knowledge base, a Lambda function, a condition, an iterator — is refused with the
//! sentence [`FlowRefusal::Unservable`] carries, because an output synthesised for a node
//! nothing ran would be indistinguishable from one the flow really produced.
//!
//! An input that addresses no INPUT NODE of the flow is a different refusal
//! ([`FlowRefusal::InputNodeUnavailable`]): the flow is invocable and the request is well
//! formed, but the node the input was sent to is not a node the flow enters at, so the
//! flow's own input node is left without the input the run depends on.
//!
//! The walk also records, for each hop it takes, the two field names the connection it
//! followed carries ([`CarriedBy`]), because those are what a TRACE of the run reports: the
//! output of the source node the value left by and the input of the target it arrived at,
//! both read from the stored flow rather than invented.
//!
//! READ-ONLY for everything a later caller can observe: the one write on its path is the
//! build-time plane materialising the status transitions its own clock had already made
//! due, which is owed whether or not anybody calls.

use std::time::SystemTime;

use ::aws_smithy_types::Document;

use ::service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::ops::types::FlowDefinition;
use ::service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state as agent_state;

/// Why a flow invocation cannot be served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowRefusal {
    /// The store the flows live in is not on this engine, so nothing can be said about the
    /// flow — reported as a server error, never as a not-found.
    StoreUnavailable,
    /// The Bedrock Agent plane holds no flow of that identifier, no such alias on it, or
    /// no version the alias routes to.
    NotFound,
    /// The flow exists and the request is well formed, but this service cannot run it: the
    /// string is the sentence the refusal states, naming what about the flow or the request
    /// it could not compute.
    Unservable(String),
    /// The request's input does not address the flow's own input node, so the flow cannot
    /// be run at all: the node it names is not one the flow holds, or is not the flow's
    /// input node, and the flow's real input node is then left with no input. The string is
    /// the name of what the input addressed, which is the dependency the failure is about.
    InputNodeUnavailable(String),
}

/// One value a flow invocation delivers: the output node it left the flow at, and the
/// document that reached it.
#[derive(Debug, Clone)]
pub struct FlowOutputValue {
    /// The name of the flow output node the value left the flow at.
    pub node_name: String,
    /// The document that reached that node.
    pub document: Document,
    /// The OUTPUT of the source node the connection carried the value from, and the INPUT
    /// of `node_name` it carried it to, as the connection the walk followed names them
    /// (`FlowDataConnectionConfiguration`). `None` for a connection that carries no such
    /// configuration, which names neither end's field.
    pub carried_by: Option<CarriedBy>,
}

/// The two field names one data connection of a flow names: which output of its source the
/// value left by, and which input of its target it arrived at.
#[derive(Debug, Clone)]
pub struct CarriedBy {
    /// The source node's output the value left by.
    pub source_output: String,
    /// The target node's input the value arrived at.
    pub target_input: String,
}

/// The `FlowNodeType` of a flow's input node, as the build-time model spells it
/// (`smithy_query(query="shape",shape="FlowNodeType")` -> Input, Output, KnowledgeBase,
/// Condition, Lex, Prompt, LambdaFunction, Storage, Agent, Retrieval, Iterator, Collector,
/// InlineCode, Loop, LoopInput, LoopController).
const INPUT_NODE: &str = "Input";

/// See [`INPUT_NODE`]: the type of a flow's output node.
const OUTPUT_NODE: &str = "Output";

/// The `FlowConnectionType` of a connection that carries a VALUE from one node to the next
/// (`smithy_query(query="shape",shape="FlowConnectionType")` -> Data, Conditional).
const DATA_CONNECTION: &str = "Data";

/// The `FlowStatus` a flow has to be in to be invoked: preparation is what turns a
/// definition into something invocable
/// (`smithy_query(query="shape",shape="FlowStatus")` -> Failed, Prepared, Preparing,
/// NotPrepared).
const PREPARED: &str = "Prepared";

/// Run `document` through the flow the alias routes to, entering at `input_node`.
///
/// The resolution is keyed to the PRINCIPAL the authorization decision was made about, not
/// to the request's identifiers alone, and `ctx` is where both halves of that key come from:
///
/// * the ACCOUNT the caller acts in (`crate::state::caller_account`). Every flow, alias and
///   version this plane runs belongs to the one account this emulator serves
///   ([`::engine::emulator_account_id`]), so a principal of any other account names none of
///   them and is answered not-found whatever identifiers it carries;
/// * the REGION the caller signed for (`crate::state::request_region`), the scope the
///   authorization resource was built in (`alias_resource_arn`).
///
/// An identifier in ARN form names ONE resource in ONE place, so an ARN whose partition,
/// service, Region or account is not that pair names no flow this call can run and is
/// answered not-found rather than reduced to a bare id that would resolve in the caller's
/// own place ([`names_this_place`]). This is the rule the sibling knowledge-base lookup of
/// this plane keeps (`crate::state::knowledge_base_retrieval::knowledge_base_id_from_identifier`).
///
/// The whole lookup happens under ONE hold of the build-time plane's store, so the
/// definition that is walked is the one the alias routed to at that instant.
pub fn run<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    flow_identifier: &str,
    flow_alias_identifier: &str,
    input_node: &str,
    document: &Document,
) -> Result<Vec<FlowOutputValue>, FlowRefusal> {
    // Decided BEFORE any flow store is read, from the caller's own identity and scope, so an
    // identifier naming another account or another place is answered the same way whether or
    // not a flow of that id exists here.
    let caller = crate::state::caller(engine, ctx);
    let account = crate::state::caller_account(&caller).unwrap_or_default();
    if account != ::engine::emulator_account_id() {
        return Err(FlowRefusal::NotFound);
    }
    let region = crate::state::request_region(ctx);
    if !names_this_place(flow_identifier, &account, &region)
        || !names_this_place(flow_alias_identifier, &account, &region)
    {
        return Err(FlowRefusal::NotFound);
    }
    let Some(agents) = agent_state::service_state(engine) else {
        return Err(FlowRefusal::StoreUnavailable);
    };
    let mut store = agents.lock();
    store.materialise_due_transitions(SystemTime::now());

    let Some(flow_id) = agent_state::flow_id_from_identifier(flow_identifier) else {
        return Err(FlowRefusal::NotFound);
    };
    let Some((aliased_flow, alias_id)) =
        agent_state::flow_alias_from_identifier(flow_alias_identifier)
    else {
        return Err(FlowRefusal::NotFound);
    };
    // An alias ARN carries the flow it belongs to: one naming another flow addresses a
    // resource the flow in the path does not hold.
    if aliased_flow.is_some_and(|named| named != flow_id) {
        return Err(FlowRefusal::NotFound);
    }
    let Some(flow) = store.flows.get(flow_id) else {
        return Err(FlowRefusal::NotFound);
    };
    let Some(alias) = store.flow_alias_of(flow_id, alias_id) else {
        return Err(FlowRefusal::NotFound);
    };
    let Some(routed) = alias
        .routing_configuration
        .first()
        .and_then(|item| item.flow_version.clone())
    else {
        return Err(FlowRefusal::NotFound);
    };

    // The definition the alias routes to, and the status of the thing that packages it.
    let (definition, status) = if routed == agent_state::DRAFT_VERSION {
        (flow.definition.as_ref(), flow.status.as_wire())
    } else {
        let Some(version) = routed
            .parse::<u32>()
            .ok()
            .and_then(|number| store.flow_versions.get(&(flow_id.to_string(), number)))
        else {
            return Err(FlowRefusal::NotFound);
        };
        (version.definition.as_ref(), version.status.as_wire())
    };
    if status != PREPARED {
        return Err(FlowRefusal::Unservable(format!(
            "the flow version {routed} this alias routes to is in status {status}: prepare \
             the flow before invoking it"
        )));
    }
    let Some(definition) = definition else {
        return Err(FlowRefusal::Unservable(format!(
            "the flow version {routed} this alias routes to carries no definition, so there \
             is no node for the input to enter at"
        )));
    };
    walk(definition, input_node, document)
}

/// Whether `identifier` names a resource of THIS caller's place: a bare identifier always
/// does — it names the resource of that id in the account and Region the caller acts in —
/// and an ARN only when its partition, its service, its Region and its account are the ones
/// this emulator would have minted for a principal of `account` signing for `region`.
///
/// Read through the ONE ARN parse this tree has (`service_common::arn`), against an ARN this
/// emulator itself mints, so nothing here spells an ARN form of its own.
fn names_this_place(identifier: &str, account: &str, region: &str) -> bool {
    if !identifier.starts_with("arn:") {
        return true;
    }
    let own = ::engine::emulator_arn(crate::state::ARN_NAMESPACE, region, "flow/own");
    let (Some(named), Some(own)) = (
        ::service_common::arn::parse(identifier),
        ::service_common::arn::parse(&own),
    ) else {
        return false;
    };
    named.partition == own.partition
        && named.vendor == own.vendor
        && named.region == own.region
        && named.account == own.account
        && named.account == account
}

/// The values the request's document reaches the flow's output nodes with.
///
/// The walk is one hop by construction: the input node's `Data` connections are followed,
/// and a target that is anything but an output node is refused. See the module docs for
/// why nothing further is computed.
fn walk(
    definition: &FlowDefinition,
    input_node: &str,
    document: &Document,
) -> Result<Vec<FlowOutputValue>, FlowRefusal> {
    let nodes = definition.nodes.as_deref().unwrap_or_default();
    let node_type = |name: &str| {
        nodes
            .iter()
            .find(|node| node.name == name)
            .map(|node| node.r#type.clone())
    };
    let Some(entry_type) = node_type(input_node) else {
        return Err(FlowRefusal::InputNodeUnavailable(input_node.to_string()));
    };
    if entry_type != INPUT_NODE {
        return Err(FlowRefusal::InputNodeUnavailable(input_node.to_string()));
    }

    let mut reached: Vec<FlowOutputValue> = Vec::new();
    for connection in definition.connections.as_deref().unwrap_or_default() {
        if connection.source != input_node || connection.r#type != DATA_CONNECTION {
            continue;
        }
        let Some(target_type) = node_type(&connection.target) else {
            return Err(FlowRefusal::Unservable(format!(
                "the connection {} leaves {input_node} for the node {}, which the flow does \
                 not hold",
                connection.name, connection.target
            )));
        };
        if target_type != OUTPUT_NODE {
            return Err(FlowRefusal::Unservable(unservable_node(
                &connection.target,
                &target_type,
            )));
        }
        reached.push(FlowOutputValue {
            node_name: connection.target.clone(),
            document: document.clone(),
            carried_by: match connection.configuration.as_ref() {
                Some(::service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::ops::types::FlowConnectionConfiguration::Data(data)) => {
                    Some(CarriedBy {
                        source_output: data.source_output.clone(),
                        target_input: data.target_input.clone(),
                    })
                }
                _ => None,
            },
        });
    }
    if reached.is_empty() {
        return Err(FlowRefusal::Unservable(format!(
            "the flow's input node {input_node} is connected to no node of type \
             {OUTPUT_NODE}, so the input this invocation carries reaches no output"
        )));
    }
    // One order for one flow: an invocation of the same flow always delivers its outputs
    // the same way round.
    reached.sort_by(|left, right| left.node_name.cmp(&right.node_name));
    Ok(reached)
}

/// The sentence a node this service cannot run earns: what the node is, and that nothing
/// in this process performs the work its kind does.
fn unservable_node(name: &str, node_type: &str) -> String {
    format!(
        "the node {name} is of type {node_type}, whose work — a model, a function or \
         another service answering — nothing in this emulator performs, so no value can be \
         computed for it; a flow this service runs sends its input node straight to its \
         output nodes"
    )
}

/// The message a flow invocation delivers inside its stream when the flow, the alias or
/// the version the alias routes to is one the Bedrock Agent plane does not hold.
///
/// Fixed text for this error class: the refusal names neither the flow nor the alias,
/// because any one of them may be the missing resource and the service does not disclose
/// which.
pub fn not_found_message() -> String {
    FLOW_RESOURCE_NOT_FOUND.to_string()
}

/// The sentence [`not_found_message`] answers with, as AWS words a missing resource on this
/// plane (the same sentence its agent invocation surface states).
const FLOW_RESOURCE_NOT_FOUND: &str = "Failed to retrieve resource because it doesn't \
                                       exist. Retry the request with a different resource \
                                       identifier.";

/// The message a flow invocation delivers inside its stream when the input it carries does
/// not address the flow's own input node ([`FlowRefusal::InputNodeUnavailable`]).
///
/// Fixed text for this error class, as the model words `DependencyFailedException`
/// (`smithy_query(query="shape",shape="DependencyFailedException")`): the run depended on
/// the flow's input node receiving the invocation's input, and it did not, so the failure is
/// the dependency's rather than a fault in a member of the request.
pub fn dependency_failed_message() -> String {
    DEPENDENCY_FAILED.to_string()
}

/// The sentence [`dependency_failed_message`] answers with.
const DEPENDENCY_FAILED: &str = "There was an issue with a dependency. Check the resource \
                                 configurations and retry the request.";

/// The resource an identity-based policy for `bedrock:InvokeFlow` names: the flow alias the
/// call invokes.
///
/// An identifier that is already an ARN IS that resource; a bare pair names the alias of
/// that flow in the region the caller signed for. Built without a store lookup, so a
/// decision made with it is made before — and independently of — whether the flow exists.
pub fn alias_resource_arn(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    flow_identifier: &str,
    flow_alias_identifier: &str,
) -> String {
    if flow_alias_identifier.starts_with("arn:") {
        return flow_alias_identifier.to_string();
    }
    let flow = agent_state::flow_id_from_identifier(flow_identifier).unwrap_or(flow_identifier);
    ::engine::emulator_arn(
        crate::state::ARN_NAMESPACE,
        &crate::state::request_region(ctx),
        &format!("flow/{flow}/alias/{flow_alias_identifier}"),
    )
}
