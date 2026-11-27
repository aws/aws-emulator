// SPDX-License-Identifier: CC0-1.0

//! Resource tagging: the tags a resource of this service carries, and the ARN a
//! tagging operation names it by.
//!
//! A tagging operation addresses its resource by ARN rather than by id, so the
//! parse of that ARN, the tag map it resolves to and the one quota every resource's
//! tags are bounded by live here together — once — and the operations that add,
//! remove and report tags cannot disagree about any of them.

use super::*;

/// The `Tags` member a read of a taggable resource reports, from the tag map the
/// resource's record holds.
///
/// Every `Get`/`Describe` in this service projects its resource's tags through
/// here, so one rule answers all of them: the member is ALWAYS reported, and a
/// resource nothing tagged reports an EMPTY map rather than no member at all —
/// which is what GuardDuty answers for an untagged resource (the recorded
/// `GetDetector` response of a detector created without tags carries an empty
/// `tags` map). A client can therefore read the map on any resource of this
/// service without a null guard.
///
/// The `len 1..200` the model puts on `TagMap`
/// (`smithy_query(query="shape", shape="TagMap")`) bounds what a REQUEST may
/// carry — which is why a create or `TagResource` sending an empty map is
/// refused at the modelled boundary — and is not a floor on what a read of a
/// resource reports.
pub fn reported_tags(tags: &BTreeMap<String, String>) -> Option<HashMap<String, String>> {
    Some(
        tags.iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    )
}

/// A detector sub-resource that carries tags of its own.
///
/// One variant per collection hanging off [`Detector`] whose record has a tag
/// map; the detector itself is the ARN that carries no sub-resource path at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggedSubResource {
    Filter,
    IpSet,
    ThreatIntelSet,
    TrustedEntitySet,
    ThreatEntitySet,
    PublishingDestination,
}

/// The ARN resource-path segment that names each sub-resource, which is the
/// segment that resource's own REST URI uses: `GetFilter` is bound to
/// `/detector/{DetectorId}/filter/{FilterName}`, `GetIPSet` to
/// `.../ipset/{IpSetId}`, `GetThreatIntelSet` to
/// `.../threatintelset/{ThreatIntelSetId}`, `GetTrustedEntitySet` to
/// `.../trustedentityset/{TrustedEntitySetId}`, `GetThreatEntitySet` to
/// `.../threatentityset/{ThreatEntitySetId}` and
/// `DescribePublishingDestination` to
/// `.../publishingDestination/{DestinationId}`
/// (`smithy_query(operation="GetFilter,GetIPSet,GetThreatIntelSet,GetTrustedEntitySet,GetThreatEntitySet,DescribePublishingDestination")`).
pub const TAGGED_SUB_RESOURCE_SEGMENTS: [(&str, TaggedSubResource); 6] = [
    ("filter", TaggedSubResource::Filter),
    ("ipset", TaggedSubResource::IpSet),
    ("threatintelset", TaggedSubResource::ThreatIntelSet),
    ("trustedentityset", TaggedSubResource::TrustedEntitySet),
    ("threatentityset", TaggedSubResource::ThreatEntitySet),
    (
        "publishingDestination",
        TaggedSubResource::PublishingDestination,
    ),
];

/// The resource a tagging operation's `ResourceArn` names, as the ARN spells it
/// out: the scope it claims plus the detector and — for a sub-resource — which
/// one of the detector's collections holds it, under which id.
///
/// Form only. Nothing here is resolved against the store, and the claimed scope
/// is not checked either: the scope is [`scoped_tagged_resource`]'s job and the
/// records are the job of the resolution both halves of the tagging surface share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaggedResource {
    /// The partition the ARN claims.
    pub partition: String,
    /// The Region the ARN claims.
    pub region: String,
    /// The account the ARN claims.
    pub account_id: String,
    /// The `DetectorId` the resource path names.
    pub detector_id: String,
    /// The sub-resource the path continues into and the id that names it within
    /// the detector, or `None` when the ARN names the detector itself.
    pub sub_resource: Option<(TaggedSubResource, String)>,
}

/// The resource `arn` names, when `arn` is the ARN of a GuardDuty resource that
/// carries tags.
///
/// `None` when it is not: a resource path that is not a detector's, or one whose
/// type segment is none of [`TAGGED_SUB_RESOURCE_SEGMENTS`] — the Malware
/// Protection plan and custom-detection-rule ARNs
/// ([`malware_protection_plan_arn`], [`custom_detection_rule_arn`]) are of that
/// kind, and are not reachable through `/tags/{ResourceArn}` at all because the
/// member's pattern admits only the detector path.
pub fn parse_tagged_resource_arn(arn: &str) -> Option<TaggedResource> {
    let [_, partition, service, region, account_id, resource] = arn_fields(arn)?;
    if service != GUARDDUTY_ARN_SERVICE {
        return None;
    }
    let segments: Vec<&str> = resource.split('/').collect();
    let (detector_id, sub_resource) = match segments.as_slice() {
        ["detector", detector_id] if !detector_id.is_empty() => ((*detector_id).to_string(), None),
        ["detector", detector_id, kind, id] if !detector_id.is_empty() && !id.is_empty() => {
            let sub_resource = TAGGED_SUB_RESOURCE_SEGMENTS
                .iter()
                .find(|(segment, _)| segment == kind)
                .map(|(_, sub_resource)| *sub_resource)?;
            (
                (*detector_id).to_string(),
                Some((sub_resource, (*id).to_string())),
            )
        }
        _ => return None,
    };
    Some(TaggedResource {
        partition: partition.to_string(),
        region: region.to_string(),
        account_id: account_id.to_string(),
        detector_id,
        sub_resource,
    })
}

/// Classify a `ResourceArn` this endpoint resolves no taggable resource for, the
/// way [`minted_id_rejection`] classifies an id: an ARN that names no resource
/// this service tags is a parameter failure, a well-formed one this
/// account/Region does not hold an ownership failure. The class comes from the
/// REQUEST, and neither message quotes the ARN back.
pub fn resource_arn_rejection(arn: &str) -> String {
    if parse_tagged_resource_arn(arn).is_some() {
        not_owned_message(RESOURCE_ARN_MEMBER)
    } else {
        invalid_parameter_message(RESOURCE_ARN_MEMBER)
    }
}

/// The most tags one resource carries — the PUBLISHED quota, not a figure this
/// emulator chose: "Tagging is currently supported for detectors, finding filters,
/// IP sets, threat intel sets, and publishing destination, with a limit of 50 tags
/// per resource" (`smithy_query(query="doc", shape="ListTagsForResource")`).
///
/// It is a different quantity from the `len 1..200` on `TagMap`
/// (`smithy_query(shape="TagMap")`), which bounds how many pairs ONE REQUEST's map
/// may carry: a request inside that bound can still ask for more tags than a
/// resource may hold, and is refused here rather than at the modelled boundary.
/// The limit is stated for the resource and not for a path to it, so it holds
/// however the tags arrive — through `TagResource` or through the `Tags` member of
/// a create. The sentence predates the newer classes that carry a `TagMap` of
/// their own (the entity sets, the Malware Protection plan, the custom
/// detection-rule association), and the figure is applied to those too: one
/// ceiling for every resource of this service, which is also what the single
/// rejection message can say.
///
/// It also BOUNDS the store: a tag key is REQUEST-chosen, so without a ceiling a
/// caller adding fresh keys to one resource could grow its map without limit — the
/// hazard [`MAX_CLIENT_TOKENS`] guards for request-chosen token keys.
pub const TAGS_PER_RESOURCE: usize = 50;

/// The message a write that would leave a resource carrying more than
/// [`TAGS_PER_RESOURCE`] tags is refused with. Like every other rejection here it
/// names the limit and the resource it bounds, never the caller's keys — see
/// [`publishing_destination_limit_message`]. The resource is named as "the
/// resource" because the limit reaches every class of them and is the same for
/// each.
pub fn tag_limit_message() -> String {
    format!(
        "The request is rejected because the resource has reached the limit of {TAGS_PER_RESOURCE} tags."
    )
}

/// Refuse a write that would leave ONE resource carrying more than
/// [`TAGS_PER_RESOURCE`] tags, with [`tag_limit_message`]; `None` when it fits.
///
/// `carried` is the map the resource holds now and `requested` the keys the
/// request carries: a key the resource already carries is OVERWRITTEN rather than
/// appended, so it grows the resource by nothing. Decided before anything is
/// written, so a refused request persists none of the request's tags — not even
/// the ones that would have fitted.
pub fn tag_count_rejection<'keys>(
    carried: &BTreeMap<String, String>,
    requested: impl IntoIterator<Item = &'keys str>,
) -> Option<String> {
    let added = requested
        .into_iter()
        .filter(|key| !carried.contains_key(*key))
        .count();
    (carried.len() + added > TAGS_PER_RESOURCE).then(tag_limit_message)
}

/// The same rule for a CREATE: the resource does not exist yet, so the whole
/// `TagMap` the request carries is what it would be created carrying. A create
/// that names no `Tags` member at all carries nothing and is never refused.
pub fn created_tag_count_rejection(tags: Option<&HashMap<String, String>>) -> Option<String> {
    tag_count_rejection(
        &BTreeMap::new(),
        tags.into_iter().flatten().map(|(key, _)| key.as_str()),
    )
}

/// The resource `arn` names, once the SCOPE it claims has been checked against the
/// endpoint serving `region`.
///
/// `None` unless the ARN could name a resource this endpoint serves: the partition
/// and account it claims must be this emulator's ([`::engine::emulator_arn`] mints
/// every ARN here under them) and the Region must be the one serving the request,
/// because a detector and everything hanging off it are regional — "you must create
/// a detector in each Region where you enable the service"
/// (`smithy_query(query="doc",shape="CreateDetector")`).
///
/// Form and scope only: nothing here is resolved against the store. Both halves of
/// the tagging surface reach the store through this one check, so neither can admit
/// an ARN the other turns away.
fn scoped_tagged_resource(region: &str, arn: &str) -> Option<TaggedResource> {
    let resource = parse_tagged_resource_arn(arn)?;
    (resource.partition == PARTITION
        && resource.region == region
        && resource.account_id == ::engine::emulator_account_id())
    .then_some(resource)
}

/// Whether a sub-resource whose delete has already been ACCEPTED still resolves —
/// the ONE way the two halves of the tagging surface differ, named here so it is the
/// only thing either half decides for itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeletedSubResources {
    /// Resolved: such a record is RETAINED to be reported and its own `Get` still
    /// reports it, so REPORTING its tags is consistent.
    Resolved,
    /// Resolved by nothing: a record that is out of use takes no WRITE, so it is
    /// refused exactly like an id the detector never held.
    Unresolved,
}

impl DeletedSubResources {
    /// Whether a record resolves under this rule, given whether its delete has
    /// been accepted ([`IpSetRecord::is_deleting`],
    /// [`ThreatIntelSetRecord::is_deleting`]).
    fn resolves(self, is_deleting: bool) -> bool {
        matches!(self, Self::Resolved) || !is_deleting
    }
}

/// The whole `ResourceArn` -> tag-map resolution, written ONCE for both borrow
/// kinds: the scope check, the detector lookup and the per-collection dispatch over
/// every sub-resource [`TAGGED_SUB_RESOURCE_SEGMENTS`] names.
///
/// [`ComAmazonawsGuarddutyGuarddutyapiserviceState::tagged_resource_tags`] and
/// [`ComAmazonawsGuarddutyGuarddutyapiserviceState::tagged_resource_tags_mut`]
/// differ only in the map lookup (`get` or `get_mut`), in the detector accessor they
/// borrow through and in whether a record whose delete has been accepted resolves
/// ([`DeletedSubResources`]), so the decision tree itself lives here: a seventh
/// taggable sub-resource is resolved once and both halves gain it together, which is
/// what makes "a report and a write cannot disagree about which ARN names which
/// resource" a property of the code rather than of two bodies kept in step by hand.
///
/// An ARN that names no taggable resource of this service early-returns `None` from
/// the enclosing function, which is why this expands only inside one returning
/// `Option`.
macro_rules! tagged_resource_tags_of {
    (
        $state:expr, $region:expr, $arn:expr,
        $detector_in:ident, $get:ident, $deleted:expr
        $(, $mutability:tt)?
    ) => {{
        let region: &str = $region;
        let deleted: DeletedSubResources = $deleted;
        let resource = scoped_tagged_resource(region, $arn)?;
        let detector = $state.$detector_in(region, &resource.detector_id)?;
        let Some((sub_resource, id)) = &resource.sub_resource else {
            return Some(& $($mutability)? detector.tags);
        };
        match sub_resource {
            // A filter and a publishing destination carry no deleting status at
            // all: their delete DROPS the record, so the id resolving at all is
            // already the whole liveness check.
            TaggedSubResource::Filter => detector
                .filters
                .$get(id)
                .map(|f| & $($mutability)? f.tags),
            TaggedSubResource::IpSet => detector
                .ip_sets
                .$get(id)
                .filter(|s| deleted.resolves(s.is_deleting()))
                .map(|s| & $($mutability)? s.tags),
            TaggedSubResource::ThreatIntelSet => detector
                .threat_intel_sets
                .$get(id)
                .filter(|s| deleted.resolves(s.is_deleting()))
                .map(|s| & $($mutability)? s.tags),
            TaggedSubResource::TrustedEntitySet => detector
                .trusted_entity_sets
                .$get(id)
                .filter(|s| deleted.resolves(s.is_deleting()))
                .map(|s| & $($mutability)? s.tags),
            TaggedSubResource::ThreatEntitySet => detector
                .threat_entity_sets
                .$get(id)
                .filter(|s| deleted.resolves(s.is_deleting()))
                .map(|s| & $($mutability)? s.tags),
            TaggedSubResource::PublishingDestination => detector
                .publishing_destinations
                .$get(id)
                .map(|d| & $($mutability)? d.tags),
        }
    }};
}

impl ComAmazonawsGuarddutyGuarddutyapiserviceState {
    /// The tag map of the resource `arn` names, as `region`'s endpoint holds it,
    /// for an operation that only REPORTS tags.
    ///
    /// Resolution is [`Self::tagged_resource_tags_mut`]'s — the same ARN form, the
    /// same scope check and the same records, because both expand the one
    /// resolution `tagged_resource_tags_of!` holds — except that this one keeps
    /// resolving a sub-resource whose delete has been accepted
    /// ([`DeletedSubResources::Resolved`]), because such a record is RETAINED to be
    /// reported and its own `Get` still reports it, so reporting its tags is
    /// consistent while writing them is not. This one borrows the store immutably,
    /// so reporting tags cannot move anything a later caller reads.
    pub fn tagged_resource_tags(
        &self,
        region: &str,
        arn: &str,
    ) -> Option<&BTreeMap<String, String>> {
        tagged_resource_tags_of!(
            self,
            region,
            arn,
            detector_in,
            get,
            DeletedSubResources::Resolved
        )
    }

    /// The tag map of the resource `arn` names, as `region`'s endpoint holds it,
    /// for an operation that WRITES tags.
    ///
    /// `None` unless the ARN names a resource this endpoint serves: the scope it
    /// claims must be the one [`scoped_tagged_resource`] admits, and the record must
    /// exist.
    ///
    /// Tags are kept on the resource's own record, so the map this hands back is
    /// the one its read reports: nothing can tag a resource in a place its
    /// `Get`/`Describe` does not look.
    ///
    /// A sub-resource whose delete has already been ACCEPTED resolves to `None`
    /// here ([`DeletedSubResources::Unresolved`]), so it is refused exactly like an
    /// id the detector never held. A record in that state is retained only to be
    /// REPORTED — it is named by no listing, occupies none of the detector's slots
    /// and no update may move it — so a write against it is a write against a
    /// resource that is out of use. The REPORTING seam
    /// ([`Self::tagged_resource_tags`]) keeps resolving it, because its own `Get`
    /// still reports it.
    pub fn tagged_resource_tags_mut(
        &mut self,
        region: &str,
        arn: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        tagged_resource_tags_of!(
            self,
            region,
            arn,
            detector_in_mut,
            get_mut,
            DeletedSubResources::Unresolved,
            mut
        )
    }
}
