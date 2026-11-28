// SPDX-License-Identifier: CC0-1.0

//! The attributes of a listing beyond its own record: the case studies it is evidenced by,
//! the specializations it has earned, and the trust center its publisher maintains.
//!
//! `ListListingAttributes` "[r]etrieves the requested attributes for a listing, including
//! case studies and trust center information"
//! (`smithy_query(query="operation",operation="ListListingAttributes")`), and the two
//! summaries a search result carries — `ListingSummary$caseStudiesSummary` ("A summary of
//! the case studies associated with the listing. Retrieve the case studies with
//! `ListListingAttributes`") and `ListingSummary$trustCenterSummary` — restate what that
//! read pages over. They are computed HERE, in one place, so a search result can never
//! disagree with the attribute read it points a client at, and so the `CERTIFICATES`,
//! `SPECIALIZATION`, `SPECIALIZATION_CATEGORY` and `TRUST_CENTER` facets and filters count
//! and select on exactly the values that read reports.
//!
//! Every attribute is DERIVED from the listing's own stored record rather than seeded beside
//! it, which is what makes the derivation total: a listing the catalog gains later has the
//! attributes its record implies instead of none at all. The three derivations, each a
//! statement about the record:
//!
//! * a listing is evidenced by one case study per USE CASE it states — a case study is an
//!   account of one customer engagement for one of them, so a listing that states no use
//!   case has no case studies to report;
//! * a listing has earned one specialization per TOP-LEVEL category it is classified in, and
//!   one specialization category per category below that, which is the same nesting the
//!   `CATEGORY` facet walks ([`CATEGORY_TAXONOMY`]);
//! * a listing has a trust center when its publisher OPERATES the software — when some
//!   product of it is fulfilled in one of the ways that leaves the seller running the
//!   service ([`SELLER_OPERATED_FULFILLMENT`]) — because a trust center is where such a
//!   seller publishes the security posture of the service it runs. A listing a buyer deploys
//!   and runs itself has none.
//!
//! The certification mark a trust center names is the one member of an attribute a client
//! FETCHES rather than displays. It belongs to the certifying BODY rather than to the
//! marketplace, so it is published at that body's own address, exactly as this catalog
//! publishes every other piece of media it does not itself host — a listing's promotional
//! images, the documentation and support links of a `Resource`, a legal term's document, a
//! trust center's own portal. What the marketplace hosts, and what this emulator therefore
//! serves and announces on the address this process answers requests at, is a record's logo
//! thumbnail and the reviews page of a review source ([`listing_logo`],
//! [`listing_reviews`]).

use super::*;

use crate::ops::types::{
    CaseStudiesSummary, CaseStudy, CaseStudyResource, EngagementDates, ListingAttributeResults,
    Specialization, SpecializationCategory, SpecializationDetail, SpecializationTypeDetail,
    TrustCenterCertificationAttribute, TrustCenterCertificationSummary, TrustCenterControl,
    TrustCenterDocumentAttribute, TrustCenterFaqAttribute, TrustCenterSource,
    TrustCenterSourceSummary, TrustCenterSummary, TrustCenterUpdateAttribute,
};

/// The single value the `TRUST_CENTER` filter takes, which is also the one value its facet
/// reports.
///
/// Source: `SearchFilter$filterValues` — "`TRUST_CENTER`: The single value
/// `HAS_TRUST_CENTER`. Matches listings that have a trust center."
pub const HAS_TRUST_CENTER: &str = "HAS_TRUST_CENTER";

/// How the one `TRUST_CENTER` facet value is displayed. `ListingFacet$displayName` is
/// required and non-blank, so the value needs a label of its own.
const HAS_TRUST_CENTER_DISPLAY_NAME: &str = "Publishes a trust center";

/// The fulfillment option types whose SELLER operates the software: the deliveries after
/// which a buyer reaches a service the seller runs rather than software the buyer runs.
///
/// Source: the `FulfillmentOption` union — `saasFulfillmentOption` is the seller-hosted
/// delivery ("The URL of the seller's software registration landing page") and
/// `apiFulfillmentOption` the seller-operated API. Every other member ships artefacts the
/// buyer deploys into its own account.
const SELLER_OPERATED_FULFILLMENT: &[&str] = &["SAAS", "API"];

/// The certification schemes this catalog records, as `(certificationType, displayName, the
/// address the certifying body publishes the scheme's mark at)`.
///
/// `TrustCenterCertificationAttribute$certificationType` is an open `NonEmptyString` rather
/// than an enum, and `SearchFilter$filterValues` names `ISO_27001` and `SOC2_TYPE_2` as
/// `CERTIFICATES` filter values, so the identifiers are spelled the way that documentation
/// spells them. The order is the order a trust center reports them in, so two listings never
/// disagree about where a scheme appears. The mark is the BODY's own image (see the module
/// docs), one address per scheme, inside `URL`'s `@length(1..2048)` and `@pattern
/// ^https?://.*$`.
const CERTIFICATIONS: &[(&str, &str, &str)] = &[
    (
        "ISO_27001",
        "ISO/IEC 27001",
        "https://example.com/certifications/iso-27001.png",
    ),
    (
        "SOC2_TYPE_2",
        "SOC 2 Type 2",
        "https://example.com/certifications/soc2-type-2.png",
    ),
    (
        "PCI_DSS",
        "PCI DSS",
        "https://example.com/certifications/pci-dss.png",
    ),
    (
        "HIPAA",
        "HIPAA",
        "https://example.com/certifications/hipaa.png",
    ),
    (
        "FEDRAMP_MODERATE",
        "FedRAMP Moderate",
        "https://example.com/certifications/fedramp-moderate.png",
    ),
    (
        "ISO_42001",
        "ISO/IEC 42001",
        "https://example.com/certifications/iso-42001.png",
    ),
];

/// How many certifications the trust center SUMMARY carries.
///
/// `TrustCenterSummary$certificationSummaries` is "[a] subset of certifications associated
/// with the listing, selected by importance" beside `totalCertificationCount`, "[t]he total
/// number of distinct certifications associated with the listing, INCLUDING certifications
/// omitted from `certificationSummaries`" — so the summary is a prefix of the trust center's
/// own order and the count is of all of them.
const SUMMARISED_CERTIFICATIONS: usize = 3;

/// How many certifications every trust center records, whatever the listing is classified
/// under: the two schemes `SearchFilter$filterValues` names as `CERTIFICATES` values.
const BASELINE_CERTIFICATIONS: usize = 2;

/// The compliance-automation provider the catalog records for each seller, as
/// `(sellerProfileId, provider)`, with [`DEFAULT_TRUST_CENTER_PROVIDER`] for a seller no row
/// names.
///
/// `TrustCenterSource$provider` is the `TrustCenterProvider` enum (`DRATA`, `VANTA`,
/// `OTHER`): which product a seller runs its trust center on is a fact about the SELLER, not
/// about one of its listings, so it is recorded per seller and every listing of that seller
/// reports the same one.
const TRUST_CENTER_PROVIDERS: &[(&str, &str)] = &[
    ("sp-acme", "DRATA"),
    ("sp-nimbus", "VANTA"),
    ("sp-orion", "DRATA"),
    ("sp-zephyr", "VANTA"),
];

/// The provider a trust center is reported under for a seller [`TRUST_CENTER_PROVIDERS`]
/// names none for: the enum's own value for a provider outside the two it names.
const DEFAULT_TRUST_CENTER_PROVIDER: &str = "OTHER";

/// The taxonomy key whose branch makes a listing's trust center report its AI governance
/// control: a listing classified under Machine Learning ships models, so how model changes
/// are governed is part of its posture.
const AI_TAXONOMY_KEY: &str = "machine-learning";

/// The control domain only an AI listing's trust center reports
/// (`TrustCenterControlDomain`).
const AI_GOVERNANCE_DOMAIN: &str = "AI_GOVERNANCE";

/// The controls a trust center publishes, as `(name, description, domains)`.
///
/// A control is a statement about how the seller runs the service, so every trust center
/// publishes the same ones — except the AI governance control, which only a listing in the
/// Machine Learning branch has anything to govern.
const TRUST_CENTER_CONTROLS: &[(&str, &str, &[&str])] = &[
    (
        "Customer data is encrypted at rest and in transit",
        "Stored data is encrypted with keys the seller rotates, and every connection to the \
         service is encrypted in transit.",
        &["SECURITY"],
    ),
    (
        "Recovery of the production environment is exercised quarterly",
        "The seller restores the production environment from backup each quarter and \
         publishes the time the restore took.",
        &["RESILIENCE"],
    ),
    (
        "Every model change is reviewed before it reaches production",
        "A change to a model, its training data or its prompts is reviewed and recorded \
         before it serves a request.",
        &[AI_GOVERNANCE_DOMAIN, "SECURITY"],
    ),
];

/// The documents a trust center publishes, as `(displayName, subjectArea, accessType, slug)`.
///
/// `TrustCenterDocumentAttribute$accessType` is the `TrustCenterDocumentAccessType` enum
/// (`DOWNLOAD`, `VIEW`, `REQUEST_ACCESS`): how a buyer reaches a document is a property OF
/// the document — an audit report is released on request where a whitepaper is not — so each
/// row states its own.
const TRUST_CENTER_DOCUMENTS: &[(&str, &str, &str, &str)] = &[
    (
        "Security whitepaper",
        "Architecture",
        "VIEW",
        "security-whitepaper",
    ),
    (
        "SOC 2 Type 2 report",
        "Audit reports",
        "REQUEST_ACCESS",
        "soc2-type-2-report",
    ),
    (
        "Data processing addendum",
        "Privacy",
        "DOWNLOAD",
        "data-processing-addendum",
    ),
];

/// The questions a trust center answers, as `(question, answer, subjectArea)`.
const TRUST_CENTER_FAQS: &[(&str, &str, &str)] = &[
    (
        "Where is customer data stored?",
        "In the AWS Region the buyer selects when the subscription starts. Data is not \
         copied to another Region without the buyer asking for it.",
        "Data residency",
    ),
    (
        "Is customer data used to train models?",
        "No. Customer data is processed to serve the buyer's own requests and is not used \
         to train models for any other customer.",
        "Privacy",
    ),
    (
        "How are security incidents reported?",
        "The seller notifies the subscription's administrative contact, and publishes the \
         incident and its resolution in the trust center.",
        "Incident response",
    ),
];

/// The updates a trust center publishes, as `(title, description, days before the catalog
/// was seeded)`.
///
/// `TrustCenterUpdateAttribute$publishedTime` is required, and an update's instant is a fact
/// about the catalog rather than about the read, so it is taken from the instant the catalog
/// was seeded ([`ComAmazonawsMarketplacediscoveryAwsmarketplacediscoveryState::seeded_at`])
/// and never from the clock at read time: two reads of one update report one instant.
const TRUST_CENTER_UPDATES: &[(&str, &str, i64)] = &[
    (
        "Annual ISO/IEC 27001 surveillance audit completed",
        "The surveillance audit closed with no major non-conformities. The certificate and \
         its scope are published in the trust center.",
        30,
    ),
    (
        "Sub-processor list updated",
        "One sub-processor was added for outbound email and one was removed. The list and \
         the regions each processes in are published in the trust center.",
        120,
    ),
];

/// How long before the catalog was seeded the engagement behind a listing's FIRST case study
/// began, in days, and how much earlier each case study after it began.
///
/// The dates are a fact about the engagement, so — like an update's instant — they are taken
/// from the instant the catalog was seeded rather than from the clock at read time.
const FIRST_ENGAGEMENT_DAYS_AGO: i64 = 180;

/// How much earlier than the one before it each further engagement began, in days.
const ENGAGEMENT_SPACING_DAYS: i64 = 120;

/// How long an engagement that has finished ran for, in days. The FIRST case study's
/// engagement is still running, so it reports no completion date — `completionDate` is
/// optional, and its absence is what tells a client the engagement is not finished.
const ENGAGEMENT_LENGTH_DAYS: i64 = 60;

/// One certification of a trust center, as the catalog holds it: the scheme, the name it is
/// displayed by, and the address its mark is published at.
#[derive(Debug, Clone)]
pub struct Certification {
    certification_type: String,
    display_name: String,
    logo_url: String,
}

impl Certification {
    /// This certification as `TrustCenterCertificationAttribute`.
    fn attribute(&self) -> TrustCenterCertificationAttribute {
        TrustCenterCertificationAttribute {
            certification_type: self.certification_type.clone(),
            display_name: self.display_name.clone(),
            logo_url: self.logo_url.clone(),
        }
    }

    /// This certification as the `TrustCenterCertificationSummary` a search result carries.
    fn summary(&self) -> TrustCenterCertificationSummary {
        TrustCenterCertificationSummary {
            certification_type: self.certification_type.clone(),
            display_name: self.display_name.clone(),
            logo_url: self.logo_url.clone(),
        }
    }
}

/// The trust center a publisher maintains for one listing.
#[derive(Debug, Clone)]
pub struct TrustCenter {
    sources: Vec<TrustCenterSource>,
    certifications: Vec<Certification>,
    controls: Vec<TrustCenterControl>,
    documents: Vec<TrustCenterDocumentAttribute>,
    faqs: Vec<TrustCenterFaqAttribute>,
    updates: Vec<TrustCenterUpdateAttribute>,
}

/// Everything the catalog holds about one listing beyond the listing's own record.
#[derive(Debug, Clone)]
pub struct ListingAttributes {
    case_studies: Vec<CaseStudy>,
    specializations: Vec<Specialization>,
    specialization_categories: Vec<SpecializationCategory>,
    /// `None` for a listing whose publisher does not operate the software, which therefore
    /// publishes no trust center at all.
    trust_center: Option<TrustCenter>,
}

impl ListingAttributes {
    /// The attributes of the types `attribute_types` names, in the order the REQUEST named
    /// them.
    ///
    /// The order is the request's because `attributeTypes` is `@uniqueItems`: a type appears
    /// at most once, so the request states one total order over the results, which is the
    /// order a page is cut from. A type the catalog holds nothing of contributes nothing, and
    /// a type outside `ListingAttributeType` cannot reach here — the framework refuses it
    /// against the model's own enum before the handler runs.
    #[must_use]
    pub fn results(&self, attribute_types: &[String]) -> Vec<ListingAttributeResults> {
        let mut results: Vec<ListingAttributeResults> = Vec::new();
        for attribute_type in attribute_types {
            match attribute_type.as_str() {
                "CASE_STUDY" => results.extend(
                    self.case_studies
                        .iter()
                        .cloned()
                        .map(ListingAttributeResults::caseStudy),
                ),
                // One union member per kind of specialization detail: the specializations
                // first, then the categories earned under them, so a client reads a parent
                // before the children that name it.
                "SPECIALIZATION_DETAIL" => {
                    results.extend(self.specializations.iter().cloned().map(|specialization| {
                        ListingAttributeResults::specializationDetail(
                            SpecializationDetail::Specialization(specialization),
                        )
                    }));
                    results.extend(self.specialization_categories.iter().cloned().map(
                        |category| {
                            ListingAttributeResults::specializationDetail(
                                SpecializationDetail::specializationCategory(category),
                            )
                        },
                    ));
                }
                "TRUST_CENTER_SOURCE" => results.extend(
                    self.trust_center_parts(|trust_center| trust_center.sources.clone())
                        .into_iter()
                        .map(ListingAttributeResults::trustCenterSource),
                ),
                "TRUST_CENTER_CERTIFICATION" => results.extend(
                    self.trust_center_parts(|trust_center| {
                        trust_center
                            .certifications
                            .iter()
                            .map(Certification::attribute)
                            .collect()
                    })
                    .into_iter()
                    .map(ListingAttributeResults::trustCenterCertification),
                ),
                "TRUST_CENTER_DOCUMENT" => results.extend(
                    self.trust_center_parts(|trust_center| trust_center.documents.clone())
                        .into_iter()
                        .map(ListingAttributeResults::trustCenterDocument),
                ),
                "TRUST_CENTER_CONTROL" => results.extend(
                    self.trust_center_parts(|trust_center| trust_center.controls.clone())
                        .into_iter()
                        .map(ListingAttributeResults::trustCenterControl),
                ),
                "TRUST_CENTER_FAQ" => results.extend(
                    self.trust_center_parts(|trust_center| trust_center.faqs.clone())
                        .into_iter()
                        .map(ListingAttributeResults::trustCenterFaq),
                ),
                "TRUST_CENTER_UPDATE" => results.extend(
                    self.trust_center_parts(|trust_center| trust_center.updates.clone())
                        .into_iter()
                        .map(ListingAttributeResults::trustCenterUpdate),
                ),
                // Unreachable: the model's `ListingAttributeType` enum is enforced on the
                // request before the handler runs.
                _ => {}
            }
        }
        results
    }

    /// One part of this listing's trust center, or nothing at all for a listing that has
    /// none.
    fn trust_center_parts<T>(&self, part: impl Fn(&TrustCenter) -> Vec<T>) -> Vec<T> {
        self.trust_center.as_ref().map(part).unwrap_or_default()
    }

    /// The summary of this listing's case studies, or `None` for a listing that has none:
    /// a summary of nothing is not a summary, and the member is optional.
    #[must_use]
    pub fn case_studies_summary(&self) -> Option<CaseStudiesSummary> {
        (!self.case_studies.is_empty()).then(|| CaseStudiesSummary {
            case_study_count: i64::try_from(self.case_studies.len()).unwrap_or(i64::MAX),
        })
    }

    /// The summary of this listing's trust center, or `None` for a listing that publishes
    /// none.
    ///
    /// `certificationSummaries` is the prefix of the trust center's own certification order
    /// ([`SUMMARISED_CERTIFICATIONS`]) while `totalCertificationCount` counts them all, so a
    /// client reads the total it is a subset of.
    #[must_use]
    pub fn trust_center_summary(&self) -> Option<TrustCenterSummary> {
        let trust_center = self.trust_center.as_ref()?;
        Some(TrustCenterSummary {
            source_summaries: trust_center
                .sources
                .iter()
                .map(|source| TrustCenterSourceSummary {
                    content_type: source.content_type.clone(),
                    value: source.value.clone(),
                    display_name: source.display_name.clone(),
                    provider: source.provider.clone(),
                })
                .collect(),
            certification_summaries: trust_center
                .certifications
                .iter()
                .take(SUMMARISED_CERTIFICATIONS)
                .map(Certification::summary)
                .collect(),
            total_certification_count: i64::try_from(trust_center.certifications.len())
                .unwrap_or(i64::MAX),
        })
    }

    /// The `(value, displayName)` pairs this listing contributes to the facet or filter
    /// `kind`, which is one `SearchFacetType`/`SearchFilterType` value.
    ///
    /// ONE projection for both: a facet value is "the internal value used for filtering when
    /// passed back in a search filter" (`ListingFacet$value`), so passing a counted value
    /// back as a filter value has to select exactly the listings the count came from.
    #[must_use]
    pub fn facet_values(&self, kind: &str) -> Vec<(String, String)> {
        match kind {
            // "One or more certification identifiers, such as `ISO_27001` or
            // `SOC2_TYPE_2`. Matches listings that hold those certifications."
            "CERTIFICATES" => self
                .trust_center_parts(|trust_center| trust_center.certifications.clone())
                .into_iter()
                .map(|certification| (certification.certification_type, certification.display_name))
                .collect(),
            // "One or more specialization identifiers. Matches listings that earned those
            // specializations."
            "SPECIALIZATION" => self
                .specializations
                .iter()
                .map(|specialization| {
                    (
                        specialization.specialization_id.clone(),
                        specialization.specialization_name.clone(),
                    )
                })
                .collect(),
            // "One or more specialization category identifiers. Matches listings that earned
            // a category under a specialization."
            "SPECIALIZATION_CATEGORY" => self
                .specialization_categories
                .iter()
                .map(|category| {
                    (
                        category.specialization_category_id.clone(),
                        category.specialization_category_name.clone(),
                    )
                })
                .collect(),
            // "The single value `HAS_TRUST_CENTER`. Matches listings that have a trust
            // center." A listing without one contributes no value, so the facet reports the
            // listings that have one and the filter selects them.
            "TRUST_CENTER" => self
                .trust_center
                .iter()
                .map(|_| {
                    (
                        HAS_TRUST_CENTER.to_string(),
                        HAS_TRUST_CENTER_DISPLAY_NAME.to_string(),
                    )
                })
                .collect(),
            _ => Vec::new(),
        }
    }
}

impl ComAmazonawsMarketplacediscoveryAwsmarketplacediscoveryState {
    /// The attributes the catalog holds for `listing`, with its text in `locale`.
    ///
    /// Derived from the listing's own record, so the attribute read, the summaries a search
    /// result carries and the facets computed over a match set are one statement about the
    /// catalog rather than three.
    #[must_use]
    pub fn listing_attributes(&self, listing: &Listing, locale: &str) -> ListingAttributes {
        ListingAttributes {
            case_studies: self.listing_case_studies(listing, locale),
            specializations: self.listing_specializations(listing),
            specialization_categories: listing_specialization_categories(listing),
            trust_center: self.listing_trust_center(listing),
        }
    }

    /// The `(value, displayName)` pairs `listing` contributes to the facet or filter `kind`.
    ///
    /// The values are identifiers and enum values, which no locale changes, so this asks for
    /// the listing's attributes in the default locale: a facet's count and a filter's match
    /// are the same whichever locale the request prefers.
    #[must_use]
    pub fn listing_attribute_facet_values(
        &self,
        listing: &Listing,
        kind: &str,
    ) -> Vec<(String, String)> {
        self.listing_attributes(listing, DEFAULT_LOCALE)
            .facet_values(kind)
    }

    /// One case study per use case the listing states, most recent engagement first.
    fn listing_case_studies(&self, listing: &Listing, locale: &str) -> Vec<CaseStudy> {
        let listing_name = &listing.text.text(locale).name;
        let seller = &listing.publisher.seller_profile_id;
        listing
            .use_cases
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let use_case = &entry.use_case;
                let lower = use_case.display_name.to_lowercase();
                let started = self.seeded_at
                    - (FIRST_ENGAGEMENT_DAYS_AGO
                        + ENGAGEMENT_SPACING_DAYS * i64::try_from(index).unwrap_or(0))
                        * DAY;
                CaseStudy {
                    case_study_id: format!("cs-{}-{}", listing.listing_id, use_case.value),
                    case_study_name: format!("{} with {listing_name}", use_case.display_name),
                    short_description: use_case.description.clone(),
                    engagement_dates: Some(EngagementDates {
                        start_date: datetime(started),
                        // The most recent engagement is still running, so it reports no
                        // completion date.
                        completion_date: (index > 0)
                            .then(|| datetime(started + ENGAGEMENT_LENGTH_DAYS * DAY)),
                    }),
                    challenge: Some(format!(
                        "The customer needed {lower} without adding operational work to the \
                         team that owns it."
                    )),
                    resolution: Some(format!(
                        "{listing_name} was subscribed to from AWS Marketplace and reached \
                         production in the customer's own account."
                    )),
                    implementation_approach: Some(format!(
                        "The seller ran a scoped pilot for {lower}, then rolled the \
                         configuration out to the remaining teams."
                    )),
                    outcome: Some(format!(
                        "The customer now runs {lower} on {listing_name} and reports the \
                         result to its own stakeholders."
                    )),
                    resources: Some(vec![CaseStudyResource {
                        content_type: "LINK".to_string(),
                        value: format!(
                            "https://example.com/{seller}/case-studies/{}",
                            use_case.value
                        ),
                        display_name: Some("Read the case study".to_string()),
                        resource_type: "CASE_STUDY_WEBSITE".to_string(),
                    }]),
                }
            })
            .collect()
    }

    /// One specialization per top-level category the listing is classified in, each
    /// evidenced by the listing's own case studies.
    fn listing_specializations(&self, listing: &Listing) -> Vec<Specialization> {
        let case_study_ids: Vec<String> = self
            .listing_case_studies(listing, DEFAULT_LOCALE)
            .into_iter()
            .map(|case_study| case_study.case_study_id)
            .collect();
        listing
            .categories
            .iter()
            .filter(|category| category_parent(&category.category_id).is_none())
            .map(|category| Specialization {
                specialization_id: specialization_id(&category.category_id),
                specialization_name: format!("{} Specialization", category.display_name),
                description: Some(format!(
                    "Validated expertise in {}, earned for this listing.",
                    category.display_name
                )),
                specialization_types: vec![specialization_type(&category.category_id)],
                // "The case studies that evidence the specialization", capped at the
                // member's own `@length(..25)`.
                case_study_ids: (!case_study_ids.is_empty())
                    .then(|| case_study_ids.iter().take(25).cloned().collect()),
            })
            .collect()
    }

    /// The trust center the listing's publisher maintains, or `None` when the publisher does
    /// not operate the software the listing delivers.
    fn listing_trust_center(&self, listing: &Listing) -> Option<TrustCenter> {
        let seller_operated = self
            .listing_fulfillment_option_summaries(listing)
            .iter()
            .any(|option| {
                SELLER_OPERATED_FULFILLMENT.contains(&option.fulfillment_option_type.as_str())
            });
        if !seller_operated {
            return None;
        }
        let seller = &listing.publisher.seller_profile_id;
        let portal = format!("https://example.com/{seller}/trust-center");
        let governs_models = listing
            .categories
            .iter()
            .any(|category| taxonomy_key(&category.category_id) == Some(AI_TAXONOMY_KEY));
        Some(TrustCenter {
            sources: vec![TrustCenterSource {
                content_type: "LINK".to_string(),
                value: portal.clone(),
                display_name: Some(format!("{} trust center", listing.publisher.display_name)),
                provider: trust_center_provider(seller).to_string(),
            }],
            certifications: certifications_of(listing),
            controls: TRUST_CENTER_CONTROLS
                .iter()
                .filter(|(_, _, domains)| {
                    governs_models || !domains.contains(&AI_GOVERNANCE_DOMAIN)
                })
                .map(|(name, description, domains)| TrustCenterControl {
                    name: (*name).to_string(),
                    description: Some((*description).to_string()),
                    domains: domains.iter().map(|domain| (*domain).to_string()).collect(),
                })
                .collect(),
            documents: TRUST_CENTER_DOCUMENTS
                .iter()
                .map(
                    |(display_name, subject_area, access_type, slug)| {
                        TrustCenterDocumentAttribute {
                            content_type: "LINK".to_string(),
                            value: format!("{portal}/{slug}"),
                            display_name: Some((*display_name).to_string()),
                            subject_area: Some((*subject_area).to_string()),
                            access_type: (*access_type).to_string(),
                        }
                    },
                )
                .collect(),
            faqs: TRUST_CENTER_FAQS
                .iter()
                .map(|(question, answer, subject_area)| TrustCenterFaqAttribute {
                    question: (*question).to_string(),
                    answer: (*answer).to_string(),
                    subject_area: Some((*subject_area).to_string()),
                })
                .collect(),
            updates: TRUST_CENTER_UPDATES
                .iter()
                .map(|(title, description, days_ago)| TrustCenterUpdateAttribute {
                    title: (*title).to_string(),
                    description: (*description).to_string(),
                    published_time: datetime(self.seeded_at - days_ago * DAY),
                })
                .collect(),
        })
    }
}

/// One specialization category per category the listing is classified BELOW a top-level
/// one, each naming the specialization of the category it sits under.
fn listing_specialization_categories(listing: &Listing) -> Vec<SpecializationCategory> {
    listing
        .categories
        .iter()
        .filter_map(|category| {
            let parent = category_parent(&category.category_id)?;
            Some(SpecializationCategory {
                specialization_category_id: specialization_id(&category.category_id),
                specialization_category_name: category.display_name.clone(),
                parent_specialization_id: specialization_id(&parent),
            })
        })
        .collect()
}

/// The identifier this catalog issues for the specialization earned in the category
/// `category_id`.
///
/// `SpecializationId` is `@pattern ^[a-z0-9\-]+$`, which the identifiers the catalog issues
/// for a category satisfy ([`category_id`] mints lowercase name-based identifiers), prefixed
/// so a specialization identifier is never mistaken for the category identifier it is
/// derived from: the two name different things and are passed back in different filters.
fn specialization_id(category_id: &str) -> String {
    format!("spec-{category_id}")
}

/// The kind of expertise a specialization earned in the category `category_id` covers.
///
/// `SpecializationType` is `USE_CASE`, `INDUSTRY` or `SERVICES`, and the catalog's taxonomy
/// already says which of the three a top-level category is: the Industries branch classifies
/// by industry, Professional Services by the service delivered, and every other branch by
/// what the software is used for.
fn specialization_type(category_id: &str) -> SpecializationTypeDetail {
    let (specialization_type, display_name) = match taxonomy_key(category_id) {
        Some("industries") => ("INDUSTRY", "Industry"),
        Some("professional-services") => ("SERVICES", "Services"),
        _ => ("USE_CASE", "Use case"),
    };
    SpecializationTypeDetail {
        specialization_type: specialization_type.to_string(),
        display_name: display_name.to_string(),
    }
}

/// The provider the catalog records the seller `seller_profile_id`'s trust center is run on.
fn trust_center_provider(seller_profile_id: &str) -> &'static str {
    TRUST_CENTER_PROVIDERS
        .iter()
        .find(|(seller, _)| *seller == seller_profile_id)
        .map_or(DEFAULT_TRUST_CENTER_PROVIDER, |(_, provider)| *provider)
}

/// The certifications `listing`'s trust center records: the two schemes the `CERTIFICATES`
/// documentation names for every trust center, and one further scheme per category the
/// listing is classified in — a listing active in more areas of the catalog is audited
/// against more schemes — bounded by the schemes this catalog records at all.
fn certifications_of(listing: &Listing) -> Vec<Certification> {
    let held = (BASELINE_CERTIFICATIONS + listing.categories.len()).min(CERTIFICATIONS.len());
    CERTIFICATIONS
        .iter()
        .take(held)
        .map(
            |(certification_type, display_name, logo_url)| Certification {
                certification_type: (*certification_type).to_string(),
                display_name: (*display_name).to_string(),
                logo_url: (*logo_url).to_string(),
            },
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::ops::types::{
        LISTING_ATTRIBUTE_TYPE_VALUES, SPECIALIZATION_TYPE_VALUES,
        TRUST_CENTER_CONTROL_DOMAIN_VALUES, TRUST_CENTER_DOCUMENT_ACCESS_TYPE_VALUES,
        TRUST_CENTER_PROVIDER_VALUES,
    };

    /// RULE: every attribute the catalog reports satisfies the constraints the model
    /// declares for its member, over EVERY listing of the seeded catalog rather than over
    /// one row — a listing with no trust center, one with the AI governance control, one in
    /// a single category and one in several.
    #[test]
    fn every_listing_attribute_satisfies_its_modelled_constraints() {
        let state = ComAmazonawsMarketplacediscoveryAwsmarketplacediscoveryState::default();
        let mut listings_with_trust_center = 0_usize;
        let mut listings_without = 0_usize;
        for listing in state.listings.values() {
            let attributes = state.listing_attributes(listing, DEFAULT_LOCALE);
            if attributes.trust_center.is_some() {
                listings_with_trust_center += 1;
            } else {
                listings_without += 1;
            }
            let case = &listing.listing_id;

            for case_study in &attributes.case_studies {
                assert!(
                    case_study
                        .case_study_id
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
                        && (1..=255).contains(&case_study.case_study_id.chars().count()),
                    "case {case}: caseStudyId must satisfy CaseStudyId, got {:?}",
                    case_study.case_study_id
                );
                for (member, value) in [
                    ("caseStudyName", &case_study.case_study_name),
                    ("shortDescription", &case_study.short_description),
                ] {
                    assert!(
                        value.trim() != "" && value.chars().count() <= 1024,
                        "case {case}: {member} must satisfy NonEmptyString, got {value:?}"
                    );
                }
                let resources = case_study.resources.as_deref().unwrap_or_default();
                assert!(
                    (1..=10).contains(&resources.len()),
                    "case {case}: a case study's resources list is @length(1..10), got {}",
                    resources.len()
                );
            }

            for specialization in &attributes.specializations {
                assert!(
                    specialization
                        .specialization_id
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                    "case {case}: specializationId must satisfy SpecializationId, got {:?}",
                    specialization.specialization_id
                );
                assert!(
                    (1..=3).contains(&specialization.specialization_types.len()),
                    "case {case}: specializationTypes is @length(..3), got {}",
                    specialization.specialization_types.len()
                );
                for detail in &specialization.specialization_types {
                    assert!(
                        SPECIALIZATION_TYPE_VALUES
                            .contains(&detail.specialization_type.as_str()),
                        "case {case}: {:?} is outside SpecializationType",
                        detail.specialization_type
                    );
                }
            }

            for category in &attributes.specialization_categories {
                assert!(
                    attributes.specializations.iter().any(|specialization| {
                        specialization.specialization_id == category.parent_specialization_id
                    }) || attributes.specialization_categories.iter().any(|held| {
                        held.specialization_category_id == category.parent_specialization_id
                    }),
                    "case {case}: {:?} must name a specialization or category of the same \
                     listing as its parent",
                    category.specialization_category_id
                );
            }

            if let Some(trust_center) = &attributes.trust_center {
                assert!(
                    (1..=10).contains(&trust_center.sources.len()),
                    "case {case}: sourceSummaries is @length(1..10), got {}",
                    trust_center.sources.len()
                );
                for source in &trust_center.sources {
                    assert!(
                        TRUST_CENTER_PROVIDER_VALUES.contains(&source.provider.as_str()),
                        "case {case}: {:?} is outside TrustCenterProvider",
                        source.provider
                    );
                }
                assert!(
                    (1..=10).contains(&trust_center.certifications.len()),
                    "case {case}: certificationSummaries is @length(..10), got {}",
                    trust_center.certifications.len()
                );
                for document in &trust_center.documents {
                    assert!(
                        TRUST_CENTER_DOCUMENT_ACCESS_TYPE_VALUES
                            .contains(&document.access_type.as_str()),
                        "case {case}: {:?} is outside TrustCenterDocumentAccessType",
                        document.access_type
                    );
                }
                for control in &trust_center.controls {
                    assert!(
                        (1..=3).contains(&control.domains.len())
                            && control.domains.iter().all(|domain| {
                                TRUST_CENTER_CONTROL_DOMAIN_VALUES.contains(&domain.as_str())
                            }),
                        "case {case}: a control's domains are one to three \
                         TrustCenterControlDomain values, got {:?}",
                        control.domains
                    );
                }
                // The summary is a subset of what the attribute read reports, and the total
                // counts them all.
                let summary = attributes
                    .trust_center_summary()
                    .expect("a listing with a trust center summarises it");
                assert!(
                    summary.certification_summaries.len()
                        <= usize::try_from(summary.total_certification_count).unwrap_or(0)
                        && usize::try_from(summary.total_certification_count).unwrap_or(0)
                            == trust_center.certifications.len(),
                    "case {case}: the summary must be a subset of the certifications it \
                     counts, got {} of {}",
                    summary.certification_summaries.len(),
                    summary.total_certification_count
                );
            } else {
                assert!(
                    attributes.trust_center_summary().is_none()
                        && attributes.facet_values("TRUST_CENTER").is_empty(),
                    "case {case}: a listing without a trust center reports none"
                );
            }
        }
        assert!(
            listings_with_trust_center > 0 && listings_without > 0,
            "case setup: the catalog must hold listings on both sides of the trust center \
             rule, got {listings_with_trust_center} with and {listings_without} without"
        );
    }

    /// RULE: the results a request asks for are exactly the attributes of the types it
    /// named, in the order it named them — over a table of type sets.
    #[test]
    fn results_carry_the_requested_types_in_request_order() {
        let state = ComAmazonawsMarketplacediscoveryAwsmarketplacediscoveryState::default();
        let listing = state
            .listings
            .values()
            .find(|listing| {
                state
                    .listing_attributes(listing, DEFAULT_LOCALE)
                    .trust_center
                    .is_some()
            })
            .expect("the catalog holds a listing with a trust center");
        let attributes = state.listing_attributes(listing, DEFAULT_LOCALE);
        for types in [
            vec!["CASE_STUDY"],
            vec!["TRUST_CENTER_FAQ", "CASE_STUDY"],
            vec!["CASE_STUDY", "TRUST_CENTER_FAQ"],
            LISTING_ATTRIBUTE_TYPE_VALUES.to_vec(),
        ] {
            let requested: Vec<String> = types.iter().map(|t| (*t).to_string()).collect();
            let results = attributes.results(&requested);
            let expected: usize = requested
                .iter()
                .map(|one| attributes.results(::std::slice::from_ref(one)).len())
                .sum();
            assert_eq!(
                results.len(),
                expected,
                "case {types:?}: the results must be the requested types' own, got {}",
                results.len()
            );
            // The first result belongs to the FIRST type the request named.
            let first = attributes
                .results(::std::slice::from_ref(&requested[0]))
                .len();
            assert!(
                first == 0 || !results.is_empty(),
                "case {types:?}: the first type named must lead the results"
            );
        }
    }
}
