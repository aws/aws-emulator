// SPDX-License-Identifier: CC0-1.0

//! The managed rule groups this service publishes: what each vendor offers, the
//! versions it offers them in, and the rules inside them.

use super::*;

/// One managed rule group as its vendor publishes it: the pair that identifies it
/// and the capacity a web ACL spends to run it.
///
/// A managed rule group is NOT a resource of this account — nothing creates,
/// updates or deletes one — so it lives in a constant catalogue rather than in
/// [`ComAmazonawsWafv2Awswaf20190729State`], which holds only what this account
/// owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManagedRuleGroupDescriptor {
    /// The vendor that publishes the group
    /// (`ManagedRuleGroupStatement$VendorName`).
    pub vendor_name: &'static str,
    /// The group's name (`ManagedRuleGroupStatement$Name`).
    pub name: &'static str,
    /// The group's PUBLISHED slug: the group segment of the namespace every label its
    /// rules add carries ([`Self::managed_label_namespace`]).
    ///
    /// Reference data the VENDOR publishes, and NOT derivable from [`Self::name`] —
    /// the WAF Developer Guide's rule-group list states each group's label namespace
    /// beside its API name, and the two differ (the core rule set's labels sit under
    /// `core-rule-set`, the SQL-database group's under `sql-database`), which is why
    /// the slug is carried here rather than computed.
    pub label_slug: &'static str,
    /// The WCUs a web ACL spends on one rule that runs this group; see
    /// [`web_acl_capacity`].
    pub capacity: i64,
    /// What the group's publisher says it does, in the words it publishes for the
    /// CATALOGUE entry — `ManagedRuleGroupSummary$Description` is "the description
    /// of the managed rule group, provided by Amazon Web Services Managed Rules or
    /// the Amazon Web Services Marketplace seller who manages it", so the text is
    /// the publisher's and is FIXED per group rather than generated.
    ///
    /// Carried beside [`Self::product_description`] rather than shared with it,
    /// because the two members the model declares are two different published
    /// texts: this one is the catalogue entry's description
    /// (`EntityDescription`), and the product entry asks for "a SHORT description
    /// of the managed rule group" (`ManagedProductDescriptor$ProductDescription`),
    /// which is the one-line summary a product listing carries. A publisher that
    /// states both states them separately, so neither is derived from the other.
    pub description: &'static str,
    /// The group's display name (`ManagedProductDescriptor$ProductTitle` is "the
    /// display name for the managed rule group"), as the WAF Developer Guide's
    /// rule-group list titles it beside its `Name`.
    pub product_title: &'static str,
    /// What the group protects against, in one sentence
    /// (`ManagedProductDescriptor$ProductDescription` is "a short description of
    /// the managed rule group"), from the same published list as
    /// [`Self::product_title`].
    pub product_description: &'static str,
    /// Whether the vendor offers this group in NAMED versions a web ACL can choose
    /// between (`ManagedRuleGroupSummary$VersioningSupported` is "whether the
    /// managed rule group is versioned. If it is, you can retrieve the versions
    /// list by calling ListAvailableManagedRuleGroupVersions", and
    /// `ManagedProductDescriptor$IsVersioningSupported` reports the same property).
    ///
    /// Reference data the VENDOR publishes per group, carried here and reported as
    /// stored rather than left at the member's modelled default, which is only the
    /// wire default for a summary that carries none: a group the guide publishes a
    /// version history for is offered in named versions and reports `true`, and one
    /// whose vendor keeps its content continuously up to date instead — the
    /// IP-address-list groups ([`unversioned`]) — reports `false`. Independent of
    /// [`Self::is_advanced_managed_rule_set`]: the advanced groups are versioned,
    /// but so are the ordinary rule-set ones ([`descriptor`]).
    pub is_versioning_supported: bool,
    /// Whether the group provides "an advanced set of protections, such as the
    /// the Amazon Web Services Managed Rules rule groups that are used for WAF
    /// intelligent threat mitigation"
    /// (`ManagedProductDescriptor$IsAdvancedManagedRuleSet`): the
    /// intelligent-threat-mitigation groups ([`advanced`]), a narrower set than the
    /// versioned ones.
    pub is_advanced_managed_rule_set: bool,
    /// The versions of this group its vendor offers, OLDEST first.
    ///
    /// "Customers can retrieve the available versions for a managed rule group by
    /// calling `ListAvailableManagedRuleGroupVersions`"
    /// (`smithy_query(query="doc",shape="PutManagedRuleSetVersions")`), and the
    /// versions on offer are the VENDOR's, not anything this account holds — so
    /// they sit here, beside the capacity, rather than in the state struct. Every
    /// catalogued group offers at least its initial version (see
    /// [`MANAGED_RULE_GROUP_CATALOGUE`]).
    pub versions: &'static [ManagedRuleGroupRelease],
    /// The version its vendor currently sets as this group's DEFAULT, when that is
    /// not simply the newest version it offers.
    ///
    /// `ListAvailableManagedRuleGroupVersionsResponse$CurrentDefaultVersion` is
    /// "the name of the version that's currently set as the default", which is the
    /// version a `ManagedRuleGroupStatement` that pins none runs: a property the
    /// VENDOR publishes per group, and not the same thing as the version it
    /// released last — a vendor that keeps a long version ladder on offer can hold
    /// the default at an earlier version so that statements pinning nothing do not
    /// move with every release. `None` means the vendor's default is the newest
    /// version on offer, which is what a group with one version on offer has; see
    /// [`Self::current_default_version`].
    pub default_version: Option<&'static str>,
    /// The versions of this group its vendor has RETIRED, oldest first: ones it
    /// published and has since stopped offering.
    ///
    /// A version is offered for a while and then expires:
    /// `DescribeManagedRuleGroup$VersionName` is "the version of the rule group.
    /// You can only use a version that is not scheduled for expiration", and a
    /// request naming one that has expired is answered with
    /// `WAFExpiredManagedRuleGroupVersionException` — "you can retrieve the
    /// available versions for the managed rule group by calling
    /// `ListAvailableManagedRuleGroupVersions`" (that error's own doc), which
    /// lists [`Self::versions`] alone. So an expired version is pruned from what
    /// the vendor OFFERS while staying KNOWN to the describe: a request naming one
    /// is refused as expired rather than as a version nobody published, which is
    /// what tells a version-pinning caller to roll forward. A group whose vendor
    /// has retired none carries an empty list.
    pub expired_versions: &'static [ManagedRuleGroupRelease],
    /// The rules this group runs, in the order its vendor lists them.
    ///
    /// `DescribeManagedRuleGroup` "provides high-level information for a managed
    /// rule group, including descriptions of the rules" (the operation's own doc),
    /// and `RuleSummary` is "high-level information about a Rule, returned by
    /// operations like DescribeManagedRuleGroup" — a name and the action the rule
    /// takes. Like [`Self::versions`] those are the VENDOR's, not anything this
    /// account holds: nothing creates, changes or removes a rule of a published
    /// group, so they sit in the catalogue beside the capacity.
    pub rules: &'static [PublishedManagedRule],
    /// The labels this group publishes that are NOT in its own namespace, each as the
    /// fully qualified text its publisher states.
    ///
    /// A group whose rules act on WAF's own client-token handling publishes the
    /// vendor-independent token labels beside its own: those labels state what WAF
    /// made of the request's token rather than what one of this group's rules
    /// inspected, so their namespace is WAF's (`awswaf:managed:token:…`) and not this
    /// group's. A group that publishes none carries an empty list.
    pub token_labels: &'static [&'static str],
}

impl ManagedRuleGroupDescriptor {
    /// The version of this group that is currently the default: the one its vendor
    /// publishes as such ([`Self::default_version`]), and otherwise the NEWEST
    /// version it offers.
    ///
    /// `ListAvailableManagedRuleGroupVersionsResponse$CurrentDefaultVersion` is
    /// "the name of the version that's currently set as the default", and
    /// `DescribeManagedRuleGroup$VersionName` says WAF "uses the vendor's default
    /// version" when a request names none. The catalogue keeps each group's
    /// versions in release order, so a group whose vendor publishes no default of
    /// its own defaults to the last of them — which is every group that keeps one
    /// version on offer.
    pub fn current_default_version(&self) -> Option<&'static str> {
        self.default_version
            .or_else(|| self.versions.last().map(|release| release.name))
    }

    /// The versions this group's vendor offers, NEWEST first: the order
    /// `ListAvailableManagedRuleGroupVersions` reports them in, which is the
    /// reverse of the release order the catalogue stores them in.
    ///
    /// A caller enumerates this list to pick a version to pin, and the newest is
    /// what it is offered first; `NextMarker` is "the marker from the prior call",
    /// the name of the last version of the page it hands back, so paging walks the
    /// ladder from the newest version down to the oldest one still offered.
    pub fn versions_newest_first(&self) -> Vec<&'static ManagedRuleGroupRelease> {
        self.versions.iter().rev().collect()
    }

    /// This entry with `versions` on offer, `expired_versions` retired and
    /// `default_version` published as the vendor's default — the version ladder of
    /// a group whose publisher states one, replacing what the constructor assumed.
    pub(super) const fn offering(
        self,
        versions: &'static [ManagedRuleGroupRelease],
        expired_versions: &'static [ManagedRuleGroupRelease],
        default_version: &'static str,
    ) -> Self {
        Self {
            versions,
            expired_versions,
            default_version: Some(default_version),
            ..self
        }
    }

    /// This published group as the managed PRODUCT a describe reports it.
    ///
    /// A managed product IS a published managed rule group —
    /// `DescribeAllManagedProducts` "provides high-level information for the Amazon
    /// Web Services Managed Rules rule groups and Amazon Web Services Marketplace
    /// managed rule groups" (the operation's own doc) — so a descriptor is built
    /// from this catalogue entry rather than from a second list that could fall
    /// out of step with it: the `(VendorName, ManagedRuleSetName)` pair that
    /// identifies the group, its title and description, and its two flags.
    ///
    /// `ProductId` is "a unique identifier for the rule group", and a
    /// vendor-published group is identified by its name within its vendor, so the
    /// name is that identifier. `ProductLink` is left off: it is "for Amazon Web
    /// Services Marketplace managed rule groups only", and this catalogue
    /// publishes no marketplace group. `SnsTopicArn` is the topic "used to provide
    /// notification of changes to the managed rule group": one topic per group, in
    /// the region the group's `scope` is reached through, minted through the
    /// emulator's own ARN builder like every other ARN this service answers.
    pub fn product(&self) -> ManagedProductDescriptor {
        ManagedProductDescriptor {
            vendor_name: Some(self.vendor_name.to_string()),
            managed_rule_set_name: Some(self.name.to_string()),
            product_id: Some(self.name.to_string()),
            product_link: None,
            product_title: Some(self.product_title.to_string()),
            product_description: Some(self.product_description.to_string()),
            sns_topic_arn: Some(self.sns_topic_arn()),
            is_versioning_supported: self.is_versioning_supported,
            is_advanced_managed_rule_set: self.is_advanced_managed_rule_set,
        }
    }

    /// The ARN of the notification topic a change to this group is announced on: the
    /// topic "used to provide notification of changes to the managed rule group"
    /// (`ManagedProductDescriptor$SnsTopicArn`, and
    /// `DescribeManagedRuleGroupResponse$SnsTopicArn` documents the same thing), so
    /// the SAME ARN whichever call reports it.
    ///
    /// Its publisher announces every change to its managed rule groups on ONE topic,
    /// in the one region that topic lives in, so the ARN carries neither the group nor
    /// the scope nor the caller's region: see
    /// [`AWS_MANAGED_RULE_NOTIFICATION_TOPIC`].
    pub fn sns_topic_arn(&self) -> String {
        ::engine::emulator_arn(
            "sns",
            AWS_MANAGED_RULE_NOTIFICATION_REGION,
            AWS_MANAGED_RULE_NOTIFICATION_TOPIC,
        )
    }

    /// The version of this group `requested` names, or the vendor's default when a
    /// request names none — and `None` when the vendor offers no such version.
    ///
    /// `DescribeManagedRuleGroup$VersionName` is "the version of the rule group…
    /// If you don't provide this, WAF uses the vendor's default version", so a
    /// request naming nothing describes [`Self::current_default_version`], and the
    /// versions a request may name are exactly the ones
    /// `ListAvailableManagedRuleGroupVersions` lists for the group — so a version
    /// its vendor has retired is not one of them, and is told apart from a version
    /// nobody published by [`Self::version_state`].
    pub fn offered_version(&self, requested: Option<&str>) -> Option<&'static str> {
        match requested {
            None => self.current_default_version(),
            Some(requested) => self
                .versions
                .iter()
                .find(|release| release.name == requested)
                .map(|release| release.name),
        }
    }

    /// What this group's vendor publishes about the version `requested` names:
    /// whether it is on offer, published and since expired, or nothing the vendor
    /// ever published.
    ///
    /// Those are the three states a version name can be in for a published group,
    /// and the model gives each its own answer: an offered one is described and can
    /// be pinned, an expired one is the modelled
    /// `WAFExpiredManagedRuleGroupVersionException` ("the specified version for the
    /// managed rule group has expired"), and one no vendor published is neither.
    /// Matched exactly, like the `(VendorName, Name)` pair itself: a
    /// `VersionKeyString` is the vendor's own name for the version.
    pub fn version_state(&self, requested: &str) -> ManagedVersionState {
        if self.versions.iter().any(|release| release.name == requested) {
            ManagedVersionState::Offered
        } else if self
            .expired_versions
            .iter()
            .any(|release| release.name == requested)
        {
            ManagedVersionState::Expired
        } else {
            ManagedVersionState::Unknown
        }
    }

    /// This group's rules as a describe reports them: a name and the action the
    /// vendor's rule takes (see [`Self::rules`]).
    pub fn rule_summaries(&self) -> Vec<RuleSummary> {
        self.rules
            .iter()
            .map(|rule| RuleSummary {
                name: Some(rule.name.to_string()),
                action: Some(rule.action.rule_action()),
            })
            .collect()
    }

    /// The labels this group's rules ADD to matching requests, each FULLY QUALIFIED,
    /// deduplicated and in a stable order.
    ///
    /// `DescribeManagedRuleGroupResponse$AvailableLabels` is "the labels that one or
    /// more rules in this rule group add to matching web requests", and what a
    /// VENDOR publishes about its own group is the whole label a matching request
    /// carries — "a fully qualified label is made up of the label namespace from the
    /// rule group … and the label from the rule, separated by a colon"
    /// (`…$LabelNamespace`) — so each entry here is
    /// [`Self::managed_label_namespace`] followed by the label the rule specifies,
    /// which is the text a `LabelMatchStatement` composing against this group is
    /// written with. (A rule group this ACCOUNT owns reports its labels as its own
    /// rules specify them, with its namespace reported separately; see
    /// [`rule_group_available_labels`].) A group's published token labels
    /// ([`Self::token_labels`]) sit in WAF's own namespace and are reported as
    /// published.
    ///
    /// Only the labels this catalogue CITES are reported: a vendor publishes its
    /// label texts separately from its rule list, so a group whose labels this
    /// catalogue cannot cite reports none rather than a text nobody published (see
    /// [`PublishedManagedRule::published_label`]).
    pub fn available_labels(&self) -> Vec<LabelSummary> {
        owned_label_summaries(
            self.qualified(self.rules.iter().filter_map(|rule| rule.label()))
                .chain(self.token_labels.iter().map(|label| (*label).to_string())),
        )
    }

    /// The labels this group's rules MATCH AGAINST, each fully qualified,
    /// deduplicated and in a stable order — the [`Self::available_labels`]
    /// counterpart for `ConsumedLabels`, "the labels that one or more rules in this
    /// rule group match against in label match statements".
    ///
    /// Reference data of the same kind, and reported the same way: a group whose
    /// rules act on the labels the group's own detection adds publishes those labels
    /// in both lists, and a group this catalogue cites no matched label for consumes
    /// none (see [`PublishedManagedRule::matched_label`]).
    pub fn consumed_labels(&self) -> Vec<LabelSummary> {
        owned_label_summaries(self.qualified(
            self.rules
                .iter()
                .filter_map(PublishedManagedRule::consumed_label),
        ))
    }

    /// Each of `labels` under this group's own namespace: the fully qualified text a
    /// matching request carries for a label one of its rules specifies.
    pub(super) fn qualified<'a>(
        &self,
        labels: impl IntoIterator<Item = &'a str>,
    ) -> impl Iterator<Item = String> {
        let namespace = self.managed_label_namespace();
        labels
            .into_iter()
            .map(|label| format!("{namespace}{label}"))
            .collect::<Vec<String>>()
            .into_iter()
    }

    /// `awswaf:managed:aws:<published slug>:` — the prefix every label added by a
    /// rule in this group carries.
    ///
    /// `DescribeManagedRuleGroupResponse$LabelNamespace` states the syntax:
    /// "The syntax for the label namespace prefix for a managed rule group is the
    /// following: `awswaf:managed:<vendor>:<rule group name>`:", and the publishing
    /// vendor states the text in each place: the vendor segment of a group AWS
    /// publishes is [`AWS_MANAGED_RULE_GROUP_LABEL_VENDOR`] and the group segment is
    /// the slug it publishes the group under ([`Self::label_slug`]), not the group's
    /// API name. A customer-owned entity's namespace is built the same way from its
    /// own identity, by [`label_namespace`].
    pub fn managed_label_namespace(&self) -> String {
        format!(
            "awswaf:managed:{AWS_MANAGED_RULE_GROUP_LABEL_VENDOR}:{}:",
            self.label_slug
        )
    }
}

/// The state one version name is in for a published managed rule group, as its
/// vendor publishes it.
///
/// Three states, because the model answers each differently: a version on offer is
/// described and can be pinned, an expired one is the modelled
/// `WAFExpiredManagedRuleGroupVersionException`, and a name no version of the group
/// ever carried is neither of those. See
/// [`ManagedRuleGroupDescriptor::version_state`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedVersionState {
    /// The vendor OFFERS this version: it is one of
    /// [`ManagedRuleGroupDescriptor::versions`], so
    /// `ListAvailableManagedRuleGroupVersions` lists it, a describe reports it and
    /// a `ManagedRuleGroupStatement` may pin it.
    Offered,
    /// The vendor published this version and has since retired it: it is one of
    /// [`ManagedRuleGroupDescriptor::expired_versions`], so it is no longer listed
    /// and every call that resolves it is refused as expired.
    Expired,
    /// No version of this group ever carried this name.
    Unknown,
}

/// One rule of a published managed rule group: its name and the action its vendor
/// has it take.
///
/// Flat and `Copy` like [`ManagedRuleGroupRelease`], because it is published data
/// rather than stored state: a caller cannot add, change or remove one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublishedManagedRule {
    /// The rule's name (`RuleSummary$Name`, an `EntityName`: `length 1..128`,
    /// pattern `^[\w\-]+$`), as the WAF Developer Guide's rule list for the group
    /// publishes it.
    pub name: &'static str,
    /// The action the vendor's rule takes on a request it matches
    /// (`RuleSummary$Action`), as the same list publishes it.
    pub action: PublishedRuleAction,
    /// The label this rule adds to a request it matches, where this catalogue
    /// cites the published text; `None` where it cannot.
    ///
    /// The label as the RULE specifies it, which the group's namespace
    /// ([`ManagedRuleGroupDescriptor::managed_label_namespace`]) qualifies. A vendor
    /// publishes its label texts separately from its rule list, and a label may read
    /// nothing like the rule that adds it, so a label is cited here only where the
    /// published text is known — never derived from the rule's name, which would
    /// report a text nobody published.
    pub published_label: Option<&'static str>,
    /// The label this rule MATCHES AGAINST in a label match statement, where this
    /// catalogue cites one; `None` where the rule matches on no label.
    ///
    /// A group's rules can act on the labels the group's own detection added earlier
    /// in the same evaluation — "labels that have been added to the web request by
    /// rules that have already run" (`LabelMatchStatement`) — and such a label is
    /// published in the group's consumed list as well as its available one. Cited
    /// from the vendor's published text, like [`Self::published_label`].
    pub matched_label: Option<&'static str>,
}

impl PublishedManagedRule {
    /// The label this rule adds to a request it matches, as the rule specifies
    /// it, or `None` where this catalogue cites none for it.
    ///
    /// `LabelSummary` reports "the label as the rule specified it", and a fully
    /// qualified label "is made up of the label namespace from the rule group …
    /// and the label from the rule, separated by a colon"
    /// (`DescribeManagedRuleGroupResponse$LabelNamespace`) — so a cited label
    /// under [`ManagedRuleGroupDescriptor::managed_label_namespace`] is what a
    /// matching request carries. Stable per rule and per call: it is
    /// [`Self::published_label`] and nothing else.
    pub fn label(&self) -> Option<&'static str> {
        self.published_label
    }

    /// The label this rule matches against, or `None` where this catalogue cites
    /// none for it. See [`Self::matched_label`].
    pub fn consumed_label(&self) -> Option<&'static str> {
        self.matched_label
    }
}

/// The action a published managed rule takes on a request it matches.
///
/// Only the actions the published rule lists use: `RuleAction` models six, but a
/// vendor's rule list states `Block`, `Count` or `Challenge` for its rules, and a
/// variant this catalogue cannot cite a rule for would be an action nobody
/// publishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishedRuleAction {
    /// Block the request (`RuleAction$Block`).
    Block,
    /// Count the request (`RuleAction$Count`).
    Count,
    /// Run a silent challenge against the request (`RuleAction$Challenge`).
    Challenge,
}

impl PublishedRuleAction {
    /// This action as the modelled `RuleAction`: the one member the action names,
    /// with no custom response or request handling, which a vendor's own rule
    /// carries none of.
    pub fn rule_action(self) -> RuleAction {
        match self {
            Self::Block => RuleAction {
                block: Some(BlockAction::default()),
                ..RuleAction::default()
            },
            Self::Count => RuleAction {
                count: Some(CountAction::default()),
                ..RuleAction::default()
            },
            Self::Challenge => RuleAction {
                challenge: Some(ChallengeAction::default()),
                ..RuleAction::default()
            },
        }
    }
}

/// One version of a managed rule group as its vendor offers it.
///
/// Flat and `Copy`, like [`ManagedRuleGroupDescriptor`], because it is published
/// data rather than stored state: a caller cannot add, change or remove one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManagedRuleGroupRelease {
    /// The version name, which is what a `ManagedRuleGroupStatement$Version` pins
    /// (`ManagedRuleGroupVersion$Name`, a `VersionKeyString`: `length 1..64`,
    /// pattern `^[\w#:\.\-/]+$`).
    pub name: &'static str,
    /// When the vendor last updated this version's information
    /// (`ManagedRuleGroupVersion$LastUpdateTimestamp`), as an RFC 3339 instant in
    /// UTC — the form [`::aws_smithy_types::DateTime::from_str`] reads with
    /// `Format::DateTime`, so the calendar arithmetic stays in that library.
    ///
    /// A PROPERTY of the published version and not a clock reading: the same
    /// version answers the same instant on every call, and only publishing a new
    /// version changes what this catalogue reports.
    pub last_update: &'static str,
}

impl ManagedRuleGroupRelease {
    /// [`Self::last_update`] as the modelled `Timestamp`, or `None` when the text
    /// is not an instant that library can read — which leaves the optional
    /// `ManagedRuleGroupVersion$LastUpdateTimestamp` member off the response
    /// rather than reporting an instant nobody published.
    pub fn last_update_timestamp(&self) -> Option<::aws_smithy_types::DateTime> {
        ::aws_smithy_types::DateTime::from_str(
            self.last_update,
            ::aws_smithy_types::date_time::Format::DateTime,
        )
        .ok()
    }
}

/// The managed rule groups this emulator publishes, with the capacity of each.
///
/// A managed rule group is identified by `(VendorName, Name)` and "you can
/// retrieve the required names by calling `ListAvailableManagedRuleGroups`"
/// (`ManagedRuleGroupStatement`'s own doc), so the set of groups a save may
/// reference is a CATALOGUE the service publishes rather than anything the request
/// carries. This is the AWS-vendor catalogue as the WAF Developer Guide's "AWS
/// Managed Rules rule groups list" publishes it: each entry's `Name` and its
/// "Capacity units required" (WCU) column, which is the cost that list states for
/// running the group in a web ACL.
///
/// Kept in the guide's own order (baseline groups, then use-case-specific ones,
/// then the IP-reputation and intelligent-threat-mitigation groups), so a new
/// group the vendor publishes is added where the guide lists it.
///
/// One capacity per group: where the guide states a group's cost per
/// CONFIGURATION (Bot Control, whose targeted inspection level costs more than its
/// common one), the entry carries the group's base figure, because nothing here
/// prices a `ManagedRuleGroupConfigs` setting.
///
/// Each entry also carries the VERSIONS its vendor offers of that group (see
/// [`ManagedRuleGroupDescriptor::versions`]), oldest first: every published group
/// offers at least its initial version, which is what
/// `ListAvailableManagedRuleGroupVersions` lists newest first, and the version its
/// vendor sets as the default among them
/// ([`ManagedRuleGroupDescriptor::current_default_version`]).
///
/// And it carries what the group is PUBLISHED as — the description its publisher
/// states for the catalogue entry
/// ([`ManagedRuleGroupDescriptor::description`]), its display title and the
/// one-line summary its managed product carries, the slug
/// its labels live under ([`ManagedRuleGroupDescriptor::label_slug`]) and the labels
/// its rules add and match ([`PublishedManagedRule`]), plus the
/// two flags that list implies (see [`descriptor`], [`unversioned`] and
/// [`advanced`]) — because a
/// managed product is one of these groups as a describe reports it
/// ([`ManagedRuleGroupDescriptor::product`]), not a separate catalogue.
pub const MANAGED_RULE_GROUP_CATALOGUE: &[ManagedRuleGroupDescriptor] = &[
    descriptor(
        "AWSManagedRulesCommonRuleSet",
        "core-rule-set",
        700,
        "Contains rules that are generally applicable to web applications. This \
         provides protection against exploitation of a wide range of \
         vulnerabilities, including those described in OWASP publications.",
        "Core rule set (CRS)",
        "Provides protection against exploitation of a wide range of \
         vulnerabilities, including high risk and commonly occurring \
         vulnerabilities described in OWASP publications.",
        COMMON_RULE_SET_RULES,
    )
    .offering(
        COMMON_RULE_SET_VERSIONS,
        COMMON_RULE_SET_RETIRED_VERSIONS,
        COMMON_RULE_SET_DEFAULT_VERSION,
    ),
    descriptor(
        "AWSManagedRulesAdminProtectionRuleSet",
        "admin-protection",
        100,
        "Contains rules that allow you to block external access to exposed admin \
         pages. This may be useful if you are running third-party software or \
         would like to reduce the risk of a malicious actor gaining \
         administrative access to your application.",
        "Admin protection",
        "Blocks external access to exposed administrative pages.",
        ADMIN_PROTECTION_RULES,
    ),
    descriptor(
        "AWSManagedRulesKnownBadInputsRuleSet",
        "known-bad-inputs",
        200,
        "Contains rules that allow you to block request patterns that are known to \
         be invalid and are associated with exploitation or discovery of \
         vulnerabilities. This can help reduce the risk of a malicious actor \
         discovering a vulnerable application.",
        "Known bad inputs",
        "Blocks request patterns that are known to be invalid and are associated \
         with exploitation or discovery of vulnerabilities.",
        KNOWN_BAD_INPUTS_RULES,
    ),
    descriptor(
        "AWSManagedRulesSQLiRuleSet",
        "sql-database",
        200,
        "Contains rules that allow you to block request patterns associated with \
         exploitation of SQL databases, like SQL injection attacks. This can help \
         prevent remote injection of unauthorized queries.",
        "SQL database",
        "Blocks request patterns associated with the exploitation of SQL \
         databases, like SQL injection attacks.",
        SQL_DATABASE_RULES,
    ),
    descriptor(
        "AWSManagedRulesLinuxRuleSet",
        "linux-os",
        200,
        "Contains rules that block request patterns associated with exploitation \
         of vulnerabilities specific to Linux, including LFI attacks. This can \
         help prevent attacks that expose file contents or execute code for which \
         the attacker should not have had access.",
        "Linux operating system",
        "Blocks request patterns associated with the exploitation of \
         vulnerabilities specific to Linux, including Linux-specific local file \
         inclusion attacks.",
        LINUX_RULES,
    ),
    descriptor(
        "AWSManagedRulesUnixRuleSet",
        "posix-os",
        100,
        "Contains rules that block request patterns associated with exploiting \
         vulnerabilities specific to POSIX/POSIX-like OS, including LFI attacks. \
         This can help prevent attacks that expose file contents or execute code \
         for which the attacker should not have had access.",
        "POSIX operating system",
        "Blocks request patterns associated with the exploitation of \
         vulnerabilities specific to POSIX and POSIX-like operating systems.",
        UNIX_RULES,
    ),
    descriptor(
        "AWSManagedRulesWindowsRuleSet",
        "windows-os",
        200,
        "Contains rules that block request patterns associated with exploiting \
         vulnerabilities specific to Windows, (e.g., PowerShell commands). This \
         can help prevent exploits that allow attacker to run unauthorized \
         commands or execute malicious code.",
        "Windows operating system",
        "Blocks request patterns associated with the exploitation of \
         vulnerabilities specific to Windows, like remote execution of PowerShell \
         commands.",
        WINDOWS_RULES,
    ),
    descriptor(
        "AWSManagedRulesPHPRuleSet",
        "php-app",
        100,
        "Contains rules that block request patterns associated with exploiting \
         vulnerabilities specific to the use of the PHP programming language, \
         including injection of unsafe PHP functions. This can help prevent \
         exploits that allow an attacker to remotely execute code or commands for \
         which they are not authorized.",
        "PHP application",
        "Blocks request patterns associated with the exploitation of \
         vulnerabilities specific to the use of the PHP programming language, \
         including the injection of unsafe PHP functions.",
        PHP_RULES,
    ),
    descriptor(
        "AWSManagedRulesWordPressRuleSet",
        "wordpress-app",
        100,
        "The WordPress Applications group contains rules that block request \
         patterns associated with the exploitation of vulnerabilities specific to \
         WordPress sites.",
        "WordPress application",
        "Blocks request patterns associated with the exploitation of \
         vulnerabilities specific to WordPress sites.",
        WORDPRESS_RULES,
    ),
    unversioned(
        "AWSManagedRulesAmazonIpReputationList",
        "amazon-ip-list",
        25,
        "This group contains rules that are based on Amazon threat intelligence. \
         This is useful if you would like to block sources associated with bots \
         or other threats.",
        "Amazon IP reputation list",
        "Blocks requests from sources that Amazon threat intelligence associates \
         with bots or other threats.",
        IP_REPUTATION_RULES,
    ),
    unversioned(
        "AWSManagedRulesAnonymousIpList",
        "anonymous-ip-list",
        50,
        "This group contains rules that allow you to block requests from services \
         that permit the obfuscation of viewer identity. This can include \
         requests originating from VPNs, proxies, Tor nodes, and hosting \
         providers. This is useful if you want to filter out viewers that may be \
         trying to hide their identity from your application.",
        "Anonymous IP list",
        "Blocks requests from services that permit the obfuscation of viewer \
         identity, such as VPNs, proxies, Tor nodes and hosting providers.",
        ANONYMOUS_IP_RULES,
    ),
    advanced(
        "AWSManagedRulesBotControlRuleSet",
        "bot-control",
        50,
        BOT_CONTROL_VERSIONS,
        NO_RETIRED_VERSIONS,
        "Provides protection against automated bots that can consume excess \
         resources, skew business metrics, cause downtime, or perform malicious \
         activities. Bot Control provides additional visibility through Amazon \
         CloudWatch and generates labels that you can use to control bot traffic \
         to your applications.",
        "Bot Control",
        "Provides visibility into and control over bot traffic, including common \
         desirable bots and the targeted detection of self-identifying and \
         evasive bots.",
        BOT_CONTROL_RULES,
        CLIENT_TOKEN_LABELS,
    ),
    advanced(
        "AWSManagedRulesATPRuleSet",
        "atp",
        50,
        ATP_VERSIONS,
        ATP_RETIRED_VERSIONS,
        "Provides protection for your login page against stolen credentials, \
         credential stuffing attacks, brute force login attempts, and other \
         anomalous login activities. With AWS WAF Fraud Control account takeover \
         prevention (ATP), you can prevent unauthorized access that might lead to \
         fraudulent activities, or inform legitimate users to take a preventive \
         action.",
        "Account takeover prevention",
        "Detects and manages requests that are part of malicious account takeover \
         attempts against your login pages.",
        ATP_RULES,
        NO_TOKEN_LABELS,
    ),
    advanced(
        "AWSManagedRulesACFPRuleSet",
        "acfp",
        50,
        ACFP_VERSIONS,
        NO_RETIRED_VERSIONS,
        "Provides protection against the creation of fraudulent accounts on your \
         site. Fraudulent accounts can be used for activities such as abusing \
         promotion and sign-up bonuses, impersonating someone, and to carry out \
         phishing and spam attacks.",
        "Account creation fraud prevention",
        "Detects and manages requests that are part of malicious account creation \
         attempts against your account registration pages.",
        ACFP_RULES,
        NO_TOKEN_LABELS,
    ),
    advanced(
        "AWSManagedRulesAntiDDoSRuleSet",
        "anti-ddos",
        50,
        ANTI_DDOS_VERSIONS,
        NO_RETIRED_VERSIONS,
        "Provides protection against distributed denial of service (DDoS) events. \
         This rule group contains rules that detect requests that are \
         contributing to a DDoS event against your protected resources and \
         challenge or block them.",
        "Anti-DDoS",
        "Detects and manages requests that are part of a distributed \
         denial-of-service event against your protected resources.",
        ANTI_DDOS_RULES,
        NO_TOKEN_LABELS,
    ),
];

/// The version a group the WAF Developer Guide publishes no version HISTORY for
/// offers: its current one, and no other.
///
/// A managed rule group is offered WITH versions — a recorded
/// `ListAvailableManagedRuleGroupVersions` response for the core rule set carries
/// at least one, and `ManagedRuleGroupStatement$Version` lets a web ACL pin one of
/// any group — so every catalogue entry offers at least this, and a group the guide
/// lists several concurrent versions for carries them instead (see
/// [`BOT_CONTROL_VERSIONS`]). The guide publishes no version NAMES or dates for the
/// groups that carry this one, so both are what THIS catalogue publishes for them
/// rather than anything observed from the real service: one version on offer,
/// stable per group and per call, which is what keeps every published group
/// pinnable and listable, with the one it superseded retired
/// ([`RETIRED_INITIAL_VERSION`]).
pub(super) const CURRENT_VERSION_ONLY: &[ManagedRuleGroupRelease] =
    &[release("Version_1.1", "2021-03-23T00:00:00Z")];

/// The versions such a group's vendor has RETIRED: the initial version
/// [`CURRENT_VERSION_ONLY`] superseded, which the vendor no longer offers.
///
/// A vendor keeps publishing new versions of a group and expires the ones they
/// supersede, which is why `ManagedRuleGroupStatement$Version` is pinned "until you
/// change it" and why the model gives the save path and the describe their own
/// `WAFExpiredManagedRuleGroupVersionException`. A group that keeps one version on
/// offer therefore has its previous one here: known, so a request that pins it is
/// told it has expired, and unlisted, so a caller rolls forward to what
/// `ListAvailableManagedRuleGroupVersions` does offer. Like the names and dates in
/// [`CURRENT_VERSION_ONLY`], this history is what this catalogue publishes.
pub(super) const RETIRED_INITIAL_VERSION: &[ManagedRuleGroupRelease] =
    &[release("Version_1.0", "2019-11-25T00:00:00Z")];

/// The retired versions of a group whose vendor has expired none of what it
/// published: every version it ever named is still on offer.
pub(super) const NO_RETIRED_VERSIONS: &[ManagedRuleGroupRelease] = &[];

/// The versions offered of `AWSManagedRulesCommonRuleSet`, oldest first.
///
/// A vendor that revises a rule set keeps the versions it has published ON OFFER
/// rather than only its latest one — `ManagedRuleGroupStatement$Version` pins one
/// "until you change it", so every version a web ACL may still be pinned to has to
/// stay listable — and the ladder therefore grows with each release and has GAPS
/// where the vendor has since retired one ([`COMMON_RULE_SET_RETIRED_VERSIONS`]).
/// This is the ladder this catalogue publishes for the core rule set, which is the
/// group whose version history WAF publishes the longest of.
///
/// The instant on each release is the one THIS catalogue publishes as that
/// version's last update — stable per version, and ordered with the list — because
/// a vendor's own update instants are not something this emulator can know; what is
/// published about a version is that it is on offer and carries an update instant
/// of its own (`ManagedRuleGroupVersion$LastUpdateTimestamp`).
pub(super) const COMMON_RULE_SET_VERSIONS: &[ManagedRuleGroupRelease] = &[
    release("Version_1.0", "2019-11-25T00:00:00Z"),
    release("Version_1.1", "2021-03-23T00:00:00Z"),
    release("Version_1.3", "2021-06-14T00:00:00Z"),
    release("Version_1.4", "2021-09-27T00:00:00Z"),
    release("Version_1.6", "2022-01-24T00:00:00Z"),
    release("Version_1.7", "2022-04-11T00:00:00Z"),
    release("Version_1.8", "2022-07-18T00:00:00Z"),
    release("Version_1.9", "2022-10-24T00:00:00Z"),
    release("Version_1.10", "2023-01-23T00:00:00Z"),
    release("Version_1.11", "2023-04-17T00:00:00Z"),
    release("Version_1.12", "2023-07-24T00:00:00Z"),
    release("Version_1.14", "2023-10-23T00:00:00Z"),
    release("Version_1.15", "2024-01-22T00:00:00Z"),
    release("Version_1.16", "2024-04-15T00:00:00Z"),
    release("Version_1.17", "2024-07-22T00:00:00Z"),
    release("Version_1.18", "2024-10-21T00:00:00Z"),
    release("Version_1.19", "2025-01-27T00:00:00Z"),
    release("Version_1.20", "2025-04-21T00:00:00Z"),
    release("Version_1.21", "2025-07-21T00:00:00Z"),
    release("Version_1.22", "2025-10-20T00:00:00Z"),
];

/// The versions of `AWSManagedRulesCommonRuleSet` its vendor has RETIRED, oldest
/// first: the ones missing from [`COMMON_RULE_SET_VERSIONS`].
///
/// A retired version is pruned from what the vendor OFFERS while staying KNOWN to
/// the describe (see [`ManagedRuleGroupDescriptor::expired_versions`]), so the gaps
/// in a published ladder are exactly the versions a pinning caller is told to roll
/// forward from — rather than names nobody published.
pub(super) const COMMON_RULE_SET_RETIRED_VERSIONS: &[ManagedRuleGroupRelease] = &[
    release("Version_1.2", "2021-04-26T00:00:00Z"),
    release("Version_1.5", "2021-11-15T00:00:00Z"),
    release("Version_1.13", "2023-08-21T00:00:00Z"),
];

/// The version its vendor currently publishes as the core rule set's DEFAULT.
///
/// Not the newest version on offer: a vendor holds the default of a long-lived rule
/// set at an earlier version so that a `ManagedRuleGroupStatement` pinning none
/// keeps running the same rules as new versions are published. See
/// [`ManagedRuleGroupDescriptor::default_version`].
pub(super) const COMMON_RULE_SET_DEFAULT_VERSION: &str = "Version_1.1";

/// The versions offered of `AWSManagedRulesBotControlRuleSet`, oldest first.
///
/// The intelligent-threat-mitigation groups below are the ones the WAF Developer
/// Guide publishes a version history for, and the names are the guide's
/// (`Version_<major>.<minor>`). The instant on each release is the one THIS
/// catalogue publishes as that version's last update — stable per version, and
/// ordered with the list — because a vendor's own update instants are not
/// something this emulator can know.
pub(super) const BOT_CONTROL_VERSIONS: &[ManagedRuleGroupRelease] = &[
    release("Version_1.0", "2021-02-09T00:00:00Z"),
    release("Version_2.0", "2022-09-21T00:00:00Z"),
    release("Version_3.0", "2023-10-24T00:00:00Z"),
];

/// The versions offered of `AWSManagedRulesATPRuleSet`: the one its vendor has not
/// retired. See [`BOT_CONTROL_VERSIONS`] and [`ATP_RETIRED_VERSIONS`].
pub(super) const ATP_VERSIONS: &[ManagedRuleGroupRelease] =
    &[release("Version_3.0", "2023-11-14T00:00:00Z")];

/// The versions of `AWSManagedRulesATPRuleSet` its vendor has RETIRED, oldest
/// first: the two that [`ATP_VERSIONS`] superseded. See
/// [`RETIRED_INITIAL_VERSION`] for why a superseded version is published rather
/// than dropped.
pub(super) const ATP_RETIRED_VERSIONS: &[ManagedRuleGroupRelease] = &[
    release("Version_1.0", "2022-06-14T00:00:00Z"),
    release("Version_2.0", "2023-05-03T00:00:00Z"),
];

/// The versions offered of `AWSManagedRulesACFPRuleSet`. See
/// [`BOT_CONTROL_VERSIONS`].
pub(super) const ACFP_VERSIONS: &[ManagedRuleGroupRelease] = &[
    release("Version_1.0", "2023-09-26T00:00:00Z"),
    release("Version_2.0", "2024-04-09T00:00:00Z"),
];

/// The versions offered of `AWSManagedRulesAntiDDoSRuleSet`. See
/// [`BOT_CONTROL_VERSIONS`].
pub(super) const ANTI_DDOS_VERSIONS: &[ManagedRuleGroupRelease] =
    &[release("Version_1.0", "2025-05-14T00:00:00Z")];

/// One [`MANAGED_RULE_GROUP_CATALOGUE`] entry, of the AWS vendor: a RULE SET its
/// vendor publishes in named versions, offered with [`CURRENT_VERSION_ONLY`] and
/// carrying [`RETIRED_INITIAL_VERSION`].
///
/// A rule set is a body of rules its vendor revises and publishes as a version a
/// web ACL can pin (`ManagedRuleGroupStatement$Version` is "the version of the
/// managed rule group to use"), so an entry built here reports its versioning as
/// supported — what tells a caller it can call
/// `ListAvailableManagedRuleGroupVersions` and pin one of the versions that answers.
/// It is not one of the intelligent-threat-mitigation groups (see [`advanced`]), and
/// it is not one of the IP-address lists either (see [`unversioned`]).
pub(super) const fn descriptor(
    name: &'static str,
    label_slug: &'static str,
    capacity: i64,
    description: &'static str,
    product_title: &'static str,
    product_description: &'static str,
    rules: &'static [PublishedManagedRule],
) -> ManagedRuleGroupDescriptor {
    ManagedRuleGroupDescriptor {
        vendor_name: AWS_MANAGED_RULE_GROUP_VENDOR,
        name,
        label_slug,
        capacity,
        description,
        product_title,
        product_description,
        is_versioning_supported: true,
        is_advanced_managed_rule_set: false,
        versions: CURRENT_VERSION_ONLY,
        default_version: None,
        expired_versions: RETIRED_INITIAL_VERSION,
        rules,
        token_labels: NO_TOKEN_LABELS,
    }
}

/// [`descriptor`] for a group its vendor does NOT publish in named versions: the
/// IP-address-list groups, whose content is an address list the vendor keeps
/// continuously up to date in place rather than a rule set it revises and
/// republishes, so a caller has no version of one to choose between or to pin.
///
/// Those groups are the only ones in this catalogue that report their versioning as
/// unsupported; they are not advanced managed rule sets either.
///
/// They still carry [`CURRENT_VERSION_ONLY`] and [`RETIRED_INITIAL_VERSION`], like
/// every other entry: the version LISTS are what this catalogue publishes so that
/// `ManagedRuleGroupStatement$Version` can pin any catalogued group and
/// `ListAvailableManagedRuleGroupVersions` answers one for every pair
/// `ListAvailableManagedRuleGroups` lists (see [`CURRENT_VERSION_ONLY`]), while this
/// flag is the vendor's published property of the group — so the two are set
/// independently here rather than one derived from the other, and narrowing what
/// these groups offer is a change to those lists, not to this flag.
pub(super) const fn unversioned(
    name: &'static str,
    label_slug: &'static str,
    capacity: i64,
    description: &'static str,
    product_title: &'static str,
    product_description: &'static str,
    rules: &'static [PublishedManagedRule],
) -> ManagedRuleGroupDescriptor {
    ManagedRuleGroupDescriptor {
        vendor_name: AWS_MANAGED_RULE_GROUP_VENDOR,
        name,
        label_slug,
        capacity,
        description,
        product_title,
        product_description,
        is_versioning_supported: false,
        is_advanced_managed_rule_set: false,
        versions: CURRENT_VERSION_ONLY,
        default_version: None,
        expired_versions: RETIRED_INITIAL_VERSION,
        rules,
        token_labels: NO_TOKEN_LABELS,
    }
}

/// [`descriptor`] for a group whose vendor offers `versions` of it, oldest first,
/// and has retired `expired_versions` of it, also oldest first.
///
/// Those are the intelligent-threat-mitigation groups: the guide publishes a
/// version history for exactly them, and they are the ones it describes as the
/// advanced protections — so an entry built here is an advanced managed rule set,
/// versioned like every other rule set.
///
/// It also carries the group's `token_labels`, because a group whose rules act on
/// WAF's own client-token handling is one of these.
#[allow(clippy::too_many_arguments)]
pub(super) const fn advanced(
    name: &'static str,
    label_slug: &'static str,
    capacity: i64,
    versions: &'static [ManagedRuleGroupRelease],
    expired_versions: &'static [ManagedRuleGroupRelease],
    description: &'static str,
    product_title: &'static str,
    product_description: &'static str,
    rules: &'static [PublishedManagedRule],
    token_labels: &'static [&'static str],
) -> ManagedRuleGroupDescriptor {
    ManagedRuleGroupDescriptor {
        vendor_name: AWS_MANAGED_RULE_GROUP_VENDOR,
        name,
        label_slug,
        capacity,
        description,
        product_title,
        product_description,
        is_versioning_supported: true,
        is_advanced_managed_rule_set: true,
        versions,
        default_version: None,
        expired_versions,
        rules,
        token_labels,
    }
}

/// The labels of WAF's own client-token handling a group that acts on a token
/// publishes beside its own ([`ManagedRuleGroupDescriptor::token_labels`]).
///
/// WAF's token handling states what it made of the token a request presented, under
/// its own vendor-independent namespace: the request carried no token, the token it
/// carried was accepted, or that token was rejected. A group whose rules act on that
/// verdict publishes all three, because the verdict is one of exactly these.
pub(super) const CLIENT_TOKEN_LABELS: &[&str] = &[
    "awswaf:managed:token:absent",
    "awswaf:managed:token:accepted",
    "awswaf:managed:token:rejected",
];

/// The token labels a group that acts on no client token publishes: none.
pub(super) const NO_TOKEN_LABELS: &[&str] = &[];

/// One rule of a published group, named `name`, that BLOCKS what it matches.
pub(super) const fn block(name: &'static str) -> PublishedManagedRule {
    PublishedManagedRule {
        name,
        action: PublishedRuleAction::Block,
        published_label: None,
        matched_label: None,
    }
}

/// One rule of a published group, named `name`, that COUNTS what it matches. See
/// [`block`].
pub(super) const fn count(name: &'static str) -> PublishedManagedRule {
    PublishedManagedRule {
        name,
        action: PublishedRuleAction::Count,
        published_label: None,
        matched_label: None,
    }
}

/// One rule of a published group, named `name`, that CHALLENGES what it matches.
/// See [`block`].
pub(super) const fn challenge(name: &'static str) -> PublishedManagedRule {
    PublishedManagedRule {
        name,
        action: PublishedRuleAction::Challenge,
        published_label: None,
        matched_label: None,
    }
}

/// [`block`] for a rule whose vendor also publishes the label it ADDS to what it
/// matches ([`PublishedManagedRule::published_label`]), stated relative to the
/// group's own namespace.
pub(super) const fn block_adding(name: &'static str, label: &'static str) -> PublishedManagedRule {
    PublishedManagedRule {
        name,
        action: PublishedRuleAction::Block,
        published_label: Some(label),
        matched_label: None,
    }
}

/// [`count`] for a rule that acts on the label the group's own detection added for
/// what it matches: the label is both what the rule matches against and what a
/// matching request carries ([`PublishedManagedRule::matched_label`]).
pub(super) const fn count_matching(name: &'static str, label: &'static str) -> PublishedManagedRule {
    PublishedManagedRule {
        name,
        action: PublishedRuleAction::Count,
        published_label: Some(label),
        matched_label: Some(label),
    }
}

/// [`count_matching`] for a rule that BLOCKS what it matches.
pub(super) const fn block_matching(name: &'static str, label: &'static str) -> PublishedManagedRule {
    PublishedManagedRule {
        name,
        action: PublishedRuleAction::Block,
        published_label: Some(label),
        matched_label: Some(label),
    }
}

// ---------------------------------------------------------------------------
// The rules each published group runs
// ---------------------------------------------------------------------------
//
// One list per catalogue entry, in the order the WAF Developer Guide's rule list
// for that group gives them: each rule's name, the action that list states for
// it (`RuleSummary` is "high-level information about a Rule, returned by
// operations like DescribeManagedRuleGroup" — a name and an action, and nothing
// about how the rule matches, which no WAFv2 response reports for a vendor's
// group) and the label that list publishes for it, stated relative to the group's
// own namespace ([`ManagedRuleGroupDescriptor::managed_label_namespace`]). They are
// the VENDOR's rules, so they live here beside the group's
// capacity and versions rather than in the state struct, and nothing a caller does
// changes one.

/// The rules of `AWSManagedRulesCommonRuleSet`.
pub(super) const COMMON_RULE_SET_RULES: &[PublishedManagedRule] = &[
    block_adding("NoUserAgent_HEADER", "NoUserAgent_Header"),
    block_adding("UserAgent_BadBots_HEADER", "UserAgent_BadBots_Header"),
    block_adding("SizeRestrictions_QUERYSTRING", "SizeRestrictions_QueryString"),
    block_adding(
        "SizeRestrictions_Cookie_HEADER",
        "SizeRestrictions_Cookie_Header",
    ),
    block_adding("SizeRestrictions_BODY", "SizeRestrictions_Body"),
    block_adding("SizeRestrictions_URIPATH", "SizeRestrictions_URIPath"),
    block_adding("EC2MetaDataSSRF_BODY", "EC2MetaDataSSRF_Body"),
    block_adding("EC2MetaDataSSRF_COOKIE", "EC2MetaDataSSRF_Cookie"),
    block_adding("EC2MetaDataSSRF_URIPATH", "EC2MetaDataSSRF_URIPath"),
    block_adding(
        "EC2MetaDataSSRF_QUERYARGUMENTS",
        "EC2MetaDataSSRF_QueryArguments",
    ),
    block_adding("GenericLFI_QUERYARGUMENTS", "GenericLFI_QueryArguments"),
    block_adding("GenericLFI_URIPATH", "GenericLFI_URIPath"),
    block_adding("GenericLFI_BODY", "GenericLFI_Body"),
    block_adding("RestrictedExtensions_URIPATH", "RestrictedExtensions_URIPath"),
    block_adding(
        "RestrictedExtensions_QUERYARGUMENTS",
        "RestrictedExtensions_QueryArguments",
    ),
    block_adding("GenericRFI_QUERYARGUMENTS", "GenericRFI_QueryArguments"),
    block_adding("GenericRFI_BODY", "GenericRFI_Body"),
    block_adding("GenericRFI_URIPATH", "GenericRFI_URIPath"),
    block_adding("CrossSiteScripting_COOKIE", "CrossSiteScripting_Cookie"),
    block_adding(
        "CrossSiteScripting_QUERYARGUMENTS",
        "CrossSiteScripting_QueryArguments",
    ),
    block_adding("CrossSiteScripting_BODY", "CrossSiteScripting_Body"),
    block_adding("CrossSiteScripting_URIPATH", "CrossSiteScripting_URIPath"),
];

/// The rules of `AWSManagedRulesAdminProtectionRuleSet`.
pub(super) const ADMIN_PROTECTION_RULES: &[PublishedManagedRule] =
    &[block_adding("AdminProtection_URIPATH", "AdminProtection_URIPath")];

/// The rules of `AWSManagedRulesKnownBadInputsRuleSet`.
pub(super) const KNOWN_BAD_INPUTS_RULES: &[PublishedManagedRule] = &[
    block_adding(
        "JavaDeserializationRCE_HEADER",
        "JavaDeserializationRCE_Header",
    ),
    block_adding("JavaDeserializationRCE_BODY", "JavaDeserializationRCE_Body"),
    block_adding(
        "JavaDeserializationRCE_URIPATH",
        "JavaDeserializationRCE_URIPath",
    ),
    block_adding(
        "JavaDeserializationRCE_QUERYSTRING",
        "JavaDeserializationRCE_QueryString",
    ),
    block_adding("Host_localhost_HEADER", "Host_localhost_Header"),
    block_adding("PROPFIND_METHOD", "PROPFIND_Method"),
    block_adding("ExploitablePaths_URIPATH", "ExploitablePaths_URIPath"),
    block_adding("Log4JRCE_HEADER", "Log4JRCE_Header"),
    block_adding("Log4JRCE_QUERYSTRING", "Log4JRCE_QueryString"),
    block_adding("Log4JRCE_BODY", "Log4JRCE_Body"),
    block_adding("Log4JRCE_URIPATH", "Log4JRCE_URIPath"),
];

/// The rules of `AWSManagedRulesSQLiRuleSet`.
pub(super) const SQL_DATABASE_RULES: &[PublishedManagedRule] = &[
    block_adding("SQLi_QUERYARGUMENTS", "SQLi_QueryArguments"),
    block_adding(
        "SQLiExtendedPatterns_QUERYARGUMENTS",
        "SQLiExtendedPatterns_QueryArguments",
    ),
    block_adding("SQLi_BODY", "SQLi_Body"),
    block_adding("SQLiExtendedPatterns_BODY", "SQLiExtendedPatterns_Body"),
    block_adding("SQLi_COOKIE", "SQLi_Cookie"),
    block_adding("SQLi_URIPATH", "SQLi_URIPath"),
];

/// The rules of `AWSManagedRulesLinuxRuleSet`.
pub(super) const LINUX_RULES: &[PublishedManagedRule] = &[
    block_adding("LFI_URIPATH", "LFI_URIPath"),
    block_adding("LFI_QUERYSTRING", "LFI_QueryString"),
    block_adding("LFI_HEADER", "LFI_Header"),
];

/// The rules of `AWSManagedRulesUnixRuleSet`.
pub(super) const UNIX_RULES: &[PublishedManagedRule] = &[
    block_adding(
        "UNIXShellCommandsVariables_QUERYARGUMENTS",
        "UNIXShellCommandsVariables_QueryArguments",
    ),
    block_adding(
        "UNIXShellCommandsVariables_BODY",
        "UNIXShellCommandsVariables_Body",
    ),
];

/// The rules of `AWSManagedRulesWindowsRuleSet`.
pub(super) const WINDOWS_RULES: &[PublishedManagedRule] = &[
    block_adding(
        "WindowsShellCommandsVariables_QUERYARGUMENTS",
        "WindowsShellCommandsVariables_QueryArguments",
    ),
    block_adding(
        "WindowsShellCommandsVariables_BODY",
        "WindowsShellCommandsVariables_Body",
    ),
    block_adding(
        "PowerShellCommandsVariables_QUERYARGUMENTS",
        "PowerShellCommandsVariables_QueryArguments",
    ),
    block_adding(
        "PowerShellCommandsVariables_BODY",
        "PowerShellCommandsVariables_Body",
    ),
];

/// The rules of `AWSManagedRulesPHPRuleSet`.
pub(super) const PHP_RULES: &[PublishedManagedRule] = &[
    block_adding(
        "PHPHighRiskMethodsVariables_HEADER",
        "PHPHighRiskMethodsVariables_Header",
    ),
    block_adding(
        "PHPHighRiskMethodsVariables_QUERYSTRING",
        "PHPHighRiskMethodsVariables_QueryString",
    ),
    block_adding(
        "PHPHighRiskMethodsVariables_BODY",
        "PHPHighRiskMethodsVariables_Body",
    ),
];

/// The rules of `AWSManagedRulesWordPressRuleSet`.
pub(super) const WORDPRESS_RULES: &[PublishedManagedRule] = &[
    block_adding(
        "WordPressExploitableCommands_QUERYSTRING",
        "WordPressExploitableCommands_QueryString",
    ),
    block_adding(
        "WordPressExploitablePaths_URIPATH",
        "WordPressExploitablePaths_URIPath",
    ),
];

/// The rules of `AWSManagedRulesAmazonIpReputationList`, whose published label for
/// each rule is the list that rule judges against — the same text as the rule's own
/// name, which is what the guide publishes for this group.
pub(super) const IP_REPUTATION_RULES: &[PublishedManagedRule] = &[
    block_adding("AWSManagedIPReputationList", "AWSManagedIPReputationList"),
    block_adding("AWSManagedReconnaissanceList", "AWSManagedReconnaissanceList"),
    block_adding("AWSManagedIPDDoSList", "AWSManagedIPDDoSList"),
];

/// The rules of `AWSManagedRulesAnonymousIpList`. See [`IP_REPUTATION_RULES`] on the
/// labels of an address-list group.
pub(super) const ANONYMOUS_IP_RULES: &[PublishedManagedRule] = &[
    block_adding("AnonymousIPList", "AnonymousIPList"),
    block_adding("HostingProviderIPList", "HostingProviderIPList"),
];

/// The rules of `AWSManagedRulesBotControlRuleSet`, whose bot-category rules count
/// the traffic they recognise and whose bot-signal rules block it.
///
/// This group's rules act on the bot labels its own detection adds — a category rule
/// on the category it names, a signal rule on the signal it names — so each of those
/// labels is published in both the group's available and its consumed list.
pub(super) const BOT_CONTROL_RULES: &[PublishedManagedRule] = &[
    count_matching("CategoryAdvertising", "bot:category:advertising"),
    count_matching("CategoryArchiver", "bot:category:archiver"),
    count_matching("CategoryContentFetcher", "bot:category:content_fetcher"),
    count_matching("CategoryEmailClient", "bot:category:email_client"),
    count_matching("CategoryHttpLibrary", "bot:category:http_library"),
    count_matching("CategoryLinkChecker", "bot:category:link_checker"),
    count_matching("CategoryMiscellaneous", "bot:category:miscellaneous"),
    count_matching("CategoryMonitoring", "bot:category:monitoring"),
    block_matching("CategoryScrapingFramework", "bot:category:scraping_framework"),
    count_matching("CategorySearchEngine", "bot:category:search_engine"),
    count_matching("CategorySecurity", "bot:category:security"),
    count_matching("CategorySeo", "bot:category:seo"),
    count_matching("CategorySocialMedia", "bot:category:social_media"),
    block_matching("SignalAutomatedBrowser", "signal:automated_browser"),
    count_matching("SignalKnownBotDataCenter", "signal:known_bot_data_center"),
    block_matching("SignalNonBrowserUserAgent", "signal:non_browser_user_agent"),
];

/// The rules of `AWSManagedRulesATPRuleSet`.
///
/// This group's published label texts are not among the ones this catalogue can
/// cite: its vendor publishes them per detection rather than per rule, so each rule
/// here carries the name and action the rule list states for it and no label — the
/// same reading as [`PublishedManagedRule::published_label`], which never derives a
/// label from a rule's name.
pub(super) const ATP_RULES: &[PublishedManagedRule] = &[
    count("AllRequests"),
    block("UnsupportedCognitoIDP"),
    block("AttributeCompromisedCredentials"),
    block("AttributeUsernameTraversal"),
    block("AttributePasswordTraversal"),
    block("VolumetricIpHigh"),
    block("VolumetricSession"),
    block("SignalMissingCredential"),
    block("TokenRejected"),
];

/// The rules of `AWSManagedRulesACFPRuleSet`. See [`ATP_RULES`] on its labels.
pub(super) const ACFP_RULES: &[PublishedManagedRule] = &[
    count("AllRequests"),
    block("UnsupportedCognitoIDP"),
    block("AttributeCompromisedCredentials"),
    block("AttributeUsernameTraversal"),
    block("AttributePasswordTraversal"),
    block("VolumetricIpHigh"),
    block("VolumetricSession"),
    block("VolumetricPhoneNumberHigh"),
    block("SignalMissingCredential"),
    block("TokenRejected"),
];

/// The rules of `AWSManagedRulesAntiDDoSRuleSet`, which challenge the traffic they
/// recognise rather than blocking it outright. See [`ATP_RULES`] on its labels.
pub(super) const ANTI_DDOS_RULES: &[PublishedManagedRule] = &[
    challenge("ChallengeDDoSRequests"),
    challenge("ChallengeAllDuringEvent"),
];

/// One version of a group, named `name` and last updated at `last_update` (see
/// [`ManagedRuleGroupRelease::last_update`]).
pub(super) const fn release(name: &'static str, last_update: &'static str) -> ManagedRuleGroupRelease {
    ManagedRuleGroupRelease { name, last_update }
}

/// The published managed rule group a `(VendorName, Name)` pair names, if the
/// catalogue holds one.
///
/// Both members are matched exactly: `ManagedRuleGroupStatement$Name` targets
/// `EntityName`, whose names WAF treats as case-sensitive, and the vendor name is
/// the vendor's own.
pub fn managed_rule_group(
    vendor_name: &str,
    name: &str,
) -> Option<&'static ManagedRuleGroupDescriptor> {
    MANAGED_RULE_GROUP_CATALOGUE
        .iter()
        .find(|group| group.vendor_name == vendor_name && group.name == name)
}

/// The refusal a rule list earns by PINNING a managed rule group version its
/// vendor has retired, if one of its rules pins one.
///
/// `ManagedRuleGroupStatement$Version` is "the version of the managed rule group to
/// use. If you specify this, the version setting is fixed until you change it", and
/// WAF resolves it when the rule is SAVED — the same moment every reference
/// [`ComAmazonawsWafv2Awswaf20190729State::unresolvable_rule_reference`] resolves at
/// — so a rule pinning a version the publishing vendor no longer offers
/// ([`ManagedVersionState::Expired`]) is refused with the modelled
/// `WAFExpiredManagedRuleGroupVersionException` and nothing is stored, rather than
/// kept as though the pin were live. A version the group never published is NOT
/// this refusal, and neither is a statement naming no version at all: that one runs
/// "the vendor's default version" (the member's own doc), which is by definition
/// offered.
///
/// Walked with an explicit worklist, like [`misplaced_rule_group_statement`]: the
/// tree is supplied by the request.
pub fn expired_managed_rule_group_version(
    rules: &[Rule],
) -> Option<WAFExpiredManagedRuleGroupVersionException> {
    let mut pending: Vec<&Statement> = rules.iter().map(|rule| &rule.statement).collect();
    while let Some(statement) = pending.pop() {
        if let Some((managed, version)) = statement
            .managed_rule_group_statement
            .as_ref()
            .and_then(|managed| {
                managed
                    .version
                    .as_deref()
                    .map(|version| (&**managed, version))
            })
            .filter(|(managed, version)| {
                managed_rule_group(&managed.vendor_name, &managed.name)
                    .is_some_and(|group| group.version_state(version) == ManagedVersionState::Expired)
            })
        {
            return Some(expired_managed_rule_group_version_exception(
                &managed.vendor_name,
                &managed.name,
                version,
            ));
        }
        pending.extend(nested_statements(statement));
    }
    None
}
