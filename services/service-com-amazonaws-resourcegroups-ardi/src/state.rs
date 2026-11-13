// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-resourcegroups-ardi service.
//!
//! Every resource group this emulator holds lives in ONE map, [`ComAmazonawsResourcegroupsArdiState::groups`],
//! so `CreateGroup`, `GetGroup`, `GetGroupConfiguration`, `PutGroupConfiguration`,
//! `ListGroups`, `GroupResources`,
//! `UngroupResources`, `ListGroupResources`, `ListGroupingStatuses`, `UpdateGroup`,
//! `SearchResources` and
//! `DeleteGroup` all read and write the same records. Those records are also the
//! only resources this service knows of, so `SearchResources` runs a query over the
//! groups of a region and the memberships recorded in them. The map is a
//! [`BTreeMap`] keyed by [`GroupKey`] — the region the group was created in and its
//! name — because this service is REGIONAL: a group belongs to the region whose
//! endpoint created it, and only that region's callers see it. Within one region
//! the key order is group-name order, which is the listing order, and what makes
//! `ListGroups` paging deterministic and resumable from an opaque token even after
//! a group in the middle of the listing is deleted.
//!
//! Every tag-sync task lives in a second map,
//! [`ComAmazonawsResourcegroupsArdiState::tag_sync_tasks`], keyed by the task's own
//! ARN — the identifier `GetTagSyncTask` and `CancelTagSyncTask` address a task by —
//! so `StartTagSyncTask` and every reader of a task read one record. A task is
//! nested inside the group it syncs into, so it lives exactly as long as that
//! group: deleting the group drops its tasks with it
//! ([`ComAmazonawsResourcegroupsArdiState::remove_tag_sync_tasks_of`]).
//!
//! The account's optional-feature settings live beside both in
//! [`ComAmazonawsResourcegroupsArdiState::account_settings`], which
//! `UpdateAccountSettings` writes and `GetAccountSettings` reports, keyed by region
//! for the same reason the groups are: the endpoint a setting was written through is
//! the endpoint that reports it.

use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AccountSettings, FailedResource, Group, GroupConfiguration, GroupConfigurationItem,
    GroupConfigurationParameter, GroupIdentifier, GroupingStatusesItem, ListGroupResourcesItem,
    QueryError, ResourceIdentifier, ResourceQuery, ResourceStatus, TagSyncTaskItem,
    GROUP_LIFECYCLE_EVENTS_DESIRED_STATUS_VALUES, QUERY_TYPE_VALUES,
};

/// Per-service state for com-amazonaws-resourcegroups-ardi. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsResourcegroupsArdiState {
    /// Every persisted resource group, keyed by the region it lives in and its
    /// name. Ordered by region and then by name, so one region's listing and its
    /// continuation token are stable.
    pub groups: BTreeMap<GroupKey, GroupRecord>,
    /// Every persisted tag-sync task, keyed by `TagSyncTaskRecord::task_arn`.
    /// Ordered, so a listing of tasks and its continuation token are stable.
    pub tag_sync_tasks: BTreeMap<String, TagSyncTaskRecord>,
    /// The optional-feature settings written for this account, keyed by region,
    /// for the same reason [`GroupKey`] carries one: this service is REGIONAL, so
    /// a setting written through one region's endpoint is the setting that
    /// region's endpoint reports, and another region keeps the default until it is
    /// written itself. A region with no entry has never been written and reports
    /// [`AccountSettingsRecord::default`].
    pub account_settings: BTreeMap<String, AccountSettingsRecord>,
}

/// Where one resource group lives: the region whose endpoint created it, and the
/// group's name.
///
/// A region-qualified key is what makes this service's state regional. A group
/// name only has to be unique within one region of one account, so the name alone
/// does not identify a group; the pair does. Ordered by region first and name
/// second, which keeps one region's groups a contiguous, name-ordered range of the
/// map ([`ComAmazonawsResourcegroupsArdiState::groups_in`]).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct GroupKey {
    /// The region the group was created in — the one in the creating caller's
    /// SigV4 credential scope, and the one its stored ARN names.
    pub region: String,
    /// The group's name, unique within [`GroupKey::region`].
    pub name: String,
}

impl GroupKey {
    /// The key of the group named `name` in `region`.
    pub fn new(region: &str, name: &str) -> Self {
        Self {
            region: region.to_string(),
            name: name.to_string(),
        }
    }

    /// A key that sorts at or before every key of `region`: the start of that
    /// region's range of the map. A group name is at least one character long (the
    /// modelled `GroupName`), so no stored key sorts before this one.
    fn first_in(region: &str) -> Self {
        Self::new(region, "")
    }
}

impl ComAmazonawsResourcegroupsArdiState {
    /// The key of the group `identifier` names IN `region`, or `None` when nothing
    /// stored there answers to it.
    ///
    /// `region` is the region the request addressed (its SigV4 credential scope),
    /// and resolution never leaves it: a group created through another region's
    /// endpoint is another region's resource, so it is invisible here however it is
    /// addressed.
    ///
    /// An identifier is either a group ARN or a bare group name, and it is read by
    /// SHAPE, not by prefix:
    ///
    /// * a full `arn:<partition>:resource-groups:<region>:<account>:group/<name>`
    ///   names exactly one resource, so it resolves only when it equals that
    ///   group's STORED ARN — which keeps an ARN for another account, partition or
    ///   region from resolving to a same-named local group;
    /// * the APPLICATION-group form of that ARN, `…:group/<name>/<id>`, names the
    ///   same group through the id that qualifies its name (see
    ///   [`GroupRecord::application_arn`]), so an ARN this service itself
    ///   reported — an application group's own identity, or a tag-sync task's —
    ///   resolves back to the group it was minted from, and only while that group
    ///   still carries that id;
    /// * anything else must be a bare `GroupName`, which the model constrains to
    ///   `[a-zA-Z0-9_\.-]{1,300}` or `<name>/<ulid>` — no colon — so a stored name
    ///   resolves, the qualified form of an application group's name resolves to
    ///   that same group, and an identifier that is neither a well-formed ARN nor
    ///   a well-formed name resolves to nothing instead of slipping past the ARN
    ///   check.
    ///
    /// The caller picks the identifier: an operation that models both members takes
    /// `Group` and falls back to the deprecated `GroupName`, so the group a request
    /// resolves to never depends on which of the two a handler happens to read.
    pub fn lookup(&self, region: &str, identifier: &str) -> Option<GroupKey> {
        if let Some((service, resource)) = parse_arn(identifier) {
            if service != "resource-groups" {
                return None;
            }
            let named = resource.strip_prefix("group/")?;
            let key = GroupKey::new(region, named);
            if let Some(record) = self.groups.get(&key) {
                if identifier != record.group_arn {
                    return None;
                }
                return Some(key);
            }
            // The application-group form: the trailing segment is the id that
            // qualifies the group's name, so the group it names is the one stored
            // under the head segment whose own qualifying id is that segment.
            let (head, id) = named.rsplit_once('/')?;
            if !is_qualifying_id(id) {
                return None;
            }
            let key = GroupKey::new(region, head);
            let record = self.groups.get(&key)?;
            if record.application_id.as_deref() != Some(id)
                || record.application_arn().as_deref() != Some(identifier)
            {
                return None;
            }
            return Some(key);
        }
        if identifier.contains(':') {
            return None;
        }
        let key = GroupKey::new(region, identifier);
        if self.groups.contains_key(&key) {
            return Some(key);
        }
        // The QUALIFIED form of a group name, `<name>/<id>`, which the modelled
        // `GroupName` admits beside a bare name: it names the group stored under
        // the head segment whose own qualifying id is that segment, so the
        // identity this service reports for an application group addresses the
        // group it was minted from.
        let (head, id) = identifier.rsplit_once('/')?;
        if !is_qualifying_id(id) {
            return None;
        }
        let key = GroupKey::new(region, head);
        let record = self.groups.get(&key)?;
        (record.application_id.as_deref() == Some(id)).then_some(key)
    }

    /// Every group stored in `region`, in group-name order — the listing order.
    ///
    /// A region's groups are a contiguous range of the map, so this walks that
    /// range alone and a group of another region is never reached.
    pub fn groups_in<'a>(&'a self, region: &'a str) -> impl Iterator<Item = &'a GroupRecord> + 'a {
        self.groups
            .range(GroupKey::first_in(region)..)
            .take_while(move |(key, _)| key.region == region)
            .map(|(_, record)| record)
    }

    /// The resources of `region` a resource query selects: those whose type the
    /// query's limit admits ([`ResourceQueryRecord::resource_type_limit`]) and
    /// whose tags satisfy every one of its filters, in ARN order and once each.
    ///
    /// Every resource of a region is one of two kinds, and a query runs over both:
    ///
    /// * each group stored in the region, whose own ARN names a resource
    ///   ([`resource_type_of_arn`]) and whose stored tags are the tags this
    ///   service reports for it (`GetTags`);
    /// * every resource ARN recorded as a static membership of one of those
    ///   groups, whose tags this service does not record — so only a query that
    ///   matches on resource type alone selects one.
    ///
    /// Nothing else is matchable: this service records no other resource, so a
    /// query runs over what the store holds rather than over a fabricated
    /// catalog. The matches are keyed by ARN, so a resource several groups record
    /// is one resource. One run is bounded by what the store holds
    /// ([`MAX_GROUPS`] groups of at most [`MAX_MEMBERS_PER_GROUP`] memberships)
    /// and allocates nothing for a resource the query does not select.
    pub fn resources_matching(
        &self,
        region: &str,
        limit: Option<&[String]>,
        filters: &[QueryTagFilter],
    ) -> Vec<MemberRecord> {
        // A membership's tags are not recorded, and a tag filter is satisfied
        // only by carrying its key, so a query that filters on any tag selects no
        // membership at all. That is decided once here rather than per membership.
        let memberships_can_match = filters.is_empty();
        let mut matched: BTreeMap<String, Option<String>> = BTreeMap::new();
        for group in self.groups_in(region) {
            let group_type = resource_type_of_arn(&group.group_arn);
            if query_admits_type(group_type, limit) && carries_query_tags(&group.tags, filters) {
                matched.insert(group.group_arn.clone(), group_type.map(str::to_string));
            }
            if !memberships_can_match {
                continue;
            }
            for member in &group.members {
                if query_admits_type(member.resource_type.as_deref(), limit) {
                    matched.insert(member.resource_arn.clone(), member.resource_type.clone());
                }
            }
        }
        matched
            .into_iter()
            .map(|(resource_arn, resource_type)| MemberRecord {
                resource_arn,
                resource_type,
                // A resource a query selects is selected NOW: there is no
                // asynchronous addition left to report a modelled
                // `ResourceStatusValue` for.
                status: None,
            })
            .collect()
    }

    /// Drop every tag-sync task of the group `key` names, and report how many
    /// went with it.
    ///
    /// A tag-sync task is a resource NESTED INSIDE the group it syncs into — the
    /// modelled `TagSyncTaskArn` writes it as that group's own ARN followed by
    /// `/tag-sync-task/<id>` — so it cannot outlive that group: the one path
    /// that removes a group removes its tasks here, in the same critical
    /// section, so no reader is left reporting a task whose group the store no
    /// longer holds and no later group created under the same name inherits the
    /// tasks of its predecessor.
    pub fn remove_tag_sync_tasks_of(&mut self, key: &GroupKey) -> usize {
        let before = self.tag_sync_tasks.len();
        self.tag_sync_tasks.retain(|_, task| !task.syncs_into(key));
        before - self.tag_sync_tasks.len()
    }

    /// The optional-feature settings `region` reports at `now`.
    ///
    /// For a REPORTING caller: the only thing it writes is the status transition
    /// the clock already made due ([`AccountSettingsRecord::settle_due`]), which
    /// was owed at this instant whether or not anybody called. A region nothing
    /// has ever been written for reports [`AccountSettingsRecord::default`]
    /// WITHOUT storing a record, so reading never creates one.
    pub fn account_settings_at(&mut self, region: &str, now: SystemTime) -> AccountSettingsRecord {
        match self.account_settings.get_mut(region) {
            Some(record) => {
                record.settle_due(now);
                record.clone()
            }
            None => AccountSettingsRecord::default(),
        }
    }

    /// The settings record of `region`, for a caller that is about to WRITE it:
    /// the record is created from the default when that region has none, and the
    /// transition the clock already owes is materialised first, so a write never
    /// discards a status that had already fallen due.
    ///
    /// `Err` carries the message the refusal reports when storing settings for a
    /// region this store holds none for would push it past
    /// [`MAX_ACCOUNT_SETTINGS_REGIONS`]; a region it already holds is always
    /// writable, so the bound never blocks changing a setting that exists.
    pub fn account_settings_mut(
        &mut self,
        region: &str,
        now: SystemTime,
    ) -> Result<&mut AccountSettingsRecord, String> {
        if !self.account_settings.contains_key(region)
            && self.account_settings.len() >= MAX_ACCOUNT_SETTINGS_REGIONS
        {
            return Err(format!(
                "this emulator holds account settings for at most \
                 {MAX_ACCOUNT_SETTINGS_REGIONS} regions",
            ));
        }
        let record = self.account_settings.entry(region.to_string()).or_default();
        record.settle_due(now);
        Ok(record)
    }
}

/// The most groups this emulator holds at once.
///
/// NOT an AWS quota: real Resource Groups enforces its own, and answers its own
/// limit error. This is the emulator's bound on in-memory fixture state, so a
/// client looping on `CreateGroup` is refused rather than growing the store
/// without end. Reaching it is a `BadRequestException`, and deleting a group
/// makes room again.
pub const MAX_GROUPS: usize = 1_000;

/// The most static memberships one group holds at once, for the same reason as
/// [`MAX_GROUPS`] and with the same refusal.
pub const MAX_MEMBERS_PER_GROUP: usize = 1_000;

/// The most tags one group carries at once, for the same reason as
/// [`MAX_GROUPS`] and with the same refusal: it bounds in-memory fixture state,
/// so a client looping on `Tag` is refused rather than growing a record without
/// end. Removing a tag makes room again.
pub const MAX_TAGS_PER_GROUP: usize = 50;

/// The most tag-sync tasks this emulator holds at once, for the same reason as
/// [`MAX_GROUPS`] and with the same refusal: it bounds in-memory fixture state,
/// so a client looping on `StartTagSyncTask` is refused rather than growing the
/// store without end. Cancelling a task makes room again.
pub const MAX_TAG_SYNC_TASKS: usize = 1_000;

/// The prefix AWS keeps for the tags it sets itself.
///
/// AWS's resource-tagging documentation states that a caller can neither edit nor
/// delete a tag whose key carries this prefix, so a request that writes one is
/// refused rather than stored. Tag keys are case-sensitive, but a key does not
/// leave the reserved namespace by re-casing the prefix, so the comparison below
/// ignores case.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` names a tag in the reserved [`RESERVED_TAG_KEY_PREFIX`]
/// namespace.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// The longest tag key this service accepts: the maximum the modelled `TagKey`
/// declares (`length 1..128`).
///
/// Source: `smithy_query(query="shape",shape="TagKey")`. A tag key is a map KEY,
/// whose constraints the request boundary does not walk, so this service decides
/// them itself — in [`check_tag`], the one place it decides them.
pub const MAX_TAG_KEY_LENGTH: usize = 128;

/// The longest tag value this service accepts: the maximum the modelled
/// `TagValue` declares (`length 0..256`), whose minimum of zero makes an empty
/// value legal.
///
/// Source: `smithy_query(query="shape",shape="TagValue")`.
pub const MAX_TAG_VALUE_LENGTH: usize = 256;

/// The character class the modelled tag pattern is built from, as regex source.
///
/// The modelled `TagKey` and `TagValue` share one pattern,
/// `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`
/// (`smithy_query(query="shape",shape="TagKey,TagValue")`): any run of the
/// characters listed here — the Unicode general categories L (letters), Z
/// (separators) and N (numbers), plus the eight punctuation characters.
///
/// The class is kept as the model writes it and judged by a regex engine, the way
/// the request boundary judges a modelled pattern, so a general category means
/// here exactly what it means in the model. Rust's own `char` predicates answer
/// Unicode PROPERTIES rather than general categories and do not substitute for
/// it: [`char::is_alphabetic`] (and so [`char::is_alphanumeric`]) is the
/// `Alphabetic` property, which also holds every `Other_Alphabetic` character —
/// the combining vowel signs and marks of categories Mn and Mc — and none of L, Z
/// and N contains those.
const TAG_CHARACTER_CLASS: &str = r"\p{L}\p{Z}\p{N}_.:/=+\-@";

/// The first character of `text` the modelled tag pattern refuses, or `None` when
/// the text satisfies that pattern (as the empty string vacuously does).
///
/// The pattern is a run of [`TAG_CHARACTER_CLASS`] anchored at both ends, so a
/// text satisfies it exactly when no character of it falls OUTSIDE the class —
/// which is what the complement of the class finds, compiled once for the process
/// and reporting the offending character so the refusal can name it.
fn first_non_tag_character(text: &str) -> Option<char> {
    static OUTSIDE_THE_CLASS: OnceLock<regex::Regex> = OnceLock::new();
    OUTSIDE_THE_CLASS
        .get_or_init(|| {
            regex::Regex::new(&format!("[^{TAG_CHARACTER_CLASS}]"))
                .expect("the modelled tag character class compiles")
        })
        .find(text)
        .and_then(|found| found.as_str().chars().next())
}

/// The request member a tagging call carries its pairs in, as the refusal names it.
pub const TAGS_MEMBER: &str = "Tags";

/// The request member an untagging call carries its keys in, as the refusal names
/// it.
pub const TAG_KEYS_MEMBER: &str = "Keys";

/// `Ok(())` when `key` names a tag a caller may change on a group, else the
/// message the refusal reports — prefixed with `member`, the request member the
/// key arrived in, so a caller reading the message alone knows what to fix.
///
/// The ONE place this service decides whether a string names a tag key a caller
/// controls, so every path that writes tags and the one that removes them judge a
/// key the same way:
///
/// * it stays inside the modelled key length and character set
///   ([`MAX_TAG_KEY_LENGTH`], [`TAG_CHARACTER_CLASS`]);
/// * it is outside the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace, which a
///   caller can neither write nor remove.
pub fn check_tag_key(member: &str, key: &str) -> Result<(), String> {
    let key_length = key.chars().count();
    if !(1..=MAX_TAG_KEY_LENGTH).contains(&key_length) {
        return Err(format!(
            "{member} is not valid: a tag key is 1 to {MAX_TAG_KEY_LENGTH} characters long, \
             and this one is {key_length}",
        ));
    }
    if let Some(character) = first_non_tag_character(key) {
        return Err(format!(
            "{member} is not valid: the tag key {key} carries {character:?}, which is not one of \
             the letters, numbers, separators and `_ . : / = + - @` a tag may use",
        ));
    }
    if is_reserved_tag_key(key) {
        return Err(format!(
            "{member} is not valid: the {RESERVED_TAG_KEY_PREFIX} prefix is reserved, so the tag \
             key {key} is not one a caller can change",
        ));
    }
    Ok(())
}

/// `Ok(())` when every key in `keys` is one a caller may remove from a group, else
/// the message the refusal reports.
///
/// The whole batch is judged BEFORE anything is removed, so a request holding one
/// key this service refuses removes none of its keys.
pub fn check_tag_keys<'a>(keys: impl IntoIterator<Item = &'a str>) -> Result<(), String> {
    for key in keys {
        check_tag_key(TAG_KEYS_MEMBER, key)?;
    }
    Ok(())
}

/// `Ok(())` when the pair `key`/`value` is one a caller may write to a group,
/// else the message the refusal reports.
///
/// The ONE place this service decides whether a tag may be written, so every path
/// that stores tags — a create that carries them and a later tagging call —
/// refuses the same pairs the same way: the key is one [`check_tag_key`] admits,
/// and the value stays inside the modelled length and character set
/// ([`MAX_TAG_VALUE_LENGTH`], [`TAG_CHARACTER_CLASS`]).
pub fn check_tag(key: &str, value: &str) -> Result<(), String> {
    check_tag_key(TAGS_MEMBER, key)?;
    let value_length = value.chars().count();
    if value_length > MAX_TAG_VALUE_LENGTH {
        return Err(format!(
            "{TAGS_MEMBER} is not valid: a tag value is at most {MAX_TAG_VALUE_LENGTH} \
             characters long, and the value of {key} is {value_length}",
        ));
    }
    if let Some(character) = first_non_tag_character(value) {
        return Err(format!(
            "{TAGS_MEMBER} is not valid: the value of the tag key {key} carries {character:?}, \
             which is not one of the letters, numbers, separators and `_ . : / = + - @` a tag \
             may use",
        ));
    }
    Ok(())
}

/// `Ok(())` when every pair in `tags` is one a caller may write, else the message
/// the refusal reports.
///
/// The whole batch is judged BEFORE anything is written, so a request holding one
/// pair this service refuses stores none of its pairs.
pub fn check_tags<'a>(tags: impl IntoIterator<Item = (&'a str, &'a str)>) -> Result<(), String> {
    for (key, value) in tags {
        check_tag(key, value)?;
    }
    Ok(())
}

/// The message this service answers when a `ResourceQuery`'s `Query` document is
/// not one that defines a membership for the query's own `Type`.
///
/// One FIXED sentence, whichever part of the document is at fault, and it names
/// nothing the caller sent: the document is already in the request, so echoing a
/// piece of it back says nothing the request does not say.
pub const QUERY_NOT_VALID_MESSAGE: &str =
    "Query not valid: Query format not valid: check JSON syntax";

/// The modelled `QueryType` whose query names a CloudFormation stack.
///
/// Source: `smithy_query(query="shape",shape="QueryType")`, whose pattern `^\w+$`
/// admits one spelling of each value, so the comparison against it is exact.
pub const STACK_QUERY_TYPE: &str = "CLOUDFORMATION_STACK_1_0";

/// The element a [`STACK_QUERY_TYPE`] query names its stack in: "applicable only
/// if `Type` = `CLOUDFORMATION_STACK_1_0`. The value of this parameter is the
/// Amazon Resource Name (ARN) of the CloudFormation stack whose resources you want
/// included in the group"
/// (`smithy_query(query="doc",shape="ResourceQuery",member="Query")`).
pub const STACK_IDENTIFIER_ELEMENT: &str = "StackIdentifier";

/// The element a tag query carries its tag filters in, and the two elements of one
/// such filter.
///
/// Source: `smithy_query(query="doc",shape="ResourceQuery",member="Query")`, whose
/// `TagFilters` entry documents the collection as
/// `[ { "Key": "Stage", "Values": [ "Gamma", "Beta" ] } ]`.
pub const TAG_FILTERS_ELEMENT: &str = "TagFilters";
const TAG_FILTER_KEY_ELEMENT: &str = "Key";
const TAG_FILTER_VALUES_ELEMENT: &str = "Values";

/// One tag filter of a tag query: the key a matching resource carries, and the
/// values of that key it may carry (none meaning any).
#[derive(Debug, Clone)]
pub struct QueryTagFilter {
    pub key: String,
    pub values: Vec<String>,
}

/// `Ok(())` when `query` is a resource query this service stores, else the
/// message the refusal reports.
///
/// The ONE place this service decides whether a `ResourceQuery` is well formed,
/// so every operation that takes one — a create, a query update, a search and a
/// tag-sync task — refuses the same documents the same way:
///
/// * `Type` is a value the modelled `QueryType` declares
///   ([`crate::ops::types::QUERY_TYPE_VALUES`]);
/// * `Query` is a JSON object, which is what the model documents it as — "the
///   query that defines a group or a search" ([`query_document`]);
/// * that object DEFINES a membership for its `Type`: a [`STACK_QUERY_TYPE`]
///   document names the stack whose resources the group holds
///   ([`stack_identifier`]), and a tag query's [`TAG_FILTERS_ELEMENT`] is the
///   documented collection ([`tag_filters`]).
///
/// A document that defines no membership is refused here rather than stored,
/// because a group whose query cannot resolve membership is one nothing can ever
/// resolve: the mistake belongs at the call that sent it.
pub fn check_resource_query(query: &ResourceQuery) -> Result<(), String> {
    if !QUERY_TYPE_VALUES.contains(&query.r#type.as_str()) {
        return Err(format!(
            "ResourceQuery.Type {} is not valid: expected one of {}",
            query.r#type,
            QUERY_TYPE_VALUES.join(", "),
        ));
    }
    let document = query_document(&query.query)?;
    if query.r#type == STACK_QUERY_TYPE {
        stack_identifier(&document)?;
    } else {
        tag_filters(&document)?;
    }
    Ok(())
}

/// The `Query` document parsed as the JSON object the model documents it as —
/// `{"ResourceTypeFilters":[..], "TagFilters":[..]}` for a tag query and
/// `{"StackIdentifier":".."}` for a stack query — or the message the refusal
/// reports when it is not one.
///
/// One parse is bounded by the modelled `Query` length (`0..4096`), so the work
/// one request asks for is bounded by the request itself.
pub fn query_document(query: &str) -> Result<serde_json::Value, String> {
    match serde_json::from_str::<serde_json::Value>(query) {
        Ok(parsed) if parsed.is_object() => Ok(parsed),
        _ => Err(QUERY_NOT_VALID_MESSAGE.to_string()),
    }
}

/// The stack `document` names, or the message the refusal reports when it names
/// none: a stack query matches the members of ONE stack, so a document carrying no
/// non-empty [`STACK_IDENTIFIER_ELEMENT`] string defines no membership at all.
pub fn stack_identifier(document: &serde_json::Value) -> Result<&str, String> {
    document
        .get(STACK_IDENTIFIER_ELEMENT)
        .and_then(serde_json::Value::as_str)
        .filter(|stack| !stack.trim().is_empty())
        .ok_or_else(|| QUERY_NOT_VALID_MESSAGE.to_string())
}

/// The tag filters `document` carries, or the message the refusal reports when its
/// [`TAG_FILTERS_ELEMENT`] is not the documented collection.
///
/// A document that carries no such element filters on no tag, which is how a query
/// that matches on resource type alone is written.
pub fn tag_filters(document: &serde_json::Value) -> Result<Vec<QueryTagFilter>, String> {
    let Some(entries) = document.get(TAG_FILTERS_ELEMENT) else {
        return Ok(Vec::new());
    };
    let Some(entries) = entries.as_array() else {
        return Err(QUERY_NOT_VALID_MESSAGE.to_string());
    };
    let mut filters: Vec<QueryTagFilter> = Vec::with_capacity(entries.len());
    for entry in entries {
        let Some(key) = entry
            .get(TAG_FILTER_KEY_ELEMENT)
            .and_then(serde_json::Value::as_str)
        else {
            return Err(QUERY_NOT_VALID_MESSAGE.to_string());
        };
        let values = match entry.get(TAG_FILTER_VALUES_ELEMENT) {
            None | Some(serde_json::Value::Null) => Vec::new(),
            Some(serde_json::Value::Array(values)) => {
                let mut collected: Vec<String> = Vec::with_capacity(values.len());
                for value in values {
                    let Some(value) = value.as_str() else {
                        return Err(QUERY_NOT_VALID_MESSAGE.to_string());
                    };
                    collected.push(value.to_string());
                }
                collected
            }
            Some(_) => return Err(QUERY_NOT_VALID_MESSAGE.to_string()),
        };
        filters.push(QueryTagFilter {
            key: key.to_string(),
            values,
        });
    }
    Ok(filters)
}

/// The modelled `QueryErrorCode` for a CloudFormation stack a query names that
/// the account holds none of.
///
/// Source: `smithy_query(query="shape",shape="QueryErrorCode")`, which declares
/// it beside the other codes in [`crate::ops::types::QUERY_ERROR_CODE_VALUES`].
pub const STACK_NOT_EXISTING_QUERY_ERROR: &str = "CLOUDFORMATION_STACK_NOT_EXISTING";

/// RUN `query` over the resources `store` records in `region`: the resources it
/// selects, in ARN order, beside the modelled `QueryError` entries that running
/// it produced.
///
/// The ONE place this service resolves a membership from a resource query, so a
/// group's own membership listing and a search over the recorded resources
/// resolve one the same way:
///
/// * a tag query selects the recorded resources whose type its
///   [`RESOURCE_TYPE_FILTERS_ELEMENT`] admits and whose tags satisfy every one of
///   its [`TAG_FILTERS_ELEMENT`] entries ([`ComAmazonawsResourcegroupsArdiState::resources_matching`]);
/// * a [`STACK_QUERY_TYPE`] query names ONE CloudFormation stack, and this
///   service records no stack and no stack's members, so no stack of the account
///   answers it: that is the condition [`STACK_NOT_EXISTING_QUERY_ERROR`] names,
///   reported beside no resource the way the model documents `QueryErrors`
///   ("each error contains an ErrorCode and Message") rather than refused.
///
/// `Err` carries the message a refusal reports for a document that defines no
/// membership for its type. A STORED query cannot reach it — the one place a
/// query is accepted refuses such a document ([`check_resource_query`]) — so it
/// is reached only by an operation that runs a query the request itself carries.
pub fn run_resource_query(
    store: &ComAmazonawsResourcegroupsArdiState,
    region: &str,
    query: &ResourceQueryRecord,
) -> Result<(Vec<MemberRecord>, Vec<QueryError>), String> {
    let document = query_document(&query.query)?;
    if query.query_type == STACK_QUERY_TYPE {
        let stack = stack_identifier(&document)?;
        return Ok((
            Vec::new(),
            vec![QueryError {
                error_code: Some(STACK_NOT_EXISTING_QUERY_ERROR.to_string()),
                message: Some(format!("Stack with id {stack} does not exist")),
            }],
        ));
    }
    // The tag filters the query matches resources on, read out of the document;
    // a document that carries none matches on resource type alone.
    let filters = tag_filters(&document)?;
    let limit = query.resource_type_limit();
    Ok((
        store.resources_matching(region, limit.as_deref(), &filters),
        // A tag query runs over the recorded resources with nothing left to
        // report: every error code the model declares is about a stack.
        Vec::new(),
    ))
}

/// Whether a resource of type `resource_type` is one a query admits: any type
/// when the query names no limit, else one the limit lists. A resource whose type
/// this emulator has no name for is admitted by no limit, since a limit lists type
/// NAMES.
///
/// [`RESOURCE_TYPE_FILTERS_ELEMENT`] values are compared exactly: the modelled
/// `ResourceType` pattern `^AWS::[a-zA-Z0-9]+::\w+$` admits one spelling of each
/// type name.
fn query_admits_type(resource_type: Option<&str>, limit: Option<&[String]>) -> bool {
    match limit {
        None => true,
        Some(types) => resource_type
            .is_some_and(|resource_type| types.iter().any(|named| named == resource_type)),
    }
}

/// Whether a resource carrying `tags` satisfies every filter in `filters`.
///
/// The documented rule: "If you specify more than one tag key, only resources
/// that match all tag keys, and at least one value of each specified tag key, are
/// returned in your query. If you specify more than one value for a tag key, a
/// resource matches the filter if it has a tag key value that matches any of the
/// specified values" (`smithy_query(query="doc",shape="ResourceQuery",member="Query")`)
/// — so the keys narrow and the values of one key widen. A filter that names no
/// value at all is satisfied by carrying the key, whatever its value.
///
/// Tag keys and values are case-sensitive, so a tag is selected only by an
/// identical key and value.
fn carries_query_tags(tags: &BTreeMap<String, String>, filters: &[QueryTagFilter]) -> bool {
    filters.iter().all(|filter| {
        tags.get(&filter.key)
            .is_some_and(|value| filter.values.is_empty() || filter.values.contains(value))
    })
}

/// The modelled `GroupConfigurationStatus` of a service configuration that is in
/// effect: nothing is being applied to the group.
///
/// `CreateGroup` is synchronous — the model gives it HTTP 200 and a
/// `GroupConfiguration` in its own response, unlike `PutGroupConfiguration`
/// (HTTP 202) — so the configuration a create stores is already the one in
/// effect, with no update in flight to report.
pub const CONFIGURATION_IN_EFFECT: &str = "UPDATE_COMPLETE";

/// The modelled `GroupConfigurationStatus` of a service configuration whose
/// update has been accepted and is not in effect yet.
///
/// `PutGroupConfiguration` is documented as asynchronous — "This occurs
/// asynchronously, and can take time to complete. You can use
/// GetGroupConfiguration to check the status of the update"
/// (`smithy_query(query="doc",shape="PutGroupConfiguration")`) — and the model
/// answers it HTTP 202 with no body, so the configuration a put accepts is
/// reported as the PROPOSED one under this status until it falls due.
pub const CONFIGURATION_UPDATING: &str = "UPDATING";

/// How long after an accepted configuration update the configuration it carries
/// is owed.
///
/// The model documents the update as asynchronous but names no duration, so this
/// span is the emulator's own choice: long enough that a caller reading straight
/// after the put still observes the update in flight, short enough that a caller
/// polling for completion does not wait. It is stored as a due INSTANT on the
/// record ([`PendingGroupConfiguration::due_at`]) and nothing ever sleeps on it.
pub const CONFIGURATION_UPDATE_SETTLE_DELAY: Duration = Duration::from_millis(500);

/// `Ok(())` when `items` are a service configuration this service stores, else
/// the message the refusal reports.
///
/// The ONE place this service decides whether a `GroupConfigurationList` is one
/// it accepts, so a create and a later put judge the same document the same way.
/// Modelled on `GroupConfigurationItem$Type`: "Each item must have a unique
/// value for type." The list's own 0..2 length and each `Type`'s pattern are
/// modelled constraints, refused at the request boundary before any handler
/// runs, so they are not restated here.
pub fn check_configuration(items: &[GroupConfigurationItem]) -> Result<(), String> {
    let duplicate = items
        .iter()
        .enumerate()
        .find(|(at, item)| items[..*at].iter().any(|earlier| earlier.r#type == item.r#type))
        .map(|(_, item)| item.r#type.as_str());
    if let Some(duplicate) = duplicate {
        return Err(format!(
            "Configuration is not valid: each configuration item must have a \
             unique Type, and {duplicate} appears more than once",
        ));
    }
    Ok(())
}

/// The parameters a service-configuration document sets: every parameter of
/// every item it carries, in configuration order, or `None` when the document
/// sets none at all.
///
/// A configuration is one document whatever item declares a parameter, which is
/// how [`GroupRecord::configuration_parameter_values`] reads one parameter
/// across the items too; this is the whole set, reported beside the items it is
/// read off so the two can never disagree. A document with no item, an item with
/// no parameters, and a group with no configuration all set nothing, which is
/// reported as the absent list it is rather than an empty one.
pub fn configuration_parameters(
    items: Option<&[GroupConfigurationItem]>,
) -> Option<Vec<GroupConfigurationParameter>> {
    let parameters: Vec<GroupConfigurationParameter> = items
        .into_iter()
        .flatten()
        .filter_map(|item| item.parameters.as_ref())
        .flatten()
        .cloned()
        .collect();
    (!parameters.is_empty()).then_some(parameters)
}

/// A service-configuration update that the clock already owes at
/// [`PendingGroupConfiguration::due_at`], whether or not anybody calls.
#[derive(Debug, Clone)]
pub struct PendingGroupConfiguration {
    /// The configuration that is in effect from `due_at` on, and `None` when the
    /// accepted update carries no configuration item at all — which leaves the
    /// group with no service configuration, exactly as a create that sent none.
    pub configuration: Option<Vec<GroupConfigurationItem>>,
    /// The instant the update finishes. Public so a test can move the instant the
    /// store holds instead of sleeping for the span.
    pub due_at: SystemTime,
}

/// The service-configuration parameter that decides whether a group may be
/// deleted.
///
/// A parameter is named in the AWS "Service configurations for resource groups"
/// reference, which is the vocabulary `GroupConfigurationParameter$Name` is
/// documented in; the modelled name pattern `^[a-z-]+$` admits exactly one
/// spelling, so the comparison in [`GroupRecord::configuration_parameter_has_value`]
/// is exact.
pub const DELETION_PROTECTION_PARAMETER: &str = "deletion-protection";

/// The [`DELETION_PROTECTION_PARAMETER`] value that keeps a group which still
/// holds resources from being deleted. Its documented alternative (`DISABLED`),
/// any other value, and the parameter being absent all leave deletion
/// unprotected.
pub const DELETION_PROTECTION_UNLESS_EMPTY: &str = "UNLESS_EMPTY";

/// The group service-configuration types a STATIC-MEMBERSHIP operation works
/// with.
///
/// `GroupResources` documents the whole list of groups it can be used with —
/// "You can only use this operation with the following groups: … Other resource
/// group types and resource types are not currently supported by this
/// operation" (`smithy_query(query="doc",shape="GroupResources")`) — so a group
/// whose stored configuration declares none of these is not one such an
/// operation serves. Sorted, and compared exactly: the modelled
/// `GroupConfigurationType` pattern `^AWS::[a-zA-Z0-9]+::[a-zA-Z0-9]+$` admits
/// one spelling of each.
pub const STATIC_MEMBERSHIP_CONFIGURATION_TYPES: &[&str] = &[
    "AWS::EC2::CapacityReservationPool",
    "AWS::EC2::HostManagement",
    APPLICATION_GROUP_CONFIGURATION_TYPE,
];

/// The service-configuration type that makes a group an APPLICATION group.
///
/// `GroupConfigurationType`'s pattern `^AWS::[a-zA-Z0-9]+::[a-zA-Z0-9]+$` admits
/// exactly one spelling of it, so a stored item's type is compared to this
/// exactly; a group declaring it is the kind of group the modelled
/// `Group$ApplicationTag` is "only supported for"
/// (`smithy_query(query="doc",shape="Group",member="ApplicationTag")`).
pub const APPLICATION_GROUP_CONFIGURATION_TYPE: &str = "AWS::ResourceGroups::ApplicationGroup";

/// The one tag key an application group's membership tag is written with.
///
/// The modelled `ApplicationTagKey` pattern is `^awsApplication$`
/// (`smithy_query(query="shape",shape="ApplicationTag")`), so the map an
/// application group reports has this single key.
pub const APPLICATION_TAG_KEY: &str = "awsApplication";

/// The message this service answers when an operation is applied to a group whose
/// FORM does not support it: a static-membership call on a group whose
/// configuration declares none of [`STATIC_MEMBERSHIP_CONFIGURATION_TYPES`], a
/// configuration call on a group that holds only a resource query, or a query call
/// on a group that holds only a configuration.
///
/// One FIXED sentence for every such refusal. It names neither the group nor the
/// operation nor the forms this service supports: the request already names the
/// group and the operation, and the catalog of supported forms is this emulator's
/// own state, not something a refusal reports.
pub const UNSUPPORTED_TARGET_MESSAGE: &str = "This operation does not support the target";

/// The service-configuration parameter that names the resource types a group's
/// members may have.
///
/// A parameter is named in the AWS "Service configurations for resource groups"
/// reference, which is the vocabulary `GroupConfigurationParameter$Name` is
/// documented in, and this one "specifies the types of resources that you can add
/// to this group"; the modelled name pattern `^[a-z-]+$` admits exactly one
/// spelling, so the comparison in
/// [`GroupRecord::configuration_parameter_values`] is exact. Its values are
/// CloudFormation-style type names, the same vocabulary
/// [`resource_type_of_arn`] answers in.
pub const ALLOWED_RESOURCE_TYPES_PARAMETER: &str = "allowed-resource-types";

/// The `FailedResource$ErrorCode` a static-membership call reports for a resource
/// ARN the addressed group does not support.
///
/// Fixed text for this error class — the per-resource counterpart of the modelled
/// request-level refusals: the call itself is answered (HTTP 200) and the
/// resources that could not be acted on are reported one by one
/// (`smithy_query(query="shape",shape="FailedResource")`).
pub const UNSUPPORTED_RESOURCE_ARN_ERROR_CODE: &str = "ResourceArnValidationException";

/// The `FailedResource$ErrorMessage` reported beside
/// [`UNSUPPORTED_RESOURCE_ARN_ERROR_CODE`].
///
/// One FIXED sentence for the class, carrying no value of its own: the entry's own
/// `ResourceArn` already names the resource the report is about.
pub const UNSUPPORTED_RESOURCE_ARN_MESSAGE: &str = "Unsupported resource ARN";

/// One persisted resource group: its identity, its describable attributes, the
/// query or the service configuration it was created with, its tags, and its
/// static members.
#[derive(Debug, Clone)]
pub struct GroupRecord {
    /// The group's name, which with the region it lives in is its
    /// [`GroupKey`] in [`ComAmazonawsResourcegroupsArdiState::groups`].
    pub name: String,
    /// `arn:aws:resource-groups:<region>:<account>:group/<name>`, minted once at
    /// create time and never recomputed, so every later read returns the same
    /// ARN. An APPLICATION group's name is QUALIFIED by its application id in
    /// that ARN (`group/<name>/<id>`), because the modelled `ApplicationArn` its
    /// membership tag carries admits only the qualified form.
    pub group_arn: String,
    /// The id of the account that owns the group, captured once at create time
    /// from the emulator's canonical account id — the same account the stored
    /// ARN names — so every later read reports the same owner.
    pub owner_id: String,
    pub description: Option<String>,
    pub criticality: Option<i32>,
    pub owner: Option<String>,
    pub display_name: Option<String>,
    /// The resource query the group was created with, if any.
    pub resource_query: Option<ResourceQueryRecord>,
    /// The service configuration the group was created with, or the one an
    /// accepted `PutGroupConfiguration` has already put in effect, if any. A
    /// group contains either a configuration or a resource query, never both,
    /// and a group created with neither carries `None` here.
    pub configuration: Option<Vec<GroupConfigurationItem>>,
    /// The configuration update the clock still owes, written by an accepted
    /// `PutGroupConfiguration` and `None` when this group has none in flight.
    /// While it is in flight the group is still configured by
    /// [`GroupRecord::configuration`], which is what makes the update the
    /// modelled `ProposedConfiguration` rather than the one in effect.
    pub pending_configuration: Option<PendingGroupConfiguration>,
    /// The group's tags, kept in key order so the stored record is deterministic
    /// (the wire map itself is unordered).
    pub tags: BTreeMap<String, String>,
    /// Static members added by `GroupResources` and dropped by
    /// `UngroupResources`, kept sorted by ARN: the sort is the listing order
    /// `ListGroupResources` pages through.
    pub members: Vec<MemberRecord>,
    /// The last grouping or ungrouping action recorded against each resource of
    /// this group, keyed by resource ARN — what `ListGroupingStatuses` reports.
    /// Ordered by ARN, so that listing and its continuation token are stable.
    pub grouping_statuses: BTreeMap<String, GroupingStatusRecord>,
    /// The id that qualifies this group's name where an ARN addresses it as an
    /// APPLICATION group: minted at create time for a group whose configuration
    /// declares it an application group (and part of its stored
    /// [`GroupRecord::group_arn`] from then on), and otherwise minted once the
    /// group first hosts a resource that needs one. `None` until then. See
    /// [`GroupRecord::application_group_arn`].
    pub application_id: Option<String>,
}

impl GroupRecord {
    /// The service-configuration update the clock still owes at `now`, or `None`
    /// when this group has none in flight.
    pub fn configuration_update_in_flight(
        &self,
        now: SystemTime,
    ) -> Option<&PendingGroupConfiguration> {
        self.pending_configuration
            .as_ref()
            .filter(|pending| pending.due_at > now)
    }

    /// The service configuration in effect at `now`: the one an accepted update
    /// has already put in effect, else the one the record holds.
    ///
    /// An update is owed at its stored instant whether or not anybody calls, so
    /// the configuration in effect is a function of the record AND the clock; the
    /// record itself catches up at the next write
    /// ([`GroupRecord::settle_configuration_due`]), which is why no read has to
    /// change it.
    pub fn configuration_at(&self, now: SystemTime) -> Option<&Vec<GroupConfigurationItem>> {
        match self.pending_configuration.as_ref() {
            Some(pending) if pending.due_at <= now => pending.configuration.as_ref(),
            _ => self.configuration.as_ref(),
        }
    }

    /// The service configuration in effect at this instant — what every attribute
    /// derived from the configuration below is derived from, so each of them
    /// answers for the configuration a caller can see right now and none of them
    /// reads a superseded one.
    fn effective_configuration(&self) -> Option<&Vec<GroupConfigurationItem>> {
        self.configuration_at(SystemTime::now())
    }

    /// Accept `configuration` as this group's new service configuration at `now`.
    ///
    /// It becomes the proposed configuration, reported under
    /// [`CONFIGURATION_UPDATING`], and falls due
    /// [`CONFIGURATION_UPDATE_SETTLE_DELAY`] later, so a caller reading straight
    /// after the write still sees the update in flight. It REPLACES the whole
    /// configuration rather than merging into it, and a request that carries no
    /// configuration item leaves the group with none at all. A second update
    /// supersedes one still in flight, which is what makes the LAST accepted
    /// request the one that decides the outcome.
    pub fn put_configuration(
        &mut self,
        configuration: Option<Vec<GroupConfigurationItem>>,
        now: SystemTime,
    ) {
        // An update that had already fallen due is in effect, so it is
        // materialised before this one supersedes it: the configuration this
        // update is reported against is then the one actually in effect.
        self.settle_configuration_due(now);
        self.pending_configuration = Some(PendingGroupConfiguration {
            // A configuration that carries no item configures nothing, so it is
            // stored exactly as a group that was sent none.
            configuration: configuration.filter(|items| !items.is_empty()),
            due_at: now + CONFIGURATION_UPDATE_SETTLE_DELAY,
        });
    }

    /// Materialise the configuration update the clock already made due at `now`.
    ///
    /// It was owed at this instant whether or not anybody called, so a caller may
    /// settle it; an update not yet due is left alone, and a record with nothing
    /// in flight is unchanged.
    pub fn settle_configuration_due(&mut self, now: SystemTime) {
        if self
            .pending_configuration
            .as_ref()
            .is_none_or(|pending| pending.due_at > now)
        {
            return;
        }
        if let Some(pending) = self.pending_configuration.take() {
            self.configuration = pending.configuration;
        }
    }

    /// Whether this group's stored service configuration declares it an
    /// APPLICATION group, i.e. carries an item of type
    /// [`APPLICATION_GROUP_CONFIGURATION_TYPE`].
    ///
    /// Read across every configuration item the group holds, because the
    /// configuration is one document whatever order its items are in; a group
    /// with no configuration declares nothing.
    pub fn is_application_group(&self) -> bool {
        self.effective_configuration()
            .into_iter()
            .flatten()
            .any(|item| item.r#type == APPLICATION_GROUP_CONFIGURATION_TYPE)
    }

    /// This group's ARN in the APPLICATION-group form — its name qualified by the
    /// 26-character id it carries,
    /// `arn:aws:resource-groups:<region>:<account>:group/<name>/<id>` — or `None`
    /// while it carries no qualifying id at all.
    ///
    /// Computed from the STORED id, so it never mints one: a group whose stored
    /// ARN already ends in that id (an application group, whose identity carries
    /// it from create time) reports that ARN unchanged, and a group whose stored
    /// NAME is already written `<name>/<id>` (the form the modelled `GroupName`
    /// also admits) is likewise already qualified.
    pub fn application_arn(&self) -> Option<String> {
        if let Some(id) = self.application_id.as_deref() {
            let suffix = format!("/{id}");
            return Some(if self.group_arn.ends_with(&suffix) {
                self.group_arn.clone()
            } else {
                format!("{}{suffix}", self.group_arn)
            });
        }
        self.name
            .rsplit_once('/')
            .filter(|(head, id)| !head.is_empty() && is_qualifying_id(id))
            .map(|_| self.group_arn.clone())
    }

    /// This group's ARN in the APPLICATION-group form, minting the qualifying id
    /// if the group has none yet.
    ///
    /// A tag-sync task is addressed by an ARN NESTED inside this form (the
    /// modelled `TagSyncTaskArn` is
    /// `…:group/<name>/<26 chars>/tag-sync-task/<26 chars>`), while a group that
    /// is not an application group is created and addressed by its bare name, so
    /// the qualifying id is minted on first use and then stored: every ARN this
    /// group hosts carries the same one, and repeated reads of a hosted resource
    /// resolve back to this group.
    pub fn application_group_arn(&mut self) -> String {
        if let Some(arn) = self.application_arn() {
            return arn;
        }
        let id = self
            .application_id
            .get_or_insert_with(new_qualifying_id)
            .as_str();
        format!("{}/{id}", self.group_arn)
    }

    /// The modelled `ApplicationTag` this group reports: the tag that defines
    /// membership of the application, `awsApplication` bound to the group's own
    /// application-group ARN — the value a caller then puts on the resources it
    /// wants in the application.
    ///
    /// Only an application group has one, so a group whose stored configuration
    /// declares none reports no tag at all. Computed from the stored identity, so
    /// reading it mints nothing.
    pub fn application_tag(&self) -> Option<std::collections::HashMap<String, String>> {
        if !self.is_application_group() {
            return None;
        }
        let arn = self.application_arn()?;
        Some(std::collections::HashMap::from([(
            APPLICATION_TAG_KEY.to_string(),
            arn,
        )]))
    }

    /// The modelled `Group` structure for this record.
    pub fn to_group(&self) -> Group {
        Group {
            owner_id: Some(self.owner_id.clone()),
            group_arn: self.group_arn.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            criticality: self.criticality,
            owner: self.owner.clone(),
            display_name: self.display_name.clone(),
            application_tag: self.application_tag(),
        }
    }

    /// The modelled `GroupIdentifier` for this record, as `ListGroups` reports it.
    pub fn to_identifier(&self) -> GroupIdentifier {
        GroupIdentifier {
            group_name: Some(self.name.clone()),
            group_arn: Some(self.group_arn.clone()),
            description: self.description.clone(),
            criticality: self.criticality,
            owner: self.owner.clone(),
            display_name: self.display_name.clone(),
        }
    }

    /// The modelled `GroupConfiguration` for this record, if it holds a service
    /// configuration or has an update to one in flight, and `None` for a group
    /// that has neither.
    ///
    /// `ProposedConfiguration` is reported only while an update is in flight,
    /// beside the configuration that is still in effect and the `UPDATING`
    /// status; a configuration with no update in flight is the one in effect, so
    /// it reports neither a proposal nor a `FailureReason` — no update this
    /// emulator accepts fails, so nothing writes that reason.
    /// `GroupParameters` restates the parameters the reported configuration
    /// sets, so the flattened list and the items it is read off can never
    /// disagree ([`configuration_parameters`]).
    pub fn to_group_configuration(&self) -> Option<GroupConfiguration> {
        let now = SystemTime::now();
        if let Some(in_flight) = self.configuration_update_in_flight(now) {
            return Some(GroupConfiguration {
                group_parameters: configuration_parameters(self.configuration.as_deref()),
                configuration: self.configuration.clone(),
                proposed_configuration: Some(
                    in_flight.configuration.clone().unwrap_or_default(),
                ),
                status: Some(CONFIGURATION_UPDATING.to_string()),
                failure_reason: None,
            });
        }
        self.configuration_at(now)
            .map(|items| GroupConfiguration {
                group_parameters: configuration_parameters(Some(items)),
                configuration: Some(items.clone()),
                proposed_configuration: None,
                status: Some(CONFIGURATION_IN_EFFECT.to_string()),
                failure_reason: None,
            })
    }

    /// Whether this record's stored service configuration sets `parameter` to
    /// `value`.
    ///
    /// Read across every configuration item the group holds and every value the
    /// parameter carries: a parameter is set when any item sets it, because the
    /// configuration is one document whatever item type declares the parameter.
    /// A group with no configuration, an item with no parameters and a parameter
    /// with no values all set nothing.
    pub fn configuration_parameter_has_value(&self, parameter: &str, value: &str) -> bool {
        self.configuration_parameter_values(parameter)
            .any(|v| v == value)
    }

    /// Every value this record's stored service configuration gives `parameter`,
    /// in configuration order.
    ///
    /// Read across every configuration item the group holds, because the
    /// configuration is one document whatever item type declares the parameter,
    /// and a parameter may carry several values. A group with no configuration,
    /// an item with no parameters and a parameter with no values all yield
    /// nothing.
    pub fn configuration_parameter_values<'record>(
        &'record self,
        parameter: &str,
    ) -> impl Iterator<Item = &'record str> {
        let parameter = parameter.to_string();
        self.effective_configuration()
            .into_iter()
            .flatten()
            .filter_map(|item| item.parameters.as_ref())
            .flatten()
            .filter(move |p| p.name == parameter)
            .filter_map(|p| p.values.as_ref())
            .flatten()
            .map(String::as_str)
    }

    /// Whether this group's stored service configuration admits a member of
    /// `resource_type` — the CloudFormation-style type name
    /// [`resource_type_of_arn`] answers, or `None` for an ARN whose type this
    /// service cannot name.
    ///
    /// A configuration that sets [`ALLOWED_RESOURCE_TYPES_PARAMETER`] LIMITS the
    /// group's members to the types it names, so a type the parameter does not
    /// name is not one this group supports — and neither is a resource whose type
    /// is unknown, since nothing shows it to be one of them. A configuration that
    /// sets the parameter nowhere declares no such limit and admits any type.
    ///
    /// Judging the unknown type as unsupported is what keeps the limit a limit:
    /// an ARN admitted here is one whose type the parameter names, so widening
    /// the answer to a type [`resource_type_of_arn`] cannot name would let a
    /// group limited to one type hold a resource of any other. It is also what
    /// binds the judgement to that table: a resource of an admitted type whose
    /// ARN form the table does not carry is reported unsupported, and the fix
    /// for that is a row there rather than a looser rule here.
    pub fn admits_resource_type(&self, resource_type: Option<&str>) -> bool {
        let mut limit = self.configuration_parameter_values(ALLOWED_RESOURCE_TYPES_PARAMETER);
        let Some(first) = limit.next() else {
            return true;
        };
        resource_type.is_some_and(|declared| {
            declared == first || limit.any(|admitted| admitted == declared)
        })
    }

    /// Whether this group's stored service configuration admits `resource_arn` as
    /// a member, judged on the resource type the ARN names
    /// ([`GroupRecord::admits_resource_type`]).
    pub fn admits_member_arn(&self, resource_arn: &str) -> bool {
        self.admits_resource_type(resource_type_of_arn(resource_arn))
    }

    /// Whether this group's own stored service configuration forbids deleting it
    /// as it stands.
    ///
    /// [`DELETION_PROTECTION_PARAMETER`] set to
    /// [`DELETION_PROTECTION_UNLESS_EMPTY`] protects a group that still contains
    /// resources, so protection is decided from the configuration AND the
    /// membership the record holds at the moment of the call: emptying the group
    /// lifts it, and filling it again restores it.
    pub fn deletion_is_protected(&self) -> bool {
        !self.members.is_empty()
            && self.configuration_parameter_has_value(
                DELETION_PROTECTION_PARAMETER,
                DELETION_PROTECTION_UNLESS_EMPTY,
            )
    }

    /// The stored service-configuration type that makes this group one a
    /// static-membership operation serves, or `None` when its configuration
    /// declares none of [`STATIC_MEMBERSHIP_CONFIGURATION_TYPES`].
    ///
    /// Read across every configuration item the group holds, because a
    /// configuration is one document and the supported type can be any item of
    /// it. A group created with a resource query is populated from that query
    /// rather than configured, and a group created with neither carries no
    /// configuration, so both answer `None`.
    pub fn static_membership_configuration_type(&self) -> Option<&str> {
        self.effective_configuration()
            .into_iter()
            .flatten()
            .map(|item| item.r#type.as_str())
            .find(|declared| STATIC_MEMBERSHIP_CONFIGURATION_TYPES.contains(declared))
    }

    /// Whether this group was POPULATED through a static-membership grouping —
    /// it holds static members, or it holds the record of a grouping action
    /// performed against one of its resources.
    ///
    /// Only [`GroupRecord::add_member`] and [`GroupRecord::remove_member`] write
    /// a grouping status, and both run only for a group a static-membership
    /// operation serves, so either of those two facts is evidence that this
    /// group is one "you populated using the GroupResources operation"
    /// (`smithy_query(query="doc",shape="UngroupResources")`) — evidence that
    /// survives a later replacement of the service configuration, which carries
    /// the memberships nowhere.
    pub fn was_populated_by_static_grouping(&self) -> bool {
        !self.members.is_empty() || !self.grouping_statuses.is_empty()
    }

    /// Whether a static-membership REMOVAL is served for this group.
    ///
    /// Removing memberships "works only with static groups that you populated
    /// using the GroupResources operation", and a group so populated stays one
    /// whatever configuration is in effect by the time the removal arrives,
    /// since replacing a group's service configuration leaves the memberships
    /// recorded against it where they are. A group that was never populated that
    /// way is served only while its configuration declares a type a
    /// static-membership operation works with — there the removal is the no-op an
    /// empty group answers — and refused otherwise, a group populated from a
    /// resource query among them.
    pub fn serves_static_membership_removal(&self) -> bool {
        self.was_populated_by_static_grouping()
            || self.static_membership_configuration_type().is_some()
    }

    /// The service-configuration types this group's stored configuration
    /// declares, in the order the configuration holds them.
    ///
    /// A configuration is one document made of `GroupConfigurationItem`s, each
    /// declaring one `GroupConfigurationType`, so a group has as many
    /// configuration types as it has items. A group created with a resource
    /// query, or with neither, declares none.
    pub fn configuration_types(&self) -> impl Iterator<Item = &str> {
        self.effective_configuration()
            .into_iter()
            .flatten()
            .map(|item| item.r#type.as_str())
    }

    /// A service-configuration type that makes this group one that CONTAINS a
    /// configuration at `now` — one its configuration declares in effect, or one
    /// an accepted update is about to put in effect — or `None` when it contains
    /// none.
    ///
    /// The modelled either/or ("A resource group can contain either a
    /// Configuration or a ResourceQuery, but not both") is judged against this,
    /// so a group whose configuration update is still in flight cannot be given a
    /// resource query that would land beside it.
    pub fn configured_type(&self, now: SystemTime) -> Option<&str> {
        self.configuration_at(now)
            .into_iter()
            .chain(
                self.configuration_update_in_flight(now)
                    .and_then(|pending| pending.configuration.as_ref()),
            )
            .flatten()
            .map(|item| item.r#type.as_str())
            .next()
    }

    /// The resource types this group's stored resource query limits itself to,
    /// or none for a group created without a query.
    ///
    /// See [`ResourceQueryRecord::resource_type_filters`] for where they are read
    /// from.
    pub fn query_resource_types(&self) -> Vec<String> {
        self.resource_query
            .as_ref()
            .map(ResourceQueryRecord::resource_type_filters)
            .unwrap_or_default()
    }

    /// The resource types this group's query LIMITS its members to, or `None`
    /// when its query imposes no such limit.
    ///
    /// A limit exists only when the stored query names the types it admits, which
    /// is decided by the query itself ([`ResourceQueryRecord::resource_type_limit`]);
    /// a group created without a query carries no limit either, so it admits any
    /// supported type.
    pub fn query_resource_type_limit(&self) -> Option<Vec<String>> {
        self.resource_query
            .as_ref()
            .and_then(ResourceQueryRecord::resource_type_limit)
    }

    /// The modelled `ResourceQuery` for this record, if it was created with one.
    pub fn to_resource_query(&self) -> Option<ResourceQuery> {
        self.resource_query.as_ref().map(|q| ResourceQuery {
            r#type: q.query_type.clone(),
            query: q.query.clone(),
        })
    }

    /// This record's tags in the shape the wire uses.
    pub fn tags_map(&self) -> std::collections::HashMap<String, String> {
        self.tags
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Merge `tags` into this record's tags: a key the batch names is set or
    /// overwritten, and a key it does not name keeps the value it had ("Existing
    /// tags on a resource group are not changed if they are not specified in the
    /// request parameters").
    ///
    /// The whole batch is judged against [`MAX_TAGS_PER_GROUP`] BEFORE anything
    /// is written, so a batch that would push the record past the bound is
    /// refused with the message in the `Err` and stores none of its pairs.
    ///
    /// `Ok` carries the pairs now STORED for the keys the batch named, read back
    /// out of the record, so a caller reports what the store holds rather than
    /// what the request said.
    pub fn merge_tags<'a>(
        &mut self,
        tags: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<std::collections::HashMap<String, String>, String> {
        let batch: Vec<(&str, &str)> = tags.into_iter().collect();
        let new_keys = batch
            .iter()
            .filter(|(key, _)| !self.tags.contains_key(*key))
            .count();
        if self.tags.len() + new_keys > MAX_TAGS_PER_GROUP {
            return Err(format!(
                "this emulator holds at most {MAX_TAGS_PER_GROUP} tags per resource group; \
                 remove a tag before adding another",
            ));
        }
        for (key, value) in &batch {
            self.tags.insert((*key).to_string(), (*value).to_string());
        }
        Ok(batch
            .iter()
            .filter_map(|(key, _)| self.tags.get_key_value(*key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect())
    }

    /// Remove the tags `keys` names from this record, and report the keys whose
    /// tag this call actually removed.
    ///
    /// A key the record does not carry removes nothing and is reported in
    /// nothing, so removing the same key twice is idempotent: the second call
    /// reports no key and leaves the same map behind. A key the batch repeats is
    /// likewise reported once. A key the batch does not name keeps the value it
    /// had, so the removal touches only the keys the request asked for.
    ///
    /// The reported keys come from what the record HELD, in the order the batch
    /// named them, so a caller reports the tags the store lost rather than the
    /// list the request sent.
    pub fn remove_tags<'a>(&mut self, keys: impl IntoIterator<Item = &'a str>) -> Vec<String> {
        let mut removed = Vec::new();
        for key in keys {
            if self.tags.remove(key).is_some() {
                removed.push(key.to_string());
            }
        }
        removed
    }

    /// Whether `resource_arn` is one of this group's static members.
    ///
    /// Found by BINARY SEARCH over [`GroupRecord::members`], which
    /// [`GroupRecord::add_member`] keeps sorted by ARN, so a membership check is
    /// logarithmic in the group's size however many members it holds.
    pub fn holds_member(&self, resource_arn: &str) -> bool {
        self.members
            .binary_search_by(|member| member.resource_arn.as_str().cmp(resource_arn))
            .is_ok()
    }

    /// Record `resource_arn` as a member, keeping [`GroupRecord::members`] sorted
    /// by ARN, and report whether the group GAINED the membership.
    ///
    /// Adding an ARN that is already a member changes no membership and reports
    /// `false`, so repeating a `GroupResources` call does not duplicate a
    /// membership and reports no addition — the mirror of
    /// [`GroupRecord::remove_member`], which reports whether the group LOST one.
    pub fn add_member(&mut self, resource_arn: &str) -> bool {
        // A membership recorded here is the outcome of a GROUP action that
        // succeeded, so the same call records the grouping status
        // `ListGroupingStatuses` reports. It is recorded for an ARN that is
        // already a member too: that grouping is the resource's LAST grouping
        // action whether or not it is the one that first made it a member, and
        // the operation reports each resource's last action.
        self.record_grouping_status(
            resource_arn,
            GROUPING_ACTION_GROUP,
            GROUPING_STATUS_SUCCESS,
            SystemTime::now(),
        );
        let at = self
            .members
            .partition_point(|m| m.resource_arn.as_str() < resource_arn);
        if self
            .members
            .get(at)
            .is_some_and(|m| m.resource_arn == resource_arn)
        {
            return false;
        }
        self.members.insert(
            at,
            MemberRecord {
                resource_arn: resource_arn.to_string(),
                resource_type: resource_type_of_arn(resource_arn).map(str::to_string),
                // The modelled `ResourceStatusValue` has one value, `PENDING`,
                // and the model documents it as "the process isn't completed
                // yet". A membership this emulator records is in place as soon
                // as the call that recorded it returns, so it carries no status.
                status: None,
            },
        );
        true
    }

    /// Drop `resource_arn` from this group's static members, and report whether
    /// the group HELD it.
    ///
    /// Removing an ARN the group does not hold changes nothing and reports
    /// `false`, so repeating an `UngroupResources` call leaves the same group
    /// behind. A removal that happened is an UNGROUP action that succeeded, so
    /// the same call records the grouping status `ListGroupingStatuses` reports
    /// for that resource — replacing the GROUP record its grouping left, since
    /// the operation reports each resource's LAST action.
    pub fn remove_member(&mut self, resource_arn: &str) -> bool {
        let Ok(at) = self
            .members
            .binary_search_by(|member| member.resource_arn.as_str().cmp(resource_arn))
        else {
            return false;
        };
        self.members.remove(at);
        self.record_grouping_status(
            resource_arn,
            GROUPING_ACTION_UNGROUP,
            GROUPING_STATUS_SUCCESS,
            SystemTime::now(),
        );
        true
    }

    /// Record that the last grouping action on `resource_arn` in this group was
    /// `action` (a modelled `GroupingType`) and ended in `status` (a modelled
    /// `GroupingStatus`), at `at`.
    ///
    /// The ONE place a grouping status is written, so every producer of grouping
    /// or ungrouping actions records them the same way and `ListGroupingStatuses`
    /// reads one record per resource. Recording a resource again REPLACES its
    /// entry: the operation reports each resource's last action, not a history.
    /// This slice records the successful GROUP actions
    /// [`GroupRecord::add_member`] performs and the successful UNGROUP actions
    /// [`GroupRecord::remove_member`] performs; an ungrouping replaces the record
    /// its grouping left, so only a NEW membership ever adds an entry.
    ///
    /// The map is bounded by [`MAX_MEMBERS_PER_GROUP`], the same ceiling the
    /// membership itself carries: once it is full, room for a new record is made
    /// by dropping the OLDEST record of a resource the group no longer holds, so
    /// a client cycling resources through one group cannot grow the store without
    /// end and every CURRENT member keeps its record.
    pub fn record_grouping_status(
        &mut self,
        resource_arn: &str,
        action: &str,
        status: &str,
        at: SystemTime,
    ) {
        self.grouping_statuses.insert(
            resource_arn.to_string(),
            GroupingStatusRecord {
                action: action.to_string(),
                status: status.to_string(),
                updated_at: at,
            },
        );
        while self.grouping_statuses.len() > MAX_MEMBERS_PER_GROUP {
            let stale = self
                .grouping_statuses
                .iter()
                .filter(|(arn, _)| !self.holds_member(arn))
                .min_by_key(|(_, record)| record.updated_at)
                .map(|(arn, _)| arn.clone());
            match stale {
                Some(arn) => self.grouping_statuses.remove(&arn),
                // Every record belongs to a current member, and the membership
                // is bounded by the same ceiling: there is nothing to drop.
                None => break,
            };
        }
    }
}

/// The modelled `GroupingType` value for adding a resource to a group — the
/// action `GroupResources` performs.
pub const GROUPING_ACTION_GROUP: &str = "GROUP";

/// The modelled `GroupingType` value for removing a resource from a group.
pub const GROUPING_ACTION_UNGROUP: &str = "UNGROUP";

/// Every value the modelled `GroupingStatus` enum declares, which is also the
/// domain of the `status` filter `ListGroupingStatuses` accepts.
///
/// Source: `smithy_query(query="shape",shape="GroupingStatus")`.
pub const GROUPING_STATUS_VALUES: [&str; 4] = ["SUCCESS", "FAILED", "IN_PROGRESS", "SKIPPED"];

/// The modelled `GroupingStatus` of an action that completed: the only outcome
/// this emulator records, because a grouping request it accepts is applied
/// synchronously and in full, and one it cannot apply is refused before any
/// state changes.
pub const GROUPING_STATUS_SUCCESS: &str = GROUPING_STATUS_VALUES[0];

/// The last grouping or ungrouping action recorded against one resource of one
/// group.
#[derive(Debug, Clone)]
pub struct GroupingStatusRecord {
    /// A modelled `GroupingType` value: [`GROUPING_ACTION_GROUP`] or
    /// [`GROUPING_ACTION_UNGROUP`].
    pub action: String,
    /// A modelled `GroupingStatus` value, one of [`GROUPING_STATUS_VALUES`].
    pub status: String,
    /// When the action this record reports was performed.
    pub updated_at: SystemTime,
}

impl GroupingStatusRecord {
    /// The modelled `GroupingStatusesItem` for this record, as
    /// `ListGroupingStatuses` reports it for `resource_arn`.
    ///
    /// `ErrorCode` and `ErrorMessage` report why an action FAILED; a recorded
    /// action that did not fail carries neither.
    pub fn to_item(&self, resource_arn: &str) -> GroupingStatusesItem {
        GroupingStatusesItem {
            resource_arn: Some(resource_arn.to_string()),
            action: Some(self.action.clone()),
            status: Some(self.status.clone()),
            error_code: None,
            error_message: None,
            updated_at: Some(to_date_time(self.updated_at)),
        }
    }
}

/// How many characters the id that qualifies an application group's name, and
/// the id of a tag-sync task, is written with.
///
/// Both modelled ARN productions spell it `[a-z0-9]{26}`, which is the length
/// and alphabet of a ULID.
///
/// Source: `smithy_query(query="shape",shape="TagSyncTaskArn")`.
pub const QUALIFYING_ID_LENGTH: usize = 26;

/// Whether `id` is shaped like the [`QUALIFYING_ID_LENGTH`]-character id the
/// modelled ARNs use.
pub fn is_qualifying_id(id: &str) -> bool {
    id.len() == QUALIFYING_ID_LENGTH
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// A fresh [`QUALIFYING_ID_LENGTH`]-character id: a ULID, lowercased.
///
/// A ULID is a millisecond timestamp followed by entropy, written in 26
/// Crockford base32 characters, so lowercasing it satisfies the modelled
/// `[a-z0-9]{26}`; the timestamp leads, so ids sort in minting order and a
/// listing keyed by one pages in creation order.
pub fn new_qualifying_id() -> String {
    ::ulid::Ulid::new().to_string().to_lowercase()
}

/// The longest group name an ARN that QUALIFIES that name by an id can carry.
///
/// Every modelled ARN production that nests something under a group writes the
/// group-name segment as `[a-zA-Z0-9_\.-]{1,150}` — the `TagSyncTaskArn`
/// (`…:group/<name>/<26 chars>/tag-sync-task/<26 chars>`), the `ApplicationArn`
/// and the qualified alternative of `GroupArn` itself — narrower than the 300
/// characters a group name may have, so a longer name cannot be qualified at
/// all.
///
/// Source: `smithy_query(query="shape",shape="TagSyncTaskArn")` and
/// `smithy_query(query="shape",shape="ApplicationArn")`.
pub const MAX_QUALIFIED_GROUP_NAME_LENGTH: usize = 150;

/// Whether an ARN that qualifies a group name by an id can carry `name`.
///
/// Judged on the leading segment, which is the one such an ARN writes: a name
/// already qualified by an id carries that id in its own trailing segment.
pub fn is_qualifiable_group_name(name: &str) -> bool {
    let segment = name.split('/').next().unwrap_or_default();
    (1..=MAX_QUALIFIED_GROUP_NAME_LENGTH).contains(&segment.len())
        && segment
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// The modelled `TagSyncTaskStatus` of a task that is managing its application's
/// resources: the model documents `ACTIVE` as the task "actively managing
/// resources" and `ERROR` as one that is not, with the reason in
/// `ErrorMessage`. A task this emulator accepts is managing its group from the
/// moment it is stored, and nothing here fails a task later, so a stored task
/// is `ACTIVE` and carries no error message.
pub const TAG_SYNC_TASK_ACTIVE: &str = "ACTIVE";

/// The message this service answers when a task ARN addresses no stored
/// tag-sync task.
///
/// The service's own FIXED wording, which names nothing the caller sent — the
/// ARN is already in the request — so every operation that addresses one task
/// answers the same sentence.
pub const TAG_SYNC_TASK_NOT_FOUND_MESSAGE: &str = "The specified tag-sync task does not exist";

/// The ARN of the tag-sync task `task_id` on the application group
/// `application_group_arn`, in the one form every operation of this service
/// reports: the modelled `TagSyncTaskArn` nests a task under the group it syncs,
/// so the task's ARN is the group's own application-group ARN (see
/// [`GroupRecord::application_group_arn`]) followed by `/tag-sync-task/<id>`.
/// Reported from the group's STORED ARN, so a task carries the region and the
/// account of the group it belongs to rather than of the request that started
/// it.
pub fn tag_sync_task_arn(application_group_arn: &str, task_id: &str) -> String {
    format!("{application_group_arn}/tag-sync-task/{task_id}")
}

/// One persisted tag-sync task: what it syncs, the group it syncs into, the role
/// it assumes, and its status.
#[derive(Debug, Clone)]
pub struct TagSyncTaskRecord {
    /// The task's own ARN, which is also its key in
    /// [`ComAmazonawsResourcegroupsArdiState::tag_sync_tasks`]. Minted once when
    /// the task is started and never recomputed, so every later read returns the
    /// same ARN.
    pub task_arn: String,
    /// The region the task lives in: the region of the group it syncs into, which
    /// is also the region its own ARN names. This service is REGIONAL, so a task
    /// is visible only to that region's callers.
    pub region: String,
    /// The STORED ARN of the group the task syncs resources into, so a reader
    /// reports the same ARN `GetGroup` does for that group.
    pub group_arn: String,
    /// The stored name of that group.
    pub group_name: String,
    /// The tag key the task syncs on, for a task started with a tag key-value
    /// pair; `None` for one started with a resource query.
    pub tag_key: Option<String>,
    /// The tag value the task syncs on, present exactly when
    /// [`TagSyncTaskRecord::tag_key`] is.
    pub tag_value: Option<String>,
    /// The query the task syncs on, for a task started with one; `None` for a
    /// task started with a tag key-value pair.
    pub resource_query: Option<ResourceQueryRecord>,
    /// The role the service assumes to tag and untag resources, stored as the
    /// caller gave it.
    pub role_arn: String,
    /// A modelled `TagSyncTaskStatus` value: [`TAG_SYNC_TASK_ACTIVE`] for every
    /// task this emulator stores.
    pub status: String,
    /// Why the task is not managing resources, reported only for a task whose
    /// status says it failed.
    pub error_message: Option<String>,
    /// When the task was started.
    pub created_at: SystemTime,
}

impl TagSyncTaskRecord {
    /// Whether this task syncs resources into the group `key` names.
    ///
    /// A group is identified by its region and its name (see [`GroupKey`]) — the
    /// two this record stores — so the answer does not depend on which of the
    /// identifier forms the group can be addressed by was used to reach it.
    pub fn syncs_into(&self, key: &GroupKey) -> bool {
        self.region == key.region && self.group_name == key.name
    }

    /// The modelled `ResourceQuery` for this record, if it was started with one.
    pub fn to_resource_query(&self) -> Option<ResourceQuery> {
        self.resource_query.as_ref().map(|q| ResourceQuery {
            r#type: q.query_type.clone(),
            query: q.query.clone(),
        })
    }

    /// The modelled `TagSyncTaskItem` for this record, as `ListTagSyncTasks`
    /// reports it.
    pub fn to_item(&self) -> TagSyncTaskItem {
        TagSyncTaskItem {
            group_arn: Some(self.group_arn.clone()),
            group_name: Some(self.group_name.clone()),
            task_arn: Some(self.task_arn.clone()),
            tag_key: self.tag_key.clone(),
            tag_value: self.tag_value.clone(),
            resource_query: self.to_resource_query(),
            role_arn: Some(self.role_arn.clone()),
            status: Some(self.status.clone()),
            error_message: self.error_message.clone(),
            created_at: Some(to_date_time(self.created_at)),
        }
    }
}

/// `at` as the modelled `timestamp` a response carries. A time before the epoch
/// cannot be reported by this service — nothing it stores predates the process —
/// so one reads as the epoch rather than panicking.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    // A whole-second count past `i64::MAX` is 292 billion years away; saturating
    // keeps the conversion total without a panic path.
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_nanos(),
    )
}

/// The resource query a group was created with.
#[derive(Debug, Clone)]
pub struct ResourceQueryRecord {
    /// A modelled `QueryType` value: `TAG_FILTERS_1_0` or `CLOUDFORMATION_STACK_1_0`.
    pub query_type: String,
    /// The query document, stored verbatim as the caller sent it.
    pub query: String,
}

/// The element of a query document that carries the resource types the query is
/// limited to.
///
/// The model documents it as applying "to all `ResourceQuery` objects of either
/// `Type`", holding either `AWS::AllSupported` or a JSON array of resource type
/// identifiers (`smithy_query(query="doc",shape="ResourceQuery",member="Query")`),
/// so it is read the same way whatever the query's type.
pub const RESOURCE_TYPE_FILTERS_ELEMENT: &str = "ResourceTypeFilters";

/// The [`RESOURCE_TYPE_FILTERS_ELEMENT`] value that names no limit at all: the
/// model documents it as making the query "match resources of any resource type
/// that also match the query"
/// (`smithy_query(query="doc",shape="ResourceQuery",member="Query")`), so a query
/// carrying it admits every type the query type supports.
pub const ALL_SUPPORTED_RESOURCE_TYPES: &str = "AWS::AllSupported";

impl ResourceQueryRecord {
    /// The resource types this query is limited to: the string entries of its
    /// [`RESOURCE_TYPE_FILTERS_ELEMENT`], in the order the document holds them.
    ///
    /// Read out of the stored document rather than from a second copy of it, so
    /// a reader sees exactly the types the query was created with; the document
    /// is a modelled `Query` (`length 0..4096`), so one read is bounded. A query
    /// whose document holds no such element, holds a non-string entry, or is not
    /// a JSON object at all is limited to nothing, and a document that spells the
    /// element as a single string carries that one type.
    pub fn resource_type_filters(&self) -> Vec<String> {
        let Ok(document) = serde_json::from_str::<serde_json::Value>(&self.query) else {
            return Vec::new();
        };
        match document.get(RESOURCE_TYPE_FILTERS_ELEMENT) {
            Some(serde_json::Value::String(one)) => vec![one.clone()],
            Some(serde_json::Value::Array(entries)) => entries
                .iter()
                .filter_map(|entry| entry.as_str().map(str::to_string))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The resource types this query LIMITS its matches to, or `None` when it
    /// imposes no such limit.
    ///
    /// The ONE place this service decides what a query's
    /// [`RESOURCE_TYPE_FILTERS_ELEMENT`] limits, so a group's membership listing
    /// and a search over the recorded resources judge a type the same way: a
    /// query whose document names no resource type at all, and one that names
    /// [`ALL_SUPPORTED_RESOURCE_TYPES`], both match any supported type, so
    /// neither carries a limit to judge a type against.
    pub fn resource_type_limit(&self) -> Option<Vec<String>> {
        let declared = self.resource_type_filters();
        if declared.is_empty()
            || declared
                .iter()
                .any(|declared| declared == ALL_SUPPORTED_RESOURCE_TYPES)
        {
            return None;
        }
        Some(declared)
    }
}

/// One resource a membership listing or a search reports: an ARN recorded as a
/// static membership by `GroupResources`, or one a resource query selects
/// ([`run_resource_query`]).
#[derive(Debug, Clone)]
pub struct MemberRecord {
    pub resource_arn: String,
    /// The CloudFormation-style type name derived from the ARN, when
    /// [`resource_type_of_arn`] knows it; `None` otherwise.
    pub resource_type: Option<String>,
    /// A modelled `ResourceStatusValue`, set only while a membership is still
    /// being applied asynchronously.
    pub status: Option<String>,
}

impl MemberRecord {
    /// The modelled `ResourceIdentifier` for this membership.
    pub fn to_identifier(&self) -> ResourceIdentifier {
        ResourceIdentifier {
            resource_arn: Some(self.resource_arn.clone()),
            resource_type: self.resource_type.clone(),
        }
    }

    /// The modelled `ListGroupResourcesItem` for this membership.
    pub fn to_item(&self) -> ListGroupResourcesItem {
        ListGroupResourcesItem {
            identifier: Some(self.to_identifier()),
            status: self
                .status
                .as_ref()
                .map(|name| ResourceStatus {
                    name: Some(name.clone()),
                }),
        }
    }
}

/// The most regions this emulator holds account settings for at once.
///
/// NOT an AWS quota: a region is not a resource a caller creates. This is the
/// emulator's bound on in-memory fixture state, so a client looping on
/// `UpdateAccountSettings` with a different region in its credential scope every
/// time is refused rather than growing the store without end. Reaching it is a
/// `BadRequestException`, and a region the store already holds stays writable.
pub const MAX_ACCOUNT_SETTINGS_REGIONS: usize = 100;

/// The modelled `GroupLifecycleEventsStatus` of a feature that is on, and the
/// `GroupLifecycleEventsDesiredStatus` a caller asks for to turn it on.
///
/// This and the two below are the three values this service WRITES, so each is a
/// name for one member of the generated value lists
/// ([`GROUP_LIFECYCLE_EVENTS_DESIRED_STATUS_VALUES`] and
/// `GROUP_LIFECYCLE_EVENTS_STATUS_VALUES`) rather than a second statement of what
/// those lists admit — which is why validation reads the list and not these.
pub const LIFECYCLE_EVENTS_ACTIVE: &str = "ACTIVE";

/// The modelled status and desired status of a feature that is off — and the
/// status of an account nothing has been written for, since the feature is one
/// `UpdateAccountSettings` "turns on or turns off", so it is off until a caller
/// asks for it.
pub const LIFECYCLE_EVENTS_INACTIVE: &str = "INACTIVE";

/// The modelled status of a feature whose switch has been accepted and is not in
/// effect yet. The model documents the switch as asynchronous: a caller "can call
/// the GetAccountSettings operation to check for completion by looking for
/// GroupLifecycleEventsStatus to change to ACTIVE".
pub const LIFECYCLE_EVENTS_IN_PROGRESS: &str = "IN_PROGRESS";

/// How long after an accepted switch the requested status is owed.
///
/// The model documents that the switch is asynchronous but no duration for it, so
/// this span is the emulator's own choice: long enough that a caller reading
/// immediately after the write still observes `IN_PROGRESS`, short enough that a
/// caller polling for completion does not wait. It is stored as a due INSTANT on
/// the record and nothing ever sleeps on it.
pub const LIFECYCLE_EVENTS_SETTLE_DELAY: Duration = Duration::from_millis(500);

/// The request member `UpdateAccountSettings` carries the desired status in, as a
/// refusal names it.
pub const DESIRED_STATUS_MEMBER: &str = "GroupLifecycleEventsDesiredStatus";

/// `Ok(())` when `desired` is a status a caller may ask for, else the message the
/// refusal reports — which names the member and the values the model admits, so a
/// caller can fix the request from the message alone.
///
/// The admitted set is the model's own
/// ([`GROUP_LIFECYCLE_EVENTS_DESIRED_STATUS_VALUES`], generated from
/// `GroupLifecycleEventsDesiredStatus`), never a second hand-written copy of it.
pub fn check_desired_status(desired: &str) -> Result<(), String> {
    if GROUP_LIFECYCLE_EVENTS_DESIRED_STATUS_VALUES.contains(&desired) {
        return Ok(());
    }
    Err(format!(
        "{DESIRED_STATUS_MEMBER} is not valid: it is one of {}",
        GROUP_LIFECYCLE_EVENTS_DESIRED_STATUS_VALUES.join(", "),
    ))
}

/// A status change that the clock already owes at [`PendingLifecycleStatus::due_at`],
/// whether or not anybody calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingLifecycleStatus {
    /// The status that is in effect from `due_at` on.
    pub status: String,
    /// The instant the switch finishes. Public so a test can hold or advance the
    /// emulator's clock without sleeping for the span.
    pub due_at: SystemTime,
}

/// The Resource Groups optional-feature settings of one region of this account:
/// what a caller last asked for, what is in effect, and the switch the clock still
/// owes between the two.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountSettingsRecord {
    /// The modelled `GroupLifecycleEventsDesiredStatus`: the status a caller last
    /// asked for, in effect from the moment it was asked for.
    pub desired_status: String,
    /// The modelled `GroupLifecycleEventsStatus`: the status actually in effect —
    /// [`LIFECYCLE_EVENTS_IN_PROGRESS`] while a requested switch has not finished.
    pub status: String,
    /// The modelled `GroupLifecycleEventsStatusMessage`, "the text of any error
    /// message occurs during an attempt to turn group lifecycle events on or
    /// off". No switch this emulator accepts fails, so nothing writes it and it is
    /// reported absent.
    pub status_message: Option<String>,
    /// The switch the clock owes, or `None` when the status in effect is already
    /// the one asked for.
    pub pending: Option<PendingLifecycleStatus>,
}

impl Default for AccountSettingsRecord {
    /// The settings of a region nothing has been written for: an optional feature
    /// a caller has never asked for is off, and both the desired and the effective
    /// status report that.
    fn default() -> Self {
        Self {
            desired_status: LIFECYCLE_EVENTS_INACTIVE.to_string(),
            status: LIFECYCLE_EVENTS_INACTIVE.to_string(),
            status_message: None,
            pending: None,
        }
    }
}

impl AccountSettingsRecord {
    /// Record that `desired` was asked for at `now`.
    ///
    /// The desired status becomes `desired` at once; the status in effect becomes
    /// [`LIFECYCLE_EVENTS_IN_PROGRESS`] and `desired` falls due
    /// [`LIFECYCLE_EVENTS_SETTLE_DELAY`] later, so a caller reading straight after
    /// the write still sees the switch in flight. A second request supersedes a
    /// switch still in flight, which is what makes the LAST request the one that
    /// decides the outcome.
    pub fn request(&mut self, desired: &str, now: SystemTime) {
        self.desired_status = desired.to_string();
        self.status = LIFECYCLE_EVENTS_IN_PROGRESS.to_string();
        // Nothing has failed, so there is no error text to report.
        self.status_message = None;
        self.pending = Some(PendingLifecycleStatus {
            status: desired.to_string(),
            due_at: now + LIFECYCLE_EVENTS_SETTLE_DELAY,
        });
    }

    /// Materialise the switch the clock already made due at `now`.
    ///
    /// It was owed at this instant whether or not anybody called, so any caller —
    /// a reporting one included — may settle it; a switch not yet due is left
    /// alone, and a record with nothing in flight is unchanged.
    pub fn settle_due(&mut self, now: SystemTime) {
        let Some(pending) = self.pending.as_ref() else {
            return;
        };
        if pending.due_at > now {
            return;
        }
        self.status = pending.status.clone();
        self.pending = None;
    }

    /// The modelled `AccountSettings` structure for this record.
    pub fn to_account_settings(&self) -> AccountSettings {
        AccountSettings {
            group_lifecycle_events_desired_status: Some(self.desired_status.clone()),
            group_lifecycle_events_status: Some(self.status.clone()),
            group_lifecycle_events_status_message: self.status_message.clone(),
        }
    }
}

/// The region every ARN this emulator mints for an unsigned request carries.
/// `emulator_server::emulator_arn`'s own documentation names `us-east-1` as the
/// emulator default.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region to mint an ARN in for this request: the one in the caller's SigV4
/// credential scope, or [`DEFAULT_REGION`] when the request carries no scope or
/// a scope region that is not shaped like an AWS region (the modelled `GroupArn`
/// pattern requires `[a-z]{2}(-[a-z]+)+-\d{1}`).
pub fn request_region(parts: &http::request::Parts) -> String {
    emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_arn_region(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The `target` every audit record this service emits carries, so a consumer can
/// select the access trail without reading the rest of the emulator's traces.
///
/// A state-changing operation emits ONE record, at `INFO`, naming the operation,
/// the caller's presented access key id, the region the request resolved to, the
/// resource it changed and the outcome. Metadata only — never a request body, a
/// tag value or a credential secret.
pub const AUDIT_TARGET: &str = "emulator::audit";

/// The identity a request presents, for an audit record: the access key id in its
/// SigV4 credential scope. `emulator_server::extract_akid` answers `"default"`
/// when the request carries no scope. An access key id NAMES a caller and is not
/// a secret — the signature and the secret key are never read here.
pub fn request_caller(parts: &http::request::Parts) -> String {
    emulator_server::extract_akid(&parts.headers)
}

/// Whether `partition` matches the partition production the modelled ARN patterns
/// use: `aws(-[a-z]+)*`.
fn is_arn_partition(partition: &str) -> bool {
    let mut segments = partition.split('-');
    segments.next() == Some("aws")
        && segments.all(|segment| {
            !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_lowercase())
        })
}

/// Whether `region` matches the region production the modelled ARN patterns use:
/// `[a-z]{2}(-[a-z]+)+-\d{1}`.
fn is_arn_region(region: &str) -> bool {
    let Some((head, digit)) = region.rsplit_once('-') else {
        return false;
    };
    if digit.len() != 1 || !digit.as_bytes()[0].is_ascii_digit() {
        return false;
    }
    let mut segments = head.split('-');
    let lowercase = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase());
    match segments.next() {
        Some(first) if first.len() == 2 && lowercase(first) => {}
        _ => return false,
    }
    let mut middle = 0usize;
    for segment in segments {
        if !lowercase(segment) {
            return false;
        }
        middle += 1;
    }
    middle >= 1
}

/// The message this service answers when a group identifier resolves to no
/// stored group.
///
/// It is the service's own FIXED wording, and it names nothing the caller sent:
/// the identifier is already in the request, so echoing it back tells the caller
/// nothing the request does not already say. Every operation that addresses one
/// group answers the same sentence, so a client cannot tell them apart by their
/// 404 text.
pub const GROUP_NOT_FOUND_MESSAGE: &str = "The specified group does not exist";

/// The message this service answers when an operation whose modelled target is
/// THE APPLICATION GROUP is applied to a stored group that is not one.
///
/// One FIXED sentence for that refusal class, carrying nothing the caller sent:
/// the request already names the group, and which kinds of group this emulator
/// holds is its own state rather than something a refusal reports.
pub const APPLICATION_GROUP_TARGET_MESSAGE: &str = "The target must be an application group";

/// Whether `group` is a target an operation documented to act on the APPLICATION
/// group may be applied to, else the one sentence that refusal carries
/// ([`APPLICATION_GROUP_TARGET_MESSAGE`]).
///
/// The ONE place this service judges that eligibility, so every operation whose
/// target the model documents as the application group — `ListGroupingStatuses`
/// reports "the status of the last grouping or ungrouping action for each resource
/// in the specified application group"
/// (`smithy_query(query="doc",shape="ListGroupingStatuses")`) — refuses the same
/// targets with the same sentence. Judged on the group's EFFECTIVE configuration
/// through [`GroupRecord::is_application_group`], so a configuration change that
/// the clock has already made due is what decides it, and reading it writes
/// nothing.
pub fn check_application_group_target(group: &GroupRecord) -> Result<(), String> {
    if group.is_application_group() {
        return Ok(());
    }
    Err(APPLICATION_GROUP_TARGET_MESSAGE.to_string())
}

/// The ARN of the group named `name` in `region`, in the form every operation of
/// this service reports for a group whose name is not qualified by an id:
/// `arn:aws:resource-groups:<region>:<account>:group/<name>`.
/// The account segment is the emulator's canonical account id.
pub fn group_arn(region: &str, name: &str) -> String {
    emulator_server::emulator_arn("resource-groups", region, &format!("group/{name}"))
}

/// The ARN of the group named `name` in `region` with its name QUALIFIED by
/// `application_id`: `arn:aws:resource-groups:<region>:<account>:group/<name>/<id>`.
///
/// This is the identity an APPLICATION group is reported with: the modelled
/// `GroupArn` admits the qualified alternative
/// `group/[a-zA-Z0-9_\.-]{1,150}/[a-z0-9]{26}`, and the modelled `ApplicationArn`
/// — the value of an application group's membership tag — admits ONLY that form
/// (`smithy_query(query="shape",shape="ApplicationArn")`).
pub fn qualified_group_arn(region: &str, name: &str, application_id: &str) -> String {
    group_arn(region, &format!("{name}/{application_id}"))
}

/// The id of the account that owns every group this emulator holds: the
/// emulator's canonical account id, which is also the account segment of every
/// ARN [`group_arn`] mints.
pub fn owning_account_id() -> String {
    emulator_server::emulator_account_id().to_string()
}

/// The largest page any listing of this service answers, and the page size it
/// answers when the caller asks for none.
///
/// Every listing's `MaxResults` targets the one modelled `MaxResults` shape
/// (`range 1..50`), and the model documents the absent case as "defaults to a
/// value that is specific to the operation", so answering at the modelled maximum
/// bounds the response and keeps the `NextToken` loop working the same way whether
/// or not `MaxResults` was sent.
pub const MAX_PAGE_SIZE: usize = 50;

/// How many items one page of a listing carries, from the caller's `MaxResults`,
/// else the message the refusal reports.
///
/// The ONE place this service decides a page size, so every listing — of groups,
/// of one group's memberships, of its grouping statuses and of the account's
/// tag-sync tasks — defaults an absent `MaxResults` to [`MAX_PAGE_SIZE`] and
/// refuses a value outside `1..=MAX_PAGE_SIZE` with the same sentence, and the
/// bound and that sentence cannot drift apart between them.
///
/// The framework refuses a value outside the modelled range before a handler runs;
/// checking it again here keeps the conversion lossless rather than letting a
/// negative value wrap into "every item".
pub fn page_size(max_results: Option<i32>) -> Result<usize, String> {
    match max_results {
        None => Ok(MAX_PAGE_SIZE),
        Some(max) => usize::try_from(max)
            .ok()
            .filter(|size| (1..=MAX_PAGE_SIZE).contains(size))
            .ok_or_else(|| {
                format!("MaxResults {max} is not valid: it must be between 1 and {MAX_PAGE_SIZE}")
            }),
    }
}

/// The most `Filters` entries any listing of this service applies to one call.
///
/// NOT an AWS quota: not one of the filter lists these listings model
/// (`GroupFilterList`, `ResourceFilterList`, `ListGroupingStatusesFilterList`,
/// `ListTagSyncTasksFilterList`) declares a length, so the request boundary admits
/// any number of entries, and each entry's own `Values` is bounded on its own
/// (`1..5` or `1..10`). This is the emulator's own bound on the work ONE request
/// can ask for, since every entry is evaluated against every item the listing
/// covers: it caps that product beside the bound the store already puts on the
/// items themselves. Reaching it is a `BadRequestException`.
pub const MAX_FILTERS: usize = 10;

/// `Ok(())` when a request's `Filters` carries a number of entries this service
/// applies to one call, else the message the refusal reports.
///
/// The ONE place this service bounds a filter list, so every listing it serves —
/// of groups, of one group's memberships, of its grouping statuses and of the
/// account's tag-sync tasks — caps the entries one call carries at
/// [`MAX_FILTERS`] and refuses a longer list with the same sentence, and the bound
/// and that sentence cannot drift apart between them.
///
/// The count is judged from the request alone, so a caller is refused BEFORE the
/// store lock is taken: a request past the bound costs no scan and blocks no other
/// operation of the service.
pub fn check_filters(count: usize) -> Result<(), String> {
    if count > MAX_FILTERS {
        return Err(format!(
            "Filters carries {count} filters, which is more than the {MAX_FILTERS} this \
             emulator applies in one call"
        ));
    }
    Ok(())
}

/// The marker every pagination token this service mints carries, before the
/// region and listing name it was issued for and its resume key. It is what makes
/// a token verifiable: a string that merely happens to be base64 is not a resume
/// point, and a token issued by one listing is not accepted by another.
const PAGE_TOKEN_PREFIX: &str = "resourcegroups/v1/";

/// Encode a listing's resume point — the key of the last item returned — as the
/// opaque `NextToken` the model describes (`^[a-zA-Z0-9+/]*={0,2}$`, i.e.
/// base64).
///
/// A token names the listing it resumes, and a listing of this service is
/// identified by the REGION the request addressed as well as by `listing`: every
/// listing here enumerates one region's resources ([`GroupKey`]), so the same
/// listing name in another region is another listing, whose items this token's
/// resume key does not place. `region` is therefore carried beside `listing` and
/// checked by [`decode_page_token`], which is what keeps a resume point from
/// crossing a region boundary.
pub fn encode_page_token(region: &str, listing: &str, last_key: &str) -> String {
    aws_smithy_types::base64::encode(format!(
        "{PAGE_TOKEN_PREFIX}{region}/{listing}/{last_key}"
    ))
}

/// The resume point a `NextToken` carries, or `None` when the token was not
/// minted by `listing` in `region`.
pub fn decode_page_token(region: &str, listing: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(aws_smithy_types::base64::decode(token).ok()?).ok()?;
    decoded
        .strip_prefix(PAGE_TOKEN_PREFIX)?
        .strip_prefix(region)?
        .strip_prefix('/')?
        .strip_prefix(listing)?
        .strip_prefix('/')
        .map(str::to_string)
}

/// The CloudFormation-style resource type an ARN names, e.g.
/// `arn:aws:ec2:us-east-1:000000000000:instance/i-0abc` -> `AWS::EC2::Instance`.
///
/// Derived from the ARN's own service segment and resource-type segment through
/// the table below; a resource whose type this table does not name resolves to
/// `None`, and the membership is then reported with no `ResourceType` rather
/// than with a guessed one (the member is optional in `ResourceIdentifier`).
///
/// The table's keys are the ARN formats in the AWS "Actions, resources, and
/// condition keys" service reference; its values are the type names from the AWS
/// CloudFormation resource and property types reference
/// (<https://docs.aws.amazon.com/AWSCloudFormation/latest/UserGuide/aws-template-resource-type-ref.html>),
/// which is the vocabulary `ResourceIdentifier$ResourceType` is documented in
/// ("such as AWS::EC2::Instance") and whose shape its `^AWS::[a-zA-Z0-9]+::\w+$`
/// pattern requires. An empty resource-type key is an ARN whose resource part is
/// a bare id (`arn:aws:sqs:<region>:<account>:<queue-name>`).
const RESOURCE_TYPES: &[(&str, &str, &str)] = &[
    ("cloudformation", "stack", "AWS::CloudFormation::Stack"),
    ("cloudwatch", "alarm", "AWS::CloudWatch::Alarm"),
    ("dynamodb", "table", "AWS::DynamoDB::Table"),
    ("ec2", "capacity-reservation", "AWS::EC2::CapacityReservation"),
    ("ec2", "dedicated-host", "AWS::EC2::Host"),
    ("ec2", "instance", "AWS::EC2::Instance"),
    ("ec2", "security-group", "AWS::EC2::SecurityGroup"),
    ("ec2", "snapshot", "AWS::EC2::Snapshot"),
    ("ec2", "subnet", "AWS::EC2::Subnet"),
    ("ec2", "volume", "AWS::EC2::Volume"),
    ("ec2", "vpc", "AWS::EC2::VPC"),
    ("ecs", "cluster", "AWS::ECS::Cluster"),
    ("ecs", "service", "AWS::ECS::Service"),
    ("iam", "role", "AWS::IAM::Role"),
    ("iam", "user", "AWS::IAM::User"),
    ("kms", "key", "AWS::KMS::Key"),
    ("lambda", "function", "AWS::Lambda::Function"),
    ("logs", "log-group", "AWS::Logs::LogGroup"),
    ("rds", "cluster", "AWS::RDS::DBCluster"),
    ("rds", "db", "AWS::RDS::DBInstance"),
    ("resource-groups", "group", "AWS::ResourceGroups::Group"),
    ("s3", "", "AWS::S3::Bucket"),
    ("sns", "", "AWS::SNS::Topic"),
    ("sqs", "", "AWS::SQS::Queue"),
    ("ssm", "document", "AWS::SSM::Document"),
    ("states", "stateMachine", "AWS::StepFunctions::StateMachine"),
];

/// The CloudFormation-style type name for `arn`, per [`RESOURCE_TYPES`].
pub fn resource_type_of_arn(arn: &str) -> Option<&'static str> {
    let (service, resource) = parse_arn(arn)?;
    // `type/id`, `type:id` and a bare `id` are the three resource forms an ARN
    // uses; the bare form has no type segment at all.
    let resource_type = resource
        .find([':', '/'])
        .map_or("", |split| &resource[..split]);
    RESOURCE_TYPES
        .iter()
        .find(|(s, t, _)| *s == service && *t == resource_type)
        .map(|(_, _, cfn)| *cfn)
}

/// Whether `arn` is shaped like the modelled `ResourceArn`
/// (`arn:<partition>:<service>:<region>:<account>:<resource>`).
pub fn is_resource_arn(arn: &str) -> bool {
    parse_arn(arn).is_some()
}

/// The most `ResourceArns` entries one request carries: the maximum the modelled
/// `ResourceArnList` declares (`length 1..10`). It is what bounds the per-request
/// work a static-membership handler does while holding the store lock.
pub const MAX_RESOURCE_ARNS: usize = 10;

/// `Ok(())` when `resource_arns` is a `ResourceArns` list this service acts on,
/// else the message the refusal reports.
///
/// The ONE place this service decides whether a static-membership request names
/// resources it can act on, so the call that records memberships and the one that
/// drops them bound the same list to [`MAX_RESOURCE_ARNS`] entries and refuse the
/// same entry with the same sentence, and the bound and those sentences cannot
/// drift apart between them:
///
/// * the list carries at most [`MAX_RESOURCE_ARNS`] entries, which is what BOUNDS
///   the work a handler does per request: every scan it makes over the request is
///   at most this many entries;
/// * every entry is shaped like the modelled `ResourceArn` ([`is_resource_arn`]) —
///   a string that is not one names no resource to act on.
///
/// The framework refuses a list outside the modelled length before a handler runs;
/// checking the maximum again here is what makes the bound the handler's own.
pub fn check_resource_arns(resource_arns: &[String]) -> Result<(), String> {
    if resource_arns.len() > MAX_RESOURCE_ARNS {
        return Err(format!(
            "ResourceArns carries {} entries, which is more than the {} this \
             operation accepts",
            resource_arns.len(),
            MAX_RESOURCE_ARNS,
        ));
    }
    if let Some(not_an_arn) = resource_arns.iter().find(|arn| !is_resource_arn(arn)) {
        return Err(format!(
            "ResourceArns contains {not_an_arn}, which is not a resource ARN"
        ));
    }
    Ok(())
}

/// The report a static-membership call makes for one resource ARN the addressed
/// group does not support: the ARN from the request, under the fixed code and
/// sentence of the class ([`UNSUPPORTED_RESOURCE_ARN_ERROR_CODE`],
/// [`UNSUPPORTED_RESOURCE_ARN_MESSAGE`]).
pub fn unsupported_resource_arn(resource_arn: &str) -> FailedResource {
    FailedResource {
        resource_arn: Some(resource_arn.to_string()),
        error_message: Some(UNSUPPORTED_RESOURCE_ARN_MESSAGE.to_string()),
        error_code: Some(UNSUPPORTED_RESOURCE_ARN_ERROR_CODE.to_string()),
    }
}

/// Split one static-membership request's `ResourceArns` into the entries `group`
/// acts on and the `Failed` reports for the entries it does not support.
///
/// The ONE place this service decides which resources a group admits, so the call
/// that records memberships and the one that drops them judge the same request the
/// same way and report a refused entry with the same code and sentence:
///
/// * an ARN whose resource type the group's service configuration admits
///   ([`GroupRecord::admits_member_arn`]) is acted on;
/// * an ARN the group already HOLDS is acted on whatever its configuration now
///   says, because a membership already recorded is one a removal must still be
///   able to drop — replacing a group's configuration carries its memberships
///   nowhere;
/// * every other ARN is reported in `Failed` and acted on in no way, once however
///   many times one request repeats it, in the order the request names them.
///
/// The work is bounded by [`MAX_RESOURCE_ARNS`] request entries, each judged by
/// BINARY SEARCH over the group's memberships ([`GroupRecord::holds_member`]).
pub fn supported_membership_arns<'arns>(
    group: &GroupRecord,
    resource_arns: &'arns [String],
) -> (Vec<&'arns str>, Vec<FailedResource>) {
    let mut supported: Vec<&str> = Vec::new();
    let mut failed: Vec<FailedResource> = Vec::new();
    let mut reported: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for resource_arn in resource_arns.iter().map(String::as_str) {
        if group.holds_member(resource_arn) || group.admits_member_arn(resource_arn) {
            supported.push(resource_arn);
        } else if reported.insert(resource_arn) {
            failed.push(unsupported_resource_arn(resource_arn));
        }
    }
    (supported, failed)
}

/// The service and resource portions of `arn`, or `None` when the string is not
/// an ARN.
///
/// The shape is the modelled `ResourceArn`'s own:
/// `^arn:aws(-[a-z]+)*:[a-z0-9\-]*:([a-z]{2}(-[a-z]+)+-\d{1})?:([0-9]{12})?:.+$`
/// — six fields; a partition matching `aws(-[a-z]+)*`; a lowercase service; a
/// region that is either empty (a global service, as in `arn:aws:s3:::bucket`) or
/// shaped like an AWS region; an account that is either empty or twelve digits;
/// and a non-empty resource.
fn parse_arn(arn: &str) -> Option<(&str, &str)> {
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    let partition = fields.next()?;
    let service = fields.next()?;
    let region = fields.next()?;
    let account = fields.next()?;
    let resource = fields.next()?;
    if !is_arn_partition(partition) {
        return None;
    }
    if service.is_empty()
        || !service
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return None;
    }
    if !region.is_empty() && !is_arn_region(region) {
        return None;
    }
    if !account.is_empty() && (account.len() != 12 || !account.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    if resource.is_empty() {
        return None;
    }
    Some((service, resource))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsResourcegroupsArdiState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsResourcegroupsArdiState>)
}
