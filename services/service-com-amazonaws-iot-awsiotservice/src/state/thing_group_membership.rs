// SPDX-License-Identifier: CC0-1.0

//! WHICH THINGS are in which thing group, and what makes a group DYNAMIC.
//!
//! A thing group's membership is the relation `AddThingToThingGroup` and
//! `RemoveThingFromThingGroup` write and `ListThingsInThingGroup` /
//! `ListThingGroupsForThing` read. It is kept on the group's own record
//! ([`super::RegisteredThingGroup::members`]) rather than in a map beside it: a
//! membership disappears with the group it is a membership of, and the group is
//! already the record both ends of the relation are reached through.
//!
//! A DYNAMIC thing group is a group whose membership is not written at all but
//! DERIVED: "Creates a dynamic thing group" (`CreateDynamicThingGroup`) records a
//! fleet-index query, and the things the group holds are the things that query
//! selects. That is why a dynamic group carries a [`DynamicGroupQuery`] and takes no
//! membership write: adding a thing to one by hand would claim a membership the query
//! does not give it.

use std::time::SystemTime;

/// How many thing groups one thing may belong to.
///
/// AWS IoT's published quota: "Maximum number of thing groups a thing can belong to"
/// is 10. `AddThingToThingGroup` models no `LimitExceededException`
/// (`smithy_query(query="operation",operation="AddThingToThingGroup")` lists
/// InternalFailure, InvalidRequest, ResourceNotFound and Throttling), so a thing
/// already in that many groups is refused with the modelled `InvalidRequestException`
/// naming the limit.
pub const MAX_THING_GROUPS_PER_THING: usize = 10;

/// How many things one thing group may hold in this emulator.
///
/// A membership relation is one a CALLER grows, a thing at a time, so it is bounded
/// for the same reason [`super::MAX_THINGS`] bounds the registry itself: past the
/// ceiling nothing is recorded and the add is refused, rather than one account's
/// group growing without bound.
pub const MAX_THINGS_PER_THING_GROUP: usize = super::MAX_THINGS;

/// How many DYNAMIC thing groups this emulator keeps for the account.
///
/// `CreateDynamicThingGroup` models `LimitExceededException` ("A limit has been
/// exceeded", HTTP 410)
/// (`smithy_query(query="operation",operation="CreateDynamicThingGroup")`), so the
/// service caps how many such groups an account may hold; the exact figure is not in
/// the model, so this is the emulator's own ceiling on a store a caller grows — the
/// same reason [`super::MAX_POLICIES`] exists — and the create past it refuses rather
/// than growing without bound.
pub const MAX_DYNAMIC_THING_GROUPS: usize = 100;

/// The `DynamicGroupStatus` a dynamic thing group holds once its query is recorded.
///
/// `DynamicGroupStatus` is `ACTIVE | BUILDING | REBUILDING`
/// (`smithy_query(query="shape",shape="DynamicGroupStatus")`, reached through
/// [`crate::ops::types::DYNAMIC_GROUP_STATUS_VALUES`]). The query a group selects its
/// members with is recorded by the create itself, so there is no build the clock still
/// owes the group and a reader reports it active.
pub const DYNAMIC_GROUP_ACTIVE: &str = "ACTIVE";

/// What makes a thing group DYNAMIC: the fleet-index query whose things it holds.
///
/// Every member is the create's own
/// (`smithy_query(query="operation",operation="CreateDynamicThingGroup")`), kept so a
/// reader of the group hands back the query that defines it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DynamicGroupQuery {
    /// `CreateDynamicThingGroup$indexName` — "The dynamic thing group index name.
    /// Currently one index is supported: AWS_Things" — so a request that named none
    /// records [`super::THING_INDEX_NAME`].
    pub index_name: String,
    /// `CreateDynamicThingGroup$queryString` — "The dynamic thing group search query
    /// string", verbatim as the writer supplied it.
    pub query_string: String,
    /// `CreateDynamicThingGroup$queryVersion`, when the request carried one.
    pub query_version: Option<String>,
}

/// How this service states that a thing group a request named is DYNAMIC, so its
/// membership is the query's to decide and not a caller's to write.
///
/// The fixed text is the class's; the variable part is the group the request named.
pub fn dynamic_group_membership_message(thing_group_name: &str) -> String {
    format!(
        "Cannot add a thing to the dynamic thing group {thing_group_name}: a dynamic thing \
         group's membership is defined by its query"
    )
}

/// How this service states that the things a DYNAMIC thing group holds are not known.
///
/// A dynamic group's members are the things its query selects, and this emulator does
/// not run a fleet-index query over the registry — so every operation that would have
/// to know those things (work targeting the group, a listing of its things) states
/// that fact and refuses, rather than answering with the nobody the group expanded to.
/// A STATIC group's members are the ones written to it
/// ([`super::ComAmazonawsIotAwsiotserviceState::things_in_thing_group`]).
pub fn dynamic_group_things_unknown_message(thing_group_name: &str) -> String {
    format!(
        "the things thing group {thing_group_name} holds are the things its query selects, \
         which this emulator does not evaluate"
    )
}

/// Why a membership write is refused.
///
/// Each variant is a DIFFERENT refusal — two of them are the modelled
/// `ResourceNotFoundException` and two the modelled `InvalidRequestException` — so a
/// handler maps one value onto the error the model declares for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MembershipRefusal {
    /// The request named a thing group this account does not hold; carries the name.
    MissingThingGroup(String),
    /// The request named a thing the registry does not hold; carries the name.
    MissingThing(String),
    /// The group is dynamic, so its membership is derived; carries the name.
    DynamicThingGroup(String),
    /// The thing already belongs to [`MAX_THING_GROUPS_PER_THING`] groups; carries
    /// the thing's name.
    TooManyGroupsForThing(String),
    /// The group already holds [`MAX_THINGS_PER_THING_GROUP`] things; carries the
    /// group's name.
    ThingGroupFull(String),
}

impl MembershipRefusal {
    /// What to tell the caller: the rule the request broke, built from the request
    /// that broke it.
    pub fn message(&self) -> String {
        match self {
            Self::MissingThingGroup(thing_group_name) => {
                super::not_found::missing_thing_group_message(thing_group_name)
            }
            Self::MissingThing(thing_name) => {
                super::not_found::missing_thing_message(thing_name)
            }
            Self::DynamicThingGroup(thing_group_name) => {
                dynamic_group_membership_message(thing_group_name)
            }
            Self::TooManyGroupsForThing(thing_name) => format!(
                "Thing {thing_name} already belongs to the maximum of \
                 {MAX_THING_GROUPS_PER_THING} thing groups"
            ),
            Self::ThingGroupFull(thing_group_name) => format!(
                "Thing group {thing_group_name} already holds the maximum of \
                 {MAX_THINGS_PER_THING_GROUP} things"
            ),
        }
    }
}

/// Which `thingGroupName` a request that may name a group EITHER by name or by ARN is
/// about, and `None` for one that named neither.
///
/// `AddThingToThingGroup` takes `thingGroupName` and `thingGroupArn` and requires
/// neither, because a group is identified by either; the name inside a thing-group
/// ARN is the group's identity, so an ARN is read down to that name and from then on
/// the two are one request.
pub fn named_thing_group(name: Option<&str>, arn: Option<&str>) -> Option<String> {
    if let Some(name) = name.filter(|name| !name.is_empty()) {
        return Some(name.to_string());
    }
    let arn = arn?;
    ::service_common::arn::resource_id_of_type(arn, "thinggroup")
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

/// Which `thingName` such a request is about, under the rule
/// [`named_thing_group`] states for a group.
pub fn named_thing(name: Option<&str>, arn: Option<&str>) -> Option<String> {
    if let Some(name) = name.filter(|name| !name.is_empty()) {
        return Some(name.to_string());
    }
    let arn = arn?;
    ::service_common::arn::resource_id_of_type(arn, "thing")
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

impl super::ComAmazonawsIotAwsiotserviceState {
    /// Record that the thing `thing_name` is in the thing group `thing_group_name`:
    /// the ONE membership write, so every writer of the relation agrees on what it
    /// takes.
    ///
    /// Both ends have to EXIST — a membership between records this account does not
    /// hold would be a relation over nothing — and the group has to be a STATIC one,
    /// since a dynamic group's membership is its query's. A thing already in the
    /// group writes nothing and succeeds: `AddThingToThingGroup` names no new
    /// identity, so repeating it leaves the same one membership.
    pub fn add_thing_to_thing_group(
        &mut self,
        thing_group_name: &str,
        thing_name: &str,
    ) -> Result<(), MembershipRefusal> {
        if !self.things.contains_key(thing_name) {
            return Err(MembershipRefusal::MissingThing(thing_name.to_string()));
        }
        let Some(group) = self.thing_groups.get(thing_group_name) else {
            return Err(MembershipRefusal::MissingThingGroup(
                thing_group_name.to_string(),
            ));
        };
        if group.dynamic_query.is_some() {
            return Err(MembershipRefusal::DynamicThingGroup(
                thing_group_name.to_string(),
            ));
        }
        if group.members.contains(thing_name) {
            return Ok(());
        }
        if group.members.len() >= MAX_THINGS_PER_THING_GROUP {
            return Err(MembershipRefusal::ThingGroupFull(
                thing_group_name.to_string(),
            ));
        }
        if self.static_thing_groups_of(thing_name).len() >= MAX_THING_GROUPS_PER_THING {
            return Err(MembershipRefusal::TooManyGroupsForThing(
                thing_name.to_string(),
            ));
        }
        if let Some(group) = self.thing_groups.get_mut(thing_group_name) {
            group.members.insert(thing_name.to_string());
        }
        Ok(())
    }

    /// The names of the STATIC thing groups the thing `thing_name` has been added to,
    /// in registry order.
    ///
    /// Derived from the groups' own membership, so there is one record of the
    /// relation and not a second copy on the thing.
    pub fn static_thing_groups_of(&self, thing_name: &str) -> Vec<String> {
        self.thing_groups
            .values()
            .filter(|group| group.members.contains(thing_name))
            .map(|group| group.thing_group_name.clone())
            .collect()
    }

    /// How a THING GROUP target of work that runs on the devices a group names is
    /// refused, or `None` for a group this service can expand.
    ///
    /// A group the registry does not hold names no devices at all, which is the
    /// modelled 404; a DYNAMIC group's devices are the things its query selects, which
    /// this emulator does not evaluate, so it is the modelled 400 — never a 200 that
    /// reports group-targeted work as dispatched while queueing nothing. A static
    /// group is expanded into its members, however many it holds, including none.
    pub fn thing_group_target_refusal(
        &self,
        thing_group_name: &str,
    ) -> Option<super::JobTargetsRefusal> {
        match self.thing_groups.get(thing_group_name) {
            None => Some(super::JobTargetsRefusal::NotFound(
                super::not_found::missing_thing_group_message(thing_group_name),
            )),
            Some(group) if group.dynamic_query.is_some() => {
                Some(super::JobTargetsRefusal::Invalid(
                    dynamic_group_things_unknown_message(thing_group_name),
                ))
            }
            Some(_) => None,
        }
    }

    /// The names of the things the thing group `thing_group_name` holds, in registry
    /// order.
    ///
    /// For a STATIC group those are the memberships that were written to it. A DYNAMIC
    /// group holds the things its query selects, which this emulator does not evaluate
    /// — so a caller is never handed this as that group's membership: every operation
    /// that would have to know them refuses first
    /// ([`dynamic_group_things_unknown_message`]).
    pub fn things_in_thing_group(&self, thing_group_name: &str) -> Vec<String> {
        self.thing_groups
            .get(thing_group_name)
            .map(|group| group.members.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// How many DYNAMIC thing groups the account holds.
    pub fn dynamic_thing_group_count(&self) -> usize {
        self.thing_groups
            .values()
            .filter(|group| group.dynamic_query.is_some())
            .count()
    }

    /// The names of every thing group BELOW `thing_group_name` in the hierarchy, in
    /// registry order.
    ///
    /// A group's place in the hierarchy is its own record's `parentGroupName`
    /// ([`super::ThingGroupConfiguration`]), so the descendants are derived from those
    /// records and never recorded beside them. The walk is bounded by the number of
    /// groups that exist, so a record whose parent chain is somehow cyclic terminates
    /// instead of spinning.
    pub fn descendant_thing_groups(&self, thing_group_name: &str) -> Vec<String> {
        let mut descendants: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut frontier = vec![thing_group_name.to_string()];
        while let Some(parent) = frontier.pop() {
            for group in self.thing_groups.values() {
                let is_child = group
                    .configuration
                    .parent_group_name
                    .as_deref()
                    .is_some_and(|parent_group_name| parent_group_name == parent);
                if is_child && descendants.insert(group.thing_group_name.clone()) {
                    frontier.push(group.thing_group_name.clone());
                }
            }
        }
        descendants.into_iter().collect()
    }
}

impl super::RegisteredThingGroup {
    /// A newly created DYNAMIC thing group: a group record like any other, plus the
    /// query that decides its membership.
    pub fn new_dynamic(
        thing_group_name: String,
        thing_group_id: String,
        thing_group_arn: String,
        configuration: super::ThingGroupConfiguration,
        tags: std::collections::BTreeMap<String, Option<String>>,
        creation_date: SystemTime,
        query: DynamicGroupQuery,
    ) -> Self {
        let mut group = Self::new(
            thing_group_name,
            thing_group_id,
            thing_group_arn,
            configuration,
            tags,
            creation_date,
        );
        group.dynamic_query = Some(query);
        group
    }
}
