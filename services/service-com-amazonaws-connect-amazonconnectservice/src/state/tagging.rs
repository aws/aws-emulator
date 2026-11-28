// SPDX-License-Identifier: CC0-1.0

//! Tags addressed by the ARN of the resource that carries them.
//!
//! One concern, so one file: Connect's tagging plane takes a resource ARN rather than
//! the instance-scoped path labels the rest of the API uses, so the lookup from "an ARN"
//! to "the tag map stored on whatever resource that ARN names" lives here once, beside
//! the records it walks, instead of being written again in each tagging operation.
//!
//! Every kind of resource this store holds that carries a `TagMap` is reachable through
//! it: the instance itself, the queues, flows, hours of operation and integration
//! associations inside it, and the phone numbers claimed to the account. A record's ARN
//! is minted when the record is stored, so this resolves exactly the resources that
//! exist — a resource a delete retired, or one of an account or Region this store never
//! minted for, resolves to nothing.
//!
//! The lookup comes in two forms over ONE resolution: a reporting form for the operations
//! that read a resource's tags, and a writing form for the ones that edit them, so the
//! two can never disagree about which resource an ARN names.

use std::collections::{BTreeMap, HashMap};

use super::ComAmazonawsConnectAmazonconnectserviceState;

/// How many tags one resource carries at most: the modelled `@length` maximum of
/// `com.amazonaws.connect#TagMap`
/// (`smithy_query(query="shape",shape="TagMap")` -> length min 1 max 50), which is the
/// shape every read of a resource's tags reports them through — so a resource holding
/// more than this many could not be reported at all.
///
/// It is also what keeps the store bounded, for the reason [`super::INSTANCE_QUOTA`]
/// gives: nothing releases a tag but another call, so without a cap a caller repeating
/// `TagResource` with fresh keys would grow one record until the host ran out of memory.
pub const TAGS_PER_RESOURCE_MAXIMUM: usize = 50;

/// The parameter name this service publishes for a request's tag map, spelled as the
/// shapes that carry it spell it (`Tags`), for a refusal that names the offending member.
pub const TAGS_PARAMETER: &str = "Tags";

/// How many keys `requested` would ADD to `stored` — the keys it names that are not
/// stored already, since a key that is stored takes a new value rather than a new entry.
///
/// One function, so every operation that adds tags measures the growth of the map the
/// same way.
pub fn tags_growth(
    stored: &BTreeMap<String, String>,
    requested: &HashMap<String, String>,
) -> usize {
    requested
        .keys()
        .filter(|key| !stored.contains_key(*key))
        .count()
}

/// Where in this store the record an ARN names lives: the kind of resource, and the
/// identifier (or pair of identifiers) it is keyed by.
///
/// Resolving an ARN and reaching the record it names are two steps, so the resolution is
/// written ONCE here and both the read and the write of the tagging plane go through it:
/// a reporting operation and a mutating one can then never disagree about which resource
/// an ARN names.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TaggedResource {
    /// A phone number claimed to the account, keyed by its phone-number id.
    PhoneNumber(String),
    /// The instance itself, keyed by its instance id.
    Instance(String),
    /// A queue, keyed by its instance's id and its own.
    Queue(String, String),
    /// A flow, keyed by its instance's id and its own.
    ContactFlow(String, String),
    /// An hours of operation, keyed by its instance's id and its own.
    HoursOfOperation(String, String),
    /// An integration association, keyed by its instance's id and its own.
    IntegrationAssociation(String, String),
}

impl ComAmazonawsConnectAmazonconnectserviceState {
    /// Which stored record `resource_arn` names, or `None` when no stored resource
    /// carries that ARN.
    fn locate_tagged_resource(&self, resource_arn: &str) -> Option<TaggedResource> {
        if let Some(number) = self
            .phone_numbers
            .values()
            .find(|number| number.arn == resource_arn)
        {
            return Some(TaggedResource::PhoneNumber(number.id.clone()));
        }
        for instance in self.instances.values() {
            if instance.arn == resource_arn {
                return Some(TaggedResource::Instance(instance.id.clone()));
            }
            // A resource inside an instance carries an ARN built from its instance's,
            // so only the instance whose ARN prefixes this one can hold it — but the
            // scan does not depend on that, which keeps one rule for every kind.
            if let Some(queue) = instance
                .queues
                .values()
                .find(|queue| queue.arn == resource_arn)
            {
                return Some(TaggedResource::Queue(
                    instance.id.clone(),
                    queue.id.clone(),
                ));
            }
            if let Some(flow) = instance
                .contact_flows
                .values()
                .find(|flow| flow.arn == resource_arn)
            {
                return Some(TaggedResource::ContactFlow(
                    instance.id.clone(),
                    flow.id.clone(),
                ));
            }
            if let Some(hours) = instance
                .hours_of_operations
                .values()
                .find(|hours| hours.arn == resource_arn)
            {
                return Some(TaggedResource::HoursOfOperation(
                    instance.id.clone(),
                    hours.id.clone(),
                ));
            }
            if let Some(association) = instance.integration_association_by_arn(resource_arn) {
                return Some(TaggedResource::IntegrationAssociation(
                    instance.id.clone(),
                    association.id.clone(),
                ));
            }
        }
        None
    }

    /// The tags stored on the resource `resource_arn` names, or `None` when no stored
    /// resource carries that ARN.
    ///
    /// An untagged resource that DOES exist answers an empty map, which is a different
    /// answer from `None`: the first is a resource with no tags, the second is no
    /// resource at all.
    pub fn tags_of_resource(&self, resource_arn: &str) -> Option<&BTreeMap<String, String>> {
        match self.locate_tagged_resource(resource_arn)? {
            TaggedResource::PhoneNumber(number) => {
                self.phone_numbers.get(&number).map(|number| &number.tags)
            }
            TaggedResource::Instance(instance) => {
                self.instances.get(&instance).map(|instance| &instance.tags)
            }
            TaggedResource::Queue(instance, queue) => self
                .instances
                .get(&instance)?
                .queues
                .get(&queue)
                .map(|queue| &queue.tags),
            TaggedResource::ContactFlow(instance, flow) => self
                .instances
                .get(&instance)?
                .contact_flows
                .get(&flow)
                .map(|flow| &flow.tags),
            TaggedResource::HoursOfOperation(instance, hours) => self
                .instances
                .get(&instance)?
                .hours_of_operations
                .get(&hours)
                .map(|hours| &hours.tags),
            TaggedResource::IntegrationAssociation(instance, association) => self
                .instances
                .get(&instance)?
                .integration_associations
                .get(&association)
                .map(|association| &association.tags),
        }
    }

    /// The same lookup as [`Self::tags_of_resource`], handing back the stored map to
    /// WRITE: the one seam through which a tagging operation edits the tags of whichever
    /// kind of resource an ARN names, so the tags it writes are the ones every later read
    /// of that resource reports.
    pub fn tags_of_resource_mut(
        &mut self,
        resource_arn: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        match self.locate_tagged_resource(resource_arn)? {
            TaggedResource::PhoneNumber(number) => self
                .phone_numbers
                .get_mut(&number)
                .map(|number| &mut number.tags),
            TaggedResource::Instance(instance) => self
                .instances
                .get_mut(&instance)
                .map(|instance| &mut instance.tags),
            TaggedResource::Queue(instance, queue) => self
                .instances
                .get_mut(&instance)?
                .queues
                .get_mut(&queue)
                .map(|queue| &mut queue.tags),
            TaggedResource::ContactFlow(instance, flow) => self
                .instances
                .get_mut(&instance)?
                .contact_flows
                .get_mut(&flow)
                .map(|flow| &mut flow.tags),
            TaggedResource::HoursOfOperation(instance, hours) => self
                .instances
                .get_mut(&instance)?
                .hours_of_operations
                .get_mut(&hours)
                .map(|hours| &mut hours.tags),
            TaggedResource::IntegrationAssociation(instance, association) => self
                .instances
                .get_mut(&instance)?
                .integration_associations
                .get_mut(&association)
                .map(|association| &mut association.tags),
        }
    }
}
