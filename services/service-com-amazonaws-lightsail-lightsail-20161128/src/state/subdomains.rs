// SPDX-License-Identifier: CC0-1.0

//! Lightsail subdomains: the name the account holds under
//! `amazonlightsail.com`, the resource each one routes traffic to, and the
//! short-lived reservations a check holds a free name with.
//!
//! The store's own accessors — the reads a reporting operation renders from,
//! the quotas a create and a reservation are bounded by, and the door a
//! subdomain is created and removed through — live in [`store`].

use super::*;

/// How the store reaches these records.
mod store;

/// The `resourceType` a subdomain and its operations report, from the modelled
/// `ResourceType` enum's own `Subdomain` member.
pub const RESOURCE_TYPE_SUBDOMAIN: &str = "Subdomain";

/// The resource portion a subdomain ARN carries before its id. Mirrors the
/// `Instance/<uuid>` form the modelled `Instance$arn` documentation shows, with
/// this resource's own `ResourceType` value.
pub const SUBDOMAIN_ARN_RESOURCE_PREFIX: &str = "Subdomain/";

/// The domain every Lightsail subdomain is published under.
///
/// From the modelled `Subdomain` documentation: "Its web address is the
/// subdomain followed by `amazonlightsail.com`, such as
/// `blog.mysite.amazonlightsail.com`."
pub const SUBDOMAIN_WEB_ADDRESS_DOMAIN: &str = "amazonlightsail.com";

/// The Region a subdomain lives in, and the one its API is served from.
///
/// A subdomain is published under one global domain, so it is not a zonal
/// resource; and its calls carry the same restriction the DNS zone and the CDN
/// distribution calls do (the modelled `InvalidInputException` documentation
/// covers "Domain and distribution APIs" together), which is
/// [`DOMAIN_REGION_NAME`] — see [`domain_region_refusal`].
pub const SUBDOMAIN_REGION_NAME: &str = DOMAIN_REGION_NAME;

/// `Subdomain$state` values, from the modelled `SubdomainState` enum and that
/// member's own documentation.
///
/// A subdomain Lightsail is still creating is `CREATING`; one that is ready and
/// associated with nothing is `UNATTACHED`; one being associated with a resource
/// is `ATTACHING` and one associated with it `ATTACHED`; one being
/// disassociated is `DETACHING` and one being moved from one resource to another
/// `TRANSFERRING`; and a name the account holds only because a nested subdomain
/// of it was created is `IMPLICIT`. The enum's remaining member,
/// `CREATE_FAILED`, describes a subdomain whose creation failed, which this
/// store cannot produce — nothing here fails a create.
pub const SUBDOMAIN_STATE_CREATING: &str = "CREATING";
/// See [`SUBDOMAIN_STATE_CREATING`].
pub const SUBDOMAIN_STATE_UNATTACHED: &str = "UNATTACHED";
/// See [`SUBDOMAIN_STATE_CREATING`].
pub const SUBDOMAIN_STATE_ATTACHING: &str = "ATTACHING";
/// See [`SUBDOMAIN_STATE_CREATING`].
pub const SUBDOMAIN_STATE_ATTACHED: &str = "ATTACHED";
/// See [`SUBDOMAIN_STATE_CREATING`].
pub const SUBDOMAIN_STATE_DETACHING: &str = "DETACHING";
/// See [`SUBDOMAIN_STATE_CREATING`].
pub const SUBDOMAIN_STATE_TRANSFERRING: &str = "TRANSFERRING";
/// See [`SUBDOMAIN_STATE_CREATING`].
pub const SUBDOMAIN_STATE_IMPLICIT: &str = "IMPLICIT";

/// `LightsailDistributionSubdomain$state` values, from the modelled
/// `LightsailDistributionSubdomainStateEnum` and that member's documentation: a
/// subdomain the service is still creating for the distribution reports
/// `Creating`, one being associated with it `Attaching`, and one associated with
/// it `Attached`. The enum's fourth member, `Error`, describes a subdomain that
/// could not be created or associated, which this store cannot produce.
pub const DISTRIBUTION_SUBDOMAIN_STATE_CREATING: &str = "Creating";
/// See [`DISTRIBUTION_SUBDOMAIN_STATE_CREATING`].
pub const DISTRIBUTION_SUBDOMAIN_STATE_ATTACHING: &str = "Attaching";
/// See [`DISTRIBUTION_SUBDOMAIN_STATE_CREATING`].
pub const DISTRIBUTION_SUBDOMAIN_STATE_ATTACHED: &str = "Attached";

/// The `SubdomainAssociationAction` values, from the modelled enum: associate a
/// subdomain with a resource, disassociate it from the one it is associated
/// with, or move it from that resource to another one.
pub const SUBDOMAIN_ACTION_ASSOCIATE: &str = "ASSOCIATE";
/// See [`SUBDOMAIN_ACTION_ASSOCIATE`].
pub const SUBDOMAIN_ACTION_DISASSOCIATE: &str = "DISASSOCIATE";
/// See [`SUBDOMAIN_ACTION_ASSOCIATE`].
pub const SUBDOMAIN_ACTION_TRANSFER: &str = "TRANSFER";

/// The most subdomains one account holds.
///
/// An AWS QUOTA, kept at the published value: "You can have up to 30 subdomains
/// in your account. Nested subdomains, subdomains in the `CREATE_FAILED` state,
/// and deleted subdomains that other accounts can't create yet count toward this
/// quota. Subdomains in the `IMPLICIT` state don't count toward this quota" (the
/// modelled `CreateSubdomain` documentation) — which is why
/// [`ComAmazonawsLightsailLightsail20161128State::subdomains_toward_quota`]
/// counts every record but the implicit ones.
pub const MAX_SUBDOMAINS: usize = 30;

/// The most reservations of a free subdomain one account holds at a time.
///
/// An AWS QUOTA, kept at the published value: "You can have up to 5 active
/// reservations at a time" (the modelled
/// `CheckSubdomainAvailabilityRequest$reserve` documentation).
pub const MAX_SUBDOMAIN_RESERVATIONS: usize = 5;

/// How long a reservation holds a free subdomain for the account.
///
/// An AWS DURATION, kept at the published value: "A reservation holds the
/// subdomain for 15 minutes so that other accounts can't create it. Checking the
/// subdomain again doesn't extend the reservation" (the modelled
/// `CheckSubdomainAvailabilityRequest$reserve` documentation).
pub const SUBDOMAIN_RESERVATION_MILLIS: i64 = 15 * 60 * 1_000;

/// How long after a transfer the next one of the same subdomain is allowed.
///
/// An AWS DURATION, kept at the published value: "You can transfer each
/// subdomain to another resource only once in a 24-hour period" (the modelled
/// `UpdateSubdomainAssociation` documentation).
pub const SUBDOMAIN_TRANSFER_INTERVAL_MILLIS: i64 = 24 * 60 * 60 * 1_000;

/// The longest subdomain name Lightsail accepts: "The name can be up to 39
/// characters long, including periods" (the modelled `subdomainName`
/// documentation).
pub const MAX_SUBDOMAIN_NAME_CHARS: usize = 39;

/// The most labels a subdomain name carries: "A subdomain can contain one or two
/// labels separated by a period (.)" (the modelled `subdomainName`
/// documentation) — which is also the largest `Subdomain$depth` a name has.
pub const MAX_SUBDOMAIN_LABELS: usize = 2;

/// The label prefix Lightsail does not serve: "Labels that start with `xn--`
/// aren't supported" (the modelled `subdomainName` documentation).
pub const SUBDOMAIN_UNSUPPORTED_LABEL_PREFIX: &str = "xn--";

/// How many subdomains one `GetSubdomains` page carries.
///
/// The operation is paginated — "The results are paginated. To get the next page
/// of results, use the `pageToken` parameter" — but the model states no page
/// size, so this is the store's own, small enough that a modest number of
/// subdomains already exercises the paging.
pub const SUBDOMAIN_PAGE_SIZE: usize = 10;

/// The ARN of the subdomain `id`. Every subdomain is published under one global
/// domain and served from [`SUBDOMAIN_REGION_NAME`], so that is the Region its
/// ARN carries — exactly as a distribution's does.
pub fn subdomain_arn(id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        SUBDOMAIN_REGION_NAME,
        &format!("{SUBDOMAIN_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The web address the subdomain `name` is reachable at: the subdomain followed
/// by [`SUBDOMAIN_WEB_ADDRESS_DOMAIN`], which is `Subdomain$fqdn`.
pub fn subdomain_fqdn(name: &str) -> String {
    format!("{name}.{SUBDOMAIN_WEB_ADDRESS_DOMAIN}")
}

/// The labels of the subdomain `name`, outermost first — the parts a period
/// separates, which is what its depth counts and what its parent is read from.
fn subdomain_labels(name: &str) -> Vec<&str> {
    name.split('.').collect()
}

/// `Subdomain$depth`: "The number of labels in the subdomain. A top-level
/// subdomain, such as `mysite`, has a depth of `1`. A nested subdomain, such as
/// `blog.mysite`, has a depth of `2`."
pub fn subdomain_depth(name: &str) -> i32 {
    // At most [`MAX_SUBDOMAIN_LABELS`] labels pass validation, so the count is
    // always within the modelled integer.
    i32::try_from(subdomain_labels(name).len()).unwrap_or(i32::MAX)
}

/// The name of the subdomain `name` is nested in — `mysite` for `blog.mysite` —
/// and `None` for a top-level subdomain, which is nested in nothing.
pub fn subdomain_parent_name(name: &str) -> Option<&str> {
    name.split_once('.').map(|(_, parent)| parent)
}

/// The TOP-LEVEL subdomain a reservation of `name` holds: "For a nested
/// subdomain, such as `blog.mysite`, the top-level subdomain (`mysite`) is
/// reserved" (the modelled `reserve` documentation).
pub fn subdomain_reserved_name(name: &str) -> &str {
    subdomain_parent_name(name).unwrap_or(name)
}

/// The refusal for a `member` that does not carry a Lightsail subdomain name,
/// and `None` when it does — the crate's ONE owner of the name rule, asked by
/// every call that takes a subdomain name.
///
/// The rule is the modelled `subdomainName` documentation, which states what the
/// free-form `string` member cannot: "A subdomain can contain one or two labels
/// separated by a period (.). Each label must start and end with a lowercase
/// letter or a number, and can contain only lowercase letters, numbers, and
/// hyphens (-). The name can be up to 39 characters long, including periods.
/// Labels that start with `xn--` aren't supported … Specify only the subdomain.
/// A web address, such as `mysite.amazonlightsail.com`, isn't accepted."
pub fn subdomain_name_refusal(member: &str, name: &str) -> Option<String> {
    let labels = subdomain_labels(name);
    let well_formed = (1..=MAX_SUBDOMAIN_NAME_CHARS).contains(&name.chars().count())
        && (1..=MAX_SUBDOMAIN_LABELS).contains(&labels.len())
        && labels.iter().all(|label| {
            !label.is_empty()
                && !label.starts_with(SUBDOMAIN_UNSUPPORTED_LABEL_PREFIX)
                && label
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                && [label.chars().next(), label.chars().next_back()]
                    .into_iter()
                    .flatten()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        });
    (!well_formed).then(|| {
        format!(
            "{member} '{name}' is not a Lightsail subdomain: it carries one or two labels \
             separated by a period, each starting and ending with a lowercase letter or a \
             number and holding only lowercase letters, numbers and hyphens, is at most \
             {MAX_SUBDOMAIN_NAME_CHARS} characters long including periods, and no label of it \
             starts with '{SUBDOMAIN_UNSUPPORTED_LABEL_PREFIX}'. Name the subdomain alone, not \
             its web address."
        )
    })
}

/// How a refusal words the condition "the subdomain's own state does not allow
/// this call", given the call's verb, the subdomain and the state it is in.
///
/// The ONE owner of that wording, like [`disk_blocked_by_state_message`]: every
/// subdomain call its subdomain's state blocks reports the condition the same
/// way, so the association calls and the delete cannot word one condition three
/// different ways.
pub fn subdomain_blocked_by_state_message(verb: &str, name: &str, state: &str) -> String {
    format!("You can't {verb} the subdomain {name} right now. Its state is: {state}.")
}

/// The resource a subdomain is associated with, or is being associated with: the
/// resource's own ARN and name, and the instant the association was made.
#[derive(Debug, Clone)]
pub struct StoredSubdomainAttachment {
    pub arn: String,
    pub name: String,
    /// The instant the subdomain became associated with the resource — the
    /// `attachedAt` a read reports. On an association still being made it is the
    /// instant the call that started it was made, and it is replaced by the
    /// instant the association fell due when the clock completes it.
    pub attached_at_ms: i64,
}

impl StoredSubdomainAttachment {
    /// The resource `name` with ARN `arn`, associated at `now_ms`.
    pub fn new(arn: &str, name: &str, now_ms: i64) -> Self {
        Self {
            arn: arn.to_string(),
            name: name.to_string(),
            attached_at_ms: now_ms,
        }
    }

    /// This association as the modelled `SubdomainAttachableResource`.
    pub fn view(&self) -> SubdomainAttachableResource {
        SubdomainAttachableResource {
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            attached_at: Some(timestamp(self.attached_at_ms)),
        }
    }
}

/// One stored subdomain: the name `CreateSubdomain` created, or the top-level
/// name the account holds only because a nested subdomain of it was created.
///
/// What the caller asked for — the name and the tags its create carried — is
/// kept as asked for; the ARN, the creation time, the web address, the depth,
/// the state and the resource the subdomain routes traffic to are assigned by
/// this store from the request plus its own allocators.
#[derive(Debug, Clone)]
pub struct StoredSubdomain {
    /// The subdomain name the request named, kept verbatim — it is this record's
    /// store key and the `subdomainName` every read reports.
    pub name: String,
    /// `None` on an IMPLICIT subdomain: "If you create a subdomain with a depth
    /// of 2, such as `blog.mysite`, without first creating the subdomain with a
    /// depth of 1 (`mysite`), Lightsail holds `mysite` for your account in the
    /// `IMPLICIT` state. It doesn't have an ARN until you create a subdomain
    /// with that name" (the modelled `Subdomain$arn` documentation).
    pub arn: Option<String>,
    /// When the account came to hold this name: the instant of the create for a
    /// subdomain a caller created, and the instant the nested subdomain under it
    /// was created (or the instant the delete left it behind) for an implicit
    /// one.
    pub created_at_ms: i64,
    pub location: StoredLocation,
    /// One of the [`SUBDOMAIN_STATE_CREATING`] family.
    pub state: &'static str,
    /// The state the clock owes this subdomain, while one is outstanding.
    pub due: Option<DueNamedState>,
    /// The resource the subdomain is associated with — and, while it is being
    /// moved, the resource it is being moved FROM: "When the subdomain is in the
    /// `TRANSFERRING` state, this is the resource that the subdomain is being
    /// moved from" (the modelled `Subdomain$attachedResource` documentation).
    pub attached: Option<StoredSubdomainAttachment>,
    /// The resource the subdomain is being associated with, reported "only when
    /// the subdomain is in the `ATTACHING` or `TRANSFERRING` state" (the
    /// modelled `Subdomain$attachingResource` documentation) — and while it is
    /// still being created for one, which is the state a distribution's own
    /// create leaves it in.
    pub attaching: Option<StoredSubdomainAttachment>,
    /// When this subdomain was last moved from one resource to another; `None`
    /// on one that has never been moved. A transfer is allowed "only once in a
    /// 24-hour period", so the instant of the last one is what says whether
    /// another is available — see [`Self::transfer_available_at`].
    pub transferred_at_ms: Option<i64>,
    pub tags: Vec<Tag>,
}

impl StoredSubdomain {
    /// The subdomain a `CreateSubdomain` at `now_ms` creates: Lightsail is
    /// creating it, and the clock owes it the state `settles_into` — ready and
    /// associated with nothing, or, for the subdomain a distribution's own
    /// create asks for, the association that create started.
    ///
    /// The span is non-zero, so a caller that creates and reads straight back
    /// still observes the subdomain being created, exactly as it does against
    /// AWS.
    pub fn created(
        name: &str,
        arn: String,
        now_ms: i64,
        tags: Vec<Tag>,
        due: DueNamedState,
    ) -> Self {
        Self {
            name: name.to_string(),
            arn: Some(arn),
            created_at_ms: now_ms,
            location: StoredLocation::regional(SUBDOMAIN_REGION_NAME),
            state: SUBDOMAIN_STATE_CREATING,
            due: Some(due),
            attached: None,
            attaching: None,
            transferred_at_ms: None,
            tags,
        }
    }

    /// The top-level name the account holds because a nested subdomain of it
    /// exists, which "doesn't have an ARN until you create a subdomain with that
    /// name" — and which no call can associate or delete.
    pub fn implicit(name: &str, now_ms: i64) -> Self {
        Self {
            name: name.to_string(),
            arn: None,
            created_at_ms: now_ms,
            location: StoredLocation::regional(SUBDOMAIN_REGION_NAME),
            state: SUBDOMAIN_STATE_IMPLICIT,
            due: None,
            attached: None,
            attaching: None,
            transferred_at_ms: None,
            tags: Vec::new(),
        }
    }

    /// Whether this record is the top-level name the account holds only because
    /// a nested subdomain of it was created: "You can't associate or delete a
    /// subdomain in this state."
    pub fn is_implicit(&self) -> bool {
        self.state == SUBDOMAIN_STATE_IMPLICIT
    }

    /// The instant the next transfer of this subdomain is allowed at, and `None`
    /// when one is allowed now — the ONE owner of the 24-hour rule.
    pub fn transfer_available_at(&self, now_ms: i64) -> Option<i64> {
        let available_at = self
            .transferred_at_ms?
            .saturating_add(SUBDOMAIN_TRANSFER_INTERVAL_MILLIS);
        (available_at > now_ms).then_some(available_at)
    }

    /// Record the association of this subdomain with `resource` as asked for at
    /// `now_ms`: the subdomain is being associated with it, and the clock owes
    /// the association.
    ///
    /// `transferring` says the subdomain is being MOVED from the resource it is
    /// already associated with, which is the state it then reports and the
    /// instant the 24-hour transfer window restarts from — the one place both
    /// are written, so an associate and a transfer cannot drift apart.
    pub fn associate_with(
        &mut self,
        resource: StoredSubdomainAttachment,
        transferring: bool,
        now_ms: i64,
    ) -> DueNamedState {
        let due = DueNamedState::after(now_ms, SUBDOMAIN_STATE_ATTACHED);
        self.attaching = Some(resource);
        self.state = if transferring {
            self.transferred_at_ms = Some(now_ms);
            SUBDOMAIN_STATE_TRANSFERRING
        } else {
            SUBDOMAIN_STATE_ATTACHING
        };
        self.due = Some(due);
        due
    }

    /// Record that this subdomain is being disassociated from the resource it is
    /// associated with: it is `DETACHING`, and the clock owes it the state of a
    /// subdomain that is ready and associated with nothing.
    pub fn disassociate(&mut self, now_ms: i64) -> DueNamedState {
        let due = DueNamedState::after(now_ms, SUBDOMAIN_STATE_UNATTACHED);
        self.state = SUBDOMAIN_STATE_DETACHING;
        self.due = Some(due);
        due
    }

    /// Materialise the state the clock had already made due at `now_ms`.
    ///
    /// A subdomain that was being created for a resource is created, and is then
    /// being associated with it — one span per step, because the association can
    /// only start once the subdomain exists. Settling into `ATTACHED` is what
    /// makes the association the call asked for the one the subdomain reports,
    /// and settling into `UNATTACHED` is what releases it.
    pub fn advance(&mut self, now_ms: i64) {
        let Some(due) = self.due.filter(|due| due.at_ms <= now_ms) else {
            return;
        };
        self.state = due.state;
        self.due = None;
        if self.state == SUBDOMAIN_STATE_ATTACHING {
            self.due = Some(DueNamedState::after(due.at_ms, SUBDOMAIN_STATE_ATTACHED));
        } else if self.state == SUBDOMAIN_STATE_ATTACHED {
            if let Some(attaching) = self.attaching.take() {
                self.attached = Some(StoredSubdomainAttachment {
                    attached_at_ms: due.at_ms,
                    ..attaching
                });
            }
        } else if self.state == SUBDOMAIN_STATE_UNATTACHED {
            self.attached = None;
            self.attaching = None;
        }
    }

    /// Whether this subdomain is the one the resource `resource_arn` is
    /// reachable at, or is being made reachable at.
    pub fn routes_to(&self, resource_arn: &str) -> bool {
        [self.attached.as_ref(), self.attaching.as_ref()]
            .into_iter()
            .flatten()
            .any(|resource| resource.arn == resource_arn)
    }

    /// This subdomain as the `subdomain` the resource `resource_arn` reports,
    /// and `None` when that resource is not reachable at this subdomain.
    ///
    /// The resource's member carries the subdomain's progress TOWARDS it: a
    /// subdomain still being created for it is `Creating`, one being associated
    /// with it `Attaching`, and one associated with it `Attached`. A subdomain
    /// LEAVING the resource — being disassociated from it, or moved to another
    /// one — reports nothing for it: the states that member models are the steps
    /// towards an association, so the resource reports no subdomain from the
    /// instant the subdomain starts leaving.
    pub fn resource_subdomain_view(
        &self,
        resource_arn: &str,
    ) -> Option<LightsailDistributionSubdomain> {
        let state = if self
            .attaching
            .as_ref()
            .is_some_and(|resource| resource.arn == resource_arn)
        {
            if self.state == SUBDOMAIN_STATE_CREATING {
                DISTRIBUTION_SUBDOMAIN_STATE_CREATING
            } else {
                DISTRIBUTION_SUBDOMAIN_STATE_ATTACHING
            }
        } else if self
            .attached
            .as_ref()
            .is_some_and(|resource| resource.arn == resource_arn)
            && self.state == SUBDOMAIN_STATE_ATTACHED
        {
            DISTRIBUTION_SUBDOMAIN_STATE_ATTACHED
        } else {
            return None;
        };
        Some(LightsailDistributionSubdomain {
            name: self.name.clone(),
            state: state.to_string(),
            // Nothing here fails a subdomain's creation or association, so no
            // resource reports the `Error` state's error code or details.
            error_code: None,
            error_details: None,
        })
    }

    /// This record as the modelled `Subdomain`, with `children` the nested
    /// subdomains of it to report and `with_tags` saying whether its tags are.
    ///
    /// Both are the reporting operation's to decide, because the model says so:
    /// `children` is "returned only by the `GetSubdomains` action when you don't
    /// specify a subdomain" and `tags` "only by the `GetSubdomains` action when
    /// you specify a subdomain".
    pub fn view(&self, children: Option<Vec<Subdomain>>, with_tags: bool) -> Subdomain {
        Subdomain {
            subdomain_name: Some(self.name.clone()),
            arn: self.arn.clone(),
            fqdn: Some(subdomain_fqdn(&self.name)),
            depth: Some(subdomain_depth(&self.name)),
            state: Some(self.state.to_string()),
            attached_resource: self
                .attached
                .as_ref()
                .map(StoredSubdomainAttachment::view),
            attaching_resource: self
                .attaching
                .as_ref()
                .map(StoredSubdomainAttachment::view),
            location: Some(self.location.view()),
            created_at: Some(timestamp(self.created_at_ms)),
            // "The web address of the parent subdomain … This parameter isn't
            // returned for a subdomain with a depth of 1."
            parent_fqdn: subdomain_parent_name(&self.name).map(subdomain_fqdn),
            children,
            tags: with_tags.then(|| self.tags.clone()),
        }
    }
}

/// A subdomain carries tags, an ARN and a location, so a tag change reaches it
/// the way it reaches every other tag-carrying Lightsail resource.
///
/// Written out rather than taken from the `tagged_records!` macro because an
/// IMPLICIT subdomain holds no ARN — it "doesn't have an ARN until you create a
/// subdomain with that name" — so the ARN a tag operation may name this record
/// by is the one it has, and no name at all while it has none.
impl TaggedRecord for StoredSubdomain {
    fn arn(&self) -> &str {
        self.arn.as_deref().unwrap_or_default()
    }

    fn location(&self) -> &StoredLocation {
        &self.location
    }

    fn tags(&self) -> &[Tag] {
        &self.tags
    }

    fn set_tags(&mut self, tags: Vec<Tag>) {
        self.tags = tags;
    }
}
