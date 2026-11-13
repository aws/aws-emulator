// SPDX-License-Identifier: CC0-1.0

//! Which stack already MANAGES a physical resource name, and the reason CloudFormation gives
//! a create that would take a name over again.
//!
//! ## The rule this module implements
//!
//! A physical resource name identifies ONE resource: CloudFormation cannot create a second
//! resource under a name a stack is already managing, so a plan that would do it is not a
//! plan that can be carried out. CloudFormation establishes that ITSELF, from its own record
//! of what the stacks of the account and Region hold, rather than from the refusal the owning
//! service would answer — so the failure is reported in CLOUDFORMATION's own words
//! ([`already_exists_in_stack`]), naming the name and the stack that holds it, with no
//! resource-handler wrapper around it.
//!
//! Three places ask it. The DEPLOYMENT asks it of the stack it is deploying, before it asks
//! any owning service to create the resource ([`taken_in_the_stack`]): two declarations of one
//! stack that resolve to one name collide with each other, and the second one fails. It asks
//! it of the OTHER live stacks of the Region when an owning service has REFUSED a create
//! because the name is in use ([`taken_by_another_stack`], over the index
//! [`ComAmazonawsCloudformationCloudformationState::managed_physical_names`] the deploying
//! operation reads under the store lock): that service can only speak about its own namespace,
//! while CloudFormation knows which STACK holds the name, so the resource's event carries this
//! reason naming that stack instead of the service's own report. The CHANGE SET asks it of
//! every stack ([`managing`]): a name another stack owns is a name this plan cannot adopt,
//! which is what `ImportExistingResources` is held to.
//!
//! The cross-stack question is asked at the REFUSAL rather than ahead of the create because
//! the owning service is what decides whether the name is in use at all: a create that service
//! ACCEPTS asked for a resource that is now the one the declaration describes — Amazon S3
//! answers a re-create of a bucket you already own in its first Region with 200, and an Amazon
//! SNS `CreateTopic` of an existing topic answers that topic — and CloudFormation reports those
//! deployments as the completions they are. So what this index adds is not a second gate but
//! the half of the diagnosis the owning service cannot give: WHICH stack holds the name it
//! refused.
//!
//! Nothing in the two questions a deployment asks reads a store: the caller holds the records
//! (a stack's own, or the index of the Region's) and passes them in, so neither needs this
//! module to lock anything.

use std::sync::Arc;

use super::{ComAmazonawsCloudformationCloudformationState, StoredResource, StoredStack};

/// The reason CloudFormation gives for a resource it will not create because the name is
/// already taken: the physical name, and the stack that manages the resource holding it.
///
/// The fixed text is CloudFormation's own; the variable parts are the physical resource id the
/// declaration resolved to and the `StackId` of the stack that manages it.
pub fn already_exists_in_stack(physical_resource_id: &str, stack_id: &str) -> String {
    format!("{physical_resource_id} already exists in stack {stack_id}")
}

/// The record of `recorded` whose resource holds the physical name `candidate` would be
/// created under, or `None` when nothing recorded there holds it.
///
/// A record of the SAME logical resource is never a collision: that is the resource itself,
/// which is what a deployment carries forward or replaces. A record with no physical name at
/// all cannot collide with anything, and the question is asked per TYPE, because a physical
/// name identifies a resource of one type — and, for a type whose resource lives INSIDE a
/// parent, within that parent ([`scope_of`]), because that is the scope the owning service
/// identifies one of its resources in.
pub fn taken_in_the_stack<'a>(
    candidate: &StoredResource,
    recorded: impl IntoIterator<Item = &'a StoredResource>,
) -> Option<&'a StoredResource> {
    if candidate.physical_resource_id.is_empty() {
        return None;
    }
    let scope = scope_of(candidate);
    recorded.into_iter().find(|existing| {
        existing.materialised
            && existing.logical_id != candidate.logical_id
            && existing.resource_type == candidate.resource_type
            && existing.physical_resource_id == candidate.physical_resource_id
            && scope_of(existing) == scope
    })
}

/// The scope a resource's physical name identifies it in: the values its declaration states
/// for the properties naming its parent ([`super::template::name_scope_properties`]), which
/// is the empty list for a type whose name identifies a resource on its own.
pub fn scope_of(resource: &StoredResource) -> Vec<String> {
    super::template::name_scope(&resource.resource_type, &resource.properties)
}

/// One physical resource name a live stack of a Region already MANAGES: the type of the
/// resource that holds it, the name itself, and the `StackId` of the stack that manages it.
///
/// This is CloudFormation's own record of who owns which name — the half of the question
/// [`taken_in_the_stack`] cannot answer, since a stack holds no record of another stack's
/// resources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedName {
    pub resource_type: String,
    pub physical_resource_id: String,
    pub stack_id: String,
    /// The scope that name identifies the resource in ([`scope_of`]): the parent a resource
    /// of this type lives inside, for a type whose name is unique only within one.
    pub scope: Vec<String>,
}

/// The index of one Region's managed names, as the deployments that are held to it share it.
///
/// SHARED rather than copied: one read of the store answers every deployment a request sets
/// off — a stack-set operation deploys into every one of its targets out of one call — so the
/// index of a Region is built once per Region and each deployment holds the same one.
pub type ManagedNames = Arc<Vec<ManagedName>>;

impl ComAmazonawsCloudformationCloudformationState {
    /// Every physical resource name the live stacks of `region` manage, each with the stack
    /// that manages it.
    ///
    /// Scoped by REGION, as [`managing`] scopes the same question: a stack's record carries no
    /// account of its own, so the account half of "this account and Region" is the one every
    /// stack of this store belongs to.
    ///
    /// Read by the operations that DEPLOY a stack, under the store lock, and passed into the
    /// deployment as part of the stack's provenance
    /// ([`super::provisioning::StackProvenance::managed_names`]): the walk that creates the
    /// resources holds no lock, so the index it is held to is the one taken when the
    /// deployment began.
    ///
    /// A stack whose deletion has completed manages nothing, and neither does a record of a
    /// resource nothing created or one a teardown has removed
    /// (`StoredResource::materialised`), so a name is in this index exactly while a live stack
    /// really holds a resource under it.
    pub fn managed_physical_names(&self, region: &str) -> ManagedNames {
        Arc::new(
            self.live_stacks(region)
                .flat_map(|stack| {
                    stack.resources.iter().filter_map(|deployed| {
                        let resource = &deployed.resource;
                        if !resource.materialised || resource.physical_resource_id.is_empty() {
                            return None;
                        }
                        Some(ManagedName {
                            resource_type: resource.resource_type.clone(),
                            physical_resource_id: resource.physical_resource_id.clone(),
                            stack_id: stack.stack_id.clone(),
                            scope: scope_of(resource),
                        })
                    })
                })
                .collect(),
        )
    }
}

/// The entry of `managed` whose stack already holds the physical name `candidate` would be
/// created under, or `None` when no OTHER stack holds it.
///
/// `deploying` is the `StackId` of the stack being deployed, whose own names are not a
/// collision: a resource of that stack is the resource itself — carried forward, replaced or
/// re-created by its own rollback — and a second declaration of it inside that stack is what
/// [`taken_in_the_stack`] answers. The question is asked per TYPE, because a physical name
/// identifies a resource of one type — and within the scope that type's resources are
/// identified in ([`scope_of`]) — and an unnamed candidate collides with nothing.
pub fn taken_by_another_stack<'a>(
    candidate: &StoredResource,
    managed: &'a [ManagedName],
    deploying: &str,
) -> Option<&'a ManagedName> {
    if candidate.physical_resource_id.is_empty() {
        return None;
    }
    let scope = scope_of(candidate);
    managed.iter().find(|held| {
        held.stack_id != deploying
            && held.resource_type == candidate.resource_type
            && held.physical_resource_id == candidate.physical_resource_id
            && held.scope == scope
    })
}

/// The live stack of `region` that manages a resource of `resource_type` under
/// `physical_resource_id`, or `None` when no stack does.
///
/// A stack whose deletion has completed manages nothing, so it is not an owner; everything
/// else a stack records as a resource it deployed is managed by it.
pub fn managing<'a>(
    stacks: impl IntoIterator<Item = &'a StoredStack>,
    region: &str,
    resource_type: &str,
    physical_resource_id: &str,
) -> Option<&'a StoredStack> {
    if physical_resource_id.is_empty() {
        return None;
    }
    stacks.into_iter().find(|stack| {
        stack.is_live()
            && stack.region == region
            && stack.resources.iter().any(|deployed| {
                deployed.resource.materialised
                    && deployed.resource.resource_type == resource_type
                    && deployed.resource.physical_resource_id == physical_resource_id
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the reason names the physical name and the stack that manages it, in the words
    /// CloudFormation words it in.
    #[test]
    fn the_reason_names_the_name_and_the_owning_stack() {
        let rows: &[(&str, &str, &str, &str)] = &[
            (
                "a parameter name",
                "/app/version",
                "arn:aws:cloudformation:us-east-1:000000000000:stack/owner/0",
                "/app/version already exists in stack \
                 arn:aws:cloudformation:us-east-1:000000000000:stack/owner/0",
            ),
            (
                "a bucket name",
                "reports",
                "arn:aws:cloudformation:eu-west-1:000000000000:stack/other/1",
                "reports already exists in stack \
                 arn:aws:cloudformation:eu-west-1:000000000000:stack/other/1",
            ),
        ];
        for (case, physical_resource_id, stack_id, expected) in rows {
            let reason = already_exists_in_stack(physical_resource_id, stack_id);
            assert_eq!(
                reason, *expected,
                "case {case:?}: expected {expected:?}, got {reason:?}"
            );
        }
    }

    /// RULE: a name a type's resources are identified by WITHIN a parent is taken only by a
    /// resource under the SAME parent — two streams of one name in two log groups are two
    /// resources — while a type whose name identifies a resource on its own collides on the
    /// name alone.
    #[test]
    fn a_child_resources_name_is_taken_only_inside_its_own_parent() {
        let stream = |logical_id: &str, group: &str| StoredResource {
            logical_id: logical_id.to_string(),
            resource_type: "AWS::Logs::LogStream".to_string(),
            physical_resource_id: "app".to_string(),
            attributes: ::std::collections::BTreeMap::new(),
            metadata: None,
            properties: ::serde_json::json!({
                "LogGroupName": group,
                "LogStreamName": "app",
            }),
            materialised: true,
            deletion_policy: None,
            update_replace_policy: None,
        };
        let rows: &[(&str, &str, &str, Option<&str>)] = &[
            (
                "the same stream name in another log group",
                "other-group",
                "one-group",
                None,
            ),
            (
                "the same stream name in the same log group",
                "one-group",
                "one-group",
                Some("Held"),
            ),
        ];
        for (case, candidate_group, held_group, expected) in rows {
            let candidate = stream("Candidate", candidate_group);
            let held = [stream("Held", held_group)];
            let taken = taken_in_the_stack(&candidate, held.iter());
            assert_eq!(
                taken.map(|existing| existing.logical_id.as_str()),
                *expected,
                "case {case:?}: expected {expected:?}, got {taken:?}"
            );
            let managed = vec![ManagedName {
                resource_type: "AWS::Logs::LogStream".to_string(),
                physical_resource_id: "app".to_string(),
                stack_id: "arn:aws:cloudformation:us-east-1:000000000000:stack/theirs/1"
                    .to_string(),
                scope: scope_of(&held[0]),
            }];
            let elsewhere = taken_by_another_stack(
                &candidate,
                &managed,
                "arn:aws:cloudformation:us-east-1:000000000000:stack/mine/0",
            );
            assert_eq!(
                elsewhere.is_some(),
                expected.is_some(),
                "case {case:?}: expected another stack to hold it: {}, got {elsewhere:?}",
                expected.is_some()
            );
        }
    }

    /// RULE: a name is taken by ANOTHER resource of the same type that really exists — never
    /// by the resource itself, by a record nothing created, by a resource of another type or
    /// by an empty name.
    #[test]
    fn a_name_is_taken_only_by_another_existing_resource_of_the_type() {
        let resource = |logical_id: &str, resource_type: &str, physical: &str, made: bool| {
            StoredResource {
                logical_id: logical_id.to_string(),
                resource_type: resource_type.to_string(),
                physical_resource_id: physical.to_string(),
                attributes: ::std::collections::BTreeMap::new(),
                metadata: None,
                properties: ::serde_json::Value::Null,
                materialised: made,
                deletion_policy: None,
                update_replace_policy: None,
            }
        };
        let parameter = "AWS::SSM::Parameter";
        let rows: &[(&str, StoredResource, Vec<StoredResource>, Option<&str>)] = &[
            (
                "another declaration resolving to the same name",
                resource("B", parameter, "/one", false),
                vec![resource("A", parameter, "/one", true)],
                Some("A"),
            ),
            (
                "the resource's own record",
                resource("A", parameter, "/one", false),
                vec![resource("A", parameter, "/one", true)],
                None,
            ),
            (
                "a record of a resource nothing created",
                resource("B", parameter, "/one", false),
                vec![resource("A", parameter, "/one", false)],
                None,
            ),
            (
                "the same name under another type",
                resource("B", "AWS::S3::Bucket", "/one", false),
                vec![resource("A", parameter, "/one", true)],
                None,
            ),
            (
                "a resource with no name of its own",
                resource("B", parameter, "", false),
                vec![resource("A", parameter, "", true)],
                None,
            ),
        ];
        for (case, candidate, recorded, expected) in rows {
            let taken = taken_in_the_stack(candidate, recorded.iter());
            assert_eq!(
                taken.map(|existing| existing.logical_id.as_str()),
                *expected,
                "case {case:?}: expected {expected:?}, got {taken:?}"
            );
        }
    }

    /// RULE: a name is taken by ANOTHER STACK's resource of the same type — never by the
    /// deploying stack's own, by a resource of another type or by an empty name.
    #[test]
    fn a_name_is_taken_by_another_stacks_resource_of_the_type() {
        let candidate = |resource_type: &str, physical: &str| StoredResource {
            logical_id: "Candidate".to_string(),
            resource_type: resource_type.to_string(),
            physical_resource_id: physical.to_string(),
            attributes: ::std::collections::BTreeMap::new(),
            metadata: None,
            properties: ::serde_json::Value::Null,
            materialised: false,
            deletion_policy: None,
            update_replace_policy: None,
        };
        let held = |resource_type: &str, physical: &str, stack_id: &str| ManagedName {
            resource_type: resource_type.to_string(),
            physical_resource_id: physical.to_string(),
            stack_id: stack_id.to_string(),
            scope: Vec::new(),
        };
        let parameter = "AWS::SSM::Parameter";
        let mine = "arn:aws:cloudformation:us-east-1:000000000000:stack/mine/0";
        let theirs = "arn:aws:cloudformation:us-east-1:000000000000:stack/theirs/1";
        let managed = vec![
            held(parameter, "/theirs", theirs),
            held(parameter, "/mine", mine),
            held("AWS::S3::Bucket", "/typed", theirs),
        ];
        let rows: &[(&str, StoredResource, Option<&str>)] = &[
            (
                "a name another stack manages",
                candidate(parameter, "/theirs"),
                Some(theirs),
            ),
            (
                "a name this very stack manages",
                candidate(parameter, "/mine"),
                None,
            ),
            (
                "the same name under another type",
                candidate(parameter, "/typed"),
                None,
            ),
            ("a name nobody manages", candidate(parameter, "/free"), None),
            ("no name at all", candidate(parameter, ""), None),
        ];
        for (case, declared, expected) in rows {
            let taken = taken_by_another_stack(declared, &managed, mine);
            assert_eq!(
                taken.map(|held| held.stack_id.as_str()),
                *expected,
                "case {case:?}: expected {expected:?}, got {taken:?}"
            );
        }
    }
}
