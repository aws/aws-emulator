// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-supportauthz-supportauthz service.
//!
//! SupportAuthZ's primary resource is the **support permit**: the record
//! `CreateSupportPermit` mints, `GetSupportPermit` and `ListSupportPermits`
//! report, `DeleteSupportPermit` revokes, and the three tagging operations
//! annotate. Every one of those handlers computes its answer from the maps
//! below — there is no second copy of a permit anywhere in the crate.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime};

use ::aws_smithy_types::DateTime;

use crate::service::types_local::{
    ActionSet, Condition, Permit, ResourceSet, SigningKeyInfo, SupportPermitSummary,
};

/// The region a minted ARN is scoped to when the request carries no SigV4
/// credential scope (the emulator can run with request authentication off).
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace, from the model's `aws.api#service` trait.
pub const ARN_NAMESPACE: &str = "supportauthz";

/// The `aws.api#service` ARN namespace doubles as the Service Quotas service
/// code this service reports.
pub const SERVICE_CODE: &str = "supportauthz";

/// The quota code reported with `ServiceQuotaExceededException`. Emulator-chosen:
/// AWS mints real quota codes per account, and no rule produces this one.
pub const SUPPORT_PERMIT_QUOTA_CODE: &str = "L-SupportPermits";

/// A permit that authorizes the support operator right now.
pub const STATUS_ACTIVE: &str = "ACTIVE";

/// A permit whose deletion has been accepted. `DeleteSupportPermit`'s
/// `status` member documents exactly this: "The status of the support permit.
/// Returns DELETING."
pub const STATUS_DELETING: &str = "DELETING";

/// The largest page `ListSupportPermits` returns, and the page size used when a
/// request names no `maxResults`.
///
/// Both the `maxResults` range (1..100) and the `supportPermits` list length
/// (0..100) cap at 100 in the model, so no page can carry more than this many
/// summaries.
pub const MAX_RESULTS: i32 = 100;

/// How many support permits this emulator holds, so a caller cannot grow the
/// store without limit. Emulator-chosen, and reported with the operation's own
/// modelled `ServiceQuotaExceededException` — "You have exceeded a service quota
/// for support permits." (The real service also caps permits per account.)
///
/// Together with [`MAX_TAGS_PER_PERMIT`] it bounds every map on the state
/// struct: an idempotency entry and a page cursor are both dropped when the
/// permit they name leaves the store (see
/// [`ComAmazonawsSupportauthzSupportauthzState::remove`]), a cursor is minted at
/// most once per live permit, and a permit's own tag map is capped
/// independently.
pub const MAX_SUPPORT_PERMITS: usize = 256;

/// How many distinct tags one support permit carries.
///
/// This is the model's OWN bound, not an invented one: `Tags` is
/// `@length(0, 50)`, so one request can carry at most this many pairs — and a
/// permit, which `TagResource` can be called on repeatedly, carries at most this
/// many in total. Key and value length are bounded by the model too (`TagKey`
/// `@length(1, 128)`, `TagValue` `@length(0, 256)`), so the whole tag set of the
/// whole store is bounded.
pub const MAX_TAGS_PER_PERMIT: usize = 50;

/// How long after `DeleteSupportPermit` accepts a deletion the permit leaves the
/// store.
///
/// AWS documents no span, so this one is emulator-chosen — and it is never zero:
/// `DeleteSupportPermit` answers with the NON-terminal status `DELETING`, so the
/// removal is a transition the clock owes, and a caller reading the permit
/// straight after the delete must still observe that transient state (and see it
/// through the `supportPermitStatuses=DELETING` filter). It is stored as a due
/// instant and never slept on.
pub const DELETION_SPAN: Duration = Duration::from_millis(500);

/// The resource type the model's `Arn` shape declares through its
/// `aws.api#arnReference` trait, reported in the `resourceType` member of this
/// service's `ResourceNotFoundException` / `ConflictException` /
/// `ServiceQuotaExceededException`.
pub const SUPPORT_PERMIT_RESOURCE_TYPE: &str = "AWS::SupportAuthZ::SupportPermit";

/// One stored support permit: every member the single-permit output shapes
/// declare, plus its tags and the instant its removal falls due.
///
/// The modeled shapes (`Permit`, `SigningKeyInfo`) are stored themselves rather
/// than mirrored into a parallel "stored" form, so answering a read is a direct
/// copy of what was written and no member can drift between the two.
#[derive(Debug, Clone)]
pub struct SupportPermitRecord {
    pub name: String,
    pub arn: String,
    pub description: Option<String>,
    pub permit: Permit,
    pub status: String,
    pub signing_key_info: SigningKeyInfo,
    pub created_at: DateTime,
    pub support_case_display_id: Option<String>,
    /// Sorted so `ListTagsForResource` answers in one stable order whatever the
    /// insertion sequence was. Capped at [`MAX_TAGS_PER_PERMIT`] by
    /// [`ComAmazonawsSupportauthzSupportauthzState::merge_tags`].
    pub tags: BTreeMap<String, String>,
    /// When this permit leaves the store, set by `DeleteSupportPermit`. `None`
    /// means no deletion has been accepted for it.
    pub removal_due_at: Option<SystemTime>,
}

impl SupportPermitRecord {
    /// This permit as `ListSupportPermits` reports it. The summary shape carries
    /// neither `description` nor `tags`, so both are dropped here rather than in
    /// the handler.
    pub fn summary(&self) -> SupportPermitSummary {
        SupportPermitSummary {
            name: self.name.clone(),
            arn: self.arn.clone(),
            permit: self.permit.clone(),
            status: self.status.clone(),
            signing_key_info: self.signing_key_info.clone(),
            created_at: self.created_at,
            support_case_display_id: self.support_case_display_id.clone(),
        }
    }

    /// This permit's tags in the wire shape (`map<string, string>`).
    pub fn tags_map(&self) -> HashMap<String, String> {
        self.tags
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Whether this permit's removal is already due at `now` — the transition
    /// the clock owed whether or not anybody called.
    pub fn removal_is_due_at(&self, now: SystemTime) -> bool {
        self.removal_due_at.is_some_and(|due| due <= now)
    }
}

/// What a `clientToken` replays to: the permit it created, and a fingerprint of
/// the request body that created it.
///
/// The fingerprint is what separates the two `@idempotencyToken` outcomes: the
/// SAME body replays the original record, a DIFFERENT body under the same token
/// is a conflict.
#[derive(Debug, Clone)]
pub struct CreateReplay {
    pub arn: String,
    pub fingerprint: String,
}

/// What a tag merge did, or why it did nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagMerge {
    /// Merged; the permit now carries this many distinct tags.
    Merged(usize),
    /// Refused, and NOTHING was written: the merge would have left the permit
    /// with this many distinct tags, past [`MAX_TAGS_PER_PERMIT`].
    TooManyTags(usize),
    /// No permit is stored under that ARN.
    NotFound,
}

/// Per-service state for com-amazonaws-supportauthz-supportauthz. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSupportauthzSupportauthzState {
    /// Every stored permit, keyed by ARN. A `BTreeMap` so iteration order is the
    /// stable ARN order a page cursor resumes within: a permit created or
    /// deleted between two pages shifts nothing, because the cursor names the
    /// last ARN handed out rather than an offset. Capped at
    /// [`MAX_SUPPORT_PERMITS`] by `CreateSupportPermit`.
    support_permits: BTreeMap<String, SupportPermitRecord>,
    /// Name -> ARN, so lookup-by-name (`GetSupportPermit` accepts either) and
    /// the create-time name conflict check are both O(log n).
    permit_names: BTreeMap<String, String>,
    /// `clientToken` -> the create it replays. Bounded by the permit cap: an
    /// entry is dropped with the permit it names.
    idempotency: HashMap<String, CreateReplay>,
    /// Live page cursors, keyed by the opaque `nextToken` handed to the client.
    /// Resolution is a LOOKUP, never a parse, so a token this service did not
    /// mint cannot resume a page. Bounded by the permit cap: at most one cursor
    /// per live permit, and a cursor is dropped when the permit it resumes at
    /// leaves the store — so a token still outstanding across that removal is
    /// refused rather than resuming at a position that no longer exists.
    list_cursors: HashMap<String, String>,
    /// Index of `list_cursors` by resume point, so asking twice for the token of
    /// one position hands back the SAME token instead of minting a second one.
    tokens_by_position: HashMap<String, String>,
    /// Counter behind the opaque token text. It carries no client-readable
    /// structure — a client must pass a `nextToken` back unchanged.
    next_token_seq: u64,
}

impl ComAmazonawsSupportauthzSupportauthzState {
    /// Store a newly created permit and index it by name.
    pub fn insert(&mut self, record: SupportPermitRecord) {
        self.permit_names
            .insert(record.name.clone(), record.arn.clone());
        self.support_permits.insert(record.arn.clone(), record);
    }

    /// The permit stored under `arn`, if any.
    pub fn get(&self, arn: &str) -> Option<&SupportPermitRecord> {
        self.support_permits.get(arn)
    }

    /// The permit `identifier` names, by ARN or by name.
    ///
    /// `GetSupportPermit`'s identifier is modeled as
    /// `^[a-zA-Z0-9:/-]{1,512}$`, which admits a bare name as well as an ARN, so
    /// both resolve here. The ARN lookup goes first: an ARN cannot also be a
    /// name, because a name is `^[a-zA-Z0-9]{1,256}$`.
    pub fn resolve(&self, identifier: &str) -> Option<&SupportPermitRecord> {
        if let Some(record) = self.support_permits.get(identifier) {
            return Some(record);
        }
        let arn = self.permit_names.get(identifier)?;
        self.support_permits.get(arn)
    }

    /// The permit stored under `name`, if any.
    pub fn by_name(&self, name: &str) -> Option<&SupportPermitRecord> {
        let arn = self.permit_names.get(name)?;
        self.support_permits.get(arn)
    }

    /// Accept a deletion for the permit stored under `arn`: move it to
    /// `DELETING` and set its removal due [`DELETION_SPAN`] after `now`.
    ///
    /// `@idempotent`, so a second delete before the removal falls due answers
    /// with the same record and does NOT push the due instant out.
    pub fn begin_delete(&mut self, arn: &str, now: SystemTime) -> Option<SupportPermitRecord> {
        let record = self.support_permits.get_mut(arn)?;
        if record.removal_due_at.is_none() {
            record.status = STATUS_DELETING.to_string();
            record.removal_due_at = Some(now + DELETION_SPAN);
        }
        Some(record.clone())
    }

    /// Drop every permit whose removal is due at `now`, and everything indexed
    /// against it; returns how many left the store.
    ///
    /// This is the transition the clock already owed at `now` — it would have
    /// happened whether or not anybody called — which is why even a reporting
    /// operation materialises it.
    pub fn purge_due(&mut self, now: SystemTime) -> usize {
        let due: Vec<String> = self
            .support_permits
            .values()
            .filter(|record| record.removal_is_due_at(now))
            .map(|record| record.arn.clone())
            .collect();
        for arn in &due {
            self.remove(arn);
        }
        due.len()
    }

    /// Remove the permit stored under `arn`, together with its name index entry,
    /// every page cursor that resumed at it, and any `clientToken` that replayed
    /// to it — a replay must not hand back a permit that no longer exists.
    fn remove(&mut self, arn: &str) -> Option<SupportPermitRecord> {
        let record = self.support_permits.remove(arn)?;
        self.permit_names.remove(&record.name);
        self.idempotency.retain(|_, replay| replay.arn != arn);
        if let Some(token) = self.tokens_by_position.remove(arn) {
            self.list_cursors.remove(&token);
        }
        Some(record)
    }

    /// How many permits are stored — what `CreateSupportPermit` checks against
    /// [`MAX_SUPPORT_PERMITS`].
    pub fn len(&self) -> usize {
        self.support_permits.len()
    }

    /// Whether no permit is stored.
    pub fn is_empty(&self) -> bool {
        self.support_permits.is_empty()
    }

    /// What `client_token` replays to, if it has been used before.
    pub fn create_replay(&self, client_token: &str) -> Option<&CreateReplay> {
        self.idempotency.get(client_token)
    }

    /// Remember that `client_token` created `arn` from the request `fingerprint`
    /// describes.
    pub fn remember_create(&mut self, client_token: &str, arn: &str, fingerprint: String) {
        self.idempotency.insert(
            client_token.to_string(),
            CreateReplay {
                arn: arn.to_string(),
                fingerprint,
            },
        );
    }

    /// One page of permit summaries in stable ARN order.
    ///
    /// `after` is the last ARN the previous page handed out (`None` for the first
    /// page); `statuses`, when given, keeps only permits whose status is in it.
    /// The second element is the ARN a `nextToken` would resume at — `Some` only
    /// when a matching permit remains BEYOND this page, so the last page carries
    /// no token.
    pub fn page(
        &self,
        after: Option<&str>,
        statuses: Option<&[String]>,
        limit: usize,
    ) -> (Vec<SupportPermitSummary>, Option<String>) {
        let mut matching = self
            .support_permits
            .iter()
            .filter(|(arn, _)| after.is_none_or(|cursor| arn.as_str() > cursor))
            .filter(|(_, record)| {
                statuses.is_none_or(|wanted| wanted.iter().any(|s| s == &record.status))
            })
            .map(|(_, record)| record);

        let page: Vec<SupportPermitSummary> = matching
            .by_ref()
            .take(limit)
            .map(SupportPermitRecord::summary)
            .collect();
        let resume = match matching.next() {
            Some(_) => page.last().map(|summary| summary.arn.clone()),
            None => None,
        };
        (page, resume)
    }

    /// The opaque `nextToken` that resumes after `after_arn`, minting one only
    /// the first time that position is handed out.
    pub fn token_for(&mut self, after_arn: &str) -> String {
        if let Some(token) = self.tokens_by_position.get(after_arn) {
            return token.clone();
        }
        // Hex digits only, so the token satisfies the modeled `NextToken`
        // pattern `^[a-zA-Z0-9/+=%_-]+$`.
        let token = format!("{:032x}", self.next_token_seq);
        self.next_token_seq += 1;
        self.list_cursors
            .insert(token.clone(), after_arn.to_string());
        self.tokens_by_position
            .insert(after_arn.to_string(), token.clone());
        token
    }

    /// Resolve a client-supplied `nextToken` to the ARN it resumes after.
    /// `None` means no live cursor backs it, which the operation refuses.
    pub fn cursor_for(&self, token: &str) -> Option<&str> {
        self.list_cursors.get(token).map(String::as_str)
    }

    /// Merge `tags` onto the permit stored under `arn`, overwriting the value of
    /// a key already present.
    ///
    /// ALL-OR-NOTHING: a merge that would leave the permit carrying more than
    /// [`MAX_TAGS_PER_PERMIT`] distinct keys writes nothing, so a stream of calls
    /// with fresh keys cannot grow one permit without limit. The modelled `Tags`
    /// bound caps one request; this caps the permit.
    pub fn merge_tags<I>(&mut self, arn: &str, tags: I) -> TagMerge
    where
        I: IntoIterator<Item = (String, String)>,
    {
        let tags: Vec<(String, String)> = tags.into_iter().collect();
        let Some(record) = self.support_permits.get_mut(arn) else {
            return TagMerge::NotFound;
        };
        let merged: BTreeSet<&str> = record
            .tags
            .keys()
            .map(String::as_str)
            .chain(tags.iter().map(|(key, _)| key.as_str()))
            .collect();
        let count = merged.len();
        if count > MAX_TAGS_PER_PERMIT {
            return TagMerge::TooManyTags(count);
        }
        drop(merged);
        record.tags.extend(tags);
        TagMerge::Merged(count)
    }

    /// Remove `keys` (and nothing else) from the permit stored under `arn`. A
    /// key that is not present is not an error. `false` when no permit is stored
    /// there.
    pub fn remove_tags<'k, I>(&mut self, arn: &str, keys: I) -> bool
    where
        I: IntoIterator<Item = &'k str>,
    {
        match self.support_permits.get_mut(arn) {
            Some(record) => {
                for key in keys {
                    record.tags.remove(key);
                }
                true
            }
            None => false,
        }
    }
}

/// The ARN of a support permit:
/// `arn:aws:supportauthz:<region>:<account>:support-permit/<name>`.
pub fn support_permit_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("support-permit/{name}"))
}

/// The region this request was signed for, defaulting to [`DEFAULT_REGION`] when
/// the request carries no SigV4 credential scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Now, as the wire timestamp type every modelled timestamp member uses.
pub fn now() -> DateTime {
    DateTime::from(SystemTime::now())
}

/// A stable, unambiguous rendering of a `CreateSupportPermit` body.
///
/// Used only to decide an `@idempotencyToken` replay: the same token with the
/// same body replays the original permit, the same token with a different body
/// is a conflict. Every variable-length part is length-prefixed, so two
/// different bodies cannot render to one string (`["a,b"]` and `["a","b"]` do
/// not collide).
pub fn create_fingerprint(
    name: &str,
    description: Option<&str>,
    permit: &Permit,
    signing_key_info: &SigningKeyInfo,
    support_case_display_id: Option<&str>,
    tags: &BTreeMap<String, String>,
) -> String {
    let mut out = String::new();
    push_field(&mut out, name);
    push_option(&mut out, description);
    push_permit(&mut out, permit);
    match signing_key_info {
        SigningKeyInfo::KmsKey(key) => {
            out.push_str("kmsKey");
            push_field(&mut out, key);
        }
    }
    push_option(&mut out, support_case_display_id);
    push_field(&mut out, &tags.len().to_string());
    for (key, value) in tags {
        push_field(&mut out, key);
        push_field(&mut out, value);
    }
    out
}

fn push_field(out: &mut String, value: &str) {
    out.push_str(&value.len().to_string());
    out.push(':');
    out.push_str(value);
    out.push(';');
}

fn push_option(out: &mut String, value: Option<&str>) {
    match value {
        Some(value) => {
            out.push('+');
            push_field(out, value);
        }
        None => out.push('-'),
    }
}

/// An instant, to the full precision the wire carries.
fn instant_fingerprint(at: &DateTime) -> String {
    format!("{}.{:09}", at.secs(), at.subsec_nanos())
}

fn push_permit(out: &mut String, permit: &Permit) {
    match &permit.actions {
        ActionSet::AllActions(_) => out.push_str("allActions;"),
        ActionSet::Actions(actions) => {
            out.push_str("actions");
            push_field(out, &actions.len().to_string());
            for action in actions {
                push_field(out, action);
            }
        }
    }
    match &permit.resources {
        ResourceSet::AllResourcesInRegion(_) => out.push_str("allResourcesInRegion;"),
        ResourceSet::Resources(resources) => {
            out.push_str("resources");
            push_field(out, &resources.len().to_string());
            for resource in resources {
                push_field(out, resource);
            }
        }
    }
    match &permit.conditions {
        None => out.push('-'),
        Some(conditions) => {
            push_field(out, &conditions.len().to_string());
            for condition in conditions {
                match condition {
                    Condition::AllowAfter(at) => {
                        out.push_str("allowAfter");
                        push_field(out, &instant_fingerprint(at));
                    }
                    Condition::AllowBefore(at) => {
                        out.push_str("allowBefore");
                        push_field(out, &instant_fingerprint(at));
                    }
                }
            }
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSupportauthzSupportauthzState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSupportauthzSupportauthzState>)
}
