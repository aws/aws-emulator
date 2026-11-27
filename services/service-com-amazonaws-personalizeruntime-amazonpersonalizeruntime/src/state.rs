// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-personalizeruntime-amazonpersonalizeruntime service:
//! the **inference registry** every operation of this service reads.
//!
//! # The registry is the whole service
//!
//! Amazon Personalize Runtime models exactly three operations —
//! `GetRecommendations`, `GetPersonalizedRanking` and `GetActionRecommendations` —
//! and every one of them is a READ against a control-plane resource owned by the
//! sibling Amazon Personalize service: a campaign, a recommender, or a filter. None
//! of the three creates, updates or deletes anything, so no handler in this crate
//! writes to the registry; the one value a call mints is its own `recommendationId`
//! (see [`ComAmazonawsPersonalizeruntimeAmazonpersonalizeruntimeState::issue_recommendation_id`]).
//!
//! Resolving one of those ARNs has TWO refusals, not one, and they are told apart by
//! what the ARN itself claims:
//!
//! * An ARN whose own resource part names another resource type than the member it
//!   arrived in ([`ResourceKind`]) is no reference to such a resource at all, so
//!   nothing is looked up: `InvalidInputException` (HTTP 400) with the message
//!   `The given <kind> ARN is invalid: <arn>` (see [`Denied::invalid_arn`]).
//! * An ARN of the RIGHT resource type this registry does not hold is
//!   `InvalidInputException` (HTTP 400) with the message `<arn> does not exist or is
//!   not active yet.` for all three operations — the inference data plane cannot tell a
//!   resource that was never created from one that was created but has not reached
//!   ACTIVE, so it refuses both through the INVALID-INPUT channel (see
//!   [`Denied::unresolvable`]). A missing
//!   [`campaigns`](ComAmazonawsPersonalizeruntimeAmazonpersonalizeruntimeState::campaigns) /
//!   [`recommenders`](ComAmazonawsPersonalizeruntimeAmazonpersonalizeruntimeState::recommenders) /
//!   [`filters`](ComAmazonawsPersonalizeruntimeAmazonpersonalizeruntimeState::filters)
//!   entry is the single source of truth for that refusal.
//!
//! Both carry the same code and status, so the message is the only observable that
//! separates a reference the caller mixed up from one whose resource is still being
//! created — and only the second is a condition that can change by itself.
//!
//! # The registration seam, and the rule it follows
//!
//! A campaign, a recommender and a filter come into being through the Amazon
//! Personalize CONTROL PLANE (`CreateCampaign`, `CreateRecommender`, `CreateFilter`)
//! — operations this runtime service does not serve. So this emulator registers them
//! through three seams, [`register_campaign`], [`register_recommender`] and
//! [`register_filter`], which an emulator-internal caller calls on the shared engine:
//! the sibling control-plane crate's own `CreateCampaign` and `CreateFilter` handlers
//! do exactly that, so a client provisioning a pipeline over the wire is what makes a
//! campaign exist here.
//!
//! The rule, stated once so no handler has to re-decide it:
//!
//! * A campaign, recommender or filter exists **only** after its seam registered it,
//!   and a campaign is served only once the clock has completed its create
//!   ([`Campaign::active_at`]). There is deliberately NO auto-registration on first
//!   reference: auto-registration
//!   would make `GetRecommendations` answer 200 for a campaign that was never
//!   created, which is the one answer the real service never gives.
//! * A seam registers a resource only under an ARN that references a resource of ITS OWN
//!   kind, through the same [`require_resource_kind`] every read path applies: an ARN
//!   whose resource type names another resource is no reference to this kind at all, so
//!   no read path could ever resolve the entry and the seam refuses it instead of storing
//!   one that is dead on arrival.
//! * The item and action catalogs are stored with the campaign or recommender that
//!   serves them, because that is what the resolved resource's solution version was
//!   trained on — and a resource fitted to a dataset group also reads what the INGESTION
//!   data plane holds for that group, because real-time ingestion keeps changing it
//!   ([`ingested`]). Every item id, action id, score and metadata value a response
//!   carries is read from those two.
//! * A filter's expression is validated when the filter is REGISTERED, exactly as
//!   `CreateFilter` validates it, so a stored filter is always one the runtime can
//!   apply. What a REQUEST binds its placeholders to is a fact about the request, so a
//!   value a clause cannot apply is reported where the request is served.
//!
//! # What the registry is NOT asked
//!
//! Whether the CALLER may make the request at all is decided before any of the above, and
//! not from anything this registry holds: see [`authorization`]. A request that fails that
//! decision never reaches a lookup, so a missing registry entry stays the single source of
//! truth for the refusals named above and for nothing else.

pub mod authorization;
pub mod filter;
pub mod ingested;
pub mod scoring;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

pub use authorization::authorization_refusal;
pub use filter::{BoundFilter, Entity, EntryFacts, FilterExpression, FilterKind};
pub use ingested::{Ingested, RecordedInteractions};

/// The only `metadataColumns` map key the service accepts: "The map key is
/// `ITEMS`" (`GetRecommendationsRequest$metadataColumns` /
/// `GetPersonalizedRankingRequest$metadataColumns` docs).
pub const METADATA_DATASET_ITEMS: &str = "ITEMS";

/// "The maximum number of columns you can provide is 10."
/// (`GetRecommendationsRequest$metadataColumns` doc.) The modelled `ColumnNamesList`
/// bound is looser (99), so this one is enforced here.
pub const MAX_METADATA_COLUMNS: usize = 10;

/// `GetRecommendations$numResults`: "The default is 25."
pub const DEFAULT_NUM_RESULTS: usize = 25;
/// `GetRecommendations$numResults`: "If you are including metadata in
/// recommendations, the maximum is 50. Otherwise, the maximum is 500."
pub const MAX_NUM_RESULTS: usize = 500;
/// The same doc sentence's lower maximum, for a request that asks for metadata.
pub const MAX_NUM_RESULTS_WITH_METADATA: usize = 50;

/// `GetActionRecommendations$numResults`: "The default is 5. The maximum is 100."
pub const DEFAULT_NUM_ACTION_RESULTS: usize = 5;
/// The maximum from the same doc sentence.
pub const MAX_NUM_ACTION_RESULTS: usize = 100;

/// `PredictedItem$reason` value for an item a request's promotion put in the list:
/// "Promoted item - Indicates the item was included as part of a promotion that you
/// applied in your recommendation request."
pub const REASON_PROMOTED_ITEM: &str = "Promoted item";

/// How many issued `recommendationId`s the registry keeps for inspection.
///
/// An EMULATOR retention bound, not an AWS limit: AWS exposes no operation that reads
/// a recommendation back, so there is no documented depth to honour. Bounded because
/// a caller must not be able to grow shared engine state without limit by calling an
/// inference operation in a loop; the total issued count is kept as a counter, so it
/// stays exact without retaining the ids.
pub const MAX_RETAINED_RECOMMENDATION_IDS: usize = 64;

/// The recipe family a campaign's solution version was trained with, which decides
/// which runtime operation serves the campaign and which request members it needs.
///
/// The four values are the recipe types the runtime operations' own documentation
/// names: `USER_PERSONALIZATION` and `RELATED_ITEMS` for `GetRecommendations`,
/// `PERSONALIZED_RANKING` for `GetPersonalizedRanking`
/// (`smithy_query(query="doc",shape="GetPersonalizedRanking")`: "The solution backing
/// the campaign must have been created using a recipe of type PERSONALIZED_RANKING")
/// and `PERSONALIZED_ACTIONS` for `GetActionRecommendations`
/// (`smithy_query(query="doc",shape="GetActionRecommendations")`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecipeType {
    /// `USER_PERSONALIZATION`: `GetRecommendations` with `userId`.
    UserPersonalization,
    /// `RELATED_ITEMS`: `GetRecommendations` with `itemId`.
    RelatedItems,
    /// `PERSONALIZED_RANKING`: `GetPersonalizedRanking`.
    PersonalizedRanking,
    /// `PERSONALIZED_ACTIONS`: `GetActionRecommendations`.
    PersonalizedActions,
}

impl RecipeType {
    /// The recipe type's name, as Amazon Personalize spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UserPersonalization => "USER_PERSONALIZATION",
            Self::RelatedItems => "RELATED_ITEMS",
            Self::PersonalizedRanking => "PERSONALIZED_RANKING",
            Self::PersonalizedActions => "PERSONALIZED_ACTIONS",
        }
    }

    /// The runtime operation that serves a campaign of this recipe type.
    pub fn operation(self) -> &'static str {
        match self {
            Self::UserPersonalization | Self::RelatedItems => "GetRecommendations",
            Self::PersonalizedRanking => "GetPersonalizedRanking",
            Self::PersonalizedActions => "GetActionRecommendations",
        }
    }

    /// Whether the runtime operation NAMED by `operation` serves a campaign of this
    /// recipe type.
    ///
    /// A campaign is served by exactly the one operation [`RecipeType::operation`]
    /// names, and refused by the other two, so every handler asks this with its own
    /// `Operation::NAME` instead of re-stating the mapping as its own predicate: the
    /// mapping lives in [`RecipeType::operation`]'s exhaustive `match` alone, which the
    /// compiler forces a new recipe type to update.
    pub fn served_by(self, operation: &str) -> bool {
        self.operation() == operation
    }
}

/// Which personalization input a resolved resource needs in the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequiredInput {
    /// Neither `userId` nor `itemId` is needed (a non-personalized use case).
    Nothing,
    /// `userId` is required.
    User,
    /// `itemId` is required.
    Item,
}

impl RequiredInput {
    /// The personalization input a resource that needs `self` actually USES, out of a
    /// request's `userId` and `itemId`: "USER_PERSONALIZATION - userId required, itemId
    /// not used RELATED_ITEMS - itemId required, userId not used"
    /// (`GetRecommendations` doc).
    ///
    /// The input a resource does not need is the input it does not use, so it never
    /// reaches a score and therefore cannot change which entries come back or in what
    /// order: sending it is simply ignored. A resource that needs NEITHER is not
    /// personalized at all (a domain use case that asks for no user and no item), so it
    /// uses neither and answers every caller the same way.
    pub fn personalization<'a>(self, user_id: &'a str, item_id: &'a str) -> &'a str {
        match self {
            Self::Nothing => "",
            Self::User => user_id,
            Self::Item => item_id,
        }
    }

    /// The user a request against a resource that needs `self` personalizes FOR, out of
    /// a request's `userId`: the user whose recorded interactions an inference call reads
    /// (see [`ingested::Ingested::interactions_of`]).
    ///
    /// This is [`RequiredInput::personalization`] asked for the user input alone, so a
    /// resource that uses the ITEM input, or neither, personalizes for no user at all: a
    /// `userId` such a request carries anyway is not used, by a filter clause over an
    /// interaction dataset any more than by a score.
    pub fn personalizing_user(self, user_id: &str) -> &str {
        self.personalization(user_id, "")
    }
}

/// The domain (use-case) recommenders this emulator knows, and the request member
/// each one requires.
///
/// Hand-written catalog. Source: the "Choosing recommender use cases" page the
/// `GetRecommendations` operation doc points at ("For recommenders, the recommender's
/// ARN is required and the required item and user input depends on the use case
/// (domain-based recipe) backing the recommender. For information on use case
/// requirements see Choosing recommender use cases"), covering the ECOMMERCE and
/// VIDEO_ON_DEMAND domains. [`register_recommender`] accepts only a use case named
/// here, so an unknown use case is refused where the resource is created rather than
/// silently given the wrong required input.
pub const RECOMMENDER_USE_CASES: &[(&str, RequiredInput)] = &[
    // ECOMMERCE
    ("Most viewed", RequiredInput::Nothing),
    ("Best sellers", RequiredInput::Nothing),
    ("Frequently bought together", RequiredInput::Item),
    ("Customers who viewed X also viewed", RequiredInput::Item),
    ("Recommended for you", RequiredInput::User),
    // VIDEO_ON_DEMAND
    ("Most popular", RequiredInput::Nothing),
    ("Trending now", RequiredInput::Nothing),
    ("Because you watched X", RequiredInput::Item),
    ("More like X", RequiredInput::Item),
    ("Top picks for you", RequiredInput::User),
];

/// The input a named domain use case requires, or `None` when the use case is not in
/// [`RECOMMENDER_USE_CASES`].
pub fn use_case_required_input(use_case: &str) -> Option<RequiredInput> {
    RECOMMENDER_USE_CASES
        .iter()
        .find(|(name, _)| *name == use_case)
        .map(|(_, required)| *required)
}

/// One entry of a stored Items catalog.
#[derive(Debug, Clone, Default)]
pub struct Item {
    /// The `itemId` a `PredictedItem` reports.
    pub item_id: String,
    /// The entry's popularity in the stored catalog: the term every prediction score
    /// is derived from (see [`scoring::Seed::relevance`]).
    pub base_score: f64,
    /// The entry's Items-dataset columns, keyed by column name. Read by
    /// `metadataColumns` and by filter expressions over `Items.<COLUMN>`.
    pub attributes: BTreeMap<String, String>,
}

impl Item {
    /// An item with a popularity and no metadata columns.
    pub fn new(item_id: impl Into<String>, base_score: f64) -> Self {
        Self {
            item_id: item_id.into(),
            base_score,
            attributes: BTreeMap::new(),
        }
    }

    /// The same item with one Items-dataset column set.
    pub fn with_attribute(mut self, column: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes.insert(column.into(), value.into());
        self
    }
}

/// One entry of a stored Actions catalog.
#[derive(Debug, Clone, Default)]
pub struct Action {
    /// The `actionId` a `PredictedAction` reports.
    pub action_id: String,
    /// The entry's popularity in the stored catalog, as for [`Item::base_score`].
    pub base_score: f64,
    /// The entry's Actions-dataset columns, read by filter expressions over
    /// `Actions.<COLUMN>`. `PredictedAction` models no metadata member, so these are
    /// never reported.
    pub attributes: BTreeMap<String, String>,
}

impl Action {
    /// An action with a popularity and no columns.
    pub fn new(action_id: impl Into<String>, base_score: f64) -> Self {
        Self {
            action_id: action_id.into(),
            base_score,
            attributes: BTreeMap::new(),
        }
    }

    /// The same action with one Actions-dataset column set.
    pub fn with_attribute(mut self, column: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes.insert(column.into(), value.into());
        self
    }
}

/// A registered campaign: a deployed solution version, plus the catalog it was
/// trained on.
#[derive(Debug, Clone)]
pub struct Campaign {
    /// The campaign ARN this record is keyed by.
    pub arn: String,
    /// The campaign's name.
    pub name: String,
    /// The solution version the campaign deploys.
    pub solution_version_arn: String,
    /// The recipe family that solution version was trained with.
    pub recipe_type: RecipeType,
    /// Whether metadata in recommendations was enabled on the campaign; a request
    /// that asks for `metadataColumns` without it is refused.
    pub metadata_enabled: bool,
    /// The Items catalog the campaign recommends from.
    pub items: Vec<Item>,
    /// The Actions catalog a `PERSONALIZED_ACTIONS` campaign recommends from.
    pub actions: Vec<Action>,
    /// The instant the campaign's own create is due to complete, after which — and only
    /// after which — the inference plane serves it.
    ///
    /// A campaign is "CREATE PENDING > CREATE IN_PROGRESS > ACTIVE" on the control plane
    /// (`Campaign$status`), and the inference plane serves a campaign only once it is
    /// ACTIVE, which is exactly what the refusal for an unresolvable ARN states (see
    /// [`Denied::unresolvable`]). A seam that registered a campaign the clock has not
    /// finished creating therefore carries that due instant, so the registry can tell
    /// "created, not ACTIVE yet" from "ACTIVE" without being told again at read time.
    /// A campaign registered as READY (the seam's default) carries the instant it was
    /// registered at.
    pub active_at: SystemTime,
    /// The dataset group whose LIVE ingestion data this campaign's inference reads
    /// beside its stored catalog, when the seam that registered it named one.
    ///
    /// A campaign deploys a solution version trained on one dataset group's data, and
    /// that group keeps taking records after the training run: an interaction recorded
    /// through the ingestion data plane and an item put into the group's Items dataset
    /// change what this campaign returns (see [`ingested`]). `None` means this campaign
    /// was registered without a group, so its stored catalog is its whole input.
    pub dataset_group_arn: Option<String>,
    /// The instant this campaign's stored catalog was FITTED to that group's data.
    ///
    /// What separates the interactions the stored popularity already counts from the ones
    /// recorded since, so neither is weighed twice (see [`ingested`]).
    pub fitted_at: Option<SystemTime>,
}

impl Campaign {
    /// A campaign over an Items catalog.
    pub fn new(
        arn: impl Into<String>,
        name: impl Into<String>,
        recipe_type: RecipeType,
        items: Vec<Item>,
    ) -> Self {
        let arn = arn.into();
        Self {
            solution_version_arn: format!("{arn}/solution-version"),
            arn,
            name: name.into(),
            recipe_type,
            metadata_enabled: false,
            items,
            actions: Vec::new(),
            // Ready as registered; a caller whose campaign is still being created says
            // so with [`Campaign::active_from`].
            active_at: SystemTime::now(),
            // Fitted to no dataset group until the seam names one: a campaign the
            // control plane deployed carries the group its solution version was trained
            // on ([`Campaign::fitted_to`]).
            dataset_group_arn: None,
            fitted_at: None,
        }
    }

    /// A `PERSONALIZED_ACTIONS` campaign over an Actions catalog.
    pub fn new_actions(
        arn: impl Into<String>,
        name: impl Into<String>,
        actions: Vec<Action>,
    ) -> Self {
        let mut campaign = Self::new(arn, name, RecipeType::PersonalizedActions, Vec::new());
        campaign.actions = actions;
        campaign
    }

    /// The same campaign with metadata in recommendations enabled.
    pub fn with_metadata_enabled(mut self) -> Self {
        self.metadata_enabled = true;
        self
    }

    /// The same campaign with the solution version it deploys named, and the instant its
    /// own create completes.
    ///
    /// For the control-plane seam: a `CreateCampaign` answers before the campaign is
    /// ACTIVE, so what it registers is the campaign AND the instant the clock owes it
    /// that status ([`Campaign::active_at`]).
    pub fn deployed(
        mut self,
        solution_version_arn: impl Into<String>,
        active_at: SystemTime,
    ) -> Self {
        self.solution_version_arn = solution_version_arn.into();
        self.active_at = active_at;
        self
    }

    /// The same campaign fitted to the dataset group `dataset_group_arn`, at the instant
    /// `fitted_at` its catalog was read from that group's data.
    ///
    /// For the control-plane seam: a campaign deploys a solution version trained on one
    /// dataset group, and the group keeps taking records after that training run, so the
    /// inference plane reads the group's live records beside the stored catalog.
    pub fn fitted_to(
        mut self,
        dataset_group_arn: impl Into<String>,
        fitted_at: SystemTime,
    ) -> Self {
        self.dataset_group_arn = Some(dataset_group_arn.into());
        self.fitted_at = Some(fitted_at);
        self
    }

    /// Whether the inference plane serves this campaign at `now`: a campaign is served
    /// once its create is complete, and refused as "not active yet" before then.
    pub fn is_active(&self, now: SystemTime) -> bool {
        now >= self.active_at
    }

    /// Which personalization input a `GetRecommendations` call against this campaign
    /// needs: "USER_PERSONALIZATION - userId required, itemId not used RELATED_ITEMS
    /// - itemId required, userId not used" (`GetRecommendations` doc).
    pub fn required_input(&self) -> RequiredInput {
        match self.recipe_type {
            RecipeType::UserPersonalization | RecipeType::PersonalizedRanking => {
                RequiredInput::User
            }
            RecipeType::RelatedItems => RequiredInput::Item,
            RecipeType::PersonalizedActions => RequiredInput::User,
        }
    }
}

/// A registered domain recommender.
#[derive(Debug, Clone)]
pub struct Recommender {
    /// The recommender ARN this record is keyed by.
    pub arn: String,
    /// The recommender's name.
    pub name: String,
    /// The domain use case backing it; one of [`RECOMMENDER_USE_CASES`].
    pub use_case: String,
    /// Whether metadata in recommendations was enabled on the recommender.
    pub metadata_enabled: bool,
    /// The Items catalog it recommends from.
    pub items: Vec<Item>,
    /// The dataset group whose LIVE ingestion data this recommender's inference reads
    /// beside its stored catalog, as [`Campaign::dataset_group_arn`] does.
    pub dataset_group_arn: Option<String>,
    /// The instant its stored catalog was fitted to that group's data, as
    /// [`Campaign::fitted_at`] is.
    pub fitted_at: Option<SystemTime>,
}

impl Recommender {
    /// A recommender for `use_case` over an Items catalog.
    pub fn new(
        arn: impl Into<String>,
        name: impl Into<String>,
        use_case: impl Into<String>,
        items: Vec<Item>,
    ) -> Self {
        Self {
            arn: arn.into(),
            name: name.into(),
            use_case: use_case.into(),
            metadata_enabled: false,
            items,
            dataset_group_arn: None,
            fitted_at: None,
        }
    }

    /// The same recommender fitted to the dataset group `dataset_group_arn` at the
    /// instant `fitted_at`, as [`Campaign::fitted_to`] states it for a campaign.
    pub fn fitted_to(
        mut self,
        dataset_group_arn: impl Into<String>,
        fitted_at: SystemTime,
    ) -> Self {
        self.dataset_group_arn = Some(dataset_group_arn.into());
        self.fitted_at = Some(fitted_at);
        self
    }

    /// The same recommender with metadata in recommendations enabled.
    pub fn with_metadata_enabled(mut self) -> Self {
        self.metadata_enabled = true;
        self
    }

    /// Which personalization input a call against this recommender needs, from
    /// [`RECOMMENDER_USE_CASES`].
    pub fn required_input(&self) -> RequiredInput {
        use_case_required_input(&self.use_case).unwrap_or(RequiredInput::Nothing)
    }
}

/// A registered filter: its expression as written, and its parsed form.
#[derive(Debug, Clone)]
pub struct Filter {
    /// The filter ARN this record is keyed by.
    pub arn: String,
    /// The `filterExpression` as the control plane received it.
    pub expression: String,
    /// The parsed expression the runtime applies.
    pub parsed: FilterExpression,
}

impl Filter {
    /// Every `$placeholder` the expression references.
    pub fn referenced_parameters(&self) -> Vec<String> {
        self.parsed.referenced_parameters()
    }
}

/// Per-service state for com-amazonaws-personalizeruntime-amazonpersonalizeruntime. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsPersonalizeruntimeAmazonpersonalizeruntimeState {
    /// Registered campaigns, keyed by campaign ARN.
    pub campaigns: BTreeMap<String, Campaign>,
    /// Registered recommenders, keyed by recommender ARN.
    pub recommenders: BTreeMap<String, Recommender>,
    /// Registered filters, keyed by filter ARN.
    pub filters: BTreeMap<String, Filter>,
    /// How many `recommendationId`s this service has issued. The sequence number is
    /// what makes a minted id unique without randomness.
    pub recommendations_issued: u64,
    /// The most recently issued `recommendationId`s, newest last — a BOUNDED window
    /// (see [`MAX_RETAINED_RECOMMENDATION_IDS`]) for an emulator-internal caller to
    /// inspect, since the service models no operation that reads one back.
    pub recent_recommendation_ids: Vec<String>,
}

impl ComAmazonawsPersonalizeruntimeAmazonpersonalizeruntimeState {
    /// Mint this call's `recommendationId` ("The ID of the recommendation").
    ///
    /// Every call gets a FRESH id, which is what the real service returns, so the id
    /// is the one value here that is not a function of the request: uniqueness can
    /// only come from state, and it comes from [`Self::recommendations_issued`]. The
    /// id is opaque — a sequence number, the scope it was issued for and the issuing
    /// instant, hashed — so nothing a caller can read off it is load-bearing.
    ///
    /// This is the only write any handler of this service makes. It mutates no
    /// campaign, recommender or filter, so what a later caller reads back about any
    /// registered resource is unchanged.
    pub fn issue_recommendation_id(&mut self, scope: &str, now: SystemTime) -> String {
        self.recommendations_issued = self.recommendations_issued.saturating_add(1);
        let sequence = self.recommendations_issued.to_string();
        let nanos = now
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_string();
        let id = format!(
            "RID-{}",
            scoring::digest_hex(&[scope, &sequence, &nanos], 16)
        );
        self.recent_recommendation_ids.push(id.clone());
        while self.recent_recommendation_ids.len() > MAX_RETAINED_RECOMMENDATION_IDS {
            self.recent_recommendation_ids.remove(0);
        }
        id
    }

    /// The registered campaign under `arn`, or a refusal.
    ///
    /// An ARN whose resource part names something other than a campaign is refused as
    /// the invalid campaign reference it is, before the registry is consulted at all
    /// ([`require_resource_kind`]).
    ///
    /// A campaign whose create the clock has not completed yet resolves to the SAME
    /// refusal as one that was never registered: the inference plane serves a campaign
    /// only once it is ACTIVE and cannot tell the two apart, which is what that refusal
    /// states (see [`Denied::unresolvable`] and [`Campaign::active_at`]). A pure read —
    /// whether the create is due is a function of the stored instant and the clock.
    pub fn campaign(&self, arn: &str) -> Result<&Campaign, Denied> {
        require_resource_kind(arn, ResourceKind::Campaign)?;
        self.campaigns
            .get(arn)
            .filter(|campaign| campaign.is_active(SystemTime::now()))
            .ok_or_else(|| Denied::unresolvable(arn))
    }

    /// The registered recommender under `arn`, or [`Denied::unresolvable`].
    ///
    /// An ARN that names no recommender resource type is refused as the invalid
    /// reference it is, before anything is looked up ([`require_resource_kind`]).
    pub fn recommender(&self, arn: &str) -> Result<&Recommender, Denied> {
        require_resource_kind(arn, ResourceKind::Recommender)?;
        self.recommenders
            .get(arn)
            .ok_or_else(|| Denied::unresolvable(arn))
    }

    /// The registered filter under `arn`, or [`Denied::unresolvable`].
    ///
    /// An ARN that names no filter resource type is refused as the invalid reference it
    /// is, before anything is looked up ([`require_resource_kind`]).
    pub fn filter(&self, arn: &str) -> Result<&Filter, Denied> {
        require_resource_kind(arn, ResourceKind::Filter)?;
        self.filters
            .get(arn)
            .ok_or_else(|| Denied::unresolvable(arn))
    }

    /// The filter named by `arn` (when the request named one), bound to this
    /// request's `filter_values` for entries of kind `entity`.
    ///
    /// `Ok(None)` means the request named no filter, so nothing is filtered out.
    pub fn bind_filter(
        &self,
        arn: Option<&str>,
        entity: Entity,
        filter_values: Option<&HashMap<String, String>>,
    ) -> Result<Option<BoundFilter>, Denied> {
        let Some(arn) = arn else {
            return Ok(None);
        };
        let stored = self.filter(arn)?;
        filter::bind(&stored.parsed, entity, filter_values)
            .map(Some)
            .map_err(Denied::invalid)
    }
}

/// Why a runtime request cannot be served.
///
/// Lives here rather than in a handler because the registry lookups all three
/// operations share are what produce it; each handler lifts it into its own generated
/// error enum.
///
/// ONE channel, not two: everything the inference plane refuses a request for — an ARN
/// it cannot resolve, a member outside a documented bound, a filter expression it
/// cannot bind — is `InvalidInputException` (HTTP 400, "Provide a valid value for the
/// field or parameter."). The `ResourceNotFoundException` (404) all three operations
/// also model stays expressible through each handler's generated error enum, but no
/// registry refusal produces it: see [`Denied::unresolvable`] for why an unresolvable
/// resource is the invalid-input channel.
#[derive(Debug, Clone)]
pub struct Denied(String);

impl Denied {
    /// A refusal stating `message`.
    pub fn invalid(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    /// The refusal for a well-formed resource ARN the registry cannot resolve to an
    /// ACTIVE campaign, recommender or filter.
    ///
    /// It is the INVALID-INPUT channel (`InvalidInputException`, HTTP 400), not the
    /// not-found one: the inference plane serves a resource only once it is ACTIVE and
    /// cannot distinguish "never created" from "created, not ACTIVE yet" — which is
    /// why the message states both alternatives, and why `filterArn`'s own
    /// documentation warns "When using this parameter, be sure the filter resource is
    /// ACTIVE." The ARN in the message is the one the request named.
    pub fn unresolvable(arn: &str) -> Self {
        Self::invalid(format!("{arn} does not exist or is not active yet."))
    }

    /// The refusal for a value that references no resource of `kind` at all: its ARN's
    /// resource part names another resource type, or none.
    ///
    /// Deliberately NOT [`Denied::unresolvable`]: both are `InvalidInputException`
    /// (HTTP 400), so the message is the only observable separating them, and they are
    /// different conditions. An ARN of the wrong resource type can never be resolved by
    /// anything the caller waits for, while "does not exist or is not active yet" is a
    /// condition that changes by itself once the create the caller started completes.
    /// `given` is the value the request sent, reported back verbatim.
    pub fn invalid_arn(kind: ResourceKind, given: &str) -> Self {
        Self::invalid(format!("The given {} ARN is invalid: {given}", kind.as_str()))
    }

    /// The refusal's message.
    pub fn message(&self) -> &str {
        &self.0
    }
}

/// Which Personalize resource an ARN-carrying request member names.
///
/// Every ARN member of this service references exactly ONE resource type —
/// `campaignArn` a campaign, `recommenderArn` a recommender, `filterArn` (and a
/// promotion's own `filterArn`) a filter. The modelled `Arn` `@pattern`
/// `^arn:([a-z\d-]+):personalize:.*:.*:.+$` admits an ARN of ANY Personalize resource
/// type, so which type a value claims is this registry's own check rather than the
/// boundary's — and it is readable from the request alone, out of the ARN's resource
/// part (`service_common::arn::resource_type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    /// A campaign, referenced by `campaignArn`.
    Campaign,
    /// A domain recommender, referenced by `recommenderArn`.
    Recommender,
    /// A filter, referenced by `filterArn`.
    Filter,
}

impl ResourceKind {
    /// The ARN resource-type segment a reference to this kind carries, which is also how
    /// a refusal naming the kind reads.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Campaign => "campaign",
            Self::Recommender => "recommender",
            Self::Filter => "filter",
        }
    }
}

/// Check that `arn` references a resource of `kind` before anything is looked up.
///
/// A reference whose resource type is another one is not a reference to a `kind` the
/// registry could hold — no campaign can ever answer to a schema ARN — so existence is
/// never consulted for it and the refusal says what is wrong with the request instead
/// (see [`Denied::invalid_arn`]). The resource type is read through this tree's one
/// caller-supplied-ARN parse (`service_common::arn`), which also answers `None` for a
/// value that is no resource ARN at all — a value that references nothing of this kind
/// either.
pub fn require_resource_kind(arn: &str, kind: ResourceKind) -> Result<(), Denied> {
    if ::service_common::arn::resource_type(arn) == Some(kind.as_str()) {
        return Ok(());
    }
    Err(Denied::invalid_arn(kind, arn))
}

/// The number of results to compute, from the request's `numResults`.
///
/// `numResults` carries `@default: 0` in the model and the real service treats
/// absent-or-zero as the documented default, so `0` selects `default`. Above `max`
/// the request is refused naming the member and the bound, the way a service states a
/// constraint the model does not carry.
pub fn effective_num_results(
    requested: i32,
    default: usize,
    max: usize,
) -> Result<usize, Denied> {
    if requested < 0 {
        return Err(Denied::invalid(format!(
            "Value '{requested}' at 'numResults' failed to satisfy constraint: Member must have \
             value greater than or equal to 0"
        )));
    }
    let requested = requested as usize;
    if requested == 0 {
        return Ok(default);
    }
    if requested > max {
        return Err(Denied::invalid(format!(
            "Value '{requested}' at 'numResults' failed to satisfy constraint: Member must have \
             value less than or equal to {max}"
        )));
    }
    Ok(requested)
}

/// The Items-dataset columns the stored catalog carries, across every entry.
pub fn catalog_columns(items: &[Item]) -> BTreeSet<String> {
    items
        .iter()
        .flat_map(|item| item.attributes.keys().cloned())
        .collect()
}

/// The columns a `metadataColumns` map asks to include in recommendations: the value of
/// its `ITEMS` key, when that key names any column.
///
/// The ONE reading of the map, which both the bound a metadata request is measured
/// against (through [`metadata_bounded_max`]) and the columns the response serves are
/// taken from, so the two cannot disagree about whether a given map asks for metadata.
/// The model states the map as
/// "The map key is `ITEMS` and the value is a list of column names from your Items
/// dataset", so a key naming another dataset type asks for no column of the Items
/// dataset, and an `ITEMS` key whose list is empty names none either.
fn items_metadata_columns(
    metadata_columns: Option<&HashMap<String, Vec<String>>>,
) -> Option<&[String]> {
    metadata_columns?
        .get(METADATA_DATASET_ITEMS)
        .map(Vec::as_slice)
        .filter(|columns| !columns.is_empty())
}

/// The columns the `ITEMS` key names, measured against "The maximum number of columns
/// you can provide is 10" — the one `metadataColumns` rule whose violation would LOWER
/// the maximum another member is measured against, since a map past the cap is a map
/// that asks for metadata.
///
/// It is therefore applied where [`metadata_bounded_max`] applies it: in front of the
/// bound it would otherwise lower, so no member is ever measured against a maximum taken
/// from a column list the service refuses.
fn capped_items_columns(
    metadata_columns: Option<&HashMap<String, Vec<String>>>,
) -> Result<Option<&[String]>, Denied> {
    let Some(requested) = items_metadata_columns(metadata_columns) else {
        return Ok(None);
    };
    if requested.len() > MAX_METADATA_COLUMNS {
        return Err(Denied::invalid(format!(
            "Value '{}' at 'metadataColumns' failed to satisfy constraint: at most \
             {MAX_METADATA_COLUMNS} columns can be provided",
            requested.len()
        )));
    }
    Ok(Some(requested))
}

/// The maximum a request's result or input list may carry, from the one doc sentence
/// that states both: [`MAX_NUM_RESULTS_WITH_METADATA`] when the request includes
/// metadata in recommendations, [`MAX_NUM_RESULTS`] otherwise
/// (`GetRecommendationsRequest$numResults` /
/// `GetPersonalizedRankingRequest$inputList` docs). The one reading of `metadataColumns`
/// both metadata-bounded operations bound their member with, so one request means one
/// bound in both.
///
/// It is fallible because the column cap is applied HERE, in front of the bound it would
/// otherwise lower: a column list past the cap is exactly what makes a request one that
/// includes metadata, so the lower maximum is only ever taken from a column list this
/// crate would serve — never from one it refuses a statement later. The remaining
/// request-only rule, the map key, is [`requested_metadata_columns`]'s and is answered
/// after the bounded member, because a map breaking it names no column of the Items
/// dataset and so leaves this bound at the "Otherwise" maximum.
pub fn metadata_bounded_max(
    metadata_columns: Option<&HashMap<String, Vec<String>>>,
) -> Result<usize, Denied> {
    if capped_items_columns(metadata_columns)?.is_some() {
        Ok(MAX_NUM_RESULTS_WITH_METADATA)
    } else {
        Ok(MAX_NUM_RESULTS)
    }
}

/// The refusal for a list member carrying more entries than the documented maximum
/// that applies to this request: `Length of '<member>' cannot be greater than <max>`.
///
/// The message is built from the request — the member the caller sent and the bound
/// that applies to it — so it states the rule rather than any one request's size.
pub fn too_long(member: &str, max: usize) -> Denied {
    Denied::invalid(format!(
        "Length of '{member}' cannot be greater than {max}"
    ))
}

/// The metadata columns a request asked for, once the remaining request-only
/// `metadataColumns` rule has admitted the map: "The map key is `ITEMS`".
///
/// That rule is a fact about the value the caller sent, so a request that breaks it is
/// refused before any campaign or recommender is resolved — the same precedence
/// `numResults` and `inputList` follow. It is answered AFTER the member those bounds
/// apply to, because a map it refuses names no column of the Items dataset and so asks
/// for no metadata: it leaves the bound at the "Otherwise" maximum, which such a
/// request genuinely breaks when it is past it. The rule that would lower the bound —
/// the column cap — is [`request_metadata`]'s, taken in front of it. The two rules that
/// need the resolved resource are [`metadata_columns_in_catalog`]'s.
///
/// `Ok(None)` means the request asked for no metadata at all, in which case
/// `PredictedItem` must carry no `metadata` member — a map naming no column of the Items
/// dataset asks for nothing, which it reads through [`items_metadata_columns`], the same
/// reading the request's bound is taken from.
pub fn requested_metadata_columns(
    metadata_columns: Option<&HashMap<String, Vec<String>>>,
) -> Result<Option<&[String]>, Denied> {
    if let Some(metadata_columns) = metadata_columns {
        // The map is modelled `len 0..1`, so one key is the ordinary case; a request that
        // sent several is walked in key order, so the key the refusal names is the same on
        // every run rather than whichever one the hash order visited first.
        let mut datasets: Vec<&String> = metadata_columns.keys().collect();
        datasets.sort();
        for dataset in datasets {
            if dataset != METADATA_DATASET_ITEMS {
                return Err(Denied::invalid(format!(
                    "Value '{dataset}' at 'metadataColumns' failed to satisfy constraint: the map \
                     key must be {METADATA_DATASET_ITEMS}"
                )));
            }
        }
    }
    capped_items_columns(metadata_columns)
}

/// The columns [`requested_metadata_columns`] admitted, validated against the RESOLVED
/// resource and its catalog.
///
/// These are the `metadataColumns` rules that cannot be answered from the request:
/// metadata in recommendations has to have been enabled on the campaign or
/// recommender, and the columns come "from your Items dataset". `None` carries through
/// a request that asked for no metadata.
pub fn metadata_columns_in_catalog(
    requested: Option<&[String]>,
    metadata_enabled: bool,
    columns_in_catalog: &BTreeSet<String>,
) -> Result<Option<Vec<String>>, Denied> {
    let Some(requested) = requested else {
        return Ok(None);
    };
    if !metadata_enabled {
        return Err(Denied::invalid(
            "metadataColumns was provided but metadata in recommendations is not enabled for this \
             resource"
                .to_string(),
        ));
    }
    let mut columns: Vec<String> = Vec::new();
    for column in requested {
        if !columns_in_catalog.contains(column) {
            return Err(Denied::invalid(format!(
                "Value '{column}' at 'metadataColumns' failed to satisfy constraint: the column is \
                 not in the Items dataset"
            )));
        }
        if !columns.contains(column) {
            columns.push(column.clone());
        }
    }
    Ok(Some(columns))
}

/// The `PredictedItem$metadata` map for one reported entry, from the Items-dataset
/// attributes that entry stores, or `None` when the request asked for no metadata —
/// in which case the member is omitted entirely.
///
/// Whether the member is present is a fact about the REQUEST, so it is decided here
/// for every reported entry alike: the argument is the entry's attribute set, which an
/// entry the catalog never held supplies as an empty one. A requested column the entry
/// does not carry is left out of the map rather than reported empty: the value is
/// absent in the stored Items dataset, and an entry carrying none of the requested
/// columns is therefore reported with an empty map, not with no member.
pub fn metadata_for(
    attributes: &BTreeMap<String, String>,
    columns: Option<&[String]>,
) -> Option<HashMap<String, String>> {
    let columns = columns?;
    Some(
        columns
            .iter()
            .filter_map(|column| {
                attributes
                    .get(column)
                    .map(|value| (column.clone(), value.clone()))
            })
            .collect(),
    )
}

/// A request member that is present and not empty, as a `&str`.
///
/// `userId` and `itemId` are bounded only by a `@length` that starts at 0, so the
/// boundary validator accepts an empty string for either; an empty string identifies no
/// user and no item, so every handler reads it as the member not being supplied. The
/// entries of `GetPersonalizedRanking`'s `inputList` target that same `ItemID` shape, so
/// an entry that is the empty string is read the same way: it names no item.
///
/// The ARN members — `campaignArn`, `recommenderArn` and `filterArn` — carry that same
/// length bound AND the `Arn` `@pattern`, which no empty or blank string satisfies, so
/// the boundary refuses an empty ARN as a pattern violation naming the member before any
/// handler runs. For those members this is therefore the ABSENT member (the optional
/// ones a request can leave out), never an empty one, and no handler ever looks an empty
/// ARN up in the registry.
pub fn present(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPersonalizeruntimeAmazonpersonalizeruntimeState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPersonalizeruntimeAmazonpersonalizeruntimeState>)
}

/// The answer for a request whose engine does not carry this service's registry at
/// all — [`service_state`] returned `None`, so there is no state to compute any
/// response from.
///
/// This service models only `InvalidInputException` (400) and
/// `ResourceNotFoundException` (404), and answering either would tell the caller
/// something false about its own request. It renders as HTTP 501 instead, with
/// wording of its own: it is deliberately NOT
/// [`Scaffolded::not_implemented`](crate::scaffolded::Scaffolded::not_implemented),
/// whose message is the machine-readable "this operation is still a stub" record.
pub fn registry_unavailable<E>(operation: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::NotImplemented(crate::scaffolded::NotImplemented {
        message: format!(
            "the engine serving {operation} does not carry this service's inference registry"
        ),
    })
}

/// Register a campaign, one of the three seams that make a resource exist.
///
/// Returns `true` when the campaign was registered, and `false` without touching
/// anything when its ARN references no campaign at all — an empty value, or one whose
/// resource type names another resource ([`require_resource_kind`], the same check every
/// read path applies) — when the ARN is already registered (a campaign is created once;
/// re-registering must not silently swap a catalog out from under a caller) or when
/// `engine` does not carry this service's state.
pub fn register_campaign<Eng: Send + Sync + 'static>(engine: &Eng, campaign: Campaign) -> bool {
    let Some(state) = service_state(engine) else {
        return false;
    };
    if require_resource_kind(&campaign.arn, ResourceKind::Campaign).is_err() {
        return false;
    }
    let mut guard = state.lock();
    if guard.campaigns.contains_key(&campaign.arn) {
        return false;
    }
    guard.campaigns.insert(campaign.arn.clone(), campaign);
    true
}

/// Register a domain recommender.
///
/// Returns `false` without touching anything when the ARN references no recommender at
/// all — an empty value, or one whose resource type names another resource
/// ([`require_resource_kind`], the same check every read path applies) — when the ARN is
/// already registered, when `use_case` is not one of [`RECOMMENDER_USE_CASES`] (the
/// control plane refuses an unknown use case at `CreateRecommender`, so the runtime never
/// sees one), or when `engine` does not carry this service's state.
pub fn register_recommender<Eng: Send + Sync + 'static>(
    engine: &Eng,
    recommender: Recommender,
) -> bool {
    let Some(state) = service_state(engine) else {
        return false;
    };
    if require_resource_kind(&recommender.arn, ResourceKind::Recommender).is_err()
        || use_case_required_input(&recommender.use_case).is_none()
    {
        return false;
    }
    let mut guard = state.lock();
    if guard.recommenders.contains_key(&recommender.arn) {
        return false;
    }
    guard
        .recommenders
        .insert(recommender.arn.clone(), recommender);
    true
}

/// Register a filter, validating its expression the way `CreateFilter` does.
///
/// `Err` carries the parse error, so an expression the runtime could not apply is
/// refused where the filter is created — never at inference time, and never by
/// quietly filtering nothing. `Ok(false)` means the ARN references no filter at all (an
/// empty value, or one whose resource type names another resource —
/// [`require_resource_kind`], the same check every read path applies), the ARN is
/// already registered, or `engine` does not carry this service's state.
pub fn register_filter<Eng: Send + Sync + 'static>(
    engine: &Eng,
    arn: impl Into<String>,
    expression: impl Into<String>,
) -> Result<bool, String> {
    let arn = arn.into();
    let expression = expression.into();
    let parsed = filter::parse(&expression)?;
    let Some(state) = service_state(engine) else {
        return Ok(false);
    };
    if require_resource_kind(&arn, ResourceKind::Filter).is_err() {
        return Ok(false);
    }
    let mut guard = state.lock();
    if guard.filters.contains_key(&arn) {
        return Ok(false);
    }
    guard.filters.insert(
        arn.clone(),
        Filter {
            arn,
            expression,
            parsed,
        },
    );
    Ok(true)
}

/// The registered campaign under `arn`, cloned — the read side of the seam, for an
/// emulator-internal caller that wants to see what it registered.
pub fn campaign<Eng: Send + Sync + 'static>(engine: &Eng, arn: &str) -> Option<Campaign> {
    service_state(engine)?.lock().campaigns.get(arn).cloned()
}

/// The registered recommender under `arn`, cloned.
pub fn recommender<Eng: Send + Sync + 'static>(engine: &Eng, arn: &str) -> Option<Recommender> {
    service_state(engine)?.lock().recommenders.get(arn).cloned()
}

/// The registered filter under `arn`, cloned.
pub fn stored_filter<Eng: Send + Sync + 'static>(engine: &Eng, arn: &str) -> Option<Filter> {
    service_state(engine)?.lock().filters.get(arn).cloned()
}

/// Every `recommendationId` this service has issued recently, oldest first, and the
/// total number issued.
pub fn issued_recommendation_ids<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<(Vec<String>, u64)> {
    let state = service_state(engine)?;
    let guard = state.lock();
    Some((
        guard.recent_recommendation_ids.clone(),
        guard.recommendations_issued,
    ))
}
