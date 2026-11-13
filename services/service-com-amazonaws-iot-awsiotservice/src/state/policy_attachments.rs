// SPDX-License-Identifier: CC0-1.0

//! The account's POLICY ATTACHMENTS: which policies are attached to which target, and
//! what a target is at all.
//!
//! `AttachPolicy` "Attaches the specified policy to the specified principal
//! (certificate or other credential)" and `DetachPolicy` "Detaches a policy from the
//! specified target", while `ListAttachedPolicies` reports the policies of one target
//! and `ListTargetsForPolicy` the targets of one policy — so the four operations are
//! four views of ONE relation, and that relation is what this module keeps.
//!
//! A target's IDENTITY is the resource it names, not the string that names it:
//! `ListAttachedPolicies$target` documents "Valid principals are CertificateArn
//! (arn:aws:iot:region:accountId:cert/certificateId), thingGroupArn
//! (arn:aws:iot:region:accountId:thinggroup/groupName) and CognitoId (region:id)"
//! (`smithy_query(query="operation",operation="ListAttachedPolicies")`), so the relation
//! is keyed by the resource each of those forms names ([`PolicyTarget::key`]) and the
//! same certificate named by two ARNs is one target.
//!
//! Only a target that EXISTS can be attached to or listed: a certificate ARN is resolved
//! against the certificate store and a thing-group ARN against the thing-group registry,
//! so an attachment never records a target nothing in this account holds. A Cognito
//! identity is resolved no further than its form — identity pools live in another
//! service, and this one holds no record of them.

use std::collections::{BTreeMap, BTreeSet};

/// How many policies may be attached to ONE target.
///
/// The relation is one a CALLER grows — one pair per `AttachPolicy` — so it is bounded
/// for the same reason [`super::MAX_CERTIFICATES`] is: past the ceiling nothing is
/// recorded and the attach is refused with the modelled `LimitExceededException`
/// (HTTP 410) naming the limit, rather than growing without bound. The figure is this
/// emulator's own capacity.
pub const MAX_POLICIES_PER_TARGET: usize = 10;

/// How many distinct targets may hold attachments at once, bounding the relation in the
/// other direction for the same reason [`MAX_POLICIES_PER_TARGET`] bounds it in the
/// first: one policy attached to target after target would otherwise grow the store
/// without limit.
pub const MAX_ATTACHED_TARGETS: usize = 1_000;

/// The largest page either policy-attachment listing answers with.
///
/// `pageSize` is modelled `range 1..250` on both `ListAttachedPolicies` and
/// `ListTargetsForPolicy` and documents no default
/// (`smithy_query(query="operation",operation="ListAttachedPolicies")`), so a request
/// that names none gets the largest page the model admits and the listing never
/// returns more rows in one answer than `pageSize` could have asked for.
pub const MAX_LIST_PAGE_SIZE: i32 = 250;

/// The resource type segment of a certificate ARN, as [`super::certificate_arn`] mints
/// it.
const CERTIFICATE_RESOURCE_TYPE: &str = "cert";

/// The resource type segment of a thing group ARN, as [`super::thing_group_arn`] mints
/// it.
const THING_GROUP_RESOURCE_TYPE: &str = "thinggroup";

/// The resource a `PolicyTarget` names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyTarget {
    /// `arn:<partition>:iot:<region>:<account>:cert/<certificateId>`: one X.509 device
    /// certificate, carrying its `certificateId`.
    Certificate(String),
    /// `arn:<partition>:iot:<region>:<account>:thinggroup/<groupName>`: one thing group,
    /// carrying its `thingGroupName`.
    ThingGroup(String),
    /// `<region>:<id>`: one Amazon Cognito identity, carrying the identity as supplied.
    CognitoIdentity(String),
}

impl PolicyTarget {
    /// The key the relation records this target under: the resource it names, so two
    /// ARNs of the same certificate are one target. The resource type prefixes the key,
    /// so a certificate and a thing group of the same name never collide.
    pub fn key(&self) -> String {
        match self {
            Self::Certificate(certificate_id) => {
                format!("{CERTIFICATE_RESOURCE_TYPE}/{certificate_id}")
            }
            Self::ThingGroup(group_name) => format!("{THING_GROUP_RESOURCE_TYPE}/{group_name}"),
            Self::CognitoIdentity(identity) => format!("cognito/{identity}"),
        }
    }
}

/// Why a `target` is not a target of this account's policies.
///
/// Each variant names the modelled error class the refusal answers with, so every
/// operation that takes a `PolicyTarget` refuses one target for one reason, in one
/// wording, with one status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyTargetRefusal {
    /// Not one of the forms a policy target takes: `InvalidRequestException` (HTTP 400).
    Invalid(String),
    /// A well-formed target naming a resource this account does not hold:
    /// `ResourceNotFoundException` (HTTP 404).
    NotFound(String),
}

impl PolicyTargetRefusal {
    /// The message the refusal states, whichever class it names.
    pub fn message(&self) -> &str {
        match self {
            Self::Invalid(message) | Self::NotFound(message) => message,
        }
    }
}

/// The message a reader or writer of one policy refuses a name the account holds no
/// policy under with, worded the way AWS IoT words a missing resource — "<resource>
/// <name> does not exist", as [`super::missing_role_alias_message`] does.
pub fn missing_policy_message(policy_name: &str) -> String {
    format!("Policy {policy_name} does not exist")
}

/// The message a certificate target nothing in this account holds is refused with.
///
/// AWS IoT words this one after the PRINCIPAL rather than after the ARN — the
/// certificate is the principal a policy is attached to — and it is the recorded
/// wording for an attach to a certificate that does not exist.
pub fn missing_certificate_target_message() -> String {
    "The certificate given in the principal does not exist.".to_string()
}

/// The message a request that named a `cognitoIdentityPoolId` of no supported form is
/// refused with.
///
/// An identity pool is named in the SAME `<region>:<id>` form a Cognito identity is
/// ([`parse_policy_target`]), and this service holds no record of a pool — identity pools
/// live in Amazon Cognito — so the id is resolved no further than its form. The fixed text
/// names the member and the form; the variable part is the value the request sent. Shared,
/// so every operation that reads the member refuses one value in one wording.
pub fn invalid_cognito_identity_pool_id_message(pool_id: &str) -> String {
    format!(
        "Invalid CognitoIdentityPoolId: {pool_id}. A Cognito identity pool id has the form \
         region:id"
    )
}

/// A policy target resolved against this account's state: everything an attachment or a
/// listing needs about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPolicyTarget {
    /// Which resource it names.
    pub target: PolicyTarget,
    /// The key the relation records it under ([`PolicyTarget::key`]).
    pub key: String,
    /// The target as every listing reports it: the ARN this account holds for the
    /// resource, or the Cognito identity as supplied.
    pub reported: String,
}

/// Parse `target` into the resource it names, or the reason it names none.
///
/// The ACCOUNT and REGION segments of an ARN are deliberately not compared, for the
/// reason [`super::parse_job_target`] documents: a caller may name a resource by an ARN
/// minted elsewhere, and these registries are keyed by id and by name.
pub fn parse_policy_target(target: &str) -> Result<PolicyTarget, PolicyTargetRefusal> {
    if let Some(resource) = iot_arn_resource(target) {
        let Some((resource_type, name)) = resource.split_once('/') else {
            return Err(unsupported_target(target));
        };
        return match resource_type {
            CERTIFICATE_RESOURCE_TYPE if !name.is_empty() => {
                Ok(PolicyTarget::Certificate(name.to_string()))
            }
            THING_GROUP_RESOURCE_TYPE if !name.is_empty() => {
                Ok(PolicyTarget::ThingGroup(name.to_string()))
            }
            _ => Err(unsupported_target(target)),
        };
    }
    if is_cognito_identity(target) {
        return Ok(PolicyTarget::CognitoIdentity(target.to_string()));
    }
    Err(unsupported_target(target))
}

/// The resource field of `target` when it is an `iot` ARN at all, else `None`.
fn iot_arn_resource(target: &str) -> Option<&str> {
    // 6 fields: arn:<partition>:<service>:<region>:<account-id>:<resource>. The
    // resource keeps any colon of its own, so the split is bounded at 6.
    let fields: Vec<&str> = target.splitn(6, ':').collect();
    let ["arn", _partition, "iot", _region, _account, resource] = fields.as_slice() else {
        return None;
    };
    Some(resource)
}

/// Whether `target` is the `<region>:<id>` form a Cognito identity takes.
///
/// A region is lowercase letters, digits and hyphens; an identity id is what Cognito
/// mints, so it is checked for its form rather than its contents: non-empty, and made
/// only of characters an identity carries.
fn is_cognito_identity(target: &str) -> bool {
    let Some((region, identity)) = target.split_once(':') else {
        return false;
    };
    let region_shaped = !region.is_empty()
        && region.len() <= 64
        && region.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        });
    let identity_shaped = !identity.is_empty()
        && identity.len() <= 128
        && identity
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-');
    region_shaped && identity_shaped
}

/// The refusal a target of no supported form is answered with. The message names the
/// forms the modelled member documents, so a caller learns what a target may be.
fn unsupported_target(target: &str) -> PolicyTargetRefusal {
    PolicyTargetRefusal::Invalid(format!(
        "Invalid policy target: {target}. A policy target is a certificate ARN \
         (arn:aws:iot:region:accountId:cert/certificateId), a thing group ARN \
         (arn:aws:iot:region:accountId:thinggroup/groupName) or a Cognito identity \
         (region:id)"
    ))
}

/// The relation between the account's policies and the targets they are attached to.
///
/// Keyed by the TARGET: `ListAttachedPolicies` reads one target's policies, which is the
/// read on the hot path (every authorization decision is about one principal), and
/// `ListTargetsForPolicy` walks the relation the other way. `BTreeMap`/`BTreeSet` (not
/// the hashed containers) because both listings are `@paginated` and a page boundary is
/// only meaningful over a stable order.
#[derive(Debug, Clone, Default)]
pub struct PolicyAttachments {
    /// One entry per target that holds at least one attachment, keyed by
    /// [`PolicyTarget::key`].
    targets: BTreeMap<String, AttachedTarget>,
}

/// One target of the relation: how it is reported, and which policies it holds.
#[derive(Debug, Clone)]
struct AttachedTarget {
    /// The target as `ListTargetsForPolicy` reports it
    /// ([`ResolvedPolicyTarget::reported`]).
    reported: String,
    /// The names of the policies attached to it.
    policies: BTreeSet<String>,
}

/// Why an attachment was not recorded: a ceiling of the relation, with the message the
/// modelled `LimitExceededException` states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyAttachmentLimit {
    message: String,
}

impl PolicyAttachmentLimit {
    /// The message the refusal states.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl PolicyAttachments {
    /// Attach `policy_name` to the resolved target, or answer the ceiling that refused
    /// it.
    ///
    /// IDEMPOTENT: a pair already recorded is recorded once, so a repeated attach
    /// changes nothing and is no nearer the ceiling — which is why the ceilings are read
    /// only when the write would actually add something.
    pub fn attach(
        &mut self,
        target: &ResolvedPolicyTarget,
        policy_name: &str,
    ) -> Result<(), PolicyAttachmentLimit> {
        match self.targets.get_mut(&target.key) {
            Some(attached) => {
                if attached.policies.contains(policy_name) {
                    return Ok(());
                }
                if attached.policies.len() >= MAX_POLICIES_PER_TARGET {
                    return Err(PolicyAttachmentLimit {
                        message: format!(
                            "a target may hold at most {MAX_POLICIES_PER_TARGET} attached \
                             policies; {} already holds that many",
                            attached.reported
                        ),
                    });
                }
                attached.policies.insert(policy_name.to_string());
            }
            None => {
                if self.targets.len() >= MAX_ATTACHED_TARGETS {
                    return Err(PolicyAttachmentLimit {
                        message: format!(
                            "the maximum of {MAX_ATTACHED_TARGETS} targets holding attached \
                             policies is already reached"
                        ),
                    });
                }
                self.targets.insert(
                    target.key.clone(),
                    AttachedTarget {
                        reported: target.reported.clone(),
                        policies: BTreeSet::from([policy_name.to_string()]),
                    },
                );
            }
        }
        Ok(())
    }

    /// Detach `policy_name` from the target recorded under `key`, answering whether an
    /// attachment was there to remove. A target left holding nothing is forgotten, so an
    /// empty entry never counts against [`MAX_ATTACHED_TARGETS`].
    pub fn detach(&mut self, key: &str, policy_name: &str) -> bool {
        let Some(attached) = self.targets.get_mut(key) else {
            return false;
        };
        let removed = attached.policies.remove(policy_name);
        if attached.policies.is_empty() {
            self.targets.remove(key);
        }
        removed
    }

    /// The names of the policies attached to the target recorded under `key`, in order.
    pub fn policies_of(&self, key: &str) -> Vec<String> {
        self.targets
            .get(key)
            .map(|attached| attached.policies.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Whether `policy_name` is attached to the target recorded under `key`.
    pub fn is_attached(&self, key: &str, policy_name: &str) -> bool {
        self.targets
            .get(key)
            .is_some_and(|attached| attached.policies.contains(policy_name))
    }

    /// Every target `policy_name` is attached to, as the listings report them, in order.
    pub fn targets_of(&self, policy_name: &str) -> Vec<String> {
        self.targets
            .values()
            .filter(|attached| attached.policies.contains(policy_name))
            .map(|attached| attached.reported.clone())
            .collect()
    }

    /// How many targets hold at least one attachment.
    pub fn attached_target_count(&self) -> usize {
        self.targets.len()
    }
}

impl super::ComAmazonawsIotAwsiotserviceState {
    /// Resolve `target` against this account's state: the resource it names, the key the
    /// relation records it under and the form every listing reports it in — or the
    /// reason it is no target of this account's policies.
    pub fn resolve_policy_target(
        &self,
        target: &str,
    ) -> Result<ResolvedPolicyTarget, PolicyTargetRefusal> {
        let parsed = parse_policy_target(target)?;
        let reported = match &parsed {
            PolicyTarget::Certificate(certificate_id) => {
                let Some(certificate) = self.certificates.get(certificate_id) else {
                    return Err(PolicyTargetRefusal::NotFound(
                        missing_certificate_target_message(),
                    ));
                };
                certificate.certificate_arn.clone()
            }
            PolicyTarget::ThingGroup(group_name) => {
                let Some(group) = self.thing_groups.get(group_name) else {
                    return Err(PolicyTargetRefusal::NotFound(
                        super::not_found::missing_thing_group_message(group_name),
                    ));
                };
                group.thing_group_arn.clone()
            }
            PolicyTarget::CognitoIdentity(identity) => identity.clone(),
        };
        Ok(ResolvedPolicyTarget {
            key: parsed.key(),
            target: parsed,
            reported,
        })
    }

    /// The keys of every target whose attached policies apply to `target`, nearest
    /// first.
    ///
    /// For a thing group asked for RECURSIVELY — `ListAttachedPolicies$recursive` is
    /// "When true, recursively list attached policies" — that is the group itself
    /// followed by each of its ancestors up to the hierarchy root, because a policy
    /// attached to a parent group governs the groups beneath it. For every other target,
    /// and for a group asked for non-recursively, it is the target alone.
    pub fn policy_attachment_scope(
        &self,
        target: &ResolvedPolicyTarget,
        recursive: bool,
    ) -> Vec<String> {
        match (&target.target, recursive) {
            (PolicyTarget::ThingGroup(group_name), true) => {
                let mut chain: Vec<String> = self
                    .root_to_parent_thing_groups(group_name)
                    .into_iter()
                    .map(|(name, _arn)| PolicyTarget::ThingGroup(name).key())
                    .collect();
                // `root_to_parent_thing_groups` answers root-first; the nearest group is
                // the one asked about, so the chain is walked the other way.
                chain.reverse();
                chain
            }
            _ => vec![target.key.clone()],
        }
    }
}

/// The character that separates a page marker's LISTING from the position it resumes
/// after. A control character, so it occurs in no policy name (`^[\w+=,.@-]+$`) and in
/// no ARN.
const MARKER_SEPARATOR: char = '\u{1f}';

/// The opaque `marker` that resumes `scope`'s listing after the row `resume_after`.
///
/// Hex-encoded, which satisfies the modelled `Marker` pattern `^[A-Za-z0-9+/]+={0,2}$`
/// and keeps the token opaque to the caller whatever the row contains. The encoding
/// carries no integrity protection and claims none: a caller that reverses it can name
/// any resume point of the listing it is already reading, which selects a page of that
/// same listing and nothing else.
pub fn encode_page_marker(scope: &str, resume_after: &str) -> String {
    ::hex::encode(format!("{scope}{MARKER_SEPARATOR}{resume_after}"))
}

/// The row a `marker` resumes after, or `None` when it is not a marker
/// [`encode_page_marker`] issued for THIS listing.
///
/// Every step is fallible on purpose: the marker is caller-supplied, so nothing read out
/// of it may panic a handler, and a marker issued for another listing is refused rather
/// than silently resuming a different one from the middle.
pub fn decode_page_marker(marker: &str, scope: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(marker).ok()?).ok()?;
    let (marker_scope, resume_after) = decoded.split_once(MARKER_SEPARATOR)?;
    (marker_scope == scope).then(|| resume_after.to_string())
}

/// One page of `rows` — the rows that come after `resume_after` in the listing's order,
/// at most `page_size` of them — and the marker that resumes after it, which is present
/// only when the listing is truncated.
///
/// `rows` must be ASCENDING and free of duplicates: that is the listing's total order,
/// which is what makes a page boundary meaningful, because the marker names a POSITION
/// in it — so a page resumes where the last one ended even if the row it named has since
/// been detached.
pub fn page_of(
    rows: Vec<String>,
    scope: &str,
    resume_after: Option<&str>,
    page_size: usize,
) -> (Vec<String>, Option<String>) {
    let mut page: Vec<String> = rows
        .into_iter()
        .filter(|row| resume_after.is_none_or(|cursor| row.as_str() > cursor))
        .take(page_size.saturating_add(1))
        .collect();
    let truncated = page.len() > page_size;
    page.truncate(page_size);
    let next_marker = if truncated {
        page.last().map(|row| encode_page_marker(scope, row))
    } else {
        None
    };
    (page, next_marker)
}
