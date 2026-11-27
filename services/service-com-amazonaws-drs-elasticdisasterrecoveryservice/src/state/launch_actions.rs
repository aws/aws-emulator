// SPDX-License-Identifier: CC0-1.0

//! The Launch Action resource: what the store holds for one action of one resource, how
//! an action is reported on the wire, and how an action — or the resource an action is
//! put on — that nothing holds is refused.
//!
//! A launch action is NOT addressed on its own: it hangs off a resource
//! (`LaunchActionResourceId`, either a Launch Configuration Template id
//! `^lct-[0-9a-zA-Z]{17}$` or a Source Server id `^s-[0-9a-zA-Z]{17}$`) and is keyed
//! within it by the `actionId` the caller chose. So the store holds one
//! [`LaunchActions`] map per resource, and the three operations over them
//! (`PutLaunchAction`, `ListLaunchActions`, `DeleteLaunchAction`) all resolve the
//! resource the same way, here, once ([`holds_resource`]): an action can only exist on a
//! resource this service holds, and a request naming a resource it does not is the
//! modelled `ResourceNotFoundException` ([`resource_not_found`]) rather than an action
//! created in the void.
//!
//! The ONE place a stored action becomes a wire action ([`LaunchActionRecord::to_wire`],
//! and its re-wrap [`LaunchActionRecord::to_put_wire`]), so an action a put reports and
//! the same action read back by the list cannot disagree about any member.

use std::collections::{BTreeMap, HashMap};

use super::{launch_configuration_templates, ComAmazonawsDrsElasticdisasterrecoveryserviceState};
use crate::ops::types::{
    LaunchAction, LaunchActionParameter, PutLaunchActionOutput, ResourceNotFoundException,
};

/// The launch actions ONE resource carries, keyed by the `actionId` the caller chose.
///
/// A [`BTreeMap`] rather than a `HashMap` because `ListLaunchActions` is `@paginated`: a
/// page boundary is only meaningful over a stable order, and the order this map iterates
/// in is the same for every caller (see [`ordered`] for the order the list reports).
pub type LaunchActions = BTreeMap<String, LaunchActionRecord>;

/// How this service names the launch action resource when it reports one in an error: the
/// `resourceType` of a not-found refusal, and the subject of its message.
pub const RESOURCE_TYPE: &str = "Launch Action";

/// How this service names the Source Server resource in the same place — the other kind
/// of resource a launch action can be put on (the model's `SourceServer`).
pub const SOURCE_SERVER_RESOURCE_TYPE: &str = "Source Server";

/// The prefix of a Source Server id: the `s-[0-9a-zA-Z]{17}` half of the model's
/// `LaunchActionResourceId` pattern, the other half being a Launch Configuration
/// Template id ([`launch_configuration_templates::ID_PREFIX`]).
pub const SOURCE_SERVER_ID_PREFIX: &str = "s-";

/// How many launch actions ONE resource holds before `PutLaunchAction` refuses to add
/// another.
///
/// The model bounds the list the service reports at 200 entries
/// (`smithy_query(query="operation",operation="ListLaunchActions")`: `items`
/// (`LaunchActions`) has `length(min: 0, max: 200)`), so a resource carrying more actions
/// than that could not be reported at all — and nothing would stop a caller growing the
/// state without end.
pub const PER_RESOURCE_LIMIT: usize = 200;

/// The two values the model's `LaunchActionType` lists
/// (`smithy_query(query="shape",shape="LaunchActionType")`), which is what an action
/// reports as its `type`.
const SSM_AUTOMATION: &str = "SSM_AUTOMATION";
const SSM_COMMAND: &str = "SSM_COMMAND";

/// How a Run Command document in the AWS-owned Systems Manager namespace is named — the
/// `AWS-Run…` family (`AWS-RunShellScript`, `AWS-RunPowerShellScript`,
/// `AWS-RunRemoteScript`, …) — which is what [`action_type`] reads an action's type off.
const RUN_COMMAND_DOCUMENT_PREFIX: &str = "AWS-Run";

/// A stored launch action: every member `PutLaunchAction` supplies, the resource it was
/// put on, and the one member the service itself determines ([`action_type`]).
#[derive(Debug, Clone, Default)]
pub struct LaunchActionRecord {
    /// The resource this action hangs off — the `resourceId` the put named, which is
    /// also the key of the [`LaunchActions`] map holding it.
    pub resource_id: String,
    /// The `actionId` the put named, which the resource's map keys this action by.
    pub action_id: String,
    pub action_code: String,
    pub action_version: String,
    /// The kind of SSM document [`Self::action_code`] names, which a put does not supply:
    /// see [`action_type`].
    pub action_type: String,
    pub name: String,
    pub active: bool,
    pub optional: bool,
    pub order: i32,
    pub category: String,
    pub description: String,
    /// Held as the put supplied it — `None` for a put that named no parameters — so a
    /// read reports the action the caller described and nothing the service invented.
    pub parameters: Option<HashMap<String, LaunchActionParameter>>,
}

impl LaunchActionRecord {
    /// The stored action as the modelled `LaunchAction` structure — the item shape of
    /// `ListLaunchActions`.
    pub fn to_wire(&self) -> LaunchAction {
        LaunchAction {
            action_id: Some(self.action_id.clone()),
            action_code: Some(self.action_code.clone()),
            r#type: Some(self.action_type.clone()),
            name: Some(self.name.clone()),
            active: Some(self.active),
            order: Some(self.order),
            action_version: Some(self.action_version.clone()),
            optional: Some(self.optional),
            parameters: self.parameters.clone(),
            description: Some(self.description.clone()),
            category: Some(self.category.clone()),
        }
    }

    /// The same action in the distinct Rust type the generator emits for
    /// `PutLaunchAction`'s output: the modelled `PutLaunchActionResponse`, which carries
    /// every member of `LaunchAction` plus the `resourceId` the action hangs off. A
    /// member-for-member re-wrap of [`Self::to_wire`] rather than a second mapping, so
    /// the put and the list cannot report one action differently.
    pub fn to_put_wire(&self) -> PutLaunchActionOutput {
        let wire = self.to_wire();
        PutLaunchActionOutput {
            resource_id: Some(self.resource_id.clone()),
            action_id: wire.action_id,
            action_code: wire.action_code,
            r#type: wire.r#type,
            name: wire.name,
            active: wire.active,
            order: wire.order,
            action_version: wire.action_version,
            optional: wire.optional,
            parameters: wire.parameters,
            description: wire.description,
            category: wire.category,
        }
    }
}

/// The type of the launch action whose `actionCode` is `action_code`.
///
/// `actionCode` is an SSM document name (the model targets `SsmDocumentName`) and `type`
/// is the TYPE of that document, which no member of a put supplies: a Run Command
/// document makes the action `SSM_COMMAND`, an Automation runbook makes it
/// `SSM_AUTOMATION`. Systems Manager's document catalogue is not part of this service, so
/// the kind is taken from the part of the name that states it — the AWS-owned Run Command
/// documents are the [`RUN_COMMAND_DOCUMENT_PREFIX`] family — and any other document name
/// is an Automation runbook.
///
/// That reading is an APPROXIMATION of a document lookup, and it is the whole of what this
/// member rests on: a Command document named outside that family — one an account owns, or
/// an AWS-owned one whose name does not begin with it — is reported as an Automation
/// runbook here. Resolving it exactly needs the document itself, which no store of this
/// emulator holds; until one does, this is the one place the reading lives, so every
/// operation reporting an action reports the same type for the same document name.
pub fn action_type(action_code: &str) -> String {
    if action_code.starts_with(RUN_COMMAND_DOCUMENT_PREFIX) {
        SSM_COMMAND.to_string()
    } else {
        SSM_AUTOMATION.to_string()
    }
}

/// Whether this service holds the resource `resource_id` addresses — the ONE reading of
/// a `LaunchActionResourceId` the launch action operations perform.
///
/// The modelled pattern admits exactly two kinds of id, and each is looked up in the
/// store that holds that kind: a Launch Configuration Template among the templates, a
/// Source Server among the source servers. A launch action exists only on a resource that
/// does, so every one of the three operations asks this first.
pub fn holds_resource(
    store: &ComAmazonawsDrsElasticdisasterrecoveryserviceState,
    resource_id: &str,
) -> bool {
    store
        .launch_configuration_templates
        .contains_key(resource_id)
        || store.source_servers.contains_key(resource_id)
}

/// The modelled `ResourceNotFoundException` (HTTP 404) for the resource a launch action
/// request named that this service does not hold.
///
/// The `resourceType` is the kind of resource the id addresses, read off the id the way
/// the model's two-branch `LaunchActionResourceId` pattern spells it, so a client is told
/// which resource was looked for; the `resourceId` is the id the request named, verbatim.
pub fn resource_not_found(resource_id: &str) -> ResourceNotFoundException {
    let resource_type = if resource_id.starts_with(SOURCE_SERVER_ID_PREFIX) {
        SOURCE_SERVER_RESOURCE_TYPE
    } else {
        launch_configuration_templates::RESOURCE_TYPE
    };
    ResourceNotFoundException {
        message: Some(format!(
            "{resource_type} with Id: '{resource_id}' was not found"
        )),
        code: None,
        resource_id: Some(resource_id.to_string()),
        resource_type: Some(resource_type.to_string()),
    }
}

/// The modelled `ResourceNotFoundException` (HTTP 404) for a launch action the resource
/// does not carry — the same message form every id-keyed operation of this service
/// reports: the service's name for the resource type, then the id it was asked for, then
/// that it was not found.
pub fn not_found(action_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "{RESOURCE_TYPE} with Id: '{action_id}' was not found"
        )),
        code: None,
        resource_id: Some(action_id.to_string()),
        resource_type: Some(RESOURCE_TYPE.to_string()),
    }
}

/// One resource's actions in the order the service reports them: by `order` ascending —
/// the member that says when an action runs relative to the others — and, for two actions
/// sharing an order, by `actionId`, so the sequence is total and the same for every
/// caller.
///
/// A list of BORROWS paired with each action's id: only the page the caller asked for is
/// ever rendered (see [`super::paginate`], whose token names the id the next page starts
/// at).
pub fn ordered(actions: &LaunchActions) -> Vec<(&str, &LaunchActionRecord)> {
    let mut ordered: Vec<(&str, &LaunchActionRecord)> = actions
        .iter()
        .map(|(id, record)| (id.as_str(), record))
        .collect();
    ordered.sort_by(|(left_id, left), (right_id, right)| {
        left.order.cmp(&right.order).then(left_id.cmp(right_id))
    });
    ordered
}
