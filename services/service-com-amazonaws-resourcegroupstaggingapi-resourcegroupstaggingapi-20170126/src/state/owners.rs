// SPDX-License-Identifier: CC0-1.0

//! The owning services the tagging API is a VIEW over.
//!
//! `TagResources` "appl[ies] one or more tags to the specified resources", and
//! the resources are not this service's: each one belongs to the service whose
//! namespace its ARN names, which is why AWS reports a per-resource
//! `FailureInfo` carrying "any valid error code returned by the Amazon Web
//! Services service that hosts the resource that the ARN key represents"
//! (`FailureInfo` documentation). An ARN is therefore RESOLVED against the
//! owning service's own state before a tag is written, and where that service
//! models the resource's tag set, that set — not a second copy here — is what
//! this API reads and writes. So for those resources a tag written here is
//! visible through the owning service's own `ListTagsFor*`, a tag written there
//! is visible through `GetResources`, and deleting the resource there removes it
//! from this API's index, with nothing to keep in step.
//!
//! That last part holds only where the owner MODELS the tag set. A resource the
//! owner resolves but models no tags for ([`Resolution::Present`] with no
//! `tags`: a Step Functions execution) keeps its tags in
//! [`crate::state`]'s own store, which the owner's delete does not reach — so
//! `GetResources` goes on reporting that ARN until something tags or untags it
//! again. The alternative, dropping the entry the moment an owner answers
//! [`Resolution::Absent`], would make a READ delete state, which a reporting
//! operation may not do; the stale row is the lesser of the two, and it
//! disappears for every owner that models tags.
//!
//! # What "linked" means
//!
//! One emulator process holds every service's state in one engine, but a crate
//! can only read a state struct it names, so this module lists the owning
//! services this crate depends on. For an ARN in any OTHER namespace the answer
//! is [`Resolution::Unowned`]: the tagging API neither confirms nor denies the
//! resource and [`crate::state`]'s own store holds its tags, which is this
//! emulator's boundary rather than an AWS rule. Adding a service here is a
//! dependency in `Cargo.toml` plus one arm of [`visit`] and one of
//! [`tagged_resources`] — and, because an ARN of that namespace naming a resource
//! the owner does not hold then becomes a per-resource failure rather than an
//! entry in this crate's own store, the fixtures of every test that used such an
//! ARN as an unowned one.
//!
//! The two halves can also be linked SEPARATELY, because they answer two
//! different questions. [`visit`] answers "may this API write a tag to the ARN
//! this request names", so a namespace it serves changes which ARNs a WRITE is
//! accepted for — and which become that per-resource failure.
//! [`tagged_resources`] answers "which tagged resources does the account hold",
//! which is what a reporting read — and every resource query resolved against
//! this projection — selects out of. A service listed in the projection ALONE
//! therefore has its own tag sets REPORTED (a resource tagged through that
//! service's API is discoverable here, and one it no longer holds is gone from
//! the projection) while an ARN in its namespace stays [`Resolution::Unowned`]
//! for a write, which keeps [`crate::state`]'s own store the tag home for the
//! ARNs this API is asked to tag directly. Amazon EC2 and AWS Systems Manager are
//! linked that way.

use std::collections::BTreeMap;
use std::time::SystemTime;

// The owning service crates, aliased to the role they play here. Each is
// declared `{ workspace = true }` in this crate's `Cargo.toml`, so the path the
// workspace resolves them at stays in one place.
use service_com_amazonaws_apigateway_backplanecontrolservice as owner_apigateway;
use service_com_amazonaws_cloudwatchlogs_logs_20140328 as owner_logs;
use service_com_amazonaws_connecthealth_connecthealth as owner_connecthealth;
use service_com_amazonaws_dynamodb_dynamodb_20120810 as owner_dynamodb;
use service_com_amazonaws_ec2_amazonec2 as owner_ec2;
use service_com_amazonaws_ecs_amazonec2containerservicev20141113 as owner_ecs;
use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10 as owner_elb;
use service_com_amazonaws_eventbridge_awsevents as owner_events;
use service_com_amazonaws_glue_awsglue as owner_glue;
use service_com_amazonaws_kms_trentservice as owner_kms;
use service_com_amazonaws_lambda_awsgirapiservice as owner_lambda;
use service_com_amazonaws_s3_amazons3 as owner_s3;
use service_com_amazonaws_secretsmanager_secretsmanager as owner_secretsmanager;
use service_com_amazonaws_sfn_awsstepfunctions as owner_sfn;
use service_com_amazonaws_sns_amazonsimplenotificationservice as owner_sns;
use service_com_amazonaws_sqs_amazonsqs as owner_sqs;
use service_com_amazonaws_ssm_amazonssm as owner_ssm;

use super::ParsedArn;
use crate::ops::types::FailureInfo;

/// Tag pairs in the one form this crate handles them in: key -> value, ordered,
/// so a projection of them is stable per read.
pub type Tags = BTreeMap<String, String>;

/// What the service that OWNS an ARN's namespace says about that ARN.
#[derive(Debug, Clone)]
pub enum Resolution {
    /// The owning service holds the resource. `tags` is `Some` where that
    /// service models the resource's own tag set — which is then the tag set
    /// this API reports and the one it writes — and `None` where it models
    /// none, leaving [`crate::state`]'s store as this resource's tag home.
    Present { tags: Option<Tags> },
    /// The owning service holds the resource, but the change would have left it
    /// carrying more tags than the documented limit, so NOTHING was written.
    /// The count is the one the change would have reached.
    TooManyTags(usize),
    /// The owning service serves this namespace and holds no such resource. The
    /// per-resource failure carries THAT service's not-found identity, which is
    /// what a client reading `FailedResourcesMap` sees from AWS.
    Absent(FailureInfo),
    /// No owning service linked into this crate serves this namespace.
    Unowned,
}

/// A tag write applied to the owning service's own tag map.
#[derive(Debug, Clone, Copy)]
pub enum Change<'a> {
    /// Upsert every pair: a key already on the resource is overwritten, every
    /// other key it carries is left alone — unless the merged key set would
    /// pass `max_tags`, which refuses the whole write for that resource so the
    /// check and the write share one acquisition of the owner's lock.
    Upsert { tags: &'a Tags, max_tags: usize },
    /// Remove exactly the named keys, leaving every other key alone.
    Remove(&'a [String]),
}

impl Change<'_> {
    /// Apply this change to a resource's tag map, or report the tag count it
    /// would have left the resource with when that is past the limit.
    pub fn apply(&self, tags: &mut Tags) -> Result<(), usize> {
        match self {
            Self::Upsert {
                tags: upserts,
                max_tags,
            } => {
                let merged = super::tag_count_after_merging(Some(tags), upserts.keys());
                if merged > *max_tags {
                    return Err(merged);
                }
                for (key, value) in upserts.iter() {
                    tags.insert(key.clone(), value.clone());
                }
            }
            Self::Remove(keys) => {
                for key in keys.iter() {
                    tags.remove(key);
                }
            }
        }
        Ok(())
    }
}

/// The per-resource failure for an ARN the owning service will not tag — one it
/// holds no resource for, or one it refuses outright: that service's own error
/// code and its own message, with the 400 the tagging API reports for a
/// resource-level client fault.
fn absent(code: &str, message: String) -> Resolution {
    absent_with_status(400, code, message)
}

/// The same per-resource failure for an owning service that words this fault with NO
/// message at all: `FailureInfo`'s `ErrorMessage` is "the error message that accompanies the
/// failure", so a service whose refusal carries none leaves it unset rather than having
/// this API invent prose the owning service never wrote.
fn absent_stating(code: &str, message: Option<String>) -> Resolution {
    Resolution::Absent(FailureInfo {
        status_code: 400,
        error_code: Some(code.to_string()),
        error_message: message,
    })
}

/// The same per-resource failure for an owning service whose error for this fault
/// carries another HTTP status: `FailureInfo` reports "the HTTP status code of the
/// common error" beside "any valid error code returned by the … service that hosts
/// the resource", so both fields are that service's own.
fn absent_with_status(status_code: i32, code: &str, message: String) -> Resolution {
    Resolution::Absent(FailureInfo {
        status_code,
        error_code: Some(code.to_string()),
        error_message: Some(message),
    })
}

/// Read, and optionally change, a tag list whose entries carry an OPTIONAL key
/// and value — the `Tag` shape most services model. `$make` builds one of the
/// owning service's own tag records from a pair.
///
/// The list is edited IN PLACE: a record the change does not name keeps its
/// position and its exact stored value, so tagging one key never rewrites the
/// rest of the owning service's tag list.
macro_rules! option_tag_list {
    ($list:expr, $change:expr, $make:expr) => {{
        let mut pairs: Tags = option_tag_pairs!($list);
        if let Some(change) = $change {
            if let Err(would_hold) = change.apply(&mut pairs) {
                return Resolution::TooManyTags(would_hold);
            }
            match change {
                Change::Upsert { tags: upserts, .. } => {
                    for (key, value) in upserts.iter() {
                        match $list
                            .iter_mut()
                            .find(|tag| tag.key.as_deref() == Some(key.as_str()))
                        {
                            Some(held) => held.value = Some(value.clone()),
                            None => $list.push($make(key, value)),
                        }
                    }
                }
                Change::Remove(keys) => $list.retain(|tag| {
                    tag.key
                        .as_deref()
                        .is_none_or(|key| !keys.iter().any(|removed| removed == key))
                }),
            }
        }
        Resolution::Present { tags: Some(pairs) }
    }};
}

/// The [`Tags`] of a list of optional-key/optional-value tag records.
macro_rules! option_tag_pairs {
    ($list:expr) => {
        $list
            .iter()
            .filter_map(|tag| {
                tag.key
                    .clone()
                    .map(|key| (key, tag.value.clone().unwrap_or_default()))
            })
            .collect::<Tags>()
    };
}

/// The [`Tags`] of a list of tag records whose key and value are REQUIRED (the
/// `Tag` shape EventBridge models).
fn plain_tag_pairs(list: &[owner_events::ops::types::Tag]) -> Tags {
    list.iter()
        .map(|tag| (tag.key.clone(), tag.value.clone()))
        .collect()
}

/// The [`Tags`] of a tag map an owning service already holds as key -> value, in the
/// one ordered form this crate handles tags in (the shape Elastic Load Balancing
/// stores a resource's tags as).
fn pairs(map: &std::collections::HashMap<String, String>) -> Tags {
    map.iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// Resolve `arn` against the service that owns its namespace, applying `change`
/// to that service's own tag map when one is supplied and the resource is there.
///
/// Locks exactly ONE owning service's state, for the length of this call: no
/// caller may hold [`crate::state::service_state`]'s guard across it, so the two
/// stores are never locked in two orders.
pub fn visit<Eng: Send + Sync + 'static>(
    engine: &Eng,
    arn: &ParsedArn,
    change: Option<&Change<'_>>,
) -> Resolution {
    match arn.service.as_str() {
        "apigateway" => apigateway(engine, arn, change),
        "health-agent" => health_agent(engine, arn, change),
        "sqs" => sqs(engine, arn, change),
        "dynamodb" => dynamodb(engine, arn, change),
        "ecs" => ecs(engine, arn, change),
        "elasticloadbalancing" => elasticloadbalancing(engine, arn, change),
        "events" => events(engine, arn, change),
        "glue" => glue(engine, arn, change),
        "kms" => kms(engine, arn, change),
        "lambda" => lambda(engine, arn, change),
        "logs" => logs(engine, arn, change),
        "s3" => s3(engine, arn, change),
        "secretsmanager" => secretsmanager(engine, arn, change),
        "sns" => sns(engine, arn, change),
        "states" => states(engine, arn, change),
        _ => Resolution::Unowned,
    }
}

/// Every resource an owning service itself holds tags for, as `(ARN, tags)`.
///
/// Only resources carrying at least one tag: "`GetResources` does not return
/// untagged resources." `region` is the serving region, needed by every owner
/// whose records carry no region of their own, so their ARNs are derived rather
/// than stored.
pub fn tagged_resources<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
) -> Vec<(String, Tags)> {
    let mut found: Vec<(String, Tags)> = Vec::new();
    let mut record = |arn: String, tags: Tags| {
        if !tags.is_empty() {
            found.push((arn, tags));
        }
    };

    if let Some(state) = owner_apigateway::state::service_state(engine) {
        for (arn, tags) in owner_apigateway::state::tag_index::tagged_resources(&state.lock(), region)
        {
            record(arn, tags);
        }
    }

    if let Some(state) = owner_connecthealth::state::service_state(engine) {
        // The resources that service holds a tag set for, resolved by the SAME rule its
        // own tagging operations resolve a `resourceArn` with (its state's
        // `holds_resource_arn`, which covers a Domain this Region still holds and a live
        // Subscription under one), so a resource this projection reports is exactly a
        // resource that service would let a caller tag or read tags from. Only ARNs it
        // has a tag entry for are walked, which is already "no untagged resources".
        //
        // A purge the clock has already made due is materialised first, as it is for the
        // secrets and state machines below: it was owed at this instant whether or not
        // anybody called, so a Domain that service would already have dropped is never
        // reported.
        let mut state = state.lock();
        state.materialise_due(SystemTime::now());
        let held: Vec<String> = state
            .tags_by_arn
            .keys()
            .filter(|arn| state.holds_resource_arn(region, arn))
            .cloned()
            .collect();
        for arn in held {
            let tags = state.tags_for(&arn).into_iter().collect();
            record(arn, tags);
        }
    }
    if let Some(state) = owner_sqs::state::service_state(engine) {
        // A queue is a REGIONAL resource of that service — it belongs to the Region its
        // create addressed, which is the Region its ARN carries — so the queues this
        // projection reports are the ones of the serving Region.
        for queue in state.lock().queues_in(region) {
            record(queue.arn.clone(), queue.tags.clone());
        }
    }
    if let Some(state) = owner_ecs::state::service_state(engine) {
        let state = state.lock();
        for cluster in state.clusters.values() {
            record(cluster.arn.clone(), option_tag_pairs!(cluster.tags));
        }
        for provider in state.capacity_providers.values() {
            record(provider.arn.clone(), option_tag_pairs!(provider.tags));
        }
        for definition in state.task_definitions.values() {
            record(definition.arn.clone(), option_tag_pairs!(definition.tags));
        }
        for service in state.services.values() {
            record(service.arn.clone(), option_tag_pairs!(service.tags));
        }
        for task in state.tasks.values() {
            record(task.task_arn.clone(), option_tag_pairs!(task.tags));
        }
        for instance in state.container_instances.values() {
            record(instance.arn.clone(), option_tag_pairs!(instance.tags));
        }
    }
    if let Some(state) = owner_dynamodb::state::service_state(engine) {
        let mut state = state.lock();
        // Like the secrets and state machines below, what the CLOCK already owes is
        // materialised before this projection is read: a table whose `DeleteTable` span
        // has elapsed is gone, and a tag write whose propagation span has elapsed is
        // visible to that service's tag readers. Both were owed at this instant whether
        // or not anybody called.
        state.apply_due_transitions(SystemTime::now());
        // A table is a REGIONAL resource of that service — a table name resolves only
        // within one Region, which is the Region its ARN carries — so the tables this
        // projection reports are the ones of the serving Region, each under the ARN that
        // service minted for it. The set reported is the one its own eventually
        // consistent tag reader reports, because there is ONE tag set per table.
        if let Some(tables) = state.tables_in(region) {
            for table in tables.values() {
                record(table.arn.clone(), table.tags.visible().clone());
            }
        }
    }
    if let Some(state) = owner_elb::state::service_state(engine) {
        // Every resource that service holds a tag set for, each under the ARN it
        // minted for it — which already carries the Region the resource lives in, so
        // the caller's Region filter applies to these rows exactly as it does to any
        // other owner's.
        let state = state.lock();
        for load_balancer in state.load_balancers.values() {
            record(load_balancer.arn.clone(), pairs(&load_balancer.tags));
        }
        for target_group in state.target_groups.values() {
            record(target_group.arn.clone(), pairs(&target_group.tags));
        }
        for listener in state.listeners.values() {
            record(listener.arn.clone(), pairs(&listener.tags));
            // A rule's record lives on its listener, and its own ARN is the one that
            // service minted for it.
            for rule in &listener.rules {
                record(rule.arn.clone(), pairs(&rule.tags));
            }
        }
        for trust_store in state.trust_stores.values() {
            record(trust_store.arn.clone(), pairs(&trust_store.tags));
        }
    }
    if let Some(state) = owner_events::state::service_state(engine) {
        // An event bus is a REGIONAL resource of that service, so the buses this
        // projection reports are the ones of the serving Region — which is also
        // the Region their ARNs carry.
        for bus in state.lock().buses_in(region) {
            record(
                owner_events::state::event_bus_arn(region, &bus.name),
                plain_tag_pairs(&bus.tags),
            );
            // A rule's ARN is the one that service MINTED for it, so a rule on
            // a bus other than the default one is reported under the form that
            // carries its bus.
            for rule in bus.rules.values() {
                record(rule.arn.clone(), plain_tag_pairs(&rule.tags));
            }
        }
    }
    if let Some(state) = owner_glue::state::service_state(engine) {
        // Glue holds the tag set of every resource whose create carries a `Tags` map, so
        // the tags a create recorded are already this projection's rows: that service's
        // own records, with the ARN minted for the serving Region (a Glue record carries
        // no Region of its own, like an EventBridge event bus). The rows are the ones of
        // the account this emulator serves, which is the account this API answers for.
        for (arn, tags) in state
            .lock()
            .tagged_resources(::engine::emulator_account_id(), region)
        {
            record(arn, tags.into_iter().collect());
        }
    }
    if let Some(state) = owner_kms::state::service_state(engine) {
        for key in state.lock().keys.values() {
            record(key.arn.clone(), key.tags.clone());
        }
    }
    if let Some(state) = owner_lambda::state::service_state(engine) {
        let mut state = state.lock();
        // Like the secrets below, a capacity provider or an event source mapping
        // whose removal the clock has already made due is gone: it was owed at
        // this instant whether or not anybody called, so what the clock owes is
        // materialised before this projection is read rather than reporting a
        // resource the owning service no longer has.
        state.materialize_all(SystemTime::now());
        // Every resource type that service holds a tag set for, each under the
        // ARN it minted for it — which already carries the Region the resource
        // lives in, so the caller's Region filter applies to these rows exactly
        // as it does to any other owner's. Only the UNQUALIFIED function ARN is a
        // row: a version and an alias carry no tag set of their own, so neither
        // is a resource this API reports.
        for function in state.functions.values() {
            record(function.arn.clone(), function.tags.clone());
        }
        for provider in state.capacity_providers.values() {
            record(provider.arn.clone(), provider.tags.clone());
        }
        for mapping in state.event_source_mappings.values() {
            record(mapping.arn.clone(), mapping.tags.clone());
        }
        for config in state.code_signing_configs.values() {
            record(config.arn.clone(), config.tags.clone());
        }
    }
    if let Some(state) = owner_logs::state::service_state(engine) {
        // Every resource that service holds a tag set for, each under the ARN it minted
        // for it — for a log group the BARE form (`LogGroup$logGroupArn`), because the
        // trailing `:*` of the other form names the group together with everything under
        // it rather than the resource, and is not a value that service's own tagging
        // member admits. A record of that service carries the Region it was created in
        // inside its ARN, so the caller's Region filter applies to these rows exactly as
        // it does to any other owner's.
        let state = state.lock();
        for group in state.log_groups.values() {
            record(group.log_group_arn.clone(), group.tags.clone());
        }
        for destination in state.destinations.values() {
            record(destination.arn.clone(), destination.tags.clone());
        }
        for delivery in state.deliveries.values() {
            record(delivery.arn.clone(), delivery.tags.clone());
        }
        for source in state.delivery_sources.values() {
            record(source.arn.clone(), source.tags.clone());
        }
        for destination in state.delivery_destinations.values() {
            record(destination.arn.clone(), destination.tags.clone());
        }
        for table in state.lookup_tables.values() {
            record(table.arn.clone(), table.tags.clone());
        }
    }
    if let Some(state) = owner_secretsmanager::state::service_state(engine) {
        let mut state = state.lock();
        // The one owner whose resources the CLOCK removes: a deletion that has
        // already fallen due (the end of a recovery window, or the short
        // asynchronous span a `ForceDeleteWithoutRecovery` deletion skips the
        // window for) was owed at this instant whether or not anybody called,
        // so it is materialised before this projection is read and a secret the
        // owning service would already have deleted is never reported. This is
        // the only write a read of this API performs, and it changes nothing a
        // later caller of that service would not have seen anyway.
        state.expire_due(SystemTime::now());
        for secret in state.secrets.values() {
            record(secret.arn.clone(), option_tag_pairs!(secret.tags));
        }
    }
    if let Some(state) = owner_s3::state::service_state(engine) {
        // A bucket is reported by the endpoint of the Region it LIVES in — "all the
        // tagged … resources that are located in the specified Amazon Web Services
        // Region" (`GetResources`' own documentation) — and a bucket ARN carries no
        // Region segment to read that from, so the Region compared is the one the bucket
        // itself records. Only the buckets of the caller's OWN S3 account are rows, read
        // through the seam that service's own `ListBuckets` reports from: a bucket AWS
        // publishes, or one another emulated service provisioned as the object store of
        // its own resource, is not a bucket of this account.
        let store = state.lock();
        for (name, bucket) in store.general_purpose_account_buckets() {
            if bucket.region != region {
                continue;
            }
            record(owner_s3::state::bucket_arn(name), bucket_tags(bucket));
        }
    }
    if let Some(state) = owner_sns::state::service_state(engine) {
        // A topic is a REGIONAL resource of that service — it belongs to the Region its
        // create addressed, which is the Region its ARN carries — so the topics this
        // projection reports are the ones of the serving Region.
        for topic in state.lock().topics_in_region(region) {
            record(topic.arn.clone(), pairs(&topic.tags));
        }
    }
    if let Some(state) = owner_ec2::state::service_state(engine) {
        let mut state = state.lock();
        // Like the owners above, what the CLOCK already owes is materialised
        // before the projection is read: a resource that service would already
        // have dropped was owed its removal at this instant whether or not
        // anybody called, so it is not reported as one the account still holds.
        state.materialize_due_transitions();
        // EVERY kind of resource that service holds tags for, read off the one
        // report it answers its own tag reader with — so a resource tagged at
        // creation or through `CreateTags` is in this projection at once, and a
        // kind that service gains is projected with it. Each row is keyed by the
        // ARN that service mints for the resource: the `ec2` namespace, the
        // serving Region, the account, and the `<type>/<id>` pair its own tag
        // report names it by.
        let mut held: BTreeMap<String, Tags> = BTreeMap::new();
        for described in state.tag_descriptions() {
            let (Some(resource_id), Some(resource_type), Some(key)) =
                (described.resource_id, described.resource_type, described.key)
            else {
                continue;
            };
            let arn = ::engine::emulator_arn(
                "ec2",
                region,
                &format!("{resource_type}/{resource_id}"),
            );
            held.entry(arn)
                .or_default()
                .insert(key, described.value.unwrap_or_default());
        }
        for (arn, tags) in held {
            record(arn, tags);
        }
    }
    if let Some(state) = owner_sfn::state::service_state(engine) {
        let mut state = state.lock();
        // Like the secrets above, a state machine whose started deletion has
        // fallen due was owed its removal at this instant whether or not
        // anybody called, so it is materialised before this projection is read
        // rather than reported as a resource the owning service no longer has.
        state.settle(owner_sfn::state::now());
        // BOTH resource types that service holds a tag set for are rows here —
        // a state machine and an activity — each under the resource type its own
        // ARN carries.
        for machine in state.state_machines.values() {
            record(machine.arn.clone(), option_tag_pairs!(machine.tags));
        }
        for activity in state.activities.values() {
            record(activity.arn.clone(), option_tag_pairs!(activity.tags));
        }
    }
    if let Some(state) = owner_ssm::state::service_state(engine) {
        let mut state = state.lock();
        // Like the owners above, what the CLOCK already owes is materialised before the
        // projection is read: an OpsItem whose delete had settled by this instant was owed
        // its removal whether or not anybody called, so it is not reported as a resource
        // the account still holds.
        state.settle_ops_item_deletions_due(SystemTime::now());
        // Every resource that service holds a tag set for and mints an ARN for, read off
        // its own tag plane — so a resource tagged at creation or through its
        // `AddTagsToResource` is in this projection at once, and one whose tag its
        // `RemoveTagsFromResource` dropped is out of it. Each row is keyed by the ARN that
        // service mints for the resource, which carries the Region the resource lives in.
        for (arn, tags) in state.tagged_resources(region) {
            record(arn, tags);
        }
    }
    found
}

/// Amazon API Gateway: every record of the service that carries a tag collection — a
/// REST API, a stage of one, an API key, a VPC link, a usage plan, a custom domain name
/// — addressed by the path-style, account-less ARN that service reports for it. The tags
/// are the record's own, so a key created with tags is in this API's answers at once and
/// a deleted one is out of them with no untag.
fn apigateway<Eng: Send + Sync + 'static>(
    engine: &Eng,
    arn: &ParsedArn,
    change: Option<&Change<'_>>,
) -> Resolution {
    let Some(state) = owner_apigateway::state::service_state(engine) else {
        return Resolution::Unowned;
    };
    // The ARN is read by the owning service's OWN parse of a `resourceArn`, so this API
    // addresses exactly the records that service's `TagResource` does. `None` divides in
    // two: an ARN of a kind that service addresses but does not tag is a bad request TO
    // IT, while the `apigateway` namespace is SHARED — API Gateway V2's resources carry
    // it as well — so an ARN naming another service's resource is left `Unowned`, the
    // answer every namespace no linked owner serves gets, and this crate's own store
    // stays its tag home.
    let Some(resource) = owner_apigateway::state::parse_tagged_resource(&arn.arn) else {
        return if owner_apigateway::state::tag_index::addresses_own_resource(&arn.arn) {
            absent(
                "BadRequestException",
                owner_apigateway::state::INVALID_ARN.to_string(),
            )
        } else {
            Resolution::Unowned
        };
    };
    let mut state = state.lock();
    // `NotFoundException`, 404 (`smithy_query(query="shape", shape="NotFoundException")`
    // on the API Gateway model), carrying that service's own message for the identifier
    // it could not resolve.
    let not_found = |message: String| absent_with_status(404, "NotFoundException", message);
    // A resolution with no change is a READ, so it goes through the owning service's
    // read accessor and borrows nothing mutably; only a tag write reaches for the
    // record's tag map itself.
    let Some(change) = change else {
        return match state.tags_of(&resource) {
            Ok(tags) => Resolution::Present {
                tags: Some(tags.clone()),
            },
            Err(message) => not_found(message),
        };
    };
    let tags = match state.tags_of_mut(&resource) {
        Ok(tags) => tags,
        Err(message) => return not_found(message),
    };
    if let Err(would_hold) = change.apply(tags) {
        return Resolution::TooManyTags(would_hold);
    }
    Resolution::Present {
        tags: Some(tags.clone()),
    }
}

/// AWS HealthScribe's `health-agent` namespace (connecthealth): a Domain, or a live
/// Subscription under one, whose tags are that service's own — it holds one tag set per
/// resource ARN, so that set is the one this API reads and writes, and a Domain deleted
/// there leaves this API's answers with no untag.
///
/// The ARN is resolved by that service's OWN rule for a tagging `resourceArn` (its
/// `holds_resource_arn`, which both its `TagResource` and its `ListTagsForResource` go
/// through), and the Region comes off the ARN — the same Region the request is addressed
/// to, since `addressable_arn` has already refused an ARN of any other — so the resources
/// this API can tag are exactly the ones that service can, with no second copy of the
/// resolution here to drift from it. The cap a write is held to is that service's own.
fn health_agent<Eng: Send + Sync + 'static>(
    engine: &Eng,
    arn: &ParsedArn,
    change: Option<&Change<'_>>,
) -> Resolution {
    let Some(state) = owner_connecthealth::state::service_state(engine) else {
        return Resolution::Unowned;
    };
    let mut state = state.lock();
    // A resolution must see the store a report of it sees: materialising the purge the
    // clock already owes is what keeps a Domain whose deletion came due from being tagged
    // back into existence.
    state.materialise_due(SystemTime::now());
    if !state.holds_resource_arn(&arn.region, &arn.arn) {
        // That service's own refusal: `ResourceNotFoundException` at HTTP 404
        // (`smithy_query(query="operation", operation="TagResource")` on the connecthealth
        // model), carrying the sentence it words for an ARN it cannot resolve.
        return absent_with_status(
            404,
            "ResourceNotFoundException",
            owner_connecthealth::service::error_detail::resource_not_found_message(&arn.arn),
        );
    }
    let mut held: Tags = state.tags_for(&arn.arn);
    if let Some(change) = change {
        if let Err(would_hold) = change.apply(&mut held) {
            return Resolution::TooManyTags(would_hold);
        }
        // That service's own cap on a resource's tag SET, which a write through this API
        // is held to exactly as a write through its own `TagResource` is.
        if held.len() > owner_connecthealth::state::MAX_TAGS_PER_RESOURCE {
            return Resolution::TooManyTags(held.len());
        }
        state.set_tags(&arn.arn, held.clone());
    }
    Resolution::Present { tags: Some(held) }
}

/// Amazon SQS: a queue, whose tags are the queue's own cost-allocation tags.
fn sqs<Eng: Send + Sync + 'static>(
    engine: &Eng,
    arn: &ParsedArn,
    change: Option<&Change<'_>>,
) -> Resolution {
    let Some(state) = owner_sqs::state::service_state(engine) else {
        return Resolution::Unowned;
    };
    let mut state = state.lock();
    // A queue ARN carries the Region the queue lives in, so it is resolved through the
    // SQS crate's own ARN reader rather than by scanning the store here.
    let Some(queue) = state.queue_by_arn_mut(&arn.arn) else {
        // `QueueDoesNotExist`, whose wire code is its `awsQueryError`:
        // `smithy_query(query="shape", shape="QueueDoesNotExist")` on the SQS
        // model reports code `AWS.SimpleQueueService.NonExistentQueue`, 400.
        return absent(
            "AWS.SimpleQueueService.NonExistentQueue",
            "The specified queue does not exist.".to_string(),
        );
    };
    if let Some(change) = change {
        if let Err(would_hold) = change.apply(&mut queue.tags) {
            return Resolution::TooManyTags(would_hold);
        }
    }
    Resolution::Present {
        tags: Some(queue.tags.clone()),
    }
}

/// Amazon SNS: a topic, whose tags are the topic's own — "an SNS tag is held by a
/// topic", so a topic's tag set is the one this API reads and writes, and a topic
/// created or deleted through SNS appears in and disappears from this API's
/// answers with nothing to keep in step.
///
/// A topic ARN carries the Region the topic lives in and is the key that service
/// holds its topics under, so it is resolved by that key rather than scanned for.
/// An SNS ARN that names something ELSE — a subscription (`<topic>:<uuid>`), a
/// platform endpoint — is told apart by that service's OWN reader
/// ([`owner_sns::state::topic_name_from_arn`], the test its tag operations apply)
/// and left [`Resolution::Unowned`], the answer every value no linked owner
/// addresses gets, so this crate's own store stays its tag home.
fn sns<Eng: Send + Sync + 'static>(
    engine: &Eng,
    arn: &ParsedArn,
    change: Option<&Change<'_>>,
) -> Resolution {
    let Some(state) = owner_sns::state::service_state(engine) else {
        return Resolution::Unowned;
    };
    if owner_sns::state::topic_name_from_arn(&arn.arn).is_none() {
        return Resolution::Unowned;
    }
    let mut state = state.lock();
    let Some(topic) = state.topics.get_mut(&arn.arn) else {
        // SNS's own `ResourceNotFoundException`, as the WIRE carries it:
        // `smithy_query(query="operation",operation="TagResource")` on the SNS
        // model reports that error's `awsQueryError` code `ResourceNotFound`
        // with HTTP 404, and `RESOURCE_DOES_NOT_EXIST` is that service's own
        // wording of the reason, which its tag operations answer with.
        return absent_with_status(
            404,
            "ResourceNotFound",
            owner_sns::state::RESOURCE_DOES_NOT_EXIST.to_string(),
        );
    };
    let mut held = pairs(&topic.tags);
    if let Some(change) = change {
        if let Err(would_hold) = change.apply(&mut held) {
            return Resolution::TooManyTags(would_hold);
        }
        // A topic's tag record carries nothing but the key and the value, so the
        // map the change left is the map that service now holds — no per-record
        // field to preserve, which is what `option_tag_list!` edits in place for.
        topic.tags = held.clone().into_iter().collect();
    }
    Resolution::Present { tags: Some(held) }
}

/// Amazon S3: a general purpose BUCKET OF THIS ACCOUNT, whose tags are the bucket's own —
/// a bucket has ONE tag set however it is written, so the set this API reads and writes IS
/// the set `GetBucketTagging` reports and `PutBucketTagging` replaces, and a bucket created
/// or deleted through S3 appears in and disappears from this API's answers with nothing to
/// keep in step.
///
/// The bucket is resolved through the SAME seam [`tagged_resources`] projects from — the
/// caller's own general purpose buckets, which is what that service's own `ListBuckets`
/// reports — so a bucket this API accepts a write for is a bucket it also reports: a
/// directory bucket, one AWS publishes, or one another emulated service provisioned as the
/// object store of its own resource is no bucket of this account, and is answered the same
/// `NoSuchBucket` as a name no bucket exists under. The Region is the one half the two
/// cannot share: a bucket ARN carries no Region segment, so a write is addressed by NAME
/// from whichever endpoint it arrives at, exactly as that service's own `PutBucketTagging`
/// is, while a READ reports only the Region the bucket lives in because `GetResources`
/// documents its projection that way.
///
/// An `s3` ARN that names anything else — an object (`<bucket>/<key>`), an access point —
/// is left [`Resolution::Unowned`], the answer every value no linked owner addresses gets,
/// so this crate's own store stays its tag home.
fn s3<Eng: Send + Sync + 'static>(
    engine: &Eng,
    arn: &ParsedArn,
    change: Option<&Change<'_>>,
) -> Resolution {
    let Some(state) = owner_s3::state::service_state(engine) else {
        return Resolution::Unowned;
    };
    let Some(name) = bucket_named_by(arn) else {
        return Resolution::Unowned;
    };
    let mut store = state.lock();
    // That service's own refusal for a name this account holds no bucket under:
    // `NoSuchBucket` at HTTP 404 (`smithy_query(query="shape",shape="NoSuchBucket")` on the
    // S3 model), carrying the sentence it words for it.
    let not_found = || {
        absent_with_status(
            404,
            owner_s3::state::NO_SUCH_BUCKET_CODE,
            owner_s3::state::NO_SUCH_BUCKET_MESSAGE.to_string(),
        )
    };
    // A resolution with no change is a READ, so it borrows the bucket immutably and the
    // store is never reached for mutably; only a tag write asks for the bucket itself.
    let Some(held) = store.bucket(name).filter(is_this_accounts).map(bucket_tags) else {
        return not_found();
    };
    let mut held: Tags = held;
    if let Some(change) = change {
        if let Err(would_hold) = change.apply(&mut held) {
            return Resolution::TooManyTags(would_hold);
        }
        // That service's own cap on a BUCKET's tag set, which a write through this API is
        // held to exactly as a write through its own `PutBucketTagging` is.
        if held.len() > owner_s3::state::BUCKET_TAG_SET_LIMITS.max_tags {
            return Resolution::TooManyTags(held.len());
        }
        let Some(bucket) = store.bucket_mut(name) else {
            return not_found();
        };
        bucket.replace_tags(
            held.iter()
                .map(|(key, value)| owner_s3::state::ResourceTag {
                    key: key.clone(),
                    value: value.clone(),
                })
                .collect(),
        );
    }
    Resolution::Present { tags: Some(held) }
}

/// Whether `bucket` is a general purpose bucket of the CALLER'S OWN S3 account, which is
/// what [`tagged_resources`] projects and therefore what this API may tag: the test that
/// service's own `general_purpose_account_buckets` applies, read off the one bucket a
/// request names.
fn is_this_accounts(bucket: &&owner_s3::state::Bucket) -> bool {
    bucket.provisioned_by().is_none() && !bucket.is_aws_published() && !bucket.is_directory()
}

/// The general purpose bucket `arn` names, or `None` for an `s3` ARN that names
/// something else.
///
/// The documented form is `arn:aws:s3:::<bucket>` — "because a bucket name is globally
/// unique" the Region and account fields are empty — and the resource field is the bucket
/// name alone, so an ARN carrying either field, or a resource field with a further segment
/// (an object, an access point), names something other than a bucket.
fn bucket_named_by(arn: &ParsedArn) -> Option<&str> {
    let names_a_bucket =
        arn.region.is_empty() && arn.account.is_empty() && !arn.resource.contains([':', '/']);
    names_a_bucket.then_some(arn.resource.as_str())
}

/// One bucket's tag set as [`Tags`]: that service holds a bucket's tags as an ordered list
/// of key/value records, and this crate handles tags as one ordered map.
fn bucket_tags(bucket: &owner_s3::state::Bucket) -> Tags {
    bucket
        .tags()
        .iter()
        .map(|tag| (tag.key.clone(), tag.value.clone()))
        .collect()
}

/// Amazon DynamoDB: a table, whose tags are the table's own — the resource that
/// service's tagging surface addresses ("Identifies the Amazon DynamoDB resource to
/// which tags should be added", `TagResource$ResourceArn`) — so the set this API reads
/// and writes IS the set its own `ListTagsOfResource` reports, and a table created with
/// `Tags` or deleted through DynamoDB appears in and disappears from this API's answers
/// with nothing to keep in step.
///
/// The ARN is read by that service's OWN rule for a tagging `ResourceArn`
/// ([`owner_dynamodb::state::resolve_resource_arn`]), so no second copy of the resolution
/// lives here to drift from it. What this API addresses in that namespace is a TABLE: the
/// resource type its own index reports (`dynamodb:table`), and the only DynamoDB resource
/// this projection holds a tag set for. A value that names anything else of that service —
/// a sub-resource ARN such as an index, or the STREAM its own tagging surface does address
/// and holds a separate tag set for — is a bad request to THIS API, reported with that
/// service's own sentence for a `ResourceArn` it cannot address a table by, so the two
/// surfaces word one refusal.
///
/// The Region comes off the ARN, which is the Region the request is addressed to since
/// [`super::addressable_arn`] has already refused an ARN of any other.
fn dynamodb<Eng: Send + Sync + 'static>(
    engine: &Eng,
    arn: &ParsedArn,
    change: Option<&Change<'_>>,
) -> Resolution {
    let Some(state) = owner_dynamodb::state::service_state(engine) else {
        return Resolution::Unowned;
    };
    let resource = match owner_dynamodb::state::resolve_resource_arn(&arn.arn, &arn.region) {
        Ok(resource) => resource,
        // That service's 400 `ValidationException` — the channel its own tagging writes
        // refuse an unaddressable `ResourceArn` through — carrying the sentence it words
        // for the value.
        Err(error) => return absent("ValidationException", error.message().to_string()),
    };
    // Of the resources that resolution reads, this API addresses the TABLE: the stream of
    // a table carries a tag set of its own in that service, which is not the set this
    // projection reports, so a stream ARN names no resource this API can tag and is
    // refused as the value it is — in that service's own words for an unaddressable
    // `ResourceArn`.
    if resource.target != owner_dynamodb::state::PolicyTarget::Table {
        return absent(
            "ValidationException",
            owner_dynamodb::state::invalid_resource_arn_message(&arn.arn),
        );
    }
    let now = SystemTime::now();
    let mut state = state.lock();
    // A resolution must see the store a report of it sees: materialising the deletion the
    // clock already owes is what keeps a table whose `DeleteTable` span has elapsed from
    // being tagged after it is gone.
    state.apply_due_transitions(now);
    let table = match state.table_by_arn_mut(&resource) {
        Ok(table) => table,
        // That service's own `ResourceNotFoundException`, modelled at HTTP 400
        // (`smithy_query(query="operation", operation="TagResource")` on the DynamoDB
        // model), with the sentence it words for an ARN it holds no table for.
        Err(error) => return absent("ResourceNotFoundException", error.message().to_string()),
    };
    let Some(change) = change else {
        // A READ reports what that service's own EVENTUALLY CONSISTENT tag reader
        // reports, which is the projection of the stored set whose propagation span has
        // elapsed.
        return Resolution::Present {
            tags: Some(table.tags.visible().clone()),
        };
    };
    // "The operation conflicts with the resource's availability" — a table being deleted
    // takes no configuration change, so a write is refused exactly as that service's own
    // tagging writes refuse it, in its own wording for the action asked for.
    if table.status(now) == owner_dynamodb::state::TableStatus::Deleting {
        return absent(
            "ResourceInUseException",
            owner_dynamodb::state::being_deleted_message(
                &resource.arn,
                match change {
                    Change::Upsert { .. } => owner_dynamodb::state::TAG_WRITE_ACTION,
                    Change::Remove(_) => owner_dynamodb::state::TAG_REMOVAL_ACTION,
                },
            ),
        );
    }
    // A WRITE reads the set the writes have left on the table, never the eventually
    // consistent projection of it, so two writes inside one propagation span compose —
    // the rule that service's own writes follow.
    let mut held: Tags = table.tags.stored().clone();
    if let Err(would_hold) = change.apply(&mut held) {
        return Resolution::TooManyTags(would_hold);
    }
    // That service's own cap on a table's tag set, which a write through this API is held
    // to exactly as a write through its own `TagResource` is.
    if held.len() > owner_dynamodb::state::MAX_TAGS_PER_TABLE {
        return Resolution::TooManyTags(held.len());
    }
    // Applied through that service's OWN editors, so the write starts the propagation its
    // own writes start: the set lands on the table now and every tag reader of it — this
    // API's projection included — reports it one span later.
    match change {
        Change::Upsert { tags: upserts, .. } => table.tags.merge((*upserts).clone(), now),
        Change::Remove(keys) => table.tags.remove(keys.iter().map(String::as_str), now),
    }
    Resolution::Present { tags: Some(held) }
}

/// Amazon ECS: clusters, capacity providers, task definitions, services, tasks
/// and container instances all carry their own `tags`.
///
/// The ARN is resolved against those records by the ECS crate's OWN resolver
/// ([`owner_ecs::state::TaggedResourceKind`] plus `resource_tags_mut`), so the
/// kinds this API can tag and the records it writes to are the ones that service
/// recognises, with no second copy of the resolution here to drift from it.
fn ecs<Eng: Send + Sync + 'static>(
    engine: &Eng,
    arn: &ParsedArn,
    change: Option<&Change<'_>>,
) -> Resolution {
    use owner_ecs::state::TaggedResourceKind;

    /// One ECS tag record from a pair.
    fn tag(key: &str, value: &str) -> owner_ecs::ops::types::Tag {
        owner_ecs::ops::types::Tag {
            key: Some(key.to_string()),
            value: Some(value.to_string()),
        }
    }

    let Some(state) = owner_ecs::state::service_state(engine) else {
        return Resolution::Unowned;
    };
    let kind = TaggedResourceKind::from_arn(&arn.arn);
    let mut state = state.lock();
    if let Some(kind) = kind {
        if let Some(tags) = state.resource_tags_mut(kind, &arn.arn) {
            return option_tag_list!(tags, change, tag);
        }
    }
    // ECS's own `TagResource` models both refusals
    // (`smithy_query(query="operation", operation="TagResource")` on the ECS
    // model): the cluster-specific one for a cluster ARN, the generic one
    // otherwise.
    if kind == Some(TaggedResourceKind::Cluster) {
        absent(
            "ClusterNotFoundException",
            owner_ecs::state::CLUSTER_NOT_FOUND_MESSAGE.to_string(),
        )
    } else {
        absent(
            "ResourceNotFoundException",
            owner_ecs::state::resource_not_found_message(&arn.arn),
        )
    }
}

/// Elastic Load Balancing: a load balancer, a target group, a listener, a listener
/// rule or a trust store — the resources that service documents as taggable ("You can
/// tag your Application Load Balancers, Network Load Balancers, Gateway Load
/// Balancers, target groups, trust stores, listeners, and rules", its `AddTags`) — each
/// holding its own tag set, which is therefore the set this API reads and writes.
///
/// The ARN is read by that service's OWN reader of a tagging `ResourceArn`
/// ([`owner_elb::state::taggable_resource`]) and resolved through its own tag
/// accessors, so the kinds this API can tag, the record each ARN resolves to and the
/// `NotFound` a missing one earns are that service's own, with no second copy here to
/// drift from them. A value in this namespace of no resource type that service
/// addresses is left [`Resolution::Unowned`], the answer every namespace no linked
/// owner serves gets, so this crate's own store stays its tag home.
fn elasticloadbalancing<Eng: Send + Sync + 'static>(
    engine: &Eng,
    arn: &ParsedArn,
    change: Option<&Change<'_>>,
) -> Resolution {
    let Some(state) = owner_elb::state::service_state(engine) else {
        return Resolution::Unowned;
    };
    let Some(kind) = owner_elb::state::taggable_resource(&arn.arn) else {
        return Resolution::Unowned;
    };
    let mut state = state.lock();
    // A tag write never brings a resource into being, so an ARN that service holds no
    // record for is a per-resource failure carrying the `NotFound` of the kind the ARN
    // itself declares — that error's own awsQuery code, the 400 it is modelled with,
    // and the wording that service composes for the kind.
    let Some(stored) = state.tags_of_mut(&arn.arn) else {
        return absent(
            elb_not_found_code(kind),
            owner_elb::state::taggable_not_found_message(kind, &arn.arn),
        );
    };
    let mut held = pairs(stored);
    if let Some(change) = change {
        if let Err(would_hold) = change.apply(&mut held) {
            return Resolution::TooManyTags(would_hold);
        }
        // In place, for the reason `option_tag_list!` gives: a key the change does not
        // name keeps the exact value that service stored for it.
        match change {
            Change::Upsert { tags: upserts, .. } => {
                for (key, value) in upserts.iter() {
                    stored.insert(key.clone(), value.clone());
                }
            }
            Change::Remove(keys) => {
                for key in keys.iter() {
                    stored.remove(key);
                }
            }
        }
    }
    Resolution::Present { tags: Some(held) }
}

/// The wire error code Elastic Load Balancing refuses an absent resource of each
/// taggable kind with: the `awsQueryError` of that kind's modelled `NotFound`
/// exception, which is what a client reads back from that service — not the shape name
/// (`smithy_query(query="shape", shape="TargetGroupNotFoundException")` and its
/// siblings on that service's model).
fn elb_not_found_code(kind: owner_elb::state::TaggableResource) -> &'static str {
    use owner_elb::state::TaggableResource;
    match kind {
        TaggableResource::LoadBalancer => "LoadBalancerNotFound",
        TaggableResource::TargetGroup => "TargetGroupNotFound",
        TaggableResource::Listener => "ListenerNotFound",
        TaggableResource::Rule => "RuleNotFound",
        TaggableResource::TrustStore => "TrustStoreNotFound",
    }
}

/// Amazon EventBridge: "rules and event buses can be tagged" (`TagResource`),
/// and that service holds the tag set of BOTH kinds, so those sets are the ones
/// this API reads and writes.
///
/// The ARN is resolved against those records by the EventBridge crate's OWN
/// resolver ([`owner_events::state::parse_tagged_resource`] plus `tags_of_mut`),
/// so the kinds this API can tag, the record each ARN resolves to and the
/// refusal a missing one gets are that service's own — with no second copy of
/// the resolution, the tags or the wording here to drift from it.
fn events<Eng: Send + Sync + 'static>(
    engine: &Eng,
    arn: &ParsedArn,
    change: Option<&Change<'_>>,
) -> Resolution {
    /// One EventBridge tag record from a pair.
    fn tag(key: &str, value: &str) -> owner_events::ops::types::Tag {
        owner_events::ops::types::Tag {
            key: key.to_string(),
            value: value.to_string(),
        }
    }

    let Some(state) = owner_events::state::service_state(engine) else {
        return Resolution::Unowned;
    };
    // An `events` ARN that resolver does not recognise as a taggable resource of
    // that service at all is NOT a missing resource: it is a request EventBridge
    // refuses, so the per-resource failure carries that refusal — its own
    // parameter wording under the `ValidationException` code, which is still
    // "any valid error code returned by the ... service that hosts the
    // resource".
    let resource = match owner_events::state::parse_tagged_resource(&arn.arn, &arn.region) {
        Ok(resource) => resource,
        Err(reason) => return absent("ValidationException", reason),
    };
    let mut state = state.lock();
    // A tag write never brings a resource into being, so an ARN that service
    // holds no record for is a per-resource failure carrying ITS not-found
    // wording: the rule named together with the event bus it was looked up on,
    // or the bus named — composed by that service, never assembled here out of
    // the ARN text.
    let Some(stored) = state.tags_of_mut(&resource) else {
        return absent(
            "ResourceNotFoundException",
            state.missing_tagged_resource(&resource),
        );
    };
    let mut pairs = plain_tag_pairs(stored);
    if let Some(change) = change {
        if let Err(would_hold) = change.apply(&mut pairs) {
            return Resolution::TooManyTags(would_hold);
        }
        // In place, for the reason `option_tag_list!` gives.
        match change {
            Change::Upsert { tags: upserts, .. } => {
                for (key, value) in upserts.iter() {
                    match stored.iter_mut().find(|held| &held.key == key) {
                        Some(held) => held.value = value.clone(),
                        None => stored.push(tag(key, value)),
                    }
                }
            }
            Change::Remove(keys) => {
                stored.retain(|held| !keys.iter().any(|removed| removed == &held.key))
            }
        }
    }
    Resolution::Present { tags: Some(pairs) }
}

/// AWS Glue: the Data Catalog databases and connections, crawlers and job definitions
/// whose create carries a `Tags` map, each of which that service holds the tag set of —
/// so that set is the one this API reads and writes.
///
/// The ARN is resolved against those records by the Glue crate's OWN resolver
/// ([`owner_glue::state::resolve_tagged_resource`] plus `resource_tags_mut`), so the kinds
/// this API can tag, the record each ARN resolves to and the refusal a missing one gets
/// are that service's own, with no second copy here to drift from it.
fn glue<Eng: Send + Sync + 'static>(
    engine: &Eng,
    arn: &ParsedArn,
    change: Option<&Change<'_>>,
) -> Resolution {
    let Some(state) = owner_glue::state::service_state(engine) else {
        return Resolution::Unowned;
    };
    // A `glue` ARN that names one of Glue's resource types is an address Glue looks an
    // entity up BY, so what it places is that service's answer: an ARN naming no entity of
    // it at all is a per-resource MISS in Glue's own generic wording, and only a value that
    // addresses no Glue resource is the request Glue refuses — the code its tag write models
    // for a bad input, with the prose it states, which for that refusal is none
    // ([`owner_glue::state::INVALID_RESOURCE_ARN_MESSAGE`]).
    let resource = match owner_glue::state::resolve_tagged_resource(&arn.arn) {
        owner_glue::state::TaggedResourceAddress::Resource(resource) => resource,
        // A kind whose record carries no tag set reads back the empty map through Glue's own
        // tag read, but holds no tag set for THIS API to read or write, so it is the same
        // per-resource miss as an ARN that service places no entity under at all.
        owner_glue::state::TaggedResourceAddress::UntaggedResource(_)
        | owner_glue::state::TaggedResourceAddress::NoEntity => {
            return absent(
                "EntityNotFoundException",
                owner_glue::state::messages::UNPLACEABLE_RESOURCE_ARN_NOT_FOUND.to_string(),
            );
        }
        owner_glue::state::TaggedResourceAddress::NotGlueResourceAddress => {
            return absent_stating(
                "InvalidInputException",
                owner_glue::state::INVALID_RESOURCE_ARN_MESSAGE,
            );
        }
    };
    let mut state = state.lock();
    // A tag write never brings a resource into being, so an ARN that service holds no
    // record for is a per-resource failure carrying ITS not-found wording, composed by
    // that service.
    let Some(stored) = state.resource_tags_mut(&resource) else {
        return absent(
            "EntityNotFoundException",
            owner_glue::state::tagged_resource_not_found_message(&resource, &arn.arn),
        );
    };
    let mut pairs: Tags = stored
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if let Some(change) = change {
        if let Err(would_hold) = change.apply(&mut pairs) {
            return Resolution::TooManyTags(would_hold);
        }
        // In place, for the reason `option_tag_list!` gives: a key the change does not
        // name keeps the exact value Glue stored for it.
        match change {
            Change::Upsert { tags: upserts, .. } => {
                for (key, value) in upserts.iter() {
                    stored.insert(key.clone(), value.clone());
                }
            }
            Change::Remove(keys) => {
                for key in keys.iter() {
                    stored.remove(key);
                }
            }
        }
    }
    Resolution::Present { tags: Some(pairs) }
}

/// AWS Lambda: a function, a capacity provider, an event source mapping or a
/// code-signing configuration — the four resources that service holds a tag set
/// for — each carrying its own tags, which are therefore the tags this API reads
/// and writes.
///
/// The ARN is read and resolved by that service's OWN reader of a tagging
/// `Resource` ([`owner_lambda::state::resolve_resource_tags`], which every one of
/// its `/2017-03-31/tags/{Resource}` operations goes through), so the resource
/// kinds this API can tag, the record each ARN resolves to and the refusal an ARN
/// that names no tag set earns are that service's own, with no second copy here
/// to drift from them. Both of its refusals travel as that service's own identity:
/// an ARN naming no resource it holds is its 404 `ResourceNotFoundException`
/// (`Function not found: <arn>` for a function), and an ARN that carries no tag
/// set at all — a function qualified by a version or an alias, which "Lambda does
/// not support adding tags to" — is its 400 `InvalidParameterValueException`
/// (`smithy_query(query="operation", operation="TagResource")` on the Lambda
/// model declares both).
///
/// The Region comes off the ARN rather than from the request, which is the same
/// Region: `addressable_arn` has already refused an ARN of any other, and that
/// service resolves a function ARN against the Region the ARN names.
fn lambda<Eng: Send + Sync + 'static>(
    engine: &Eng,
    arn: &ParsedArn,
    change: Option<&Change<'_>>,
) -> Resolution {
    let Some(state) = owner_lambda::state::service_state(engine) else {
        return Resolution::Unowned;
    };
    let mut state = state.lock();
    let tags = match owner_lambda::state::resolve_resource_tags(
        &mut state,
        &arn.arn,
        &arn.region,
        SystemTime::now(),
    ) {
        Ok(tags) => tags,
        Err(owner_lambda::state::TagResolveFailure::Invalid(message)) => {
            return absent("InvalidParameterValueException", message)
        }
        Err(owner_lambda::state::TagResolveFailure::NotFound(message)) => {
            return absent_with_status(404, "ResourceNotFoundException", message)
        }
    };
    if let Some(change) = change {
        if let Err(would_hold) = change.apply(tags) {
            return Resolution::TooManyTags(would_hold);
        }
    }
    Resolution::Present {
        tags: Some(tags.clone()),
    }
}

/// Amazon KMS: a customer managed key, whose tags are the key's own.
fn kms<Eng: Send + Sync + 'static>(
    engine: &Eng,
    arn: &ParsedArn,
    change: Option<&Change<'_>>,
) -> Resolution {
    let Some(state) = owner_kms::state::service_state(engine) else {
        return Resolution::Unowned;
    };
    let mut state = state.lock();
    let Some(key) = state.keys.values_mut().find(|key| key.arn == arn.arn) else {
        return absent(
            "NotFoundException",
            format!("Key '{}' does not exist", arn.arn),
        );
    };
    if let Some(change) = change {
        if let Err(would_hold) = change.apply(&mut key.tags) {
            return Resolution::TooManyTags(would_hold);
        }
    }
    Resolution::Present {
        tags: Some(key.tags.clone()),
    }
}

/// Amazon CloudWatch Logs: the resources that service holds a tag set for — a log group,
/// a logical destination, a delivery and the delivery source and delivery destination it
/// pairs, and a lookup table — whose tags are the resource's own.
///
/// The ARN is read by the SAME resolution that service's own tagging operations read a
/// `resourceArn` with (its `tagged_resource`, then the one tag set that resource's record
/// holds), so there is ONE tag set per resource: a tag written here is what its
/// `ListTagsForResource` reports, and a tag written there is what `GetResources`
/// discovers.
///
/// A `logs` ARN naming a kind that service holds no tag set for — a log stream, the
/// account's stored query results — is that ARN's own per-resource failure carrying that
/// service's refusal for a `resourceArn` it cannot make a tagging target of, and a value
/// its tagging member does not admit at all is that ARN's own validation failure.
fn logs<Eng: Send + Sync + 'static>(
    engine: &Eng,
    arn: &ParsedArn,
    change: Option<&Change<'_>>,
) -> Resolution {
    let Some(state) = owner_logs::state::service_state(engine) else {
        return Resolution::Unowned;
    };
    // That service's tagging operations name the ARN in a member whose own constraint
    // admits no `*`, so a value carrying one — the wildcard-suffixed log-group ARN form
    // its `DescribeLogGroups` reports, which names the group together with everything
    // under it rather than one resource — is refused as the value it is, before any
    // resource is resolved: a VALIDATION failure for that ARN alone, naming the member
    // that service takes an ARN in.
    if !owner_logs::state::is_tagging_resource_arn_value(&arn.arn) {
        return Resolution::Absent(crate::state::invalid_member_failure(
            owner_logs::state::TAGGING_RESOURCE_ARN_MEMBER,
        ));
    }
    let Some(resource) = owner_logs::state::tagged_resource(&arn.arn) else {
        return absent(
            "InvalidParameterException",
            owner_logs::state::UNTAGGABLE_RESOURCE_ARN.to_string(),
        );
    };
    let mut state = state.lock();
    // The Region comes off the ARN, which is the Region the call was made in:
    // `addressable_arn` has already refused an ARN of any other, and that service holds
    // its Region-scoped resources (the delivery family) under the Region they were
    // created in.
    let Some(tags) = state.resource_tags_mut(&arn.region, &resource) else {
        return absent(
            "ResourceNotFoundException",
            owner_logs::state::resource_not_found_message(resource.kind()),
        );
    };
    if let Some(change) = change {
        if let Err(would_hold) = change.apply(tags) {
            return Resolution::TooManyTags(would_hold);
        }
    }
    Resolution::Present {
        tags: Some(tags.clone()),
    }
}

/// AWS Secrets Manager: a secret, whose tags are the secret's own.
fn secretsmanager<Eng: Send + Sync + 'static>(
    engine: &Eng,
    arn: &ParsedArn,
    change: Option<&Change<'_>>,
) -> Resolution {
    /// One Secrets Manager tag record from a pair.
    fn tag(key: &str, value: &str) -> owner_secretsmanager::state::StoredTag {
        owner_secretsmanager::state::StoredTag {
            key: Some(key.to_string()),
            value: Some(value.to_string()),
        }
    }

    let Some(state) = owner_secretsmanager::state::service_state(engine) else {
        return Resolution::Unowned;
    };
    let mut state = state.lock();
    // As in `tagged_resources`: a deletion the clock has already made due is
    // materialised first, so an ARN whose secret the owning service would
    // already have purged resolves as absent rather than being tagged.
    state.expire_due(SystemTime::now());
    let Some(name) = state.arn_index.get(&arn.arn).cloned() else {
        return absent(
            "ResourceNotFoundException",
            "Secrets Manager can't find the specified secret.".to_string(),
        );
    };
    let Some(secret) = state.secrets.get_mut(&name) else {
        return absent(
            "ResourceNotFoundException",
            "Secrets Manager can't find the specified secret.".to_string(),
        );
    };
    option_tag_list!(&mut secret.tags, change, tag)
}

/// AWS Step Functions: a state machine or an activity, whose tags are that
/// resource's own — the two resource types that service holds a tag set for. An
/// execution is resolved but carries no tag set of its own.
fn states<Eng: Send + Sync + 'static>(
    engine: &Eng,
    arn: &ParsedArn,
    change: Option<&Change<'_>>,
) -> Resolution {
    /// One Step Functions tag record from a pair.
    fn tag(key: &str, value: &str) -> owner_sfn::ops::types::Tag {
        owner_sfn::ops::types::Tag {
            key: Some(key.to_string()),
            value: Some(value.to_string()),
        }
    }

    let Some(state) = owner_sfn::state::service_state(engine) else {
        return Resolution::Unowned;
    };
    let mut state = state.lock();
    // A resolution must see the store a report of it sees: materialising a
    // deletion the clock already owes is what keeps a state machine whose
    // removal came due from being tagged back into existence.
    state.settle(owner_sfn::state::now());
    if let Some(machine) = state.state_machines.get_mut(&arn.arn) {
        return option_tag_list!(&mut machine.tags, change, tag);
    }
    if let Some(activity) = state.activities.get_mut(&arn.arn) {
        return option_tag_list!(&mut activity.tags, change, tag);
    }
    if state.executions.contains_key(&arn.arn) {
        return Resolution::Present { tags: None };
    }
    // Step Functions' own `ResourceNotFound`, whose message the service builds
    // from the request: "Resource not found: '<arn>'".
    absent(
        "ResourceNotFound",
        format!("Resource not found: '{}'", arn.arn),
    )
}
