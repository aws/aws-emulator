// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-resourcegroupstaggingapi-resourcegroupstaggingapi-20170126 service, and the pure
//! projections over it that the tag read/write loop is built from.
//!
//! The tagging API is a VIEW, not a second copy: for every service [`owners`]
//! links against, a resource's tags live in that service's own state — which is
//! why `TagResources` can only tag an ARN the owning service really holds, and
//! why a resource tagged through the owning service's own API (or deleted
//! through it) appears in, and disappears from, this API's answers. For a
//! service this crate does not link against, the ARN -> tag map below IS the tag
//! home.
//!
//! Everything the readers need (ARN parsing, filter matching, paging) lives here
//! so the five handlers share ONE implementation of each rule rather than five
//! copies.

pub mod owners;
pub mod supported_services;

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{FailureInfo, ResourceTagMapping, Tag, TagFilter};

/// Per-service state for com-amazonaws-resourcegroupstaggingapi-resourcegroupstaggingapi-20170126. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsResourcegroupstaggingapiResourcegroupstaggingapi20170126State {
    /// The tagged resources whose tags this service is the HOME for: the ones
    /// whose owning service this crate does not link against, or links against
    /// without that service modelling the resource's own tag set. Keyed by ARN.
    ///
    /// A `BTreeMap` rather than a `HashMap` so ARN order — and therefore every
    /// page boundary and every resumed cursor — is the same on every call.
    ///
    /// Bounded by [`MAX_TAGGED_RESOURCES`]: an entry is deliberately KEPT after
    /// its last tag is removed (the service reports a resource that ever had a
    /// tag with an empty tag set), so nothing else would ever shrink this map.
    pub resources: BTreeMap<String, ResourceEntry>,
}

/// The most tagged resources this emulator holds at once.
///
/// NOT an AWS quota: the real service has no such limit. This is the emulator's
/// bound on in-memory fixture state, so a client looping on `TagResources` with
/// fresh ARNs is refused rather than growing the store without end. Reaching it
/// refuses only the ARNs that would be new, as that ARN's own
/// `FailedResourcesMap` entry; re-tagging a resource already in the store always
/// succeeds.
pub const MAX_TAGGED_RESOURCES: usize = 1_000;

/// The most tags one resource holds at once.
///
/// An AWS limit, not an emulator one: "Each resource can have up to 50 tags"
/// (`TagResources` documentation), which is also the modelled `TagMap` bound on a
/// single request. The modelled bound caps ONE request; this caps what repeated
/// requests can accumulate on one resource.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The partition this emulator serves, as `::engine::emulator_arn` mints it
/// (`arn:aws:…`). An ARN parameter naming any other partition is refused: "The
/// partition specified in an ARN parameter in the request doesn't match the
/// partition where you invoked the operation" (`InvalidParameterException`
/// documentation).
pub const PARTITION: &str = "aws";

/// The region a request is scoped to when it carries no SigV4 credential scope
/// (an unsigned call). Matches the region the emulator's own generated config
/// defaults to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The reserved tag-key namespace: keys in it are SYSTEM tags that AWS itself
/// writes (`aws:cloudformation:stack-name`, …) and a caller may not.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// What `TagResources`/`UntagResources` answer when a request names a key in
/// [`RESERVED_TAG_KEY_PREFIX`].
pub const SYSTEM_TAGS_MESSAGE: &str = "The caller isn't allowed to modify system tags.";

impl ComAmazonawsResourcegroupstaggingapiResourcegroupstaggingapi20170126State {
    /// How many tags `arn` would carry after `keys` were merged into it, so a
    /// write can be refused BEFORE it grows the store past
    /// [`MAX_TAGS_PER_RESOURCE`].
    pub fn tag_count_after_merging<'k>(
        &self,
        arn: &str,
        keys: impl IntoIterator<Item = &'k String>,
    ) -> usize {
        let held = self.resources.get(arn).map(|entry| &entry.tags);
        tag_count_after_merging(held, keys)
    }

    /// The entry for `arn`, created with an empty tag map when this is the first
    /// tag ever written to it, or `None` when the ARN is new and the store is
    /// already holding [`MAX_TAGGED_RESOURCES`].
    ///
    /// A resource stays in the store once tagged, even after its last tag is
    /// removed: the service reports a resource that "ever had a tag" with an
    /// empty tag set (see the `TagFilters` documentation on `GetResources`).
    pub fn entry_mut(&mut self, arn: &ParsedArn) -> Option<&mut ResourceEntry> {
        if !self.resources.contains_key(&arn.arn) && self.resources.len() >= MAX_TAGGED_RESOURCES {
            return None;
        }
        Some(
            self.resources
                .entry(arn.arn.clone())
                .or_insert_with(|| ResourceEntry {
                    arn: arn.arn.clone(),
                    tags: BTreeMap::new(),
                    resource_type: arn.resource_type(),
                }),
        )
    }

    /// Every resource the tagging API reports for `region`: the ones the owning
    /// services themselves hold tags for (`owned`, from
    /// [`owners::tagged_resources`]) plus this store's own entries, in ARN
    /// order.
    ///
    /// The owning service WINS where both name one ARN, because where a service
    /// models a resource's tag set that set is the resource's tags; this store
    /// only ever holds an ARN the owning services do not answer for.
    ///
    /// Resources outside `region` are left out: "Returns all the tagged or
    /// previously tagged resources that are located in the specified Amazon Web
    /// Services Region for the account" (`GetResources` documentation). An ARN
    /// with an empty region segment names a global resource, which every
    /// regional endpoint reports.
    pub fn resources_in_region(
        &self,
        owned: Vec<(String, owners::Tags)>,
        region: &str,
    ) -> Vec<ResourceEntry> {
        let mut rows: BTreeMap<String, ResourceEntry> = BTreeMap::new();
        for entry in self.resources.values() {
            rows.insert(entry.arn.clone(), entry.clone());
        }
        for (arn, tags) in owned {
            if let Some(parsed) = parse_arn(&arn) {
                rows.insert(arn, ResourceEntry::new(&parsed, tags));
            }
        }
        rows.into_values()
            .filter(|entry| {
                parse_arn(&entry.arn).is_some_and(|parsed| parsed.is_served_by(region))
            })
            .collect()
    }
}

/// How many tags a resource already holding `held` would carry after `keys` were
/// merged into it.
pub fn tag_count_after_merging<'k>(
    held: Option<&BTreeMap<String, String>>,
    keys: impl IntoIterator<Item = &'k String>,
) -> usize {
    let mut merged: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    if let Some(held) = held {
        merged.extend(held.keys().cloned());
    }
    merged.extend(keys.into_iter().cloned());
    merged.len()
}

/// One tagged resource: its ARN, its current tags, and the
/// `service[:resource-type]` key that `ResourceTypeFilters` matches against.
///
/// `resource_type` is derived from the ARN when the entry is created and never
/// changes afterwards, because the ARN it is derived from never changes.
#[derive(Debug, Clone)]
pub struct ResourceEntry {
    /// The resource's ARN, exactly as the caller wrote it.
    pub arn: String,
    /// The resource's tags, key -> value. Ordered, so `Tags` is stable per read.
    pub tags: BTreeMap<String, String>,
    /// The `service:resource-type` key this resource answers to, or just
    /// `service` for an ARN whose type this emulator cannot name.
    pub resource_type: String,
}

impl ResourceEntry {
    /// One row of the projection: an ARN and the tags it carries.
    pub fn new(arn: &ParsedArn, tags: BTreeMap<String, String>) -> Self {
        Self {
            arn: arn.arn.clone(),
            tags,
            resource_type: arn.resource_type(),
        }
    }

    /// This resource's tags as the modelled `Tag` list, in key order.
    pub fn tag_list(&self) -> Vec<Tag> {
        self.tags
            .iter()
            .map(|(key, value)| Tag {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    }

    /// How many tag pairs this resource costs against a `TagsPerPage` budget.
    ///
    /// "A resource with no tags is counted as having one tag (one key and value
    /// pair)" — `GetResources`' `TagsPerPage` documentation.
    pub fn tags_per_page_cost(&self) -> usize {
        self.tags.len().max(1)
    }

    /// [`Self::tags_per_page_cost`] as the cost function [`page`] takes.
    pub fn tags_per_page_cost_of(entry: &Self) -> usize {
        entry.tags_per_page_cost()
    }

    /// This resource as a `GetResources` row.
    ///
    /// `ComplianceDetails` is always absent: it reports a resource's compliance
    /// with the effective tag policy, and this emulator models no tag policy at
    /// all, so there is nothing to judge a resource against. `GetResources`
    /// refuses `IncludeComplianceDetails` as not implemented rather than filling
    /// the member with a value nothing computed.
    pub fn mapping(&self) -> ResourceTagMapping {
        ResourceTagMapping {
            resource_arn: Some(self.arn.clone()),
            tags: Some(self.tag_list()),
            compliance_details: None,
        }
    }

    /// Whether this resource satisfies every entry of `filters`.
    ///
    /// Filters are AND-ed; within one filter the `Values` are OR-ed, and a filter
    /// with no values matches the key with any value (`GetResources`' `TagFilters`
    /// documentation). An empty filter list matches every resource.
    pub fn matches_tag_filters(&self, filters: &[TagFilter]) -> bool {
        filters.iter().all(|filter| {
            let Some(key) = filter.key.as_deref() else {
                // A keyless filter is refused before this runs; treat it as
                // matching nothing rather than matching everything.
                return false;
            };
            match self.tags.get(key) {
                None => false,
                Some(value) => match filter.values.as_deref() {
                    None | Some([]) => true,
                    Some(values) => values.iter().any(|wanted| wanted == value),
                },
            }
        })
    }

    /// Whether this resource's type is selected by `filters`.
    ///
    /// "The format of each resource type is `service[:resourceType]`. For
    /// example, specifying a service of `ec2` returns all Amazon EC2 resources
    /// (which includes EC2 instances). Specifying a resource type of
    /// `ec2:instance` returns only EC2 instances" (`GetResources`'
    /// `ResourceTypeFilters` documentation): a BARE service token selects every
    /// resource of that service and the QUALIFIED token only that one type, so
    /// both halves of this resource's own `service:resourceType` key are
    /// compared. Several filters select the union of what each selects, and an
    /// empty list selects every resource.
    pub fn matches_resource_type_filters(&self, filters: &[String]) -> bool {
        filters.is_empty()
            || filters.iter().any(|filter| {
                filter == &self.resource_type
                    || self
                        .resource_type
                        .split_once(':')
                        .is_some_and(|(service, _)| filter == service)
            })
    }
}

/// The six fields of an ARN: `arn:partition:service:region:account-id:resource`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedArn {
    /// The ARN as written, so a handler stores exactly what the caller sent.
    pub arn: String,
    /// Field 2 — `aws`, `aws-cn`, `aws-us-gov`, …
    pub partition: String,
    /// Field 3 — the service namespace (`s3`, `sqs`, …).
    pub service: String,
    /// Field 4 — the region, empty for a global resource.
    pub region: String,
    /// Field 5 — the account id, empty where the service omits it.
    pub account: String,
    /// Field 6 and everything after it: `resource-id`, `resource-type/id` or
    /// `resource-type:id`.
    pub resource: String,
}

impl ParsedArn {
    /// The `service[:resource-type]` key `ResourceTypeFilters` matches against.
    ///
    /// "The string for each service name and resource type is the same as that
    /// embedded in a resource's ARN" (`GetResources`' `ResourceTypeFilters`
    /// documentation), so the type is the resource field's leading segment where
    /// it has one — and, only where it has none, the single resource type
    /// [`implicit_resource_type`] names for that service.
    ///
    /// A PATH-STYLE resource field, which opens with the path separator rather
    /// than with the type (`/apikeys/<id>`, `/restapis/<id>/stages/<name>`, as
    /// API Gateway's ARNs are written), has its leading segment after that one
    /// separator, so the two spellings of one resource field name one type.
    pub fn resource_type(&self) -> String {
        let path = self.resource.strip_prefix('/').unwrap_or(&self.resource);
        match path.split_once(['/', ':']) {
            Some((kind, _)) if !kind.is_empty() => format!("{}:{kind}", self.service),
            _ => match implicit_resource_type(&self.service) {
                Some(kind) => format!("{}:{kind}", self.service),
                None => self.service.clone(),
            },
        }
    }

    /// Whether a regional endpoint serving `region` answers for this ARN: the
    /// partition must be the one this emulator serves, and the region must be
    /// that one or empty (a global resource, which every regional endpoint
    /// reports).
    pub fn is_served_by(&self, region: &str) -> bool {
        self.partition == PARTITION && (self.region.is_empty() || self.region == region)
    }
}

/// The single resource type of a service whose ARN format embeds NO resource-type
/// segment, so `service:resourceType` cannot be read off the ARN.
///
/// Source: each service's documented ARN format in the "Amazon Resource Names"
/// reference — `arn:aws:s3:::bucket_name`, `arn:aws:sqs:region:account:queue_name`
/// and `arn:aws:sns:region:account:topic_name` carry the resource id alone, while
/// the resource type the tagging API names them by is `s3:bucket`, `sqs:queue`
/// and `sns:topic`. It is a FALLBACK: one of these services' ARNs that does
/// carry a type segment (`arn:aws:s3:…:accesspoint/ap-a`) keeps the type its ARN
/// states. A service absent from this table has its type read off the ARN, so an
/// ARN of a type-less form this table does not name answers only to its bare
/// service token: a qualified filter for it selects nothing (an empty page, not
/// an error), and the fix is a row here.
pub fn implicit_resource_type(service: &str) -> Option<&'static str> {
    match service {
        "s3" => Some("bucket"),
        "sqs" => Some("queue"),
        "sns" => Some("topic"),
        _ => None,
    }
}

/// Parse an ARN, or `None` when the string is not one.
///
/// An ARN is the literal `arn`, then five `:`-separated fields, of which the
/// partition, the service and the resource must be non-empty; the region and the
/// account are legitimately empty for global and account-less resources
/// (`arn:aws:s3:::bucket-a`). The resource field is everything after the fifth
/// `:`, because it may itself contain `:`.
pub fn parse_arn(arn: &str) -> Option<ParsedArn> {
    let mut fields = arn.splitn(6, ':');
    let scheme = fields.next()?;
    let partition = fields.next()?;
    let service = fields.next()?;
    let region = fields.next()?;
    let account = fields.next()?;
    let resource = fields.next()?;
    if scheme != "arn" || partition.is_empty() || service.is_empty() || resource.is_empty() {
        return None;
    }
    Some(ParsedArn {
        arn: arn.to_string(),
        partition: partition.to_string(),
        service: service.to_string(),
        region: region.to_string(),
        account: account.to_string(),
        resource: resource.to_string(),
    })
}

/// The ARN `value` names, or the `InvalidParameterException` message for a value
/// no regional endpoint in `region` may be asked to tag.
///
/// Three request-level faults, each refusing the WHOLE call before any state is
/// written (the batch is atomic), as distinct from a well-formed ARN that names
/// nothing — which is a per-resource `FailedResourcesMap` entry:
///
/// * the value is not an ARN at all ("A provided string parameter is
///   malformed"), reported by [`not_an_arn_message`];
/// * its partition is not the one the operation was invoked in ("The partition
///   specified in an ARN parameter in the request doesn't match the partition
///   where you invoked the operation");
/// * its region is another one — "You can only tag resources that are located
///   in the specified Amazon Web Services Region" (`TagResources`
///   documentation) and this endpoint serves exactly one — reported by
///   [`region_mismatch_message`].
///
/// `operation` is the name of the operation being invoked, which the
/// region-mismatch message names — so each caller passes its own
/// `Operation::NAME` rather than a literal.
pub fn addressable_arn(value: &str, region: &str, operation: &str) -> Result<ParsedArn, String> {
    let Some(parsed) = parse_arn(value) else {
        return Err(not_an_arn_message(value));
    };
    if parsed.partition != PARTITION {
        return Err(format!(
            "The partition specified in an ARN parameter in the request doesn't match the \
             partition where you invoked the operation: {value}"
        ));
    }
    if !parsed.region.is_empty() && parsed.region != region {
        return Err(region_mismatch_message(value, operation));
    }
    Ok(parsed)
}

/// The `InvalidParameterException` message for an ARN-list value that cannot be
/// parsed as an ARN.
///
/// The offending value comes first and the rest is fixed text: the service
/// reports the value it could not parse, not the member it arrived in, so the
/// same text serves every ARN-list parameter of every operation here.
pub fn not_an_arn_message(value: &str) -> String {
    format!("{value} is not a valid AmazonResourceName (ARN)")
}

/// The `InvalidParameterException` message for an ARN whose region field is not
/// the region the call was invoked in.
///
/// The two variable parts are the offending ARN, echoed as the request wrote it,
/// and the invoked operation's name — not the endpoint's region, which the
/// message does not state.
pub fn region_mismatch_message(value: &str, operation: &str) -> String {
    format!(
        "Region in the ARN {value} does not match with the region in which {operation} API is \
         invoked"
    )
}

/// Whether `key` is in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace.
///
/// Compared without case, as AWS reserves the prefix rather than one spelling of
/// it, and over BYTES rather than a `&str` slice: a tag key is caller-supplied
/// text whose 4th byte may fall inside a multi-byte character (`awsé`, `日本`),
/// and slicing a `&str` there would panic instead of answering.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.as_bytes()
        .get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX.as_bytes()))
}

/// The per-resource failure `TagResources` reports for a write that would push
/// one resource past [`MAX_TAGS_PER_RESOURCE`].
///
/// A documented service limit, so the message states the limit and the count the
/// request would have reached.
pub fn too_many_tags_failure(arn: &str, would_hold: usize) -> FailureInfo {
    FailureInfo {
        status_code: 400,
        error_code: Some("InvalidParameterException".to_string()),
        error_message: Some(format!(
            "{arn} would hold {would_hold} tags; each resource can have up to \
             {MAX_TAGS_PER_RESOURCE} tags"
        )),
    }
}

/// The per-resource failure `TagResources` reports for a NEW ARN once the store
/// already holds [`MAX_TAGGED_RESOURCES`].
///
/// The bound is the emulator's, not the service's, so the message says so rather
/// than quoting an AWS quota that does not exist.
pub fn store_at_capacity_failure(arn: &str) -> FailureInfo {
    FailureInfo {
        status_code: 400,
        error_code: Some("InvalidParameterException".to_string()),
        error_message: Some(format!(
            "this emulator holds at most {MAX_TAGGED_RESOURCES} tagged resources and is already \
             at that limit, so {arn} was not tagged"
        )),
    }
}

/// The documented minimum of `GetResources`' `ResourcesPerPage`.
pub const MIN_RESOURCES_PER_PAGE: i32 = 1;

/// The documented maximum of `GetResources`' `ResourcesPerPage`: "You can specify
/// a minimum of 1 and a maximum value of 100."
pub const MAX_RESOURCES_PER_PAGE: i32 = 100;

/// The documented minimum of `GetResources`' `TagsPerPage`: "You can set
/// `TagsPerPage` to a minimum of 100 items up to a maximum of 500 items."
pub const MIN_TAGS_PER_PAGE: i32 = 100;

/// The documented maximum of `GetResources`' `TagsPerPage`.
pub const MAX_TAGS_PER_PAGE: i32 = 500;

/// The `InvalidParameterException` message for a page-size member outside its
/// documented window.
///
/// The window is a fixed API constant, not an account quota, so the text is the
/// member's name and that constant window — the SAME text for every value
/// outside it, with the submitted value not echoed.
pub fn out_of_range_message(member: &str, min: i32, max: i32) -> String {
    format!("{member} is not in the valid range of {min} and {max}")
}

/// What `GetResources` answers when `ResourceARNList` is combined with any of
/// the filtering or pagination members it is mutually exclusive with.
///
/// ONE sentence covers every one of those combinations — the documented
/// prohibition is the same for `TagFilters`, `ResourceTypeFilters`,
/// `ResourcesPerPage`, `TagsPerPage` and `PaginationToken` — and it is entirely
/// fixed text: it names no member, value or limit, so there is nothing in it to
/// substitute from the request.
pub const FILTERS_WITH_RESOURCE_LIST_MESSAGE: &str =
    "Filters is not allowed when providing a resource list.";

/// The `InvalidParameterException` message for a list member whose element is
/// missing a field the element's own shape requires (`TagFilters`' `Key`).
///
/// The two variable parts are the member the list arrived in and the field the
/// element left out, both spelled as the API spells them; the rest is fixed, so
/// the same text serves every such element of every list member.
pub fn missing_required_field_message(member: &str, field: &str) -> String {
    format!("An element of {member} is missing required field {field}")
}

/// The `InvalidParameterException` message for a list member that names the same
/// value twice.
///
/// The one variable part is the repeated value, echoed as the request wrote it;
/// the rest is fixed, including the trailing period, so the same text serves
/// every list member whose entries have to be distinct.
pub fn duplicate_value_message(value: &str) -> String {
    format!("{value} is a duplicate, please input unique resources.")
}

/// The refusal for the FIRST value `values` repeats, or `None` when every value
/// is distinct.
///
/// Values are compared exactly as the request wrote them: two entries differ
/// unless they are the same string.
pub fn duplicate_value_refusal(values: &[String]) -> Option<String> {
    let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    values
        .iter()
        .find(|value| !seen.insert(value.as_str()))
        .map(|value| duplicate_value_message(value))
}

/// The `InvalidParameterException` message for a request that sets a member the
/// documentation admits only alongside another one: it names the member that has
/// to be specified, and the value it has to carry.
///
/// Stated by the member that was MISSING rather than by the one the caller did
/// send — a caller is told what to add, not what to drop — so the text is the same
/// for every dependent member of the same enabling one. `member` is spelled as
/// the API spells it (`GetResources`' `ExcludeCompliantResources`: "You can use
/// this parameter only if the `IncludeComplianceDetails` parameter is also set to
/// true").
pub fn must_also_specify_message(member: &str) -> String {
    format!("You must specify the {member} parameter and set it to true.")
}

/// The page size `GetTagKeys` and `GetTagValues` serve.
///
/// Neither operation exposes a page-size member, so the page size is the
/// server's to choose; this emulator serves the same 100 items a page that
/// `GetResources` documents as its `ResourcesPerPage` maximum.
pub const KEYS_PER_PAGE: usize = 100;

/// How long a minted `PaginationToken` resumes a scan: "A `PaginationToken` is
/// valid for a maximum of 15 minutes" (`PaginationTokenExpiredException`
/// documentation).
pub const PAGINATION_TOKEN_TTL: Duration = Duration::from_secs(15 * 60);

/// Why a `PaginationToken` cannot be resumed — two DIFFERENT answers the service
/// models separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenRejection {
    /// Not a token this service minted for this query at all: a malformed
    /// parameter, answered with `InvalidParameterException`, which tells the
    /// client the call will never succeed.
    NotMinted,
    /// A token this service DID mint, past its [`PAGINATION_TOKEN_TTL`] window:
    /// answered with `PaginationTokenExpiredException`, which tells the client
    /// to restart the scan.
    Expired,
}

/// The `InvalidParameterException` message for a token this service never
/// minted.
pub fn invalid_token_message(token: &str) -> String {
    format!("Pagination token is invalid={token}")
}

/// A stable, short digest over the request members a page is scoped to, the
/// instant the token was minted and the item it resumes after.
///
/// Covering the scope is what makes a token minted for one projection — another
/// operation, or another filter set — unusable against a list it was never cut
/// from; covering the whole token body is what tells a token this service minted
/// from a string a caller made up. Lengths are hashed alongside the parts so two
/// different scopes cannot collide by re-splitting the same bytes.
fn token_digest(scope: &[&str], issued: &str, after: &str) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    for part in scope.iter().copied().chain([issued, after]) {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    hex::encode(&hasher.finalize()[..8])
}

/// Seconds since the Unix epoch, the unit a token records its mint instant in.
fn epoch_seconds(at: SystemTime) -> u64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// The opaque token that resumes a page of `scope` after the item keyed `after`.
pub fn encode_pagination_token(scope: &[&str], after: &str) -> String {
    encode_pagination_token_at(scope, after, SystemTime::now())
}

/// [`encode_pagination_token`] with the mint instant supplied, which is the seam
/// a test mints an aged token through.
///
/// The token is OPAQUE, as the member's documentation describes it ("a string
/// that indicates that there is more data available than this response
/// contains", to be handed back unread): the cursor body is base64 of
/// `digest:issued:after`, so what a client sees is one unstructured run of
/// base64 characters rather than the resource ARN and mint instant in clear.
/// Inside the body `digest` is hex and `issued` is digits, so the first two `:`
/// always separate the three parts even when `after` is an ARN full of them. The
/// longest body this builds is 16 hex digits, a 10-digit second count, two
/// separators and a 1011-character `ResourceARN`, and base64 of that is about
/// 1.4 K characters: inside the modelled 2048-character `PaginationToken` bound.
pub fn encode_pagination_token_at(scope: &[&str], after: &str, issued: SystemTime) -> String {
    use base64::Engine as _;

    let issued = epoch_seconds(issued).to_string();
    let digest = token_digest(scope, &issued, after);
    base64::engine::general_purpose::STANDARD.encode(format!("{digest}:{issued}:{after}"))
}

/// The resume point `token` carries, or why it cannot be resumed.
pub fn decode_pagination_token(token: &str, scope: &[&str]) -> Result<String, TokenRejection> {
    use base64::Engine as _;

    // A string that is not base64 at all is not a token this service minted, so
    // it earns the same answer as base64 carrying a body this service never cut.
    let Ok(body) = base64::engine::general_purpose::STANDARD.decode(token) else {
        return Err(TokenRejection::NotMinted);
    };
    let Ok(body) = String::from_utf8(body) else {
        return Err(TokenRejection::NotMinted);
    };
    let mut parts = body.splitn(3, ':');
    let (Some(digest), Some(issued), Some(after)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err(TokenRejection::NotMinted);
    };
    if after.is_empty() || digest != token_digest(scope, issued, after) {
        return Err(TokenRejection::NotMinted);
    }
    let Ok(issued) = issued.parse::<u64>() else {
        return Err(TokenRejection::NotMinted);
    };
    if epoch_seconds(SystemTime::now()).saturating_sub(issued) > PAGINATION_TOKEN_TTL.as_secs() {
        return Err(TokenRejection::Expired);
    }
    Ok(after.to_string())
}

/// One page of a key-ordered projection, plus the token that resumes after it.
///
/// `items` must be ascending and unique by key — the key is the cursor, so the
/// resumed call returns exactly the items the page did not reach, with no
/// repeats. `max_items` caps the page by count. `max_cost` caps it by a second,
/// item-weighted budget (`GetResources`' `TagsPerPage`), and is never allowed to
/// empty a page: an item costing more than the whole budget is served alone
/// rather than stalling pagination, because the service "does not split a
/// resource and its associated tags across pages".
///
/// A walk ends on an EMPTY page, never on a full one: a page that reached
/// `max_items` carries a resume token even when it consumed the last item, so
/// the client makes one more call and gets an empty page with no token. Only a
/// page holding fewer than `max_items` items ends the walk at once, because that
/// is the only page the server can tell is short of matches rather than short of
/// room. A page the cost budget cut short is therefore tokened only while items
/// really remain: it is under-full by count, and the budget never trims a page
/// that had nothing left to trim.
pub fn page<T>(
    items: Vec<(String, T)>,
    after: Option<&str>,
    max_items: usize,
    max_cost: Option<usize>,
    cost: impl Fn(&T) -> usize,
    scope: &[&str],
) -> (Vec<T>, Option<String>) {
    let max_items = max_items.max(1);
    let mut rest: Vec<(String, T)> = items
        .into_iter()
        .filter(|(key, _)| after.is_none_or(|after| key.as_str() > after))
        .collect();

    let mut taken = 0usize;
    let mut spent = 0usize;
    for (_, item) in &rest {
        if taken == max_items {
            break;
        }
        let item_cost = cost(item);
        if max_cost.is_some_and(|max| taken > 0 && spent + item_cost > max) {
            break;
        }
        spent += item_cost;
        taken += 1;
    }

    // A page is resumable when it was filled to its item cap — whether or not
    // more matches are known to remain — or when it was cut short with matches
    // still unreached. Either way the cursor is the last key the page served, so
    // `rest` is never empty when a token is minted.
    let filled = taken == max_items;
    let truncated = rest.len() > taken;
    rest.truncate(taken);
    let next = (filled || truncated)
        .then(|| {
            rest.last()
                .map(|(key, _)| encode_pagination_token(scope, key))
        })
        .flatten();

    (rest.into_iter().map(|(_, item)| item).collect(), next)
}

/// The region a request is scoped to: the one its SigV4 credential scope names,
/// or [`DEFAULT_REGION`] when the request carries no scope (an unsigned call).
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsResourcegroupstaggingapiResourcegroupstaggingapi20170126State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsResourcegroupstaggingapiResourcegroupstaggingapi20170126State>)
}
