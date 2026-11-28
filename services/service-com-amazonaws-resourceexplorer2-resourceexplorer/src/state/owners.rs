// SPDX-License-Identifier: CC0-1.0

//! The owning services whose resources a Region's index holds.
//!
//! An index is a VIEW over resources this service does not own: a `Resource` is one
//! "that Amazon Web Services Resource Explorer has discovered, and for which it has
//! stored information in the index of the Amazon Web Services Region that contains the
//! resource" (the shape's own documentation), and `Resource$Service` is "the Amazon Web
//! Services service that owns the resource and is responsible for creating and updating
//! it". So every row here is read from the OWNING service's own records at query time
//! and nothing is copied into this service's store: a resource created through that
//! service's API is in the index with no Resource Explorer call in between, one deleted
//! there leaves the index, and a tag written there is the tag a `tag:` filter matches.
//!
//! # What "linked" means
//!
//! One emulator process holds every service's state in one engine, but a crate can only
//! read a state struct it names, so this module lists the owning services this crate
//! depends on; a resource of any other service is not indexed, which is this emulator's
//! boundary rather than an AWS rule (AWS indexes the types its "Supported resource
//! types" list names). Adding a service here is a dependency in `Cargo.toml`, an
//! [`OwnedType`] per resource type it owns and one block of [`indexed_resources`].
//!
//! [`INDEXED_TYPES`] is the other side of that boundary: the resource types an index here
//! can hold at all — the ones below plus this service's own index and view — and so the
//! types a query's `resourcetype:` filter can actually match here. It is a SUBSET of the
//! catalogue [`super::supported_types`] publishes, which is the service-wide list of every
//! type Resource Explorer supports and is what `ListSupportedResourceTypes` reports: what
//! an index can hold is service metadata of Resource Explorer, not of this emulator's
//! dependency list.

use std::collections::HashMap;
use std::time::SystemTime;

// The owning service crates, aliased to the role they play here. Each is declared
// `{ workspace = true }` in this crate's `Cargo.toml`, so the path the workspace
// resolves them at stays in one place.
use service_com_amazonaws_sns_amazonsimplenotificationservice as owner_sns;
use service_com_amazonaws_sqs_amazonsqs as owner_sqs;

use super::IndexedResource;

/// One resource type an index here holds, as Resource Explorer names it.
#[derive(Debug, Clone, Copy)]
pub struct OwnedType {
    /// The `<service>:<type>` key this type is reported and filtered by
    /// (`Resource$ResourceType`), from the Resource Explorer "Supported resource types"
    /// list. Its first segment is the ARN's own service namespace, which is what
    /// `Resource$Service` carries.
    pub resource_type: &'static str,
    /// "The CloudFormation resource type identifier for the resource, such as
    /// `AWS::EC2::Instance` or `AWS::S3::Bucket`" (`Resource$CfnResourceType`): the
    /// name this type is registered under in the CloudFormation registry.
    pub cfn_resource_type: &'static str,
}

impl OwnedType {
    /// "The Amazon Web Services service that is associated with the resource type"
    /// (`SupportedResourceType$Service`): the first segment of the `<service>:<type>` key,
    /// which is the ARN service namespace resources of this type carry.
    pub fn service(&self) -> &'static str {
        super::supported_types::service_of(self.resource_type)
    }
}

/// An Amazon SQS queue.
const SQS_QUEUE: OwnedType = OwnedType {
    resource_type: "sqs:queue",
    cfn_resource_type: "AWS::SQS::Queue",
};

/// An Amazon SNS topic.
const SNS_TOPIC: OwnedType = OwnedType {
    resource_type: "sns:topic",
    cfn_resource_type: "AWS::SNS::Topic",
};

/// A Resource Explorer index — a resource of this service's own namespace, which its own
/// index holds like any other.
pub const OWN_INDEX: OwnedType = OwnedType {
    resource_type: "resource-explorer-2:index",
    cfn_resource_type: "AWS::ResourceExplorer2::Index",
};

/// A Resource Explorer view, likewise.
pub const OWN_VIEW: OwnedType = OwnedType {
    resource_type: "resource-explorer-2:view",
    cfn_resource_type: "AWS::ResourceExplorer2::View",
};

/// Every resource type an index here can hold: this service's own two, and one row per
/// type of each owning service above.
///
/// A SUBSET of [`super::supported_types::CATALOGUE`] — an index holding a resource of
/// only some of the types Resource Explorer supports is a property of this emulator's
/// dependency list, while the published catalogue is the service's own metadata.
///
/// Ordered by the `<service>:<type>` key, like the catalogue, so a listing derived from
/// these rows is in a stable order that does not depend on which resources happen to
/// exist.
pub const INDEXED_TYPES: [OwnedType; 4] = [OWN_INDEX, OWN_VIEW, SNS_TOPIC, SQS_QUEUE];

/// Every resource an owning service holds in `region`, in the form the index reports
/// it.
///
/// Locks one owning service's state at a time, for the length of this call: no caller
/// may hold [`super::service_state`]'s guard across it, so the two stores are never
/// locked in two orders.
pub fn indexed_resources<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
) -> Vec<IndexedResource> {
    let mut found: Vec<IndexedResource> = Vec::new();

    if let Some(state) = owner_sqs::state::service_state(engine) {
        // A queue is a REGIONAL resource of that service — it belongs to the Region its
        // create addressed, which is the Region its ARN carries — so the queues an
        // index holds are the ones of its own Region.
        let guard = state.lock();
        for queue in guard.queues_in(region) {
            record(
                &mut found,
                region,
                &queue.arn,
                SQS_QUEUE,
                // When an attribute of the queue last changed, which is the latest
                // thing an index could have reported about it.
                queue.last_modified_at,
                &queue.tags,
            );
        }
    }

    if let Some(state) = owner_sns::state::service_state(engine) {
        let guard = state.lock();
        for topic in guard.topics.values() {
            record(
                &mut found,
                region,
                &topic.arn,
                SNS_TOPIC,
                topic.created_at,
                &topic.tags,
            );
        }
    }

    found
}

/// Record one owned resource, described from the ARN its owner minted for it.
///
/// The account, the Region and the service namespace are READ from that ARN rather
/// than assumed: those three fields are what an ARN states about a resource, so an
/// owner that minted an ARN in another Region — or a value that is not an ARN at all,
/// which names nothing an index could hold — contributes no row.
fn record<'tags, Tags>(
    into: &mut Vec<IndexedResource>,
    region: &str,
    arn: &str,
    kind: OwnedType,
    last_reported_at: SystemTime,
    tags: Tags,
) where
    Tags: IntoIterator<Item = (&'tags String, &'tags String)>,
{
    let Some(parsed) = ::service_common::arn::parse(arn) else {
        return;
    };
    if parsed.region != region {
        return;
    }
    into.push(IndexedResource {
        arn: arn.to_string(),
        owning_account_id: parsed.account.to_string(),
        region: parsed.region.to_string(),
        service: parsed.vendor.to_string(),
        resource_type: kind.resource_type.to_string(),
        cfn_resource_type: Some(kind.cfn_resource_type.to_string()),
        last_reported_at,
        tags: tags
            .into_iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<HashMap<String, String>>(),
    });
}
