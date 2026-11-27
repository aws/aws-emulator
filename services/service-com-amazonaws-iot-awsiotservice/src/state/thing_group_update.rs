// SPDX-License-Identifier: CC0-1.0

//! UPDATING a registered thing group: what a change to a group record asks for, what
//! refuses one, and the single registry write that applies it.
//!
//! `UpdateThingGroup` "Update a thing group": it addresses a group by the name it was
//! created under — the `@httpLabel` of `/thing-groups/{thingGroupName}` — and carries
//! `thingGroupProperties` as a `@required` member, so the properties the request carries
//! are the properties the group has afterwards. `AttributePayload$merge` is the one
//! exception the model states: it "Specifies whether the list of attributes provided in
//! the `AttributePayload` is merged with the attributes stored in the registry, instead
//! of overwriting them", and the note on it names `UpdateThingGroup` as one of the two
//! operations the flag is valid for, so without it the provided list OVERWRITES what the
//! record held.
//!
//! The update runs under OPTIMISTIC CONCURRENCY the same way a thing's does:
//! `UpdateThingGroup$expectedVersion` is "The expected version of the thing group. If
//! this does not match the version of the thing group being updated, the update will
//! fail."
//!
//! The write lives here rather than in the handler for the same reason
//! [`super::ComAmazonawsIotAwsiotserviceState::update_thing`] does: it is the registry's
//! own rule about which changes a group record takes, and a handler maps the refusal onto
//! the error ITS model declares.

use super::{
    thing_group_version_conflict_message, thing_group_write_not_found_message, AttributeUpdate,
    ComAmazonawsIotAwsiotserviceState, RegisteredThingGroup,
};

/// What an update asks the registry to change about one thing group.
///
/// It is the `thingGroupProperties` of the request, in the form the registry applies it:
/// `description` is what the group's description is afterwards (a request carrying none
/// leaves the group without one, because the properties it carries ARE the group's
/// properties), and `attributes` is the provided attribute list with its merge flag.
#[derive(Debug, Clone, Default)]
pub struct ThingGroupUpdate {
    /// `ThingGroupProperties$thingGroupDescription`, as the request carried it.
    pub description: Option<String>,
    /// The attribute list the request provided, with its merge flag. A request carrying
    /// no `attributePayload`, or a payload carrying no `attributes` map, provides the
    /// EMPTY list — which, not being a merge, leaves the group with no attributes.
    pub attributes: AttributeUpdate,
    /// `UpdateThingGroup$expectedVersion` — the version the caller believes the record is
    /// at.
    pub expected_version: Option<i64>,
}

/// Why a thing group record cannot take an update — what
/// [`ComAmazonawsIotAwsiotserviceState::update_thing_group`] answers instead of the
/// updated record.
///
/// It is the registry's reason, not a wire error: the handler maps it onto the error its
/// own model declares, and the wording lives here so every writer states one rule once.
#[derive(Debug, Clone)]
pub enum ThingGroupUpdateRefusal {
    /// `thingGroupName` names no thing group in the registry.
    MissingThingGroup(String),
    /// `expectedVersion` does not match the version the record holds, so the caller is
    /// updating a record it has not seen.
    ///
    /// The two versions are carried for DIAGNOSTICS alone (a `Debug` rendering of why the
    /// write was refused): this family's refusal sentence reports neither of them, only
    /// the group the request addressed.
    VersionConflict {
        /// The group the request addressed.
        thing_group_name: String,
        /// The version the request expected.
        expected: i64,
        /// The version the registry holds.
        registered: i64,
    },
}

impl ThingGroupUpdateRefusal {
    /// What the refusal reports, in the wording this service states each of these in.
    pub fn message(&self) -> String {
        match self {
            // The WRITE half of the thing-group family's not-found sentence: this refusal
            // is answered by the operation that writes the group record addressed by name,
            // which states the group name alone rather than the account-qualified sentence
            // every READER of a group states.
            Self::MissingThingGroup(thing_group_name) => {
                thing_group_write_not_found_message(thing_group_name)
            }
            Self::VersionConflict {
                thing_group_name, ..
            } => thing_group_version_conflict_message(thing_group_name),
        }
    }
}

impl ComAmazonawsIotAwsiotserviceState {
    /// Apply `update` to the thing group registered as `thing_group_name`: the ONE
    /// registry write that changes a group record, answering the record as it stands
    /// afterwards.
    ///
    /// Everything that can refuse the update is decided BEFORE the record is touched, so
    /// a refused update leaves the group — and its version — exactly as it was, which is
    /// the point of `expectedVersion`. A successful one advances the record's `version`,
    /// which is what a later reader reports and a later `expectedVersion` has to match.
    pub fn update_thing_group(
        &mut self,
        thing_group_name: &str,
        update: ThingGroupUpdate,
    ) -> Result<RegisteredThingGroup, ThingGroupUpdateRefusal> {
        let group = self.thing_groups.get_mut(thing_group_name).ok_or_else(|| {
            ThingGroupUpdateRefusal::MissingThingGroup(thing_group_name.to_string())
        })?;
        if let Some(expected) = update.expected_version {
            if group.version != expected {
                return Err(ThingGroupUpdateRefusal::VersionConflict {
                    thing_group_name: group.thing_group_name.clone(),
                    expected,
                    registered: group.version,
                });
            }
        }

        // The group's place in the hierarchy is NOT part of an update: `UpdateThingGroup`
        // models no `parentGroupName`, so the record keeps the parent it was created
        // under.
        group.configuration.description = update.description;
        update.attributes.apply_to(&mut group.configuration.attributes);
        // One update is one write of the record, so the registry version it carries
        // advances by one — which is what a caller holding the old version then conflicts
        // against.
        group.version += 1;
        Ok(group.clone())
    }
}
