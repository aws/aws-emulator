// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-servicecatalog-aws242servicecatalogservice service.

mod cloudformation;

pub use cloudformation::*;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::SystemTime;

/// How a portfolio-principal association's identifier is resolved: its own concern, in
/// its own file.
mod principal_identity;

pub use principal_identity::{
    association_principal_identifier, principal_identifier, request_caller_principal,
    request_principal_arn, CallerPrincipal, IamState, PRINCIPAL_TYPE_IAM_PATTERN,
};

/// What the stack behind a provisioned product is TAGGED with: its own concern, in its
/// own file.
mod launch_tags;

pub use launch_tags::*;

/// The identity a call this service makes OF ITS OWN presents: its own concern, in its own
/// file.
mod service_principal_call;

pub use service_principal_call::{ServicePrincipalCall, SERVICE_CATALOG_SERVICE_PRINCIPAL};

use crate::ops::types::{
    BudgetDetail, ConstraintDetail, ConstraintSummary, PortfolioDetail, ProductViewDetail,
    ProductViewSummary, ProvisionedProductAttribute, ProvisionedProductDetail,
    ProvisionedProductPlanDetails, ProvisioningArtifact as ModelledProvisioningArtifact,
    ProvisioningArtifactDetail, ProvisioningArtifactSummary, RecordDetail, RecordError, RecordTag,
    ResourceDetail, ServiceActionDetail, ServiceActionSummary, SourceConnectionDetail, Tag,
    TagOptionDetail, UpdateProvisioningParameter,
};

/// The `ResourceType` naming a portfolio, one of the two kinds a TagOption can be
/// associated with.
///
/// The model documents exactly these two values for the member that narrows a
/// TagOption's resources (`smithy_query query="doc"
/// shape="ListResourcesForTagOptionInput" member="ResourceType"`), and they are the
/// two kinds `AssociateTagOptionWithResource` accepts as a `ResourceId`.
pub const RESOURCE_TYPE_PORTFOLIO: &str = "Portfolio";

/// The `ResourceType` naming a product; see [`RESOURCE_TYPE_PORTFOLIO`].
pub const RESOURCE_TYPE_PRODUCT: &str = "Product";

/// The prefix every portfolio id carries, which is what tells a member that takes
/// "either a portfolio-id or a product-id" which of the two kinds it was handed.
pub const PORTFOLIO_ID_PREFIX: &str = "port-";

/// The kind of resource a `ResourceId` identifies, read from the prefix a Service
/// Catalog id carries: a portfolio id names a portfolio ([`RESOURCE_TYPE_PORTFOLIO`])
/// and every other id names a product ([`RESOURCE_TYPE_PRODUCT`]).
///
/// What a resource-addressing operation needs to say WHICH resource it could not find,
/// for an id no stored resource of either kind answers to: the id itself is all the
/// request carries, and its prefix is what names the kind.
pub fn addressed_resource_kind(resource_id: &str) -> &'static str {
    if resource_id.starts_with(PORTFOLIO_ID_PREFIX) {
        RESOURCE_TYPE_PORTFOLIO
    } else {
        RESOURCE_TYPE_PRODUCT
    }
}

/// The sentence an operation that takes a `ResourceId` of either kind refuses an id
/// that resolves to no stored resource with: the kind that id identifies, the id, and
/// nothing else.
pub fn resource_not_found_message(resource_id: &str) -> String {
    format!(
        "{} {resource_id} not found.",
        addressed_resource_kind(resource_id)
    )
}

/// A persisted portfolio — the record every portfolio operation reads and writes.
///
/// Tags are a map rather than a list because the API addresses them by key:
/// `UpdatePortfolio`'s `AddTags` overwrites an existing key's value and
/// `RemoveTags` deletes by key, and a portfolio therefore cannot hold the same
/// key twice. The map is a `BTreeMap` so the `Tags` list a response carries has
/// a stable, deterministic order.
#[derive(Debug, Clone)]
pub struct Portfolio {
    /// The minted portfolio id (`port-…`), also this record's key in [`ComAmazonawsServicecatalogAws242servicecatalogserviceState::portfolios`].
    pub id: String,
    /// The ARN minted at create time and read back verbatim afterwards.
    pub arn: String,
    pub display_name: String,
    pub description: Option<String>,
    pub provider_name: String,
    /// Creation instant, minted once at create time.
    pub created_time: SystemTime,
    /// Tag key -> tag value.
    pub tags: BTreeMap<String, String>,
    /// The principals associated with this portfolio, keyed by principal ARN: the
    /// API addresses an association BY that ARN, so re-associating one ARN changes
    /// its type rather than listing it twice. The stored value carries the identifier
    /// a read of the association REPORTS for its principal, and the order the
    /// associations were made in, which is the order
    /// `ListPrincipalsForPortfolio` reports and pages over.
    pub principals: BTreeMap<String, PrincipalAssociation>,
    /// The TagOptions associated with this portfolio, by TagOption id. A set because
    /// `AssociateTagOptionWithResource` addresses an association by the (resource,
    /// TagOption) PAIR, so one resource holds each TagOption at most once.
    pub tag_option_ids: BTreeSet<String>,
    /// The budgets associated with this portfolio, by budget NAME — the only member
    /// the modelled `BudgetDetail` carries. A set because
    /// `AssociateBudgetWithResource` addresses an association by the (resource,
    /// budget) pair, so one resource holds each budget at most once, and a `BTreeSet`
    /// so every report of them walks one stable order.
    pub budget_names: BTreeSet<String>,
}

/// One portfolio-principal association: what was associated, and when in the
/// portfolio's life.
#[derive(Debug, Clone)]
pub struct PrincipalAssociation {
    /// `PrincipalType` (`IAM` | `IAM_PATTERN`), as the request declared it.
    pub principal_type: String,
    /// The identifier this association reports for its principal, resolved ONCE when
    /// the association was stored: the principal's own unique id in the account's
    /// identity records for a concrete (`IAM`) principal, and the pattern itself for
    /// an `IAM_PATTERN`, which names no single principal. See
    /// [`association_principal_identifier`].
    pub principal_identifier: String,
    /// The position of this association in the order they were made, so a list
    /// reports them in that order however the ARNs sort. Re-associating an ARN keeps
    /// its original position: it is the same association with a new type.
    pub sequence: u64,
}

impl Portfolio {
    /// This record as the modelled `PortfolioDetail`: every member comes from
    /// stored state, so Create/Describe/List/Update all report the same values.
    pub fn detail(&self) -> PortfolioDetail {
        PortfolioDetail {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            display_name: Some(self.display_name.clone()),
            description: self.description.clone(),
            created_time: Some(reported_time(self.created_time)),
            provider_name: Some(self.provider_name.clone()),
        }
    }

    /// This record as the modelled `ResourceDetail` — the shape a list of the
    /// resources associated with a TagOption carries. Every member is the same
    /// stored value [`Self::detail`] reports, so one portfolio cannot read two
    /// ways; a portfolio's `Name` is its display name.
    pub fn resource_detail(&self) -> ResourceDetail {
        ResourceDetail {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.display_name.clone()),
            description: self.description.clone(),
            created_time: Some(reported_time(self.created_time)),
        }
    }

    /// The stored tags as the modelled `Tags` list, in key order.
    pub fn tag_list(&self) -> Vec<Tag> {
        tag_list(&self.tags)
    }

    /// The stored associations in the order they were made — the one order every
    /// response that reports them walks.
    ///
    /// Neither the model nor AWS's documentation fixes an order for
    /// `ListPrincipalsForPortfolio`, so this service picks one and reports it
    /// consistently: the order the associations were made in, which is also the
    /// order its page cursor walks.
    pub fn principals_in_association_order(&self) -> Vec<(&String, &PrincipalAssociation)> {
        let mut associations: Vec<(&String, &PrincipalAssociation)> =
            self.principals.iter().collect();
        associations.sort_by_key(|(_, association)| association.sequence);
        associations
    }

    /// Whether this portfolio GRANTS `caller` access to what it carries.
    ///
    /// "When provisioning a product that's been added to a portfolio, you must grant your
    /// user, group, or role access to the portfolio" (`ListLaunchPaths`): the grant IS one
    /// of the stored associations, and it is in force for a caller one of them names
    /// ([`CallerPrincipal::names`]). A portfolio that holds no association grants nobody,
    /// and a grant removed by `DisassociatePrincipalFromPortfolio` stops being in force at
    /// once — the decision is read from the associations the store holds NOW, so it is
    /// never a latch.
    pub fn grants(&self, caller: &CallerPrincipal) -> bool {
        self.principals
            .iter()
            .any(|(arn, association)| caller.names(arn, &association.principal_type))
    }
}

/// One stored instant as the timestamp a response carries.
///
/// Both wire formats this service declares (`smithy_query(query="service_info")`:
/// protocols `["awsJson1_1", "rpcv2Cbor"]`) put a timestamp on the wire as epoch seconds,
/// and the finest resolution the `Timestamp` they carry resolves to there is the
/// MILLISECOND: an instant is reported truncated to whole milliseconds, so a stored
/// value read from the host clock — which resolves finer than that — is reported at the
/// resolution the service reports rather than at the one the host happens to keep.
/// Truncation is monotone, so a later instant never reports as an earlier one and two
/// members of one record keep their order.
///
/// Every timestamp this service reports goes through this one function, so two reporters
/// of one stored instant cannot report it two different ways.
pub fn reported_time(instant: SystemTime) -> ::aws_smithy_types::DateTime {
    let stamp = ::aws_smithy_types::DateTime::from(instant);
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        stamp.secs(),
        stamp.subsec_nanos() / NANOS_PER_MILLI * NANOS_PER_MILLI,
    )
}

/// The nanoseconds in one millisecond, which [`reported_time`] truncates to.
const NANOS_PER_MILLI: u32 = 1_000_000;

/// A tag map as the modelled `Tags` list, in key order: every operation that
/// reports a resource's tags renders them through this one function, so they read
/// the same everywhere.
pub fn tag_list(tags: &BTreeMap<String, String>) -> Vec<Tag> {
    tags.iter()
        .map(|(key, value)| Tag {
            key: key.clone(),
            value: value.clone(),
        })
        .collect()
}

/// Which of the two members that address one resource the request gave.
///
/// Several of this service's inputs come in that pair — a product, a launch path, a
/// product version — and every pair carries the same contract: "You must provide the
/// name or ID, but not both." Shared, so the operations that resolve the same triple
/// (a provisioning request and the read that precedes it) cannot read one request two
/// different ways.
#[derive(Debug, Clone, Copy)]
pub enum Addressed<'a> {
    Id(&'a str),
    Name(&'a str),
}

impl Addressed<'_> {
    /// The value the request addressed the resource by, which a refusal names.
    pub fn identifier(&self) -> &str {
        match self {
            Addressed::Id(id) => id,
            Addressed::Name(name) => name,
        }
    }
}

/// How the request addressed one resource: by `id`, by `name`, or by neither
/// (`None`). `Err` carries the refusal for a request that gave BOTH, which the
/// members' own documentation forbids.
///
/// An empty string addresses nothing: the modelled minimum length of each of these
/// members is 1, so a member that arrives empty names no resource.
pub fn addressed<'a>(
    id: Option<&'a str>,
    name: Option<&'a str>,
    id_member: &str,
    name_member: &str,
) -> Result<Option<Addressed<'a>>, String> {
    match (
        id.filter(|value| !value.is_empty()),
        name.filter(|value| !value.is_empty()),
    ) {
        (Some(_), Some(_)) => Err(format!(
            "{id_member} and {name_member} cannot both be provided: you must provide the name or \
             ID, but not both."
        )),
        (Some(id), None) => Ok(Some(Addressed::Id(id))),
        (None, Some(name)) => Ok(Some(Addressed::Name(name))),
        (None, None) => Ok(None),
    }
}

/// The refusal for a request that named a resource it must address by neither of the
/// two members that address it.
pub fn either_required(id_member: &str, name_member: &str) -> String {
    format!("Either {id_member} or {name_member} must be provided.")
}

/// The sentence this service refuses a request for a product that resolves no launch
/// path with: the product sits in no portfolio, or nothing stored carries the
/// identifier at all.
pub fn no_launch_paths_message(identifier: &str) -> String {
    format!("No launch paths found for resource: {identifier}")
}

/// The sentence every not-found in this service reports for an identifier that
/// resolves to nothing: the KIND of resource the request addressed and the identifier
/// it used.
pub fn not_found_message(kind: &str, identifier: &str) -> String {
    format!("{kind} not found: {identifier}")
}

/// The sentence a read whose answer lives in a provisioned product's CloudFormation
/// stack refuses an identifier with: that read resolves the stack the launch stood up,
/// so an identifier naming no stack is refused as the STACK LOOKUP failed, in
/// CloudFormation's own words. All of it is fixed text except the name the lookup used,
/// which is the identifier the request carried.
pub fn no_stack_named_message(stack_name: &str) -> String {
    format!("No stack named {stack_name} exists.")
}

/// The sentence the END-USER product VIEW read refuses an `Id` that resolves no visible
/// product view with: it names the member the request addressed the view by and echoes
/// the value it carried, which is the only variable part.
///
/// Worded on its own rather than shared with the end-user product read
/// ([`access_or_existence_message`]), because Service Catalog words each read's not-found
/// for the resource that read addresses: this one addresses a product VIEW by
/// `ProductViewId`, and the two templates must not be shared.
pub fn product_view_not_found_message(product_view_id: &str) -> String {
    format!("Product not found for ProductViewId: {product_view_id}")
}

/// The sentence the END-USER product views refuse an identifier with: one sentence that
/// covers both reasons such a read can fail — nothing stored carries the identifier, and
/// a stored product this view cannot reach through any launch path — so a caller cannot
/// learn from the refusal that a product it has no access to exists. The identifier the
/// request addressed the product by is the only variable part.
///
/// The end-user read of a PRODUCT (`DescribeProduct`, which addresses it by `Id` or
/// `Name`) words its refusal this way. The end-user read of a product VIEW addresses a
/// different resource and words its own ([`product_view_not_found_message`]), and the
/// ADMIN views see every stored product and so report [`not_found_message`] instead;
/// the three are deliberately not shared.
pub fn access_or_existence_message(identifier: &str) -> String {
    format!("{identifier} does not exist or access was denied")
}

/// The resource kind a launch path is, as its not-found names it.
pub const KIND_LAUNCH_PATH: &str = "Launch path";

/// The prefix every ARN carries, which is what tells a member that takes a name OR an
/// ARN which of the two it was given.
pub const ARN_PREFIX: &str = "arn:";

/// The resource kind a provisioned product is, as its not-found names it.
pub const KIND_PROVISIONED_PRODUCT: &str = "Provisioned product";

/// The resource kind a request operation's record is, as its not-found names it.
pub const KIND_RECORD: &str = "Record";

/// The resource kind a product version is, as its not-found names it.
pub const KIND_PROVISIONING_ARTIFACT: &str = "Provisioning artifact";

/// The resource kind the operations that take "the resource identifier" of a portfolio
/// OR a product report in their not-found: the request named neither kind, so the
/// refusal names the generic resource it addressed rather than guessing which map the
/// caller meant.
pub const KIND_RESOURCE: &str = "Resource";

/// The refusal for a request that names NEITHER `PathId` nor `PathName` for a product
/// whose launch path cannot be inferred — "this value is optional if the product has a
/// default path, and required if the product has more than one path", so a product that
/// resolves anything other than exactly one path leaves the pair required.
///
/// AWS's own sentence for that refusal, which is all fixed text: it names the two
/// members and carries no value from the request or from stored state. Shared by every
/// operation that resolves a launch, so one request cannot be refused two ways.
pub const PATH_REQUIRED: &str = "Either Path Id or Path Name needs to be specified";

/// The refusal for a request whose `PathId` or `PathName` identifies none of the launch
/// paths it is read against — including every path of a product that resolves none.
///
/// AWS words this error class by naming the member the request used and echoing the
/// value it carried, which is the one variable part: the launch path is a parameter of
/// the request, so a value that identifies no launch path is a wrong parameter rather
/// than a resource the caller may wait for.
pub fn invalid_path_message(member: &str, identifier: &str) -> String {
    format!("Invalid {member}: {identifier}")
}

/// A persisted constraint — the record `CreateConstraint` mints and
/// `DescribeConstraint` reports.
#[derive(Debug, Clone)]
pub struct Constraint {
    /// The minted constraint id (`cons-…`).
    pub id: String,
    /// `ConstraintType` (`LAUNCH`, `NOTIFICATION`, `STACKSET`, `TEMPLATE`, …), as
    /// the request declared it: the model constrains it only by length, so the
    /// stored value is the requested one.
    pub constraint_type: String,
    pub description: Option<String>,
    /// The constraint's `Parameters` document, stored verbatim so a describe
    /// reports the parameters the create was given.
    pub parameters: String,
    pub portfolio_id: String,
    pub product_id: String,
    /// The instant this constraint stops being `CREATING` and becomes `AVAILABLE`.
    /// `CreateConstraint` answers in the non-terminal `CREATING` status, so the
    /// transition to the terminal one is owed by the CLOCK: it is stored as the
    /// instant it falls due and never slept for, and every report derives the
    /// status from it rather than from who asked.
    pub available_at: SystemTime,
}

/// How long after its create a constraint stays `CREATING`.
///
/// A span this emulator CHOOSES (AWS documents none): long enough that a caller
/// acting straight after the create still sees the transient status and the state
/// it implies, short enough that nothing waits on it.
pub const CONSTRAINT_AVAILABLE_AFTER: std::time::Duration = std::time::Duration::from_millis(250);

/// `Status`'s terminal value for a constraint that is ready for use
/// (`smithy_query query="shape" shape="Status"`: `AVAILABLE` | `CREATING` |
/// `FAILED`).
pub const STATUS_AVAILABLE: &str = "AVAILABLE";

/// `Status`'s non-terminal value, which the model documents as "creation has
/// started; the [resource] is not ready for use".
pub const STATUS_CREATING: &str = "CREATING";

impl Constraint {
    /// This constraint's `Status` at `now`: the non-terminal one until the clock
    /// owes the transition, the terminal one afterwards.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.available_at {
            STATUS_CREATING
        } else {
            STATUS_AVAILABLE
        }
    }

    /// This record as the modelled `ConstraintDetail`.
    pub fn detail(&self) -> ConstraintDetail {
        ConstraintDetail {
            constraint_id: Some(self.id.clone()),
            r#type: Some(self.constraint_type.clone()),
            description: self.description.clone(),
            // `ConstraintDetail$Owner` targets `AccountId`, so it is the account that
            // created the constraint — this emulator's canonical account.
            owner: Some(::emulator_server::emulator_account_id().to_string()),
            product_id: Some(self.product_id.clone()),
            portfolio_id: Some(self.portfolio_id.clone()),
        }
    }

    /// This record as the modelled `ConstraintSummary` — the shorter form every
    /// response that reports the constraints ON a portfolio-product relationship
    /// carries, which the model gives only a type and a description.
    pub fn summary(&self) -> ConstraintSummary {
        ConstraintSummary {
            r#type: Some(self.constraint_type.clone()),
            description: self.description.clone(),
        }
    }
}

/// A persisted product — the record every product operation reads and writes.
///
/// A product owns its provisioning artifacts (its "versions"): the model binds every
/// artifact to one product (`CreateProvisioningArtifact` takes a `ProductId`), so they
/// are stored inside the product record rather than in a second, parallel map.
#[derive(Debug, Clone)]
pub struct Product {
    /// The minted product id (`prod-…`), also this record's key in
    /// [`ComAmazonawsServicecatalogAws242servicecatalogserviceState::products`].
    pub id: String,
    /// The minted product VIEW id (`prodview-…`). `ProductViewSummary` carries both
    /// `Id` (the view) and `ProductId` (the product), so they are distinct values.
    pub view_id: String,
    /// The ARN minted at create time and read back verbatim afterwards.
    pub arn: String,
    pub name: String,
    pub owner: String,
    /// `ProductViewSummary.ShortDescription`, which `CreateProduct` takes as
    /// `Description`.
    pub short_description: Option<String>,
    pub distributor: Option<String>,
    pub support_email: Option<String>,
    pub support_description: Option<String>,
    pub support_url: Option<String>,
    /// A `ProductType` value, as the request gave it.
    pub product_type: String,
    /// The product's terminal `Status`, [`PRODUCT_STATUS_CREATED`] from create time on.
    /// Read through [`Product::status`], which answers the non-terminal one while the
    /// clock still owes this product's transition.
    pub status: String,
    /// Creation instant, minted once at create time.
    pub created_time: SystemTime,
    /// The instant this product stops being `CREATING` and reads as [`Self::status`].
    ///
    /// A product `CreateProduct` minted is ready at once — AWS answers that create with
    /// a terminal status — so its due instant IS its create instant. A product
    /// `CopyProduct` is still filling in is the one that reads as `CREATING`: the copy
    /// is asynchronous, so the transition is owed by the CLOCK, stored as the instant it
    /// falls due and never slept for.
    pub available_at: SystemTime,
    /// Tag key -> tag value.
    pub tags: BTreeMap<String, String>,
    /// The connection the product syncs from, when the create request named one.
    pub source_connection: Option<SourceConnectionDetail>,
    /// This product's provisioning artifacts, keyed by artifact id. A `BTreeMap` so
    /// every list of them has a stable, deterministic order.
    pub provisioning_artifacts: BTreeMap<String, ProvisioningArtifact>,
    /// The portfolios this product is associated with, by portfolio id. A product may
    /// live in several, and `AssociateProductWithPortfolio` addresses one association
    /// by the PAIR, so the set holds each portfolio once.
    pub portfolio_ids: std::collections::BTreeSet<String>,
    /// The TagOptions associated with this product, by TagOption id — the same
    /// association [`Portfolio::tag_option_ids`] holds, because
    /// `AssociateTagOptionWithResource` takes either kind of resource.
    pub tag_option_ids: BTreeSet<String>,
    /// The budgets associated with this product, by budget name — the same
    /// association [`Portfolio::budget_names`] holds, because
    /// `AssociateBudgetWithResource` takes either kind of resource.
    pub budget_names: BTreeSet<String>,
}

/// The `Status` a product carries from the moment it is created.
///
/// `CreateProduct` answers a product that is ready to use — the recorded AWS responses
/// this emulator is graded against carry `CREATED`, and the calls that follow a create
/// use the new `ProductId` immediately — so the record is stored in that one terminal
/// status and no clock transition is owed. A product a `CopyProduct` is still filling
/// in reads as [`STATUS_CREATING`] until that copy's due instant and as this value
/// afterwards. The modelled `Status` enum (`smithy_query query="shape" shape="Status"`)
/// additionally admits `AVAILABLE` and `FAILED`, which only the provisioning paths this
/// emulator has not implemented yet can produce.
pub const PRODUCT_STATUS_CREATED: &str = "CREATED";

impl Product {
    /// This product's `Status` at `now`: `CREATING` — which the model documents as
    /// "product creation has started; the product is not ready for use" — until the
    /// clock owes this product's transition, and the stored terminal status afterwards.
    pub fn status(&self, now: SystemTime) -> &str {
        if now < self.available_at {
            STATUS_CREATING
        } else {
            &self.status
        }
    }

    /// This record as the modelled `ProductViewSummary`, with `HasDefaultPath` derived
    /// from the portfolios that carry the product — the reading an ADMIN view takes,
    /// which is about the product rather than about one caller's access.
    pub fn view_summary(&self) -> ProductViewSummary {
        self.view_summary_with_paths(self.portfolio_ids.len())
    }

    /// This record as the modelled `ProductViewSummary` for a reader that has already
    /// resolved how many launch paths it can see — the END-USER views, whose paths are
    /// the portfolios that grant the calling principal access
    /// ([`ComAmazonawsServicecatalogAws242servicecatalogserviceState::launch_path_portfolios`]).
    ///
    /// Reported from the same count the response's own `LaunchPaths` is built from, so one
    /// response cannot say a product has a default path and then report no path at all.
    pub fn view_summary_with_paths(&self, launch_paths: usize) -> ProductViewSummary {
        ProductViewSummary {
            id: Some(self.view_id.clone()),
            product_id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            owner: Some(self.owner.clone()),
            short_description: self.short_description.clone(),
            r#type: Some(self.product_type.clone()),
            distributor: self.distributor.clone(),
            // "If the product does not have a default path, call ListLaunchPaths to
            // disambiguate between paths" — a product reachable through exactly one
            // path needs no disambiguation, so that one path is its default; zero or
            // several paths leave it without one.
            has_default_path: launch_paths == 1,
            support_email: self.support_email.clone(),
            support_description: self.support_description.clone(),
            support_url: self.support_url.clone(),
        }
    }

    /// This record as the modelled `ProductViewDetail`: every member comes from stored
    /// state, so Create and Describe report the same values. `now` is what the derived
    /// [`Self::status`] is read at, so every reporter of one product agrees.
    pub fn detail(&self, now: SystemTime) -> ProductViewDetail {
        ProductViewDetail {
            product_view_summary: Some(self.view_summary()),
            status: Some(self.status(now).to_string()),
            product_arn: Some(self.arn.clone()),
            created_time: Some(reported_time(self.created_time)),
            source_connection: self.source_connection.clone(),
        }
    }

    /// This record as the modelled `ResourceDetail`, the counterpart of
    /// [`Portfolio::resource_detail`]: a product's `Description` is the short
    /// description its `ProductViewSummary` carries, so a product reads the same
    /// here as it does there.
    pub fn resource_detail(&self) -> ResourceDetail {
        ResourceDetail {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            description: self.short_description.clone(),
            created_time: Some(reported_time(self.created_time)),
        }
    }

    /// The stored tags as the modelled `Tags` list, in key order.
    pub fn tag_list(&self) -> Vec<Tag> {
        tag_list(&self.tags)
    }
}

/// One persisted provisioning artifact (a product "version").
#[derive(Debug, Clone)]
pub struct ProvisioningArtifact {
    /// The minted artifact id (`pa-…`).
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    /// A `ProvisioningArtifactType` value, or `None` when neither the request nor the
    /// product type named one.
    pub r#type: Option<String>,
    /// Whether the artifact can be used to provision; a created artifact is active.
    pub active: bool,
    /// A `ProvisioningArtifactGuidance` value: [`ARTIFACT_GUIDANCE_DEFAULT`] until an
    /// update deprecates it.
    pub guidance: String,
    /// Creation instant, minted once at create time.
    pub created_time: SystemTime,
    /// The artifact's `Info` template source, as the request gave it.
    pub info: BTreeMap<String, String>,
    /// The version this one was copied from, for an artifact `CopyProduct` created;
    /// `None` for one a create minted, which was copied from nothing.
    pub copied_from: Option<ArtifactCopyLineage>,
}

/// Where a copied provisioning artifact came from: the product that was copied and the
/// version of it this one is a copy of.
///
/// A copy records its IMMEDIATE source, so copying a copy reports the product it was
/// copied from rather than the one at the head of the chain.
#[derive(Debug, Clone)]
pub struct ArtifactCopyLineage {
    /// The id of the product this version was copied from.
    pub source_product_id: String,
    /// The id that version carries in the product it was copied from.
    pub source_provisioning_artifact_id: String,
}

/// The `ProvisioningArtifactMetadata` key carrying the product a copied version was
/// copied from.
///
/// The metadata map is open (`ProvisioningArtifactInfo`, a string -> string map
/// `smithy_query query="shape" shape="ProvisioningArtifactInfo"`), so the keys a
/// version carries are the ones the service that created it recorded: a copied version
/// reports its source product and source version under these two.
pub const ARTIFACT_METADATA_SOURCE_PRODUCT_ID: &str = "SourceProductId";

/// The `ProvisioningArtifactMetadata` key carrying the version a copied version was
/// copied from; see [`ARTIFACT_METADATA_SOURCE_PRODUCT_ID`].
pub const ARTIFACT_METADATA_SOURCE_PROVISIONING_ARTIFACT_ID: &str =
    "SourceProvisioningArtifactId";

/// The `Guidance` a newly created provisioning artifact carries.
///
/// The modelled `ProvisioningArtifactGuidance` enum is `DEFAULT | DEPRECATED`
/// (`smithy_query query="shape" shape="ProvisioningArtifactGuidance"`), and
/// `UpdateProvisioningArtifact`'s `Guidance` member is what moves an artifact to
/// `DEPRECATED`, so a create leaves it at `DEFAULT`.
pub const ARTIFACT_GUIDANCE_DEFAULT: &str = "DEFAULT";

impl ProvisioningArtifact {
    /// This record as the modelled `ProvisioningArtifactDetail`.
    pub fn detail(&self) -> ProvisioningArtifactDetail {
        ProvisioningArtifactDetail {
            id: Some(self.id.clone()),
            name: self.name.clone(),
            description: self.description.clone(),
            r#type: self.r#type.clone(),
            created_time: Some(reported_time(self.created_time)),
            active: Some(self.active),
            guidance: Some(self.guidance.clone()),
            // A source revision exists only for an artifact a connection sync created,
            // and no sync has run in this emulator.
            source_revision: None,
        }
    }

    /// This record as the modelled `ProvisioningArtifactSummary`.
    pub fn summary(&self) -> ProvisioningArtifactSummary {
        ProvisioningArtifactSummary {
            id: Some(self.id.clone()),
            name: self.name.clone(),
            description: self.description.clone(),
            created_time: Some(reported_time(self.created_time)),
            provisioning_artifact_metadata: self.metadata(),
        }
    }

    /// This version's `ProvisioningArtifactMetadata`: the `Info` template source it was
    /// created with, plus — for a version a copy created — the source product and source
    /// version it was copied from, so a reader of a copy can tell what it is a copy of.
    ///
    /// `ProvisioningArtifactInfo` holds between 1 and 100 entries, so a version with
    /// nothing to report carries no map rather than an empty one.
    fn metadata(&self) -> Option<std::collections::HashMap<String, String>> {
        let mut metadata: std::collections::HashMap<String, String> = self
            .info
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        if let Some(lineage) = &self.copied_from {
            metadata.insert(
                ARTIFACT_METADATA_SOURCE_PRODUCT_ID.to_string(),
                lineage.source_product_id.clone(),
            );
            metadata.insert(
                ARTIFACT_METADATA_SOURCE_PROVISIONING_ARTIFACT_ID.to_string(),
                lineage.source_provisioning_artifact_id.clone(),
            );
        }
        Some(metadata).filter(|metadata| !metadata.is_empty())
    }

    /// This record as the modelled `ProvisioningArtifact`, the shorter shape
    /// `DescribeProduct` lists a product's versions as.
    pub fn listed(&self) -> ModelledProvisioningArtifact {
        ModelledProvisioningArtifact {
            id: Some(self.id.clone()),
            name: self.name.clone(),
            description: self.description.clone(),
            created_time: Some(reported_time(self.created_time)),
            guidance: Some(self.guidance.clone()),
        }
    }
}

/// One asynchronous product copy — the record `CopyProduct` mints and
/// `DescribeCopyProductStatus` reports, addressed by the token the copy answered.
///
/// The copy itself is performed where it is asked for, so the target product and the
/// versions it gained are stored the moment the copy starts and every reader of the
/// catalog sees the same thing. What the CLOCK still owes is the copy's own
/// completion: the operation is documented as asynchronous, so it reads as
/// `IN_PROGRESS` until [`Self::succeeded_at`].
#[derive(Debug, Clone)]
pub struct ProductCopy {
    /// The minted `CopyProductToken` (`copy-product-…`), also this record's key in
    /// [`ComAmazonawsServicecatalogAws242servicecatalogserviceState::product_copies`].
    pub token: String,
    /// The product the versions were copied FROM.
    pub source_product_id: String,
    /// The product they were copied INTO — the one the request named, or the one this
    /// copy created.
    pub target_product_id: String,
    /// The instant this copy stops being `IN_PROGRESS` and becomes `SUCCEEDED`, stored
    /// as the instant it falls due and never slept for.
    pub succeeded_at: SystemTime,
}

/// `CopyProductStatus`'s non-terminal value, which the operation answers while the
/// asynchronous copy it started is still owed
/// (`smithy_query query="shape" shape="CopyProductStatus"`: `SUCCEEDED` |
/// `IN_PROGRESS` | `FAILED`).
pub const COPY_PRODUCT_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// `CopyProductStatus`'s terminal value for a copy that completed.
pub const COPY_PRODUCT_STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// How long after the call that started it a product copy stays `IN_PROGRESS`.
///
/// A span this emulator CHOOSES (AWS documents none): never zero, so a caller acting
/// straight after `CopyProduct` still sees the non-terminal status and the
/// not-yet-ready product it implies, and short enough that nothing waits on it.
pub const PRODUCT_COPY_SUCCEEDS_AFTER: std::time::Duration = std::time::Duration::from_secs(3);

/// The most copy records this store keeps, which bounds it.
///
/// A copy's record is what `DescribeCopyProductStatus` reads, so it outlives the call
/// that made it; a caller copying into one target product repeatedly would otherwise
/// grow this map without bound. As with [`MAX_PRODUCTS`] this is the emulator's own
/// bound, not a quoted AWS figure: the newest this many copies are the ones whose
/// token still resolves.
pub const MAX_PRODUCT_COPIES: usize = 500;

impl ProductCopy {
    /// This copy's `CopyProductStatus` at `now`: the non-terminal one until the clock
    /// owes its completion, the terminal one afterwards.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.succeeded_at {
            COPY_PRODUCT_STATUS_IN_PROGRESS
        } else {
            COPY_PRODUCT_STATUS_SUCCEEDED
        }
    }
}

/// A persisted provisioned product — the resourced instance of a product that
/// `ProvisionProduct` mints and `DescribeProvisionedProduct` and
/// `SearchProvisionedProducts` report.
#[derive(Debug, Clone)]
pub struct ProvisionedProduct {
    /// The minted provisioned-product id (`pp-…`), also this record's key in
    /// [`ComAmazonawsServicecatalogAws242servicecatalogserviceState::provisioned_products`].
    pub id: String,
    /// The ARN minted at provision time and read back verbatim afterwards.
    pub arn: String,
    /// `ProvisionedProductName`, which the model documents "must be unique for the
    /// Amazon Web Services account and cannot be updated after the product is
    /// provisioned" — so it addresses this record as firmly as its id does.
    pub name: String,
    /// A `ProvisionedProductType` value, derived from the product's `ProductType`
    /// by [`provisioned_product_type`] when the provisioning request was served.
    pub r#type: String,
    pub product_id: String,
    pub provisioning_artifact_id: String,
    /// The launch path this product was provisioned through: the id of the
    /// portfolio that path is the product's relationship with.
    pub path_id: String,
    /// Creation instant, minted once at provision time.
    pub created_time: SystemTime,
    /// The `ProvisionToken` the provisioning request carried, reported back as
    /// `IdempotencyToken`.
    pub provision_token: String,
    /// Tag key -> tag value, as the provisioning request asked for them.
    pub tags: BTreeMap<String, String>,
    /// The records of the operations performed on this provisioned product, oldest
    /// first: the one stored history every `Last*RecordId` member is derived from,
    /// so two responses cannot disagree about which record was last.
    pub record_ids: Vec<String>,
    /// The instant the termination of this provisioned product settles, for one a
    /// `TerminateProvisionedProduct` has been accepted for; `None` for one that has
    /// not been terminated.
    ///
    /// A termination is not instantaneous either: until that instant the provisioned
    /// product still exists, with the termination's record in flight; from it on it
    /// exists no longer, which is the clock's doing and not any caller's
    /// ([`ProvisionedProduct::is_terminated`]).
    pub terminated_at: Option<SystemTime>,
    /// The Region this provisioned product was provisioned in, which is where the
    /// stack behind it lives: every later read of that stack looks for it there, and
    /// the Region a later request is signed for does not move it.
    pub region: String,
    /// The id CloudFormation minted for the stack this provisioned product's launch
    /// stood up, as that stack's own create answered it; `None` for a provisioned
    /// product no launch has stood a stack up for — one a PLAN created, one of a
    /// product type that is not CloudFormation-backed, or one whose stack
    /// CloudFormation refused.
    ///
    /// The stack is CloudFormation's resource, not this service's: this is the handle
    /// the launch was answered with, and what the stack holds — its status, its
    /// resources, its outputs — is read from CloudFormation every time rather than
    /// copied here.
    pub stack_id: Option<String>,
    /// Whether a PLAN brought this provisioned product into existence, rather than a
    /// provisioning request.
    ///
    /// Such a provisioned product has had no operation performed on it — "the plan
    /// operations were performed to provision a new product, but resources have not yet
    /// been created" — which is what makes it
    /// [`PROVISIONED_PRODUCT_STATUS_PLAN_IN_PROGRESS`] until one is
    /// ([`ComAmazonawsServicecatalogAws242servicecatalogserviceState::provisioned_product_status`]).
    /// It is stored rather than inferred from an empty history, because a history can
    /// also be empty for a provisioned product whose records the store evicted to stay
    /// within [`MAX_RECORDS`], and that provisioned product's last operation did
    /// complete.
    pub plan_created: bool,
}

impl ProvisionedProduct {
    /// Whether this provisioned product has stopped existing at `now`: its
    /// termination was accepted AND the clock owes that termination's completion.
    ///
    /// A terminated provisioned product is gone — "after the provisioned product is
    /// terminated, subsequent requests to terminate the same provisioned product
    /// always return ResourceNotFound" — so every lookup over the store skips it and
    /// its name is free for a later provisioning.
    pub fn is_terminated(&self, now: SystemTime) -> bool {
        self.terminated_at.is_some_and(|settled| now >= settled)
    }

    /// The stack this provisioned product's launch stood up, as CloudFormation addresses
    /// it: the id that launch was answered with. `None` for a provisioned product no
    /// launch has stood a stack up for, which therefore has no stack to read and no
    /// resources behind it.
    pub fn stack(&self) -> Option<&str> {
        self.stack_id.as_deref()
    }
}

/// A persisted plan — "a plan includes the list of resources to be created (when
/// provisioning a new product) or modified (when updating a provisioned product) when
/// the plan is executed", which `CreateProvisionedProductPlan` mints and
/// `DescribeProvisionedProductPlan` reports.
///
/// A plan belongs to ONE provisioned product — "you can create one plan for each
/// provisioned product" — so it names that provisioned product and every member a
/// response reports about it is read from this record or from the provisioned product
/// it names, never from the request that read it.
#[derive(Debug, Clone)]
pub struct ProvisionedProductPlan {
    /// The minted plan id (`plan-…`), also this record's key in
    /// [`ComAmazonawsServicecatalogAws242servicecatalogserviceState::provisioned_product_plans`].
    pub id: String,
    /// `PlanName`, as the request asked for it.
    pub name: String,
    /// A `ProvisionedProductPlanType` value — the model declares exactly
    /// `CLOUDFORMATION` (`smithy_query query="shape"
    /// shape="ProvisionedProductPlanType"`).
    pub plan_type: String,
    /// The provisioned product this plan changes: the one the request named, or the
    /// one the plan itself created, which stays in
    /// [`PROVISIONED_PRODUCT_STATUS_PLAN_IN_PROGRESS`] until the plan is executed.
    pub provisioned_product_id: String,
    /// The product the plan provisions a version of.
    pub product_id: String,
    /// The product version the plan provisions.
    pub provisioning_artifact_id: String,
    /// The launch path the plan provisions through: the id of the portfolio that path
    /// is the product's relationship with, resolved exactly as a provisioning request
    /// resolves it.
    pub path_id: String,
    /// The SNS topic ARNs the request passed, in request order — "passed to
    /// CloudFormation", which takes them as an ordered list.
    pub notification_arns: Vec<String>,
    /// The provisioning parameters the request carried, verbatim and in request
    /// order: the list `ProvisionedProductPlanDetails$ProvisioningParameters` reports
    /// back. A parameter carries `UsePreviousValue` beside its key and value, so the
    /// set is not a key -> value map.
    pub provisioning_parameters: Vec<UpdateProvisioningParameter>,
    /// Tag key -> tag value, as the request asked for them. A map rather than a list
    /// because a resource holds each tag key once, exactly as a provisioned product's
    /// tags are stored.
    pub tags: BTreeMap<String, String>,
    /// Creation instant, minted once when the plan was created. The instant the plan
    /// record was last written is this one until something EXECUTES the plan, which is
    /// the one operation that writes a stored plan again
    /// ([`ProvisionedProductPlan::execution`]).
    pub created_time: SystemTime,
    /// The `IdempotencyToken` the request that created this plan carried.
    pub idempotency_token: String,
    /// The Region the change set behind this plan was created in — the Region the
    /// request that created the plan was signed for.
    ///
    /// A change set belongs to the Region it was made in, so the plan is read back
    /// there: the Region of the request that READS a plan does not move the change set
    /// the plan is.
    pub region: String,
    /// The CloudFormation change set this plan IS: the one the create asked for, or the
    /// reason CloudFormation could not plan the change at all.
    ///
    /// `None` only between the instant the plan is stored and the instant the create
    /// records what CloudFormation answered — a plan whose change set is still being
    /// asked for, which is a plan still being created.
    pub change_set: Option<PlanChangeSet>,
    /// The execution of this plan: what `ExecuteProvisionedProductPlan` recorded when it
    /// provisioned or modified the product the plan planned a change to. `None` for a
    /// plan nothing has executed, which is the only plan an execution can be accepted
    /// for.
    pub execution: Option<PlanExecution>,
}

/// The execution of one plan: the record of the request that executed it, and when.
///
/// Executing a plan is one request operation ON the provisioned product the plan plans a
/// change to — "provisions or modifies a product based on the resource changes for the
/// specified plan" — so what became of it lives in that operation's RECORD, and the plan
/// holds the handle to it rather than a second copy of its status
/// ([`ComAmazonawsServicecatalogAws242servicecatalogserviceState::executed_plan_status`]).
#[derive(Debug, Clone)]
pub struct PlanExecution {
    /// The id of the record of the request that executed this plan.
    pub record_id: String,
    /// The `IdempotencyToken` that request carried, so a repeat of it answers the same
    /// record rather than executing the plan a second time.
    pub idempotency_token: String,
    /// The instant the plan was executed, which is the instant the plan record was last
    /// written.
    pub executed_at: SystemTime,
}

/// What CloudFormation answered when it was asked to compute the change set a plan is.
#[derive(Debug, Clone)]
pub enum PlanChangeSet {
    /// The id of the change set CloudFormation is computing, or has computed: the plan's
    /// status and its resource changes are read from it
    /// ([`plan_report`]).
    Created(String),
    /// CloudFormation could not plan the change, and this is the reason it gave — which
    /// is what the plan reports as its status message.
    Refused(String),
}

/// `ProvisionedProductPlanStatus`'s non-terminal value for a plan whose creation is
/// still in flight (`smithy_query query="shape"
/// shape="ProvisionedProductPlanStatus"`: `CREATE_IN_PROGRESS` | `CREATE_SUCCESS` |
/// `CREATE_FAILED` | `EXECUTE_IN_PROGRESS` | `EXECUTE_SUCCESS` | `EXECUTE_FAILED`).
pub const PLAN_STATUS_CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";

/// `ProvisionedProductPlanStatus`'s terminal value for a plan that was created: the
/// plan can be described and executed from this status on.
pub const PLAN_STATUS_CREATE_SUCCESS: &str = "CREATE_SUCCESS";

/// `ProvisionedProductPlanStatus`'s terminal value for a plan that could not be
/// created: the change set behind it failed, or could not be computed at all, and the
/// plan's status message is the reason CloudFormation gave.
pub const PLAN_STATUS_CREATE_FAILED: &str = "CREATE_FAILED";

/// `ProvisionedProductPlanStatus`'s non-terminal value for a plan whose EXECUTION is
/// still in flight: the change it planned has been carried out and the operation that
/// carried it out has not completed yet.
pub const PLAN_STATUS_EXECUTE_IN_PROGRESS: &str = "EXECUTE_IN_PROGRESS";

/// `ProvisionedProductPlanStatus`'s terminal value for a plan whose execution completed.
pub const PLAN_STATUS_EXECUTE_SUCCESS: &str = "EXECUTE_SUCCESS";

/// `ProvisionedProductPlanStatus`'s terminal value for a plan whose execution did not
/// happen: the deployment it carried out was refused, and the plan's status message is
/// the reason the service that owns that deployment gave.
pub const PLAN_STATUS_EXECUTE_FAILED: &str = "EXECUTE_FAILED";

/// The sentence a read of a plan refuses a plan id that resolves to no stored plan
/// with: this service words a missing plan by the identifier the request addressed it
/// by, which is the only variable part — the rest is fixed text.
///
/// Deliberately NOT the generic [`not_found_message`] every other lookup in this
/// service reports: a plan's own refusal is worded this way, so the plan reads share
/// this sentence rather than the service-wide default.
pub fn plan_not_found_message(plan_id: &str) -> String {
    format!("No plan with ID {plan_id} exists.")
}

/// The `ProvisionedProductStatus` values an EXISTING provisioned product must be in
/// for a plan to be created for it: "to create a plan for an existing provisioned
/// product, the product status must be AVAILABLE or TAINTED"
/// (`smithy_query query="doc" shape="CreateProvisionedProductPlan"`). Both are the
/// stable statuses the status member documents; the transitive ones are what a plan
/// cannot be created against.
pub const PLAN_READY_PROVISIONED_PRODUCT_STATUSES: &[&str] = &[
    PROVISIONED_PRODUCT_STATUS_AVAILABLE,
    PROVISIONED_PRODUCT_STATUS_TAINTED,
];

/// The `ProvisionedProductStatus` values a provisioned product must be in for the plan
/// of a change to it to be EXECUTED: the stable ones a plan can be created against
/// ([`PLAN_READY_PROVISIONED_PRODUCT_STATUSES`]) plus the one a provisioned product a
/// plan brought into existence sits in — "the plan operations were performed to provision
/// a new product, but resources have not yet been created. After reviewing the list of
/// resources to be created, execute the plan".
///
/// The transitive statuses are what no plan can be executed against: an operation on the
/// provisioned product is already in flight, so there is nothing settled for the planned
/// change to be applied to.
pub const PLAN_EXECUTABLE_PROVISIONED_PRODUCT_STATUSES: &[&str] = &[
    PROVISIONED_PRODUCT_STATUS_AVAILABLE,
    PROVISIONED_PRODUCT_STATUS_TAINTED,
    PROVISIONED_PRODUCT_STATUS_PLAN_IN_PROGRESS,
];

impl ProvisionedProductPlan {
    /// The stored tags as the modelled `Tags` list, in key order.
    pub fn tag_list(&self) -> Vec<Tag> {
        tag_list(&self.tags)
    }
}

/// A persisted record — "information about a request operation", which
/// `ProvisionProduct` answers with and `DescribeRecord` reports.
#[derive(Debug, Clone)]
pub struct Record {
    /// The minted record id (`rec-…`).
    pub id: String,
    /// A `RecordType` value: which request this record is the record OF.
    pub record_type: String,
    pub provisioned_product_id: String,
    pub provisioned_product_name: String,
    pub provisioned_product_type: String,
    pub product_id: String,
    pub provisioning_artifact_id: String,
    pub path_id: String,
    /// Creation instant, minted once when the request this record describes arrived.
    pub created_time: SystemTime,
    /// The instant the operation this record describes stops being in flight.
    /// The request answers in the non-terminal `CREATED` status, so the transition
    /// to the terminal one is owed by the CLOCK: it is stored as the instant it
    /// falls due and never slept for, and every report derives the status from it
    /// rather than from who asked.
    pub completed_at: SystemTime,
    /// Tag key -> tag value, reported as this record's `RecordTags`.
    pub tags: BTreeMap<String, String>,
    /// Why the operation this record describes did NOT happen, for one whose deployment
    /// the service that owns it refused; `None` for one that is happening.
    ///
    /// A request is accepted before the deployment it submits is made — the record answers
    /// in the non-terminal status — so what the request is accepted as says nothing about
    /// what became of it: the OUTCOME is this field, and it is what the terminal status the
    /// clock owes is derived from ([`Record::status`]). The failure is the refusing
    /// service's own ([`LaunchFailure`]), reported as this record's single `RecordError`
    /// and as the status message of the provisioned product it is about.
    pub failure: Option<LaunchFailure>,
}

/// The `RecordType` of a record `ProvisionProduct` creates
/// (`smithy_query query="doc" shape="RecordDetail" member="RecordType"`:
/// `PROVISION_PRODUCT` | `UPDATE_PROVISIONED_PRODUCT` |
/// `TERMINATE_PROVISIONED_PRODUCT`).
pub const RECORD_TYPE_PROVISION_PRODUCT: &str = "PROVISION_PRODUCT";

/// The `RecordType` of a record `TerminateProvisionedProduct` creates; see
/// [`RECORD_TYPE_PROVISION_PRODUCT`] for the three values the member documents.
pub const RECORD_TYPE_TERMINATE_PROVISIONED_PRODUCT: &str = "TERMINATE_PROVISIONED_PRODUCT";

/// The `RecordType` of a record a request that MODIFIES an existing provisioned product
/// creates — `UpdateProvisionedProduct`, and the execution of a plan of a change to one
/// ([`RECORD_TYPE_PROVISION_PRODUCT`] names the three values the member documents).
pub const RECORD_TYPE_UPDATE_PROVISIONED_PRODUCT: &str = "UPDATE_PROVISIONED_PRODUCT";

/// The record types `ProvisionedProductDetail$LastProvisioningRecordId` counts as a
/// provisioning request: its documentation names ProvisionProduct,
/// UpdateProvisionedProduct, ExecuteProvisionedProductPlan and
/// TerminateProvisionedProduct, each of which records one of the three `RecordType`
/// values above.
pub const PROVISIONING_RECORD_TYPES: &[&str] = &[
    RECORD_TYPE_PROVISION_PRODUCT,
    RECORD_TYPE_UPDATE_PROVISIONED_PRODUCT,
    RECORD_TYPE_TERMINATE_PROVISIONED_PRODUCT,
];

/// `RecordStatus`'s non-terminal first value, which the model documents as "the
/// request was created but the operation has not started"
/// (`smithy_query query="shape" shape="RecordStatus"`: `CREATED` | `IN_PROGRESS` |
/// `IN_PROGRESS_IN_ERROR` | `SUCCEEDED` | `FAILED`).
pub const RECORD_STATUS_CREATED: &str = "CREATED";

/// `RecordStatus`'s terminal value for an operation that completed: "the requested
/// operation has successfully completed".
pub const RECORD_STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// `RecordStatus`'s other terminal value, which the model documents as "the requested
/// operation has unsuccessfully completed. Investigate using the error messages returned" —
/// the status of a record whose operation did not happen ([`Record::failure`]).
pub const RECORD_STATUS_FAILED: &str = "FAILED";

/// `ProvisionedProductStatus`'s transitive value, which the model documents as
/// "operations performed might not have valid results. Wait for an AVAILABLE status
/// before performing operations"
/// (`smithy_query query="shape" shape="ProvisionedProductStatus"`).
pub const PROVISIONED_PRODUCT_STATUS_UNDER_CHANGE: &str = "UNDER_CHANGE";

/// `ProvisionedProductStatus`'s stable value: "ready to perform any operation. The
/// most recent operation succeeded and completed".
pub const PROVISIONED_PRODUCT_STATUS_AVAILABLE: &str = "AVAILABLE";

/// `ProvisionedProductStatus`'s value for a provisioned product whose resources are not
/// standing, which the model documents as "an unexpected error occurred. The provisioned
/// product exists but the stack is not running. For example, CloudFormation received a
/// parameter value that was not valid and could not launch the stack" — the status of a
/// provisioned product whose last operation did not happen ([`Record::failure`]).
pub const PROVISIONED_PRODUCT_STATUS_ERROR: &str = "ERROR";

/// `ProvisionedProductStatus`'s other stable value, which the model documents as
/// "ready to perform any operation. The stack has completed the requested operation
/// but is not exactly what was requested" — a status nothing in this emulator reaches,
/// named here because a plan may be created for a provisioned product in it
/// ([`PLAN_READY_PROVISIONED_PRODUCT_STATUSES`]).
pub const PROVISIONED_PRODUCT_STATUS_TAINTED: &str = "TAINTED";

/// `ProvisionedProductStatus`'s other transitive value, which the model documents as
/// "the plan operations were performed to provision a new product, but resources have
/// not yet been created. After reviewing the list of resources to be created, execute
/// the plan" — the status of a provisioned product a plan created and nothing has
/// executed yet.
pub const PROVISIONED_PRODUCT_STATUS_PLAN_IN_PROGRESS: &str = "PLAN_IN_PROGRESS";

/// How long after the request that starts it a provisioning operation stays in
/// flight.
///
/// A span this emulator CHOOSES (AWS documents none, and a real launch takes as long
/// as the resources behind it do): long enough that a caller acting straight after
/// the request still sees the transient record status and the `UNDER_CHANGE`
/// provisioned product it implies, short enough that nothing waits on it.
pub const PROVISIONING_COMPLETES_AFTER: std::time::Duration = std::time::Duration::from_secs(3);

/// The `ProvisionedProductType` a product of `product_type` provisions as.
///
/// `ProvisionedProductDetail$Type` documents the supported values as `CFN_STACK`,
/// `CFN_STACKSET`, `TERRAFORM_OPEN_SOURCE`, `TERRAFORM_CLOUD` and `EXTERNAL`, and
/// `ProductType` (`smithy_query query="shape" shape="ProductType"`) is
/// `CLOUD_FORMATION_TEMPLATE` | `MARKETPLACE` | `TERRAFORM_OPEN_SOURCE` |
/// `TERRAFORM_CLOUD` | `EXTERNAL`: the three Terraform/external names are shared, and
/// the two CloudFormation-backed product types (a template of this account's own, and
/// a Marketplace product, which is a CloudFormation template too) provision one
/// stack each.
pub fn provisioned_product_type(product_type: &str) -> &'static str {
    match product_type {
        "TERRAFORM_OPEN_SOURCE" => "TERRAFORM_OPEN_SOURCE",
        "TERRAFORM_CLOUD" => "TERRAFORM_CLOUD",
        "EXTERNAL" => "EXTERNAL",
        _ => PROVISIONED_PRODUCT_TYPE_CFN_STACK,
    }
}

/// The `ProvisionedProductType` of a provisioned product that IS one CloudFormation
/// stack — what [`provisioned_product_type`] answers for either CloudFormation-backed
/// product type, and so what tells a launch it has a stack to stand up.
pub const PROVISIONED_PRODUCT_TYPE_CFN_STACK: &str = "CFN_STACK";

impl Record {
    /// This record's `RecordStatus` at `now`: the non-terminal one until the clock
    /// owes the transition, and afterwards the terminal one its OUTCOME names — the
    /// operation did not happen, or it did ([`Record::failure`]).
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.completed_at {
            RECORD_STATUS_CREATED
        } else if self.failure.is_some() {
            RECORD_STATUS_FAILED
        } else {
            RECORD_STATUS_SUCCEEDED
        }
    }

    /// "The errors that occurred" on the operation this record describes, at `now`.
    ///
    /// A record reports the errors of an operation that has FINISHED unsuccessfully: while
    /// the operation is still in flight nothing has gone wrong yet, which is why the
    /// request that starts it is answered with none, and one that completed had none to
    /// report. The failure is the refusing service's own, as one `RecordError` pairs its
    /// code with its description.
    pub fn errors(&self, now: SystemTime) -> Vec<RecordError> {
        self.failure
            .iter()
            .filter(|_| self.status(now) == RECORD_STATUS_FAILED)
            .map(|failure| RecordError {
                code: failure.code.clone(),
                description: Some(failure.description.clone()),
            })
            .collect()
    }

    /// Whether this record is the record of a provisioning request — see
    /// [`PROVISIONING_RECORD_TYPES`].
    pub fn is_provisioning(&self) -> bool {
        PROVISIONING_RECORD_TYPES.contains(&self.record_type.as_str())
    }

    /// "The time when the record was last updated": the instant it was created
    /// while the operation is still in flight, and the instant the operation
    /// completed once the clock owes that transition.
    pub fn updated_time(&self, now: SystemTime) -> SystemTime {
        if now < self.completed_at {
            self.created_time
        } else {
            self.completed_at
        }
    }

    /// This record as the modelled `RecordDetail`: every member comes from stored
    /// state, so the provisioning request and a later read report the same values.
    pub fn detail(&self, now: SystemTime) -> RecordDetail {
        RecordDetail {
            record_id: Some(self.id.clone()),
            provisioned_product_name: Some(self.provisioned_product_name.clone()),
            status: Some(self.status(now).to_string()),
            created_time: Some(reported_time(self.created_time)),
            updated_time: Some(reported_time(self.updated_time(now))),
            provisioned_product_type: Some(self.provisioned_product_type.clone()),
            record_type: Some(self.record_type.clone()),
            provisioned_product_id: Some(self.provisioned_product_id.clone()),
            product_id: Some(self.product_id.clone()),
            provisioning_artifact_id: Some(self.provisioning_artifact_id.clone()),
            path_id: Some(self.path_id.clone()),
            // "The errors that occurred": the ones the operation this record describes
            // finished unsuccessfully with, and none at all while it is in flight or once
            // it has completed ([`Record::errors`]).
            record_errors: Some(self.errors(now)),
            record_tags: Some(self.tag_list()),
            // A launch role comes from a LAUNCH constraint's `RoleArn`, and nothing
            // assumes a role in this emulator, so no record carries one.
            launch_role_arn: None,
        }
    }

    /// The stored tags as the modelled `RecordTags` list, in key order.
    pub fn tag_list(&self) -> Vec<RecordTag> {
        self.tags
            .iter()
            .map(|(key, value)| RecordTag {
                key: Some(key.clone()),
                value: Some(value.clone()),
            })
            .collect()
    }
}

/// A persisted self-service action.
#[derive(Debug, Clone)]
pub struct ServiceAction {
    /// The minted service-action id (`act-…`).
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// A `ServiceActionDefinitionType` value, as the request gave it.
    pub definition_type: String,
    /// The action's definition, keyed by `ServiceActionDefinitionKey`. A `BTreeMap` so
    /// the map a response carries has a stable order.
    pub definition: BTreeMap<String, String>,
}

impl ServiceAction {
    /// This record as the modelled `ServiceActionSummary`, which `ListServiceActions`
    /// reports instead of the fuller detail.
    pub fn summary(&self) -> ServiceActionSummary {
        ServiceActionSummary {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            definition_type: Some(self.definition_type.clone()),
        }
    }

    /// This record as the modelled `ServiceActionDetail`.
    pub fn detail(&self) -> ServiceActionDetail {
        ServiceActionDetail {
            service_action_summary: Some(self.summary()),
            definition: Some(
                self.definition
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
        }
    }
}

/// A persisted TagOption.
#[derive(Debug, Clone)]
pub struct TagOption {
    /// The minted TagOption id (`tag-…`).
    pub id: String,
    pub key: String,
    pub value: String,
    /// Whether the TagOption may be associated with a resource; `UpdateTagOption`'s
    /// `Active` member is what changes it.
    pub active: bool,
    /// The account that created the TagOption.
    pub owner: String,
}

impl TagOption {
    /// This record as the modelled `TagOptionDetail`.
    pub fn detail(&self) -> TagOptionDetail {
        TagOptionDetail {
            key: Some(self.key.clone()),
            value: Some(self.value.clone()),
            active: Some(self.active),
            id: Some(self.id.clone()),
            owner: Some(self.owner.clone()),
        }
    }
}

/// The most tags one portfolio can carry.
///
/// From the model: every operation that returns a portfolio's `Tags` targets the
/// `Tags` list, whose length is constrained `0..50`
/// (`smithy_query query="operation" operation="UpdatePortfolio"`), so a stored
/// portfolio can never hold more than 50 tags.
pub const MAX_PORTFOLIO_TAGS: usize = 50;

/// The most tags one product can carry.
///
/// From the model, the same way: every operation that returns a product's `Tags`
/// targets the `Tags` list constrained `0..50`
/// (`smithy_query query="operation" operation="UpdateProduct"`), so a stored product
/// can never hold more than 50 tags either.
pub const MAX_PRODUCT_TAGS: usize = 50;

/// The most portfolios one account can hold, which also bounds this store.
///
/// AWS's documented default Service Catalog quota is 100 portfolios per account
/// per Region, and `CreatePortfolio` models `LimitExceededException` ("The current
/// limits of the service would have been exceeded by this operation") for exactly
/// that refusal. Because the live set is capped, so is every response built from
/// it: `ListPortfolios` cannot return more than this many records on a page.
pub const MAX_PORTFOLIOS: usize = 100;

/// The most principals one portfolio can carry, bounding that map the same way and
/// answered with the `LimitExceededException` that
/// `AssociatePrincipalWithPortfolio` models. The emulator's own bound, finite and
/// small, checked before every association is stored.
pub const MAX_PRINCIPALS_PER_PORTFOLIO: usize = 100;

/// The most constraints this account can hold, which also bounds this store.
///
/// `CreateConstraint` models `LimitExceededException` for a create past the account's
/// quota. As with [`MAX_PRODUCTS`] this is the emulator's own bound, not a quoted AWS
/// figure, and it is checked before every insert.
pub const MAX_CONSTRAINTS: usize = 500;

/// The most products this account can hold, which also bounds this store.
///
/// `CreateProduct` models `LimitExceededException` ("The current limits of the service
/// would have been exceeded by this operation") for a create past the account's product
/// quota. AWS's published default figure is not reproduced here — this constant is the
/// EMULATOR's bound, set at the scale of the one quota this file does cite (100
/// portfolios) — and it is what keeps a shared in-memory store from growing without
/// bound: every stored product holds its provisioning artifacts and at most one
/// idempotency entry, so bounding the count bounds all three.
pub const MAX_PRODUCTS: usize = 500;

/// The most provisioning artifacts (versions) one product can carry, which bounds every
/// product record.
///
/// `CopyProduct` adds the source's versions to the target product, so without a cap on
/// the versions one product holds a caller could grow one record without bound by
/// repeating the copy. As with [`MAX_PRODUCTS`] this is the EMULATOR's bound, not a
/// quoted AWS figure — set at the scale of the one quota this file does cite (100
/// portfolios) — and it is checked before any version is stored.
pub const MAX_PROVISIONING_ARTIFACTS_PER_PRODUCT: usize = 100;

/// The most provisioned products this account can hold, which also bounds this
/// store and the records inside it.
///
/// `ProvisionProduct` models no limit error, so a provisioning request past this
/// bound is refused with the parameter error it does model, naming the account's
/// provisioned-product quota. As with [`MAX_PRODUCTS`] the figure is the EMULATOR's
/// bound, not a quoted AWS one, and it is checked before anything is stored: each
/// provisioned product holds its tags and the records of the operations performed on
/// it, so bounding the count bounds all three.
pub const MAX_PROVISIONED_PRODUCTS: usize = 500;

/// The most records this store keeps, which bounds it.
///
/// A record OUTLIVES the provisioned product it describes — terminating one "does not
/// delete any records associated with the provisioned product" — so a caller that
/// provisions and terminates repeatedly would otherwise grow this map without bound,
/// however few provisioned products the account holds at once. As with
/// [`MAX_PRODUCT_COPIES`] the figure is the emulator's own bound, not a quoted AWS one:
/// a record stored past it evicts the OLDEST record of a provisioned product the store
/// no longer holds, and the records of the provisioned products it does hold are kept
/// ([`ComAmazonawsServicecatalogAws242servicecatalogserviceState::remember_record`]).
pub const MAX_RECORDS: usize = 1000;

/// The most self-service actions this account can hold, which also bounds this store.
///
/// `CreateServiceAction` models `LimitExceededException` for a create past the account's
/// quota. As with [`MAX_PRODUCTS`] this is the emulator's own bound, not a quoted AWS
/// figure.
pub const MAX_SERVICE_ACTIONS: usize = 500;

/// The most TagOptions one resource — a portfolio or a product — can carry,
/// bounding every resource record and answered with the `LimitExceededException`
/// that `AssociateTagOptionWithResource` models. The emulator's own bound, finite
/// and small, checked before every association is stored; no AWS figure is quoted
/// here.
pub const MAX_TAG_OPTIONS_PER_RESOURCE: usize = 25;

/// The most budgets one resource — a portfolio or a product — can carry, bounding
/// every resource record and answered with the `LimitExceededException` that
/// `AssociateBudgetWithResource` models. The emulator's own bound, finite and small,
/// checked before every association is stored; no AWS figure is quoted here. Without
/// it a caller could grow one resource's record without bound by associating fresh
/// budget names.
pub const MAX_BUDGETS_PER_RESOURCE: usize = 25;

/// The most TagOptions this account can hold, which also bounds this store.
///
/// `CreateTagOption` models `LimitExceededException` for a create past the account's
/// quota. As with [`MAX_PRODUCTS`] this is the emulator's own bound, not a quoted AWS
/// figure. It also bounds the uniqueness scan every create runs over the live set.
pub const MAX_TAG_OPTIONS: usize = 1000;

/// Per-service state for com-amazonaws-servicecatalog-aws242servicecatalogservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsServicecatalogAws242servicecatalogserviceState {
    /// Every created, not-yet-deleted portfolio, keyed by portfolio id. A
    /// `BTreeMap` so `ListPortfolios` and its page tokens walk one stable order.
    pub portfolios: BTreeMap<String, Portfolio>,
    /// Every created, not-yet-deleted constraint, keyed by constraint id. A
    /// `BTreeMap` so `ListConstraintsForPortfolio` and its page tokens walk one
    /// stable order.
    pub constraints: BTreeMap<String, Constraint>,
    /// Every created, not-yet-deleted product, keyed by product id. A `BTreeMap` so
    /// every walk over the set has one stable order.
    pub products: BTreeMap<String, Product>,
    /// Every created, not-yet-deleted self-service action, keyed by action id.
    pub service_actions: BTreeMap<String, ServiceAction>,
    /// Every provisioned provisioned product, keyed by provisioned-product id. A
    /// `BTreeMap` so every walk over the set has one stable order.
    ///
    /// An entry whose termination the clock has settled no longer exists for any
    /// caller: read the set through [`Self::live_provisioned_products`],
    /// [`Self::provisioned_product`] or [`Self::provisioned_product_by_name`], which
    /// skip it, rather than over this map directly.
    pub provisioned_products: BTreeMap<String, ProvisionedProduct>,
    /// The record of every operation performed on a provisioned product, keyed by
    /// record id. Each provisioned product names its own records in
    /// [`ProvisionedProduct::record_ids`], so this map is the one place a record is
    /// stored. Written through [`Self::remember_record`], which bounds it by
    /// [`MAX_RECORDS`]: a record outlives the provisioned product it describes.
    pub records: BTreeMap<String, Record>,
    /// Every created, not-yet-deleted TagOption, keyed by TagOption id.
    pub tag_options: BTreeMap<String, TagOption>,
    /// Every remembered product copy, keyed by the `CopyProductToken` `CopyProduct`
    /// answered. A `BTreeMap` so the walk the eviction in [`Self::remember_product_copy`]
    /// makes is deterministic. Bounded by [`MAX_PRODUCT_COPIES`].
    pub product_copies: BTreeMap<String, ProductCopy>,
    /// Every created, not-yet-deleted plan, keyed by plan id. A `BTreeMap` so every
    /// walk over the set has one stable order.
    ///
    /// A plan whose provisioned product no longer exists is dropped with it
    /// ([`Self::forget_terminated_provisioned_products`]), so the set is bounded by
    /// [`MAX_PROVISIONED_PRODUCTS`]: a provisioned product carries at most one plan.
    /// Read it through [`Self::provisioned_product_plan`], which skips a plan whose
    /// provisioned product the clock has already removed.
    pub provisioned_product_plans: BTreeMap<String, ProvisionedProductPlan>,
    /// `CreatePortfolio` idempotency: `IdempotencyToken` -> portfolio id, so a
    /// repeated create replays the first one instead of minting a second portfolio.
    ///
    /// PRIVATE, like the other token maps: every write goes through
    /// [`Self::remember_portfolio_token`], which prunes entries whose portfolio is gone,
    /// so the map holds at most one entry per live portfolio and a caller cycling fresh
    /// tokens cannot grow it past [`MAX_PORTFOLIOS`]. A handler reads it through
    /// [`Self::replayed_portfolio`].
    portfolio_idempotency: HashMap<String, String>,
    /// `CreateProduct` idempotency: `IdempotencyToken` -> product id. Kept apart from
    /// the portfolio map because the token namespace is per operation: the same token
    /// may legitimately create one portfolio and one product. Private and pruned on
    /// every write, exactly as above, so it is bounded by [`MAX_PRODUCTS`].
    product_idempotency: HashMap<String, String>,
    /// `CreateServiceAction` idempotency: `IdempotencyToken` -> service-action id.
    /// Private and pruned on every write, so it is bounded by [`MAX_SERVICE_ACTIONS`].
    service_action_idempotency: HashMap<String, String>,
    /// `CreateConstraint` idempotency: `IdempotencyToken` -> constraint id. Private and
    /// pruned on every write, so it is bounded by [`MAX_CONSTRAINTS`].
    constraint_idempotency: HashMap<String, String>,
    /// `CopyProduct` idempotency: `IdempotencyToken` -> `CopyProductToken`, so a
    /// repeated copy answers the token of the copy it already started instead of
    /// starting a second one. Private and pruned on every write, so it is bounded by
    /// [`MAX_PRODUCT_COPIES`].
    product_copy_idempotency: HashMap<String, String>,
    /// `ProvisionProduct` idempotency: `ProvisionToken` -> record id, so a repeated
    /// provisioning request replays the record the first one minted instead of
    /// provisioning a second product. Private and pruned on every write, exactly as
    /// the maps above, so it is bounded by [`MAX_PROVISIONED_PRODUCTS`].
    provision_idempotency: HashMap<String, String>,
    /// `CreateProvisionedProductPlan` idempotency: `IdempotencyToken` -> plan id, so a
    /// repeated create replays the plan the first one minted instead of planning a
    /// second change. Private and pruned on every write, exactly as the maps above,
    /// so it holds at most one entry per stored plan.
    plan_idempotency: HashMap<String, String>,
    /// Counter behind every `next_*_id`. Deterministic, so a fresh
    /// emulator mints the same id sequence every run.
    pub next_id_seed: u64,
}

impl ComAmazonawsServicecatalogAws242servicecatalogserviceState {
    /// The portfolio a create carrying `token` already made, if that create's portfolio
    /// is still stored. `None` for an unseen token, or one whose portfolio was deleted —
    /// which is no longer a replay.
    pub fn replayed_portfolio(&self, token: &str) -> Option<&Portfolio> {
        self.portfolios.get(self.portfolio_idempotency.get(token)?)
    }

    /// Remember a `CreatePortfolio` idempotency token so a repeat replays
    /// `portfolio_id`.
    ///
    /// Every entry whose portfolio is no longer stored is dropped first, so the map holds
    /// at most one entry per live portfolio: a caller cycling fresh tokens — including
    /// after a delete, or once the quota is reached — cannot grow it past
    /// [`MAX_PORTFOLIOS`].
    pub fn remember_portfolio_token(&mut self, token: String, portfolio_id: String) {
        let portfolios = &self.portfolios;
        self.portfolio_idempotency
            .retain(|_, id| portfolios.contains_key(id));
        self.portfolio_idempotency.insert(token, portfolio_id);
    }

    /// Drop every idempotency token pointing at `portfolio_id`, which a delete has just
    /// removed: a create repeating one of them creates a new portfolio.
    pub fn forget_portfolio_tokens(&mut self, portfolio_id: &str) {
        self.portfolio_idempotency
            .retain(|_, id| id != portfolio_id);
    }

    /// The product a create carrying `token` already made, if it is still stored.
    pub fn replayed_product(&self, token: &str) -> Option<&Product> {
        self.products.get(self.product_idempotency.get(token)?)
    }

    /// Remember a `CreateProduct` idempotency token so a repeat replays `product_id`,
    /// bounded exactly as [`Self::remember_portfolio_token`] is.
    pub fn remember_product_token(&mut self, token: String, product_id: String) {
        let products = &self.products;
        self.product_idempotency
            .retain(|_, id| products.contains_key(id));
        self.product_idempotency.insert(token, product_id);
    }

    /// The self-service action a create carrying `token` already made, if it is still
    /// stored.
    pub fn replayed_service_action(&self, token: &str) -> Option<&ServiceAction> {
        self.service_actions
            .get(self.service_action_idempotency.get(token)?)
    }

    /// Remember a `CreateServiceAction` idempotency token, bounded the same way.
    pub fn remember_service_action_token(&mut self, token: String, action_id: String) {
        let actions = &self.service_actions;
        self.service_action_idempotency
            .retain(|_, id| actions.contains_key(id));
        self.service_action_idempotency.insert(token, action_id);
    }

    /// Drop every idempotency token pointing at `action_id`, which a delete has just
    /// removed.
    pub fn forget_service_action_tokens(&mut self, action_id: &str) {
        self.service_action_idempotency
            .retain(|_, id| id != action_id);
    }

    /// The constraint a create carrying `token` already made, if it is still stored.
    pub fn replayed_constraint(&self, token: &str) -> Option<&Constraint> {
        self.constraints.get(self.constraint_idempotency.get(token)?)
    }

    /// Remember a `CreateConstraint` idempotency token, bounded the same way.
    pub fn remember_constraint_token(&mut self, token: String, constraint_id: String) {
        let constraints = &self.constraints;
        self.constraint_idempotency
            .retain(|_, id| constraints.contains_key(id));
        self.constraint_idempotency.insert(token, constraint_id);
    }

    /// Drop every idempotency token pointing at `constraint_id`, which a delete has just
    /// removed.
    pub fn forget_constraint_tokens(&mut self, constraint_id: &str) {
        self.constraint_idempotency
            .retain(|_, id| id != constraint_id);
    }

    /// The copy a `CopyProduct` carrying `token` already started, if its record is
    /// still stored. `None` for an unseen token, or one whose record has since been
    /// evicted — which is no longer a replay.
    pub fn replayed_product_copy(&self, token: &str) -> Option<&ProductCopy> {
        self.product_copies
            .get(self.product_copy_idempotency.get(token)?)
    }

    /// Store `copy` and remember the `IdempotencyToken` that started it, so a repeat
    /// of that request answers the same `CopyProductToken`.
    ///
    /// Keeps the map at [`MAX_PRODUCT_COPIES`] by dropping the copy that started
    /// longest ago, then drops every idempotency entry whose copy is no longer stored
    /// — so neither map grows without bound however many copies a caller makes.
    pub fn remember_product_copy(&mut self, idempotency_token: String, copy: ProductCopy) {
        while self.product_copies.len() >= MAX_PRODUCT_COPIES {
            let oldest = self
                .product_copies
                .values()
                .min_by(|left, right| {
                    left.succeeded_at
                        .cmp(&right.succeeded_at)
                        .then_with(|| left.token.cmp(&right.token))
                })
                .map(|copy| copy.token.clone());
            match oldest {
                Some(token) => {
                    self.product_copies.remove(&token);
                }
                None => break,
            }
        }
        self.product_copy_idempotency
            .insert(idempotency_token, copy.token.clone());
        self.product_copies.insert(copy.token.clone(), copy);
        let copies = &self.product_copies;
        self.product_copy_idempotency
            .retain(|_, token| copies.contains_key(token));
    }

    /// The record a provisioning request carrying `token` already made, if that
    /// record's provisioned product still resolves at `now`. `None` for an unseen
    /// token, or one whose provisioned product has been terminated — which is no
    /// longer a replay, exactly as a create token whose resource was deleted is not
    /// ([`Self::replayed_portfolio`]): the token can no longer answer with a
    /// provisioned product that exists.
    pub fn replayed_provision_record(&self, token: &str, now: SystemTime) -> Option<&Record> {
        let record = self.records.get(self.provision_idempotency.get(token)?)?;
        self.provisioned_product(&record.provisioned_product_id, now)
            .map(|_| record)
    }

    /// Remember a `ProvisionToken` so a repeat replays `record_id`, bounded exactly
    /// as [`Self::remember_portfolio_token`] is: every entry whose record names a
    /// provisioned product the store no longer holds is dropped first, so the map
    /// holds at most one entry per stored provisioned product.
    pub fn remember_provision_token(&mut self, token: String, record_id: String) {
        let records = &self.records;
        let provisioned = &self.provisioned_products;
        self.provision_idempotency.retain(|_, id| {
            records
                .get(id)
                .is_some_and(|record| provisioned.contains_key(&record.provisioned_product_id))
        });
        self.provision_idempotency.insert(token, record_id);
    }

    /// The portfolios `product_id` can be launched through BY `caller` — its launch paths.
    ///
    /// "A path is dependent on a specific product, porfolio, and principal"
    /// (`ListLaunchPaths`), so a path is a portfolio that satisfies BOTH halves of that
    /// triple: it carries the product (`AssociateProductWithPortfolio`) AND it grants the
    /// calling principal access to the portfolio ([`Portfolio::grants`], which
    /// `AssociatePrincipalWithPortfolio` stores and
    /// `DisassociatePrincipalFromPortfolio` removes). Either half missing is no path: a
    /// product no portfolio holds resolves none, a product in portfolios that grant this
    /// caller nothing resolves none either, and neither does a product id nothing stored
    /// carries.
    ///
    /// The one place a launch path is resolved, so every response derived from a product's
    /// launchability — the paths `ListLaunchPaths` pages over, `DescribeProduct`'s
    /// `LaunchPaths`, the `HasDefaultPath` of an end-user product view, and the path a
    /// provisioning request names — is derived from the same set and cannot contradict
    /// another.
    pub fn launch_path_portfolios(
        &self,
        product_id: &str,
        caller: &CallerPrincipal,
    ) -> Vec<&Portfolio> {
        self.products
            .get(product_id)
            .into_iter()
            .flat_map(|product| product.portfolio_ids.iter())
            .filter_map(|portfolio_id| self.portfolios.get(portfolio_id))
            .filter(|portfolio| portfolio.grants(caller))
            .collect()
    }

    /// The provisioned product `name` addresses, which the model makes unique for the
    /// account, or `None` when this account holds none by that name at `now`.
    pub fn provisioned_product_by_name(
        &self,
        name: &str,
        now: SystemTime,
    ) -> Option<&ProvisionedProduct> {
        self.live_provisioned_products(now)
            .find(|provisioned| provisioned.name == name)
    }

    /// The provisioned product `id` addresses, or `None` when this account holds none
    /// by that id at `now`.
    pub fn provisioned_product(&self, id: &str, now: SystemTime) -> Option<&ProvisionedProduct> {
        self.provisioned_products
            .get(id)
            .filter(|provisioned| !provisioned.is_terminated(now))
    }

    /// The provisioned product a `ProvisionedProductNameOrArn` addresses.
    ///
    /// That member takes "the name of the provisioned product" or its ARN — its
    /// modelled pattern admits a name and an `arn:…` value alike — so a value of the
    /// ARN form resolves against the stored ARN and every other value against the
    /// stored name.
    pub fn provisioned_product_by_name_or_arn(
        &self,
        name_or_arn: &str,
        now: SystemTime,
    ) -> Option<&ProvisionedProduct> {
        if name_or_arn.starts_with(ARN_PREFIX) {
            return self
                .live_provisioned_products(now)
                .find(|provisioned| provisioned.arn == name_or_arn);
        }
        self.provisioned_product_by_name(name_or_arn, now)
    }

    /// Every provisioned product this account holds at `now`, in stored (id) order.
    ///
    /// One that has been terminated and whose termination the clock has settled is NOT
    /// one of them: it stopped existing when that termination fell due, so no read
    /// reports it and no uniqueness check counts its name. The record of its
    /// termination stays stored — terminating a provisioned product "does not delete
    /// any records associated with the provisioned product" — and
    /// [`Self::forget_terminated_provisioned_products`] is what drops the entry itself.
    pub fn live_provisioned_products(
        &self,
        now: SystemTime,
    ) -> impl DoubleEndedIterator<Item = &ProvisionedProduct> {
        self.provisioned_products
            .values()
            .filter(move |provisioned| !provisioned.is_terminated(now))
    }

    /// Drop every provisioned product whose termination the clock has settled by `now`,
    /// materialising a removal that was owed at that instant whether or not anybody
    /// asked — so the store does not keep an entry for a resource that no longer
    /// exists. Called by the operations that WRITE the store; a read reports the same
    /// set without it ([`Self::live_provisioned_products`]).
    ///
    /// A plan plans the change of ONE provisioned product, so a plan whose provisioned
    /// product this drops is dropped with it: nothing is left planning a change to a
    /// resource that no longer exists.
    pub fn forget_terminated_provisioned_products(&mut self, now: SystemTime) {
        self.provisioned_products
            .retain(|_, provisioned| !provisioned.is_terminated(now));
        let provisioned = &self.provisioned_products;
        self.provisioned_product_plans
            .retain(|_, plan| provisioned.contains_key(&plan.provisioned_product_id));
        let plans = &self.provisioned_product_plans;
        self.plan_idempotency.retain(|_, id| plans.contains_key(id));
    }

    /// The plan `plan_id` addresses, or `None` when this account holds none by that id
    /// at `now` — including a plan whose provisioned product has been terminated, which
    /// plans the change of a resource that no longer exists.
    pub fn provisioned_product_plan(
        &self,
        plan_id: &str,
        now: SystemTime,
    ) -> Option<&ProvisionedProductPlan> {
        let plan = self.provisioned_product_plans.get(plan_id)?;
        self.provisioned_product(&plan.provisioned_product_id, now)
            .map(|_| plan)
    }

    /// The plan of the provisioned product `provisioned_product_id` names, or `None`
    /// for one no plan has been created for: "you can create one plan for each
    /// provisioned product", so there is at most one.
    pub fn plan_of_provisioned_product(
        &self,
        provisioned_product_id: &str,
        now: SystemTime,
    ) -> Option<&ProvisionedProductPlan> {
        self.provisioned_product_plans
            .values()
            .find(|plan| plan.provisioned_product_id == provisioned_product_id)
            .filter(|plan| self.provisioned_product(&plan.provisioned_product_id, now).is_some())
    }

    /// The plan a create carrying `token` already made, if that plan is still stored.
    /// `None` for an unseen token, or one whose plan is gone — which is no longer a
    /// replay.
    pub fn replayed_provisioned_product_plan(
        &self,
        token: &str,
        now: SystemTime,
    ) -> Option<&ProvisionedProductPlan> {
        self.provisioned_product_plan(self.plan_idempotency.get(token)?, now)
    }

    /// Remember a `CreateProvisionedProductPlan` idempotency token so a repeat replays
    /// `plan_id`, bounded exactly as [`Self::remember_portfolio_token`] is: every entry
    /// whose plan is no longer stored is dropped first, so the map holds at most one
    /// entry per stored plan.
    pub fn remember_provisioned_product_plan_token(&mut self, token: String, plan_id: String) {
        let plans = &self.provisioned_product_plans;
        self.plan_idempotency.retain(|_, id| plans.contains_key(id));
        self.plan_idempotency.insert(token, plan_id);
    }

    /// `plan` as the modelled `ProvisionedProductPlanDetails`, EXCEPT the two members
    /// the change set behind it owns: its `Status` and that status's message, which the
    /// caller fills in from [`plan_report`] — the status of a plan is the status of the
    /// change set it is, and reading it is a call into CloudFormation rather than a read
    /// of this store.
    ///
    /// Every other member comes from the stored plan or from the provisioned product it
    /// plans the change of, so the create and the describe of one plan cannot report it
    /// two ways.
    pub fn provisioned_product_plan_details(
        &self,
        plan: &ProvisionedProductPlan,
    ) -> ProvisionedProductPlanDetails {
        ProvisionedProductPlanDetails {
            created_time: Some(reported_time(plan.created_time)),
            path_id: Some(plan.path_id.clone()),
            product_id: Some(plan.product_id.clone()),
            plan_name: Some(plan.name.clone()),
            plan_id: Some(plan.id.clone()),
            provision_product_id: Some(plan.provisioned_product_id.clone()),
            provision_product_name: self.planned_provisioned_product_name(plan),
            plan_type: Some(plan.plan_type.clone()),
            provisioning_artifact_id: Some(plan.provisioning_artifact_id.clone()),
            status: None,
            // "The UTC time stamp when the plan was last updated": the instant the plan
            // was created, until something EXECUTES it — the one operation that writes a
            // stored plan again.
            updated_time: Some(reported_time(
                plan.execution
                    .as_ref()
                    .map_or(plan.created_time, |executed| executed.executed_at),
            )),
            notification_arns: Some(plan.notification_arns.clone()),
            provisioning_parameters: Some(plan.provisioning_parameters.clone()),
            tags: Some(plan.tag_list()),
            status_message: None,
        }
    }

    /// The name of the provisioned product `plan` plans the change of.
    pub fn planned_provisioned_product_name(
        &self,
        plan: &ProvisionedProductPlan,
    ) -> Option<String> {
        self.provisioned_products
            .get(&plan.provisioned_product_id)
            .map(|provisioned| provisioned.name.clone())
    }

    /// The `ProvisionedProductPlanStatus` an EXECUTED plan reports at `now`, and the
    /// message that status carries; `None` for a plan nothing has executed, whose status
    /// is the status of the change set it is ([`plan_report`]).
    ///
    /// Executing a plan "provisions or modifies a product based on the resource changes
    /// for the specified plan", which is one request operation on that provisioned
    /// product: so what became of the execution is what the RECORD of that request says
    /// — in flight until the clock owes that record's completion, and afterwards the
    /// terminal value its outcome names, carrying the refusing service's own sentence for
    /// a deployment that did not happen.
    pub fn executed_plan_status(
        &self,
        plan: &ProvisionedProductPlan,
        now: SystemTime,
    ) -> Option<(&'static str, Option<String>)> {
        let execution = plan.execution.as_ref()?;
        // A plan whose record the store has evicted to stay within [`MAX_RECORDS`] was
        // executed all the same, and that execution completed: the same reading
        // [`Self::provisioned_product_status`] makes of a provisioned product with no
        // readable history.
        let Some(record) = self.records.get(&execution.record_id) else {
            return Some((PLAN_STATUS_EXECUTE_SUCCESS, None));
        };
        let status = match record.status(now) {
            RECORD_STATUS_CREATED => PLAN_STATUS_EXECUTE_IN_PROGRESS,
            RECORD_STATUS_FAILED => PLAN_STATUS_EXECUTE_FAILED,
            _ => PLAN_STATUS_EXECUTE_SUCCESS,
        };
        let message = record
            .errors(now)
            .into_iter()
            .next()
            .and_then(|error| error.description);
        Some((status, message))
    }

    /// Store the record of one request operation, keeping the map at [`MAX_RECORDS`].
    ///
    /// Every record goes through here, so the one place a record is stored is also the
    /// one place the store is bounded, and the cap holds whatever the store contains:
    /// the record evicted to make room is the oldest one whose provisioned product the
    /// store no longer holds, so a caller who provisions and terminates repeatedly
    /// cannot grow this map while every record of a provisioned product that still
    /// exists stays readable — and once there is no such record left, the oldest record
    /// of all is evicted, so a caller operating on ONE provisioned product forever
    /// cannot grow it either.
    pub fn remember_record(&mut self, record: Record) {
        while self.records.len() >= MAX_RECORDS {
            let provisioned = &self.provisioned_products;
            let collectable =
                |record: &Record| !provisioned.contains_key(&record.provisioned_product_id);
            let evictable = self
                .records
                .values()
                .min_by(|left, right| {
                    // A record whose provisioned product is gone goes first (`false`
                    // sorts before `true`, so the sides are compared the other way
                    // round), then the oldest, then by id so the choice is
                    // deterministic.
                    collectable(right)
                        .cmp(&collectable(left))
                        .then_with(|| left.created_time.cmp(&right.created_time))
                        .then_with(|| left.id.cmp(&right.id))
                })
                .map(|stored| stored.id.clone());
            match evictable {
                Some(id) => self.forget_record(&id),
                // The store is empty, so there is nothing to evict and nothing the cap
                // has to hold back.
                None => break,
            }
        }
        self.records.insert(record.id.clone(), record);
    }

    /// Record that the operation `record_id` describes did NOT happen, with the failure the
    /// service that owns the deployment refused it with.
    ///
    /// The request was accepted before the deployment was submitted, so this is where its
    /// OUTCOME is written: the record settles [`RECORD_STATUS_FAILED`] once the clock owes
    /// its completion, carrying that failure as its one `RecordError`, and the provisioned
    /// product it is about reports [`PROVISIONED_PRODUCT_STATUS_ERROR`] with the same
    /// sentence. Nothing is written for a record the store no longer holds.
    pub fn fail_record(&mut self, record_id: &str, failure: LaunchFailure) {
        if let Some(record) = self.records.get_mut(record_id) {
            record.failure = Some(failure);
        }
    }

    /// Drop one stored record, and the reference to it the provisioned product it
    /// describes holds.
    ///
    /// A provisioned product's history names only records the store still holds, so
    /// that history is bounded by [`MAX_RECORDS`] along with the store itself: evicting
    /// a record is what keeps both from growing.
    fn forget_record(&mut self, record_id: &str) {
        let Some(record) = self.records.remove(record_id) else {
            return;
        };
        if let Some(provisioned) = self
            .provisioned_products
            .get_mut(&record.provisioned_product_id)
        {
            provisioned.record_ids.retain(|id| id != record_id);
        }
    }

    /// The records of the operations performed on `provisioned`, oldest first.
    fn provisioned_product_records<'a>(
        &'a self,
        provisioned: &'a ProvisionedProduct,
    ) -> impl DoubleEndedIterator<Item = &'a Record> {
        provisioned
            .record_ids
            .iter()
            .filter_map(|record_id| self.records.get(record_id))
    }

    /// The record of the termination already accepted for `provisioned`, or `None` for
    /// one no termination has been accepted for.
    ///
    /// A provisioned product is terminated once: the request is answered with the
    /// record of the termination it started, and a repeat of it while that termination
    /// is in flight is the same termination rather than a second one.
    pub fn termination_record<'a>(
        &'a self,
        provisioned: &'a ProvisionedProduct,
    ) -> Option<&'a Record> {
        self.provisioned_product_records(provisioned)
            .rfind(|record| record.record_type == RECORD_TYPE_TERMINATE_PROVISIONED_PRODUCT)
    }

    /// The record of the last provisioning of `provisioned` that SUCCEEDED at `now`, or
    /// `None` when no provisioning of it has succeeded yet: its launch is still in
    /// flight, or the store has evicted the record of the one that completed.
    ///
    /// The one derivation of "the last successful provisioning record" in this service,
    /// so the record `ProvisionedProductDetail$LastSuccessfulProvisioningRecordId`
    /// names is the same one the launch's outputs are reported for.
    pub fn last_successful_provisioning_record<'a>(
        &'a self,
        provisioned: &'a ProvisionedProduct,
        now: SystemTime,
    ) -> Option<&'a Record> {
        self.provisioned_product_records(provisioned)
            .rfind(|record| {
                record.is_provisioning() && record.status(now) == RECORD_STATUS_SUCCEEDED
            })
    }

    /// `provisioned`'s `ProvisionedProductStatus` at `now`: the transitive one while
    /// the operation its last record describes is still in flight, and once the clock owes
    /// that record's completion the stable one its OUTCOME names — the resources are
    /// standing, or the operation did not happen and they are not
    /// ([`PROVISIONED_PRODUCT_STATUS_ERROR`]).
    ///
    /// A provisioned product a PLAN created and nothing has performed an operation on
    /// yet reports the transitive status the model documents for exactly that:
    /// [`PROVISIONED_PRODUCT_STATUS_PLAN_IN_PROGRESS`]
    /// ([`ProvisionedProduct::plan_created`]).
    pub fn provisioned_product_status(
        &self,
        provisioned: &ProvisionedProduct,
        now: SystemTime,
    ) -> &'static str {
        match self.provisioned_product_records(provisioned).next_back() {
            Some(last) => match last.status(now) {
                RECORD_STATUS_CREATED => PROVISIONED_PRODUCT_STATUS_UNDER_CHANGE,
                RECORD_STATUS_FAILED => PROVISIONED_PRODUCT_STATUS_ERROR,
                _ => PROVISIONED_PRODUCT_STATUS_AVAILABLE,
            },
            None if provisioned.plan_created => PROVISIONED_PRODUCT_STATUS_PLAN_IN_PROGRESS,
            // A provisioned product with no readable history is one whose records the
            // store has evicted: its last operation completed.
            None => PROVISIONED_PRODUCT_STATUS_AVAILABLE,
        }
    }

    /// `provisioned`'s `ProvisionedProductStatusMessage` at `now`: "the current status
    /// message of the provisioned product".
    ///
    /// The message belongs to the status, so it is read from the same record that status is
    /// derived from: a provisioned product whose last operation did not happen reports why
    /// — the refusing service's own sentence, the one that record's `RecordError` carries —
    /// and one whose resources are standing or whose operation is still in flight has
    /// nothing to report rather than an invented message.
    pub fn provisioned_product_status_message(
        &self,
        provisioned: &ProvisionedProduct,
        now: SystemTime,
    ) -> Option<String> {
        let last = self.provisioned_product_records(provisioned).next_back()?;
        last.errors(now)
            .into_iter()
            .next()
            .and_then(|error| error.description)
    }

    /// `provisioned` as the modelled `ProvisionedProductDetail`.
    ///
    /// Every `Last*RecordId` member is derived from the one stored history
    /// [`ProvisionedProduct::record_ids`] holds, so a detail and a search result
    /// cannot disagree about which record was last.
    pub fn provisioned_product_detail(
        &self,
        provisioned: &ProvisionedProduct,
        now: SystemTime,
    ) -> ProvisionedProductDetail {
        ProvisionedProductDetail {
            name: Some(provisioned.name.clone()),
            arn: Some(provisioned.arn.clone()),
            r#type: Some(provisioned.r#type.clone()),
            id: Some(provisioned.id.clone()),
            status: Some(self.provisioned_product_status(provisioned, now).to_string()),
            // "The current status message of the provisioned product": what the resources
            // behind it last said, read from the same record the status is
            // ([`Self::provisioned_product_status_message`]).
            status_message: self.provisioned_product_status_message(provisioned, now),
            created_time: Some(reported_time(provisioned.created_time)),
            idempotency_token: Some(provisioned.provision_token.clone()),
            last_record_id: provisioned.record_ids.last().cloned(),
            last_provisioning_record_id: self
                .provisioned_product_records(provisioned)
                .rfind(|record| record.is_provisioning())
                .map(|record| record.id.clone()),
            last_successful_provisioning_record_id: self
                .last_successful_provisioning_record(provisioned, now)
                .map(|record| record.id.clone()),
            product_id: Some(provisioned.product_id.clone()),
            provisioning_artifact_id: Some(provisioned.provisioning_artifact_id.clone()),
            // As with a record: no launch role is assumed in this emulator.
            launch_role_arn: None,
        }
    }

    /// `provisioned` as the modelled `ProvisionedProductAttribute` — the shape a
    /// search over provisioned products reports, which carries the same stored values
    /// [`Self::provisioned_product_detail`] does plus the product and artifact NAMES,
    /// read from the product the provisioning resolved.
    pub fn provisioned_product_attribute(
        &self,
        provisioned: &ProvisionedProduct,
        now: SystemTime,
    ) -> ProvisionedProductAttribute {
        let detail = self.provisioned_product_detail(provisioned, now);
        let product = self.products.get(&provisioned.product_id);
        ProvisionedProductAttribute {
            name: detail.name,
            arn: detail.arn,
            r#type: detail.r#type,
            id: detail.id,
            status: detail.status,
            status_message: detail.status_message,
            created_time: detail.created_time,
            idempotency_token: detail.idempotency_token,
            last_record_id: detail.last_record_id,
            last_provisioning_record_id: detail.last_provisioning_record_id,
            last_successful_provisioning_record_id: detail.last_successful_provisioning_record_id,
            tags: Some(tag_list(&provisioned.tags)),
            // "The assigned identifier for the resource, such as an EC2 instance ID or
            // an S3 bucket name": a CloudFormation-backed provisioned product IS one
            // stack, so the resource behind it is that stack, reported by the id
            // CloudFormation minted for it when the launch stood it up. A provisioned
            // product no launch has stood a stack up for has no such resource.
            physical_id: provisioned.stack_id.clone(),
            product_id: detail.product_id,
            product_name: product.map(|product| product.name.clone()),
            provisioning_artifact_id: detail.provisioning_artifact_id,
            provisioning_artifact_name: product
                .and_then(|product| {
                    product
                        .provisioning_artifacts
                        .get(&provisioned.provisioning_artifact_id)
                })
                .and_then(|artifact| artifact.name.clone()),
            // The ARN of the user that provisioned the product, and of the session it
            // used: the emulated service verifies no caller identity, so neither is
            // known rather than fabricated.
            user_arn: None,
            user_arn_session: None,
        }
    }

    /// The TagOptions associated with the resource `resource_id` names, or `None`
    /// when this account holds no such resource.
    ///
    /// `AssociateTagOptionWithResource` takes "the resource identifier" of a
    /// portfolio OR a product, so both kinds resolve through this one lookup and no
    /// handler has to know which maps a resource can live in.
    pub fn resource_tag_option_ids(&self, resource_id: &str) -> Option<&BTreeSet<String>> {
        self.portfolios
            .get(resource_id)
            .map(|portfolio| &portfolio.tag_option_ids)
            .or_else(|| {
                self.products
                    .get(resource_id)
                    .map(|product| &product.tag_option_ids)
            })
    }

    /// The same set, for the handler that associates or disassociates a TagOption.
    pub fn resource_tag_option_ids_mut(
        &mut self,
        resource_id: &str,
    ) -> Option<&mut BTreeSet<String>> {
        if let Some(portfolio) = self.portfolios.get_mut(resource_id) {
            return Some(&mut portfolio.tag_option_ids);
        }
        self.products
            .get_mut(resource_id)
            .map(|product| &mut product.tag_option_ids)
    }

    /// The stored details of the TagOptions associated with `resource_id`, in
    /// TagOption id order — the one list every response that reports a resource's
    /// `TagOptions` carries, so a portfolio and a product report an association the
    /// same way. Empty for a resource this account does not hold, which is a
    /// not-found for the caller to answer.
    pub fn resource_tag_option_details(&self, resource_id: &str) -> Vec<TagOptionDetail> {
        self.resource_tag_option_ids(resource_id)
            .into_iter()
            .flatten()
            .filter_map(|id| self.tag_options.get(id))
            .map(TagOption::detail)
            .collect()
    }

    /// The budgets associated with the resource `resource_id` names, or `None` when
    /// this account holds no such resource.
    ///
    /// `AssociateBudgetWithResource` takes "the resource identifier. Either a
    /// portfolio-id or a product-id", so both kinds resolve through this one lookup
    /// and no handler has to know which map a resource lives in.
    pub fn resource_budget_names(&self, resource_id: &str) -> Option<&BTreeSet<String>> {
        self.portfolios
            .get(resource_id)
            .map(|portfolio| &portfolio.budget_names)
            .or_else(|| {
                self.products
                    .get(resource_id)
                    .map(|product| &product.budget_names)
            })
    }

    /// The same set, for the handler that associates or disassociates a budget.
    pub fn resource_budget_names_mut(
        &mut self,
        resource_id: &str,
    ) -> Option<&mut BTreeSet<String>> {
        if let Some(portfolio) = self.portfolios.get_mut(resource_id) {
            return Some(&mut portfolio.budget_names);
        }
        self.products
            .get_mut(resource_id)
            .map(|product| &mut product.budget_names)
    }

    /// The stored budget associations of `resource_id` as the modelled
    /// `BudgetDetail`s, in budget-name order — the one list every response that
    /// reports a resource's `Budgets` carries, so a portfolio and a product report an
    /// association the same way and `ListBudgetsForResource` pages over that same
    /// order. Empty for a resource this account does not hold, which is a not-found
    /// for the caller to answer.
    ///
    /// A `BudgetDetail` carries the budget's name and nothing else
    /// (`smithy_query query="shape" shape="BudgetDetail"`), so the stored association
    /// is the whole of what a read reports: the budget itself is AWS Budgets' record,
    /// not this service's.
    pub fn resource_budget_details(&self, resource_id: &str) -> Vec<BudgetDetail> {
        self.resource_budget_names(resource_id)
            .into_iter()
            .flatten()
            .map(|name| BudgetDetail {
                budget_name: Some(name.clone()),
            })
            .collect()
    }

    /// The portfolios and products `tag_option_id` is associated with, portfolios
    /// first and each group in id order. A TagOption one of them still holds is in
    /// use, which is what `DeleteTagOption` refuses a delete for.
    pub fn resources_with_tag_option(&self, tag_option_id: &str) -> Vec<String> {
        self.portfolios
            .values()
            .filter(|portfolio| portfolio.tag_option_ids.contains(tag_option_id))
            .map(|portfolio| portfolio.id.clone())
            .chain(
                self.products
                    .values()
                    .filter(|product| product.tag_option_ids.contains(tag_option_id))
                    .map(|product| product.id.clone()),
            )
            .collect()
    }

    /// The stored details of the resources `tag_option_id` is associated with, in
    /// resource-id order, narrowed to one kind when `resource_type` names one
    /// ([`RESOURCE_TYPE_PORTFOLIO`] or [`RESOURCE_TYPE_PRODUCT`]); `None`, or a kind
    /// name that is empty, narrows nothing.
    ///
    /// The exact inverse of [`Self::resource_tag_option_details`] over the one stored
    /// set of (resource, TagOption) pairs: a resource reports a TagOption precisely
    /// when that TagOption reports the resource. Neither the model nor AWS's
    /// documentation fixes an order for the list, so this service reports
    /// resource-id order — one deterministic order across both kinds, which is also
    /// the order its page cursor walks.
    pub fn resource_details_with_tag_option(
        &self,
        tag_option_id: &str,
        resource_type: Option<&str>,
    ) -> Vec<ResourceDetail> {
        let wanted = resource_type.filter(|kind| !kind.is_empty());
        let wants = |kind: &str| wanted.is_none_or(|requested| requested == kind);
        let mut details: Vec<ResourceDetail> = Vec::new();
        if wants(RESOURCE_TYPE_PORTFOLIO) {
            details.extend(
                self.portfolios
                    .values()
                    .filter(|portfolio| portfolio.tag_option_ids.contains(tag_option_id))
                    .map(Portfolio::resource_detail),
            );
        }
        if wants(RESOURCE_TYPE_PRODUCT) {
            details.extend(
                self.products
                    .values()
                    .filter(|product| product.tag_option_ids.contains(tag_option_id))
                    .map(Product::resource_detail),
            );
        }
        details.sort_by(|left, right| left.id.cmp(&right.id));
        details
    }

    /// Mint an unused portfolio id of AWS's form: `port-` followed by 12
    /// lowercase base-36 characters, which satisfies the modelled `Id` pattern
    /// `^[a-zA-Z0-9_\-]*$` and its 1..100 length.
    pub fn next_portfolio_id(&mut self) -> String {
        loop {
            let id = self.mint_id("port");
            if !self.portfolios.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an unused constraint id (`cons-…`), the same shape as a portfolio id.
    pub fn next_constraint_id(&mut self) -> String {
        loop {
            let id = self.mint_id("cons");
            if !self.constraints.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an unused product id (`prod-…`), the same shape as a portfolio id.
    pub fn next_product_id(&mut self) -> String {
        loop {
            let id = self.mint_id("prod");
            if !self.products.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an unused product VIEW id (`prodview-…`), which `ProductViewSummary.Id`
    /// carries beside the product's own id.
    pub fn next_product_view_id(&mut self) -> String {
        loop {
            let id = self.mint_id("prodview");
            if !self.products.values().any(|product| product.view_id == id) {
                return id;
            }
        }
    }

    /// Mint an unused provisioning-artifact id (`pa-…`), unique across every product's
    /// artifacts.
    pub fn next_provisioning_artifact_id(&mut self) -> String {
        loop {
            let id = self.mint_id("pa");
            if !self
                .products
                .values()
                .any(|product| product.provisioning_artifacts.contains_key(&id))
            {
                return id;
            }
        }
    }

    /// Mint an unused service-action id (`act-…`).
    pub fn next_service_action_id(&mut self) -> String {
        loop {
            let id = self.mint_id("act");
            if !self.service_actions.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an unused provisioned-product id (`pp-…`), the same shape as a portfolio
    /// id.
    pub fn next_provisioned_product_id(&mut self) -> String {
        loop {
            let id = self.mint_id("pp");
            if !self.provisioned_products.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an unused plan id (`plan-…`), the same shape as a portfolio id.
    pub fn next_provisioned_product_plan_id(&mut self) -> String {
        loop {
            let id = self.mint_id("plan");
            if !self.provisioned_product_plans.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an unused record id (`rec-…`).
    pub fn next_record_id(&mut self) -> String {
        loop {
            let id = self.mint_id("rec");
            if !self.records.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an unused TagOption id (`tag-…`).
    pub fn next_tag_option_id(&mut self) -> String {
        loop {
            let id = self.mint_id("tag");
            if !self.tag_options.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an unused `CopyProductToken` (`copy-product-…`) — the token `CopyProduct`
    /// answers and `DescribeCopyProductStatus` addresses the operation by. The same form
    /// as every other minted identifier here, which the modelled `Id` pattern
    /// `^[a-zA-Z0-9_\-]*$` and its 1..100 length admit.
    pub fn next_copy_product_token(&mut self) -> String {
        loop {
            let token = self.mint_id("copy-product");
            if !self.product_copies.contains_key(&token) {
                return token;
            }
        }
    }

    /// One id of the form `<prefix>-<12 lowercase base-36 characters>`, advancing the
    /// shared counter. Collision against the live set is the caller's check, which is
    /// what the `next_*_id` methods above add.
    fn mint_id(&mut self, prefix: &str) -> String {
        self.next_id_seed = self.next_id_seed.wrapping_add(1);
        format!("{prefix}-{}", base36_12(scramble(self.next_id_seed)))
    }
}

/// SplitMix64's finalizer: spreads a small counter over the whole 64-bit range so
/// successive ids do not look sequential. Deterministic and injective.
fn scramble(counter: u64) -> u64 {
    let mut z = counter.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// `value` as exactly 12 lowercase base-36 digits (`0-9a-z`), most significant
/// first. 36^12 > 4.7e18, so the modulo keeps the full width of the alphabet.
fn base36_12(value: u64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    const WIDTH: usize = 12;
    let mut n = value % 36u64.pow(WIDTH as u32);
    let mut out = [b'0'; WIDTH];
    for slot in out.iter_mut().rev() {
        *slot = DIGITS[(n % 36) as usize];
        n /= 36;
    }
    out.iter().map(|digit| char::from(*digit)).collect()
}

/// Encode the stored key a page resumes AT as an opaque page token: hex, so a
/// client cannot read a token as a resource id and the value stays inside the
/// modelled `PageToken` pattern and its 0..2024 length.
///
/// The one cursor codec every paged list over an id-keyed map in this service
/// shares, so a token one of them mints is the same shape as the next one's.
pub fn encode_page_cursor(key: &str) -> String {
    hex::encode(key)
}

/// The inverse of [`encode_page_cursor`]. `None` for anything this service did not
/// mint: not hex, not UTF-8, or not a value the modelled `Id` pattern
/// `^[a-zA-Z0-9_\-]*$` allows.
pub fn decode_page_cursor(token: &str) -> Option<String> {
    let key = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let well_formed = !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    well_formed.then_some(key)
}

/// The same inverse for a paged list whose cursor key is a modelled FREE-FORM string
/// rather than an `Id` — a budget name, say, which the model constrains only by length —
/// so the key cannot be held to the `Id` pattern. `None` for anything this service did
/// not mint: not hex, not UTF-8, or empty.
pub fn decode_text_page_cursor(token: &str) -> Option<String> {
    let key = String::from_utf8(hex::decode(token).ok()?).ok()?;
    (!key.is_empty()).then_some(key)
}

/// The failure a handler answers when this service's state cannot be reached —
/// i.e. the emulator was built on an engine type this service cannot use, so the
/// operation cannot be served at all. Unreachable in the assembled emulator,
/// which builds exactly one `InMemoryEngine`; never answer it by constructing a
/// private engine, whose writes no later request can see.
pub fn state_unavailable<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::NotImplemented(crate::scaffolded::NotImplemented {
        message: "this emulator was built on an engine this service cannot reach its state \
                  through, so no Service Catalog operation can be served"
            .to_string(),
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsServicecatalogAws242servicecatalogserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsServicecatalogAws242servicecatalogserviceState>)
}

/// The region this request was signed for, which a minted ARN carries. Falls back to
/// the emulator's default region for an unsigned request, which has no scope to read a
/// region from.
///
/// Shared by every handler that mints an ARN, so one request's region cannot be read
/// two different ways.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The region an unsigned request's ARNs are minted in.
const DEFAULT_REGION: &str = "us-east-1";
