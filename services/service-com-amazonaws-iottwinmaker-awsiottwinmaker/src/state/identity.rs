// SPDX-License-Identifier: CC0-1.0

//! How a resource of this service is NAMED — its ARN, the Region a request
//! addresses — and the messages a refusal that could not resolve one carries.
//!
//! One concern of [`crate::state`], kept in its own file: every operation
//! reaches these through the parent module, which re-exports them.

use super::*;

/// The greatest number of characters the modelled `ErrorMessage` shape — the
/// member every one of this service's modelled refusals states its reason on —
/// admits: `@length(0..2048)`, from
/// `smithy_query(query="shape", shape="ErrorMessage")`.
pub const ERROR_MESSAGE_MAX_CHARS: usize = 2048;

/// What a message that does not fit the member it is reported on is cut with.
///
/// The cut is marked rather than silent, so a caller can tell a message the
/// service shortened from one it worded that way.
const MESSAGE_ELISION: &str = "...";

/// `message` rendered so it satisfies the `@length` maximum of the member it is
/// reported on, which is `max_chars` characters.
///
/// A message names what the request named, and an identifier the model bounds
/// as widely as — or more widely than — the member the message travels on
/// renders text past that member's own bound: a `ComponentPath` of 2048
/// characters, or a `Name` of 256, stated on a member capped at 256. The
/// message that already fits is returned unchanged; a longer one keeps its
/// leading characters AND its trailing ones around [`MESSAGE_ELISION`], because
/// these messages name the thing first and give the reason last, so cutting
/// only the tail would drop the reason. The result is exactly `max_chars`
/// characters — Smithy measures a string's length in characters, so the cut is
/// counted and made in characters rather than in bytes.
pub fn clamped_message(message: String, max_chars: usize) -> String {
    let characters: Vec<char> = message.chars().collect();
    if characters.len() <= max_chars {
        return message;
    }
    let elision: Vec<char> = MESSAGE_ELISION.chars().collect();
    // A member too small to hold the marker itself holds as much of the
    // message as it admits and nothing else.
    if max_chars <= elision.len() {
        return characters[..max_chars].iter().collect();
    }
    let kept = max_chars - elision.len();
    let tail = kept / 2;
    let head = kept - tail;
    characters[..head]
        .iter()
        .chain(elision.iter())
        .chain(characters[characters.len() - tail..].iter())
        .collect()
}

/// The message for a resource no ARN in this service's index names.
///
/// Built from the request, naming the ARN that was looked for — the same
/// construction as [`entity_not_found_message`], one level up from an id to
/// the ARN the tagging operations address a resource by.
///
/// The ARN is a `TwinMakerArn`, which the model bounds as widely as the member
/// every refusal of this service states its reason on, so the message is brought
/// within that bound HERE ([`clamped_message`]) rather than at each reader of
/// it: the bound then holds for every caller of this construction, including one
/// added later.
pub fn resource_not_found_message(resource_arn: &str) -> String {
    clamped_message(
        format!("Resource {resource_arn} not found."),
        ERROR_MESSAGE_MAX_CHARS,
    )
}

/// The message for an entity the workspace the request named does not hold.
///
/// Built from the request: it names the entity that was looked for and the
/// workspace it was looked for in, which is the construction the service's own
/// `ResourceNotFoundException` for an entity uses.
pub fn entity_not_found_message(workspace_id: &str, entity_id: &str) -> String {
    format!("Entity {entity_id} in workspace {workspace_id} not found.")
}

/// The message for a component type the workspace the request named does not
/// hold. Built from the request, naming what was looked for and where, in the
/// sentence the service itself answers for that miss: "Cannot find component
/// type ", " in workspace " and the trailing period are fixed, and the two ids
/// come from the request.
pub fn component_type_not_found_message(workspace_id: &str, component_type_id: &str) -> String {
    format!("Cannot find component type {component_type_id} in workspace {workspace_id}.")
}

/// What still references a component type, as the ids of the resources the
/// references live on: the answer
/// [`Workspace::component_type_references`] gives, and what the in-use refusal
/// of a delete is built from.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ComponentTypeReferences {
    /// The ids of the entities one of whose components — top-level or composite
    /// — instantiates the component type, each listed once however many of its
    /// components do, in id order.
    pub entity_ids: Vec<String>,
    /// The ids of the other component types that extend the component type or
    /// name it in their composite declarations, each listed once, in id order.
    pub component_type_ids: Vec<String>,
}

impl ComponentTypeReferences {
    /// Whether nothing stored references the component type, which is what makes
    /// it deletable.
    pub fn is_empty(&self) -> bool {
        self.entity_ids.is_empty() && self.component_type_ids.is_empty()
    }

    /// The message a delete blocked by these references is refused with, or
    /// `None` when there are none.
    ///
    /// The refusal names the RESOURCES that have to go first, as a bracketed,
    /// comma-separated list of their ids, and asks for them to be deleted: that
    /// is what lets a caller drive the remediation off the refusal. Entities come
    /// first where both kinds reference the type, so the caller is asked for one
    /// kind at a time; nothing else of the service's state is named.
    pub fn in_use_message(&self, component_type_id: &str) -> Option<String> {
        if !self.entity_ids.is_empty() {
            return Some(format!(
                "Component type {component_type_id} is being used by entities [{}], please \
                 delete the entities first.",
                self.entity_ids.join(", ")
            ));
        }
        if !self.component_type_ids.is_empty() {
            return Some(format!(
                "Component type {component_type_id} is being used by component types [{}], \
                 please delete the component types first.",
                self.component_type_ids.join(", ")
            ));
        }
        None
    }
}

/// The message for a component an entity the request named does not have —
/// whichever member addressed it, the `componentName` it is called by or the
/// `componentPath` it would sit at, since either way the value names the
/// component that could not be resolved.
///
/// Built from the request, naming the component, the entity it was looked for on
/// and the workspace that entity lives in. This is the ONE construction for
/// "the entity is there, the component on it is not", which is a different miss
/// from [`entity_not_found_message`]'s and is reported as a bad reference rather
/// than as a missing resource.
pub fn component_not_found_message(
    workspace_id: &str,
    entity_id: &str,
    component: &str,
) -> String {
    format!("Component {component} of Entity {entity_id} in workspace {workspace_id} not found.")
}

/// The message for a scene the workspace the request named does not hold.
/// Built from the request, naming what was looked for and where — the same
/// construction as [`entity_not_found_message`] one resource across.
pub fn scene_not_found_message(workspace_id: &str, scene_id: &str) -> String {
    format!("Scene {scene_id} in workspace {workspace_id} not found.")
}

/// The message for a cancellation of a job that has already finished. Built
/// from the request and the store: it names the job and the terminal state it
/// finished in, which is what makes the conflict actionable for a caller that
/// polled a moment too late.
pub fn metadata_transfer_job_terminal_message(
    metadata_transfer_job_id: &str,
    state: &str,
) -> String {
    format!(
        "MetadataTransferJob {metadata_transfer_job_id} is {state} and can no longer be cancelled."
    )
}

/// The message this service reports for a sync job the workspace the request
/// named does not hold.
///
/// ONE fixed sentence for that miss, whichever operation reports it: the service
/// interpolates nothing into it — neither the sync source that was looked for
/// nor the workspace it was looked for in — so a client matching on the text
/// sees the same string from every reader of a sync job. Unlike
/// [`scene_not_found_message`] and [`entity_not_found_message`], which this
/// service does build from the request, this miss is reported as a constant.
pub const SYNC_JOB_NOT_FOUND_MESSAGE: &str = "Provided Sync Job does not exist!";

/// The message for a workspace this service does not hold, on a path that LOOKS
/// a workspace UP to serve the request: built from the request, naming the
/// workspace that was looked for.
///
/// The service's not-found message is per-operation, not one string for the
/// whole API: the look-up path reports what it could not find for the id it was
/// given, while the workspace's own UPDATE path reports the account-scoped form
/// [`workspace_does_not_exist_message`] builds. Every operation that resolves a
/// workspace in order to read or write something it contains uses this one.
///
/// A workspace is addressed by an id on most of those paths and by a
/// `TwinMakerArn` on the metadata-transfer one, and that ARN is bounded as
/// widely as the member the reason travels on, so the message is brought within
/// that bound HERE ([`clamped_message`]) rather than at each reader of it.
pub fn workspace_not_found_message(workspace_id: &str) -> String {
    clamped_message(
        format!("Unable to find workspace for id: {workspace_id}"),
        ERROR_MESSAGE_MAX_CHARS,
    )
}

/// The message the workspace UPDATE path reports for a workspace this service
/// does not hold: built from the request and the caller's account, which is what
/// scopes a workspace id.
///
/// The other construction of the same miss is [`workspace_not_found_message`];
/// see it for why the two differ.
pub fn workspace_does_not_exist_message(workspace_id: &str) -> String {
    format!(
        "Workspace with id {}|{workspace_id} does not exist.",
        ::emulator_server::emulator_account_id()
    )
}

/// The message for a delete of an entity that still has children, which only
/// `isRecursive` removes: built from the request, naming the entity the delete
/// addressed and the workspace it addressed it in.
pub fn entity_has_children_message(workspace_id: &str, entity_id: &str) -> String {
    format!("Cannot delete entity {entity_id} in {workspace_id} as it has child entities.")
}

/// The message for a create whose id is already TAKEN, as the message a
/// `ConflictException` carries.
///
/// Built entirely from the request, in the service's own construction: the kind
/// of resource and the id the create asked for first, then the workspace that
/// holds it where the resource lives inside one, then what is wrong with it —
/// the same order the not-found family above reads in
/// ([`entity_not_found_message`]), with "already exists" in place of "not
/// found". `kind` is the resource's name as the service writes it in this
/// sentence; `workspace_id` is `None` for a resource that nests under no
/// workspace. Shared by every create of this service, so one conflict reads one
/// way wherever it is refused.
pub fn already_exists_message(kind: &str, resource_id: &str, workspace_id: Option<&str>) -> String {
    match workspace_id {
        Some(workspace_id) => {
            format!("{kind} {resource_id} in workspace {workspace_id} already exists.")
        }
        None => format!("{kind} {resource_id} already exists."),
    }
}

/// The Region this request is served in: the region of its SigV4 credential
/// scope, or [`DEFAULT_REGION`] when it is unsigned.
///
/// The ONE answer to "which Region is this?", used both to pick the store that
/// answers the request
/// ([`region`](ComAmazonawsIottwinmakerAwsiottwinmakerState::region)) and to
/// build the ARNs it mints, so a resource can never be filed in one Region and
/// stamped with another.
///
/// A scope region that is not of the shape the modelled `TwinMakerArn` admits
/// for its region segment (`[a-z0-9-]+`) is replaced by the default, so a
/// caller cannot drive this service into minting an ARN its own model forbids.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of a workspace.
pub fn workspace_arn(region: &str, workspace_id: &str) -> String {
    ::engine::emulator_arn(
        "iottwinmaker",
        region,
        &format!("{WORKSPACE_ARN_RESOURCE_PREFIX}{workspace_id}"),
    )
}

/// The resource prefix of a workspace's ARN, shared by the ARN [`workspace_arn`]
/// mints and the id [`workspace_id_in_arn`] reads back out of it, so one
/// definition says what the shape is.
const WORKSPACE_ARN_RESOURCE_PREFIX: &str = "workspace/";

/// The `workspaceId` inside a workspace ARN, or `None` when the value is not
/// one: the inverse of [`workspace_arn`], for a caller holding a stored ARN and
/// comparing it against an id.
///
/// Unlike
/// [`resolve_workspace_id`](RegionalState::resolve_workspace_id),
/// this reads the id out of the ARN rather than resolving it against the stored
/// workspaces, so it still names the workspace of an ARN whose workspace has
/// been deleted. An ARN's resource portion is its sixth colon-separated field.
pub fn workspace_id_in_arn(arn: &str) -> Option<&str> {
    arn.splitn(6, ':')
        .nth(5)?
        .strip_prefix(WORKSPACE_ARN_RESOURCE_PREFIX)
}

/// The ARN of an entity, nested under its workspace's resource path.
pub fn entity_arn(region: &str, workspace_id: &str, entity_id: &str) -> String {
    ::engine::emulator_arn(
        "iottwinmaker",
        region,
        &format!("workspace/{workspace_id}/entity/{entity_id}"),
    )
}

/// The ARN of a component type, nested under its workspace's resource path the
/// way [`entity_arn`] nests an entity's.
pub fn component_type_arn(region: &str, workspace_id: &str, component_type_id: &str) -> String {
    ::engine::emulator_arn(
        "iottwinmaker",
        region,
        &format!("workspace/{workspace_id}/componentType/{component_type_id}"),
    )
}

/// The ARN of a scene, nested under its workspace's resource path the way
/// [`entity_arn`] nests an entity's.
pub fn scene_arn(region: &str, workspace_id: &str, scene_id: &str) -> String {
    ::engine::emulator_arn(
        "iottwinmaker",
        region,
        &format!("workspace/{workspace_id}/scene/{scene_id}"),
    )
}

/// The ARN of a sync job, nested under its workspace's resource path the way
/// [`scene_arn`] nests a scene's. The sync SOURCE takes the place of the id,
/// being the job's identity inside its workspace.
pub fn sync_job_arn(region: &str, workspace_id: &str, sync_source: &str) -> String {
    ::engine::emulator_arn(
        "iottwinmaker",
        region,
        &format!("workspace/{workspace_id}/sync-job/{sync_source}"),
    )
}

/// The ARN of a metadata transfer job. An ACCOUNT-level resource, so — unlike
/// [`scene_arn`] — its resource portion nests under no workspace.
pub fn metadata_transfer_job_arn(region: &str, metadata_transfer_job_id: &str) -> String {
    ::engine::emulator_arn(
        "iottwinmaker",
        region,
        &format!("metadata-transfer-job/{metadata_transfer_job_id}"),
    )
}

/// The ARN of the role a metadata transfer job runs under: the service-linked
/// role at [`METADATA_TRANSFER_JOB_ROLE_PATH`], in this emulator's account.
///
/// IAM is global, so the ARN carries no region — which is also what the
/// modelled `RoleArn` pattern (`^arn:…:iam::[0-9]{12}:role/`) requires.
pub fn metadata_transfer_job_role() -> String {
    ::engine::emulator_arn("iam", "", METADATA_TRANSFER_JOB_ROLE_PATH)
}

/// The message for a metadata transfer job this account does not hold. Built
/// from the request, naming the job that was looked for — the same construction
/// as [`resource_not_found_message`], one resource across.
pub fn metadata_transfer_job_not_found_message(metadata_transfer_job_id: &str) -> String {
    format!("MetadataTransferJob {metadata_transfer_job_id} not found.")
}
