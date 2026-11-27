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
//! The flow, the alias and the version are three separate resources, and a request naming one
//! this plane does not hold is a not-found for THAT resource: the refusal names the type of
//! the resource the resolution failed on ([`FlowResourceType`]), which is the only place a
//! caller can read it from, since the status and the error code are the same for all three.
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
//! both read from the stored flow rather than invented. The whole walk is reported the same
//! way ([`FlowRunTrace`]): where the input entered, which field of which node each value
//! left by and arrived at, and the node a failed walk is about — which is what the EVENTS of
//! a persisted run are composed from (`super::flow_execution_event`).
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
    /// no version the alias routes to. The value is the TYPE of resource the resolution
    /// failed on, which is what the refusal names.
    NotFound(FlowResourceType),
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

/// Which resource of the flow surface a resolution failed on.
///
/// A request to this surface addresses a chain of resources — the flow its path names, the
/// alias of that flow, and the version that alias routes to — and every one of them is a
/// separate resource of the Bedrock Agent plane, answered 404 on its own. The refusal names
/// which one did not resolve, because the status and the error code are identical for all
/// three, so the message is the only place a caller can read it from.
///
/// Each variant is spelled the way the build-time model spells the resource itself
/// (`smithy_query(query="operation",operation="GetFlow,GetFlowAlias,GetFlowVersion")`), which
/// is the spelling this plane's own execution resource already uses
/// ([`super::flow_execution::execution_not_found`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowResourceType {
    /// The flow the `flowIdentifier` names.
    Flow,
    /// The alias of that flow the `flowAliasIdentifier` names.
    FlowAlias,
    /// The version that alias routes to.
    FlowVersion,
}

impl FlowResourceType {
    /// The resource type as the refusal names it.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Flow => "Flow",
            Self::FlowAlias => "FlowAlias",
            Self::FlowVersion => "FlowVersion",
        }
    }
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
    run_resolved(
        engine,
        ctx,
        flow_identifier,
        flow_alias_identifier,
        input_node,
        document,
    )
    .and_then(|run| run.outcome.map_err(FlowRefusal::Unservable))
}

/// What one run of a flow really ran: the flow and the alias the identifiers named, the
/// version that alias routed to at that instant, and what the walk of that version's
/// definition came to.
///
/// An operation that PERSISTS a run (`StartFlowExecution`) records the three identities
/// beside the outcome, so what it stored is the flow, the alias and the version the run was
/// resolved against rather than the identifier forms the request happened to carry.
#[derive(Debug)]
pub struct FlowRun {
    /// The flow the run resolved to, as the build-time plane identifies it.
    pub flow_id: String,
    /// The alias of that flow the run was addressed through.
    pub alias_id: String,
    /// The `flowVersion` the alias routed to when the run was resolved.
    pub flow_version: String,
    /// The flow metadata and definition that version packaged at that instant
    /// ([`super::flow_snapshot::FlowSnapshot`]), copied so that an operation which PERSISTS
    /// a run can report what the run went through however the flow moves afterwards.
    pub snapshot: super::flow_snapshot::FlowSnapshot,
    /// What the walk came to: the values it delivered, or the sentence saying which of the
    /// flow's own nodes it could compute no value for.
    ///
    /// The two are a RUN's outcome rather than a refusal of the call, because the flow was
    /// invocable and the request was well formed: a synchronous invocation has to deliver one
    /// or the other inside its stream ([`run`]), while an EXECUTION records which of them the
    /// run came to as its own status.
    pub outcome: Result<Vec<FlowOutputValue>, String>,
    /// What the walk of that definition really did, node by node ([`FlowRunTrace`]), so an
    /// operation that PERSISTS a run can report the events of it afterwards.
    pub trace: FlowRunTrace,
}

/// What one walk of a flow's definition did: where the input entered, which field of which
/// node each value left by and arrived at, and the node a failed walk is about.
///
/// Every member is read from the STORED definition — the node names, the field names, the
/// expression a target input extracts with, the declared IO types — so an operation that
/// reports the steps of a run reports what the flow itself says they were, and nothing a
/// request happened to word.
#[derive(Debug, Clone)]
pub struct FlowRunTrace {
    /// The flow input node the run's input entered at.
    pub input_node: String,
    /// The field the document entered as: the input node's own declared output, which is
    /// what the flow's input schema names the value that enters there. `None` for an input
    /// node whose definition declares no output, which names no field for it.
    pub input_field: Option<String>,
    /// The declared `FlowNodeIODataType` of [`FlowRunTrace::input_field`], when there is one.
    pub input_field_type: Option<String>,
    /// The document the run entered the flow with.
    pub document: Document,
    /// One entry per hop the walk took out of the input node, ordered by the node the hop
    /// reached — the order the walk delivers its outputs in.
    pub hops: Vec<FlowRunHop>,
    /// The node a failed walk is about: the one whose kind nothing in this emulator performs
    /// the work of. `None` when the failure is about the flow's own wiring rather than one of
    /// its nodes, so nothing ran and no node of it failed.
    pub failed_node: Option<String>,
}

/// One hop of a walk: the node the value reached, and the fields of both ends the connection
/// named, as the stored definition declares them.
#[derive(Debug, Clone)]
pub struct FlowRunHop {
    /// The node the hop reached.
    pub target_node: String,
    /// The source node's output the value left by, for a connection that names it.
    pub source_output: Option<String>,
    /// The target node's input the value arrived at, for a connection that names it.
    pub target_input: Option<String>,
    /// The expression that input extracts its data with, as the target node declares it.
    pub target_input_expression: Option<String>,
    /// The declared `FlowNodeIODataType` of that input.
    pub target_input_type: Option<String>,
    /// The declared `FlowNodeInputCategory` of that input, for one the target node puts in a
    /// category. Both planes spell the two enums identically
    /// (`smithy_query(query="shape",shape="FlowNodeIODataType,FlowNodeInputCategory")` in
    /// either model), so what the definition declares is what a reader of the run reports.
    pub target_input_category: Option<String>,
}

/// [`run`], reporting the identities the run was resolved against beside its outcome.
///
/// The `Err` channel is for a call that cannot run the flow AT ALL — a flow, alias or
/// version this plane does not hold, a version nothing has prepared, an input that addresses
/// no input node of the flow — while a path the walk can compute no value for is the RUN's
/// own outcome ([`FlowRun::outcome`]).
pub fn run_resolved<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    flow_identifier: &str,
    flow_alias_identifier: &str,
    input_node: &str,
    document: &Document,
) -> Result<FlowRun, FlowRefusal> {
    let placed = placement(engine, ctx, flow_identifier, flow_alias_identifier)?;
    let (flow_id, alias_id) = (placed.flow_id, placed.alias_id);
    let Some(agents) = agent_state::service_state(engine) else {
        return Err(FlowRefusal::StoreUnavailable);
    };
    let mut store = agents.lock();
    store.materialise_due_transitions(SystemTime::now());

    let flow_id = flow_id.as_str();
    let Some(flow) = store.flows.get(flow_id) else {
        return Err(FlowRefusal::NotFound(FlowResourceType::Flow));
    };
    let Some(alias) = store.flow_alias_of(flow_id, &alias_id) else {
        return Err(FlowRefusal::NotFound(FlowResourceType::FlowAlias));
    };
    // The alias resolved, so what is missing is the VERSION it routes to: an alias carrying
    // no routing entry names none.
    let Some(routed) = alias
        .routing_configuration
        .first()
        .and_then(|item| item.flow_version.clone())
    else {
        return Err(FlowRefusal::NotFound(FlowResourceType::FlowVersion));
    };

    // The definition the alias routes to, and the status of the thing that packages it.
    // The two members that travel WITH that definition in the execution's snapshot are read
    // from the same thing, so an execution on a published version reports that version's
    // role and key rather than the draft's (`super::flow_snapshot`).
    let (definition, status, execution_role_arn, encryption_key) = if routed
        == agent_state::DRAFT_VERSION
    {
        (
            flow.definition.as_ref(),
            flow.status.as_wire(),
            flow.execution_role_arn.clone(),
            flow.customer_encryption_key_arn.clone(),
        )
    } else {
        let Some(version) = routed
            .parse::<u32>()
            .ok()
            .and_then(|number| store.flow_versions.get(&(flow_id.to_string(), number)))
        else {
            return Err(FlowRefusal::NotFound(FlowResourceType::FlowVersion));
        };
        (
            version.definition.as_ref(),
            version.status.as_wire(),
            version.execution_role_arn.clone(),
            version.customer_encryption_key_arn.clone(),
        )
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
    let walked = walk(definition, input_node, document)?;
    Ok(FlowRun {
        flow_id: flow_id.to_string(),
        alias_id,
        flow_version: routed,
        // Copied here, under the one hold of the build-time store the resolution took, so
        // the snapshot is the flow as it stood when this run resolved it and no later edit
        // of the flow can move it (`super::flow_snapshot`).
        snapshot: super::flow_snapshot::FlowSnapshot {
            definition: definition.clone(),
            execution_role_arn,
            customer_encryption_key_arn: encryption_key,
        },
        outcome: walked.outcome,
        trace: walked.trace,
    })
}

/// The flow and the alias a pair of modelled identifiers names, for a caller that may name
/// them at all.
///
/// This is the half of the resolution that reads NO store: the account the caller acts in,
/// the place an identifier in ARN form names, and the two ids the identifier forms carry. It
/// is decided before any flow store is read, so an identifier naming another account or
/// another place is answered the same way whether or not a flow of that id exists here, and
/// an operation that only addresses a resource of this plane's OWN store (an execution of a
/// flow alias) resolves its identifiers through it without reading the build-time plane at
/// all.
pub fn placement<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    flow_identifier: &str,
    flow_alias_identifier: &str,
) -> Result<FlowPlacement, FlowRefusal> {
    let caller = crate::state::caller(engine, ctx);
    let account = crate::state::caller_account(&caller).unwrap_or_default();
    // A principal of another account names no flow of this place, so the resolution fails on
    // the first resource the path addresses: the flow.
    if account != ::engine::emulator_account_id() {
        return Err(FlowRefusal::NotFound(FlowResourceType::Flow));
    }
    let region = crate::state::request_region(ctx);
    // Each identifier is resolved on its own, so an ARN of another place fails on the
    // resource THAT identifier addresses rather than on whichever the chain starts with.
    if !names_this_place(flow_identifier, &account, &region) {
        return Err(FlowRefusal::NotFound(FlowResourceType::Flow));
    }
    if !names_this_place(flow_alias_identifier, &account, &region) {
        return Err(FlowRefusal::NotFound(FlowResourceType::FlowAlias));
    }
    let Some(flow_id) = agent_state::flow_id_from_identifier(flow_identifier) else {
        return Err(FlowRefusal::NotFound(FlowResourceType::Flow));
    };
    let Some((aliased_flow, alias_id)) =
        agent_state::flow_alias_from_identifier(flow_alias_identifier)
    else {
        return Err(FlowRefusal::NotFound(FlowResourceType::FlowAlias));
    };
    // An alias ARN carries the flow it belongs to: one naming another flow addresses a
    // resource the flow in the path does not hold.
    if aliased_flow.is_some_and(|named| named != flow_id) {
        return Err(FlowRefusal::NotFound(FlowResourceType::FlowAlias));
    }
    Ok(FlowPlacement {
        flow_id: flow_id.to_string(),
        alias_id: alias_id.to_string(),
    })
}

/// The flow and the alias [`placement`] resolved a request's identifiers to.
#[derive(Debug, Clone)]
pub struct FlowPlacement {
    /// The flow the `flowIdentifier` names, as the build-time plane identifies it.
    pub flow_id: String,
    /// The alias the `flowAliasIdentifier` names.
    pub alias_id: String,
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

/// What one walk came to, and the trace of the steps it took ([`FlowRunTrace`]).
///
/// The outcome is the RUN's — the values it delivered, or the sentence saying what it could
/// compute no value for — because a path the walk cannot compute is something the run came
/// to rather than a refusal of the call that started it; the trace is recorded either way,
/// so the steps a failed run did take are reported too.
struct Walked {
    outcome: Result<Vec<FlowOutputValue>, String>,
    trace: FlowRunTrace,
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
) -> Result<Walked, FlowRefusal> {
    let nodes = definition.nodes.as_deref().unwrap_or_default();
    let node_named = |name: &str| nodes.iter().find(|node| node.name == name);
    let Some(entry) = node_named(input_node) else {
        return Err(FlowRefusal::InputNodeUnavailable(input_node.to_string()));
    };
    if entry.r#type != INPUT_NODE {
        return Err(FlowRefusal::InputNodeUnavailable(input_node.to_string()));
    }

    // The field the flow's input arrived as, read from the input node's own declared output:
    // that is what the flow's input schema names the value entering there.
    let entry_output = entry.outputs.as_deref().unwrap_or_default().first();
    let mut trace = FlowRunTrace {
        input_node: input_node.to_string(),
        input_field: entry_output.map(|output| output.name.clone()),
        input_field_type: entry_output.map(|output| output.r#type.clone()),
        document: document.clone(),
        hops: Vec::new(),
        failed_node: None,
    };

    let mut reached: Vec<FlowOutputValue> = Vec::new();
    for connection in definition.connections.as_deref().unwrap_or_default() {
        if connection.source != input_node || connection.r#type != DATA_CONNECTION {
            continue;
        }
        let Some(target) = node_named(&connection.target) else {
            // The connection leaves for a node the flow does not hold, so NOTHING ran and no
            // node of the flow is what the failure is about.
            return Ok(Walked {
                outcome: Err(format!(
                    "the connection {} leaves {input_node} for the node {}, which the flow \
                     does not hold",
                    connection.name, connection.target
                )),
                trace,
            });
        };
        if target.r#type != OUTPUT_NODE {
            trace.failed_node = Some(connection.target.clone());
            return Ok(Walked {
                outcome: Err(unservable_node(&connection.target, &target.r#type)),
                trace,
            });
        }
        let carried_by = match connection.configuration.as_ref() {
            Some(::service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::ops::types::FlowConnectionConfiguration::Data(data)) => {
                Some(CarriedBy {
                    source_output: data.source_output.clone(),
                    target_input: data.target_input.clone(),
                })
            }
            _ => None,
        };
        // The target's own declaration of the input the value arrived at: the expression it
        // extracts data with and its declared type, both the flow's rather than invented.
        let declared = carried_by.as_ref().and_then(|carried| {
            target
                .inputs
                .as_deref()
                .unwrap_or_default()
                .iter()
                .find(|input| input.name == carried.target_input)
        });
        trace.hops.push(FlowRunHop {
            target_node: connection.target.clone(),
            source_output: carried_by.as_ref().map(|carried| carried.source_output.clone()),
            target_input: carried_by.as_ref().map(|carried| carried.target_input.clone()),
            target_input_expression: declared.map(|input| input.expression.clone()),
            target_input_type: declared.map(|input| input.r#type.clone()),
            target_input_category: declared.and_then(|input| input.category.clone()),
        });
        reached.push(FlowOutputValue {
            node_name: connection.target.clone(),
            document: document.clone(),
            carried_by,
        });
    }
    if reached.is_empty() {
        return Ok(Walked {
            outcome: Err(format!(
                "the flow's input node {input_node} is connected to no node of type \
                 {OUTPUT_NODE}, so the input this invocation carries reaches no output"
            )),
            trace,
        });
    }
    // One order for one flow: an invocation of the same flow always delivers its outputs
    // the same way round, and the trace of the run follows the same order.
    reached.sort_by(|left, right| left.node_name.cmp(&right.node_name));
    trace
        .hops
        .sort_by(|left, right| left.target_node.cmp(&right.target_node));
    Ok(Walked {
        outcome: Ok(reached),
        trace,
    })
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

/// The message this surface states when the flow, the alias or the version the alias routes
/// to is one the Bedrock Agent plane does not hold — answered as the HTTP error of its
/// modelled class by an operation that refuses the call, and delivered inside the stream by
/// one that has already begun answering.
///
/// Fixed text for this error class, stated through the ONE frame this plane's flow surface
/// words every unresolved resource with ([`crate::state::flow_resource_not_found_message`]):
/// the sentence names the TYPE of resource the resolution failed on
/// ([`FlowResourceType`]) and no identifier at all.
pub fn not_found_message(resource: FlowResourceType) -> String {
    crate::state::flow_resource_not_found_message(resource.as_wire())
}

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
