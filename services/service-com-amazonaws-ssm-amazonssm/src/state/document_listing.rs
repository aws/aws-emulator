// SPDX-License-Identifier: CC0-1.0

//! Listing the SSM documents an account holds in a Region: which documents a listing
//! walks, the filters that narrow it, and the identity each one is reported by.
//!
//! A listing reports the documents reachable through the Region's endpoint — the ones a
//! caller created there AND the ones Systems Manager publishes in every account
//! ([`super::PUBLISHED_DOCUMENTS`]) — so every name a read resolves
//! ([`ComAmazonawsSsmAmazonssmState::resolve_document`]) is a name the listing walks, and
//! nothing appears that no read can resolve. The walk is in ascending name order, which
//! is what makes a page's resume cursor total: the cursor is the NAME the next page starts
//! at, not an offset into a list that may have changed between two pages.
//!
//! Each document is reported ONCE, by the identity of its DEFAULT version — the version a
//! read that names no selector resolves to ([`super::DocumentRecord::select`]) — and
//! everything that identity carries beyond the whole document's name, owner, type and
//! tags comes from that version ([`document_identifier`]).
//!
//! A filter narrows the walk by one attribute of a document
//! ([`DocumentListingFilter::matches`]). Filters are ANDed and the values of one filter
//! ORed: "If you specify more than two keys, only documents that are identified by all the
//! tags are returned in the results. If you specify more than two values for a key,
//! documents that are identified by any of the values are returned in the results"
//! (`DocumentKeyValuesFilter`). The keys are the Amazon Web Services-provided ones that
//! same doc lists ([`DOCUMENT_KEY_VALUES_FILTER_KEYS`]) and the `tag:tagName` form; a key
//! outside them names no attribute of a document at all, which is refused as a
//! `ValidationException` naming the key ([`unknown_document_filter_key_message`]) rather
//! than through the member-less modelled `InvalidFilterKey` shape, which could not say
//! which key was wrong.
//!
//! The other listing of this slice walks the versions of ONE document rather than the
//! documents of a Region, and is held beside this one in
//! [`super::document_version_listing`].

use std::borrow::Cow;

use crate::ops::types::DocumentIdentifier;

use super::{
    tagging, ComAmazonawsSsmAmazonssmState, DocumentRecord, DocumentVersionRecord,
    PUBLISHED_DOCUMENTS, PUBLISHED_DOCUMENT_OWNER, TAG_FILTER_KEY_PREFIX,
};

/// The filter key whose values are NAME PREFIXES: "If you use `Name` as a key, you can use
/// a name prefix to return a list of documents" (`DocumentKeyValuesFilter`).
pub const FILTER_KEY_NAME: &str = "Name";

/// The filter key that selects by who owns a document ([`DOCUMENT_OWNER_VALUES`]).
pub const FILTER_KEY_OWNER: &str = "Owner";

/// The filter key that selects by a platform a document's content is compatible with.
pub const FILTER_KEY_PLATFORM_TYPES: &str = "PlatformTypes";

/// The filter key that selects by a document's `DocumentType`.
pub const FILTER_KEY_DOCUMENT_TYPE: &str = "DocumentType";

/// The filter key that selects by a document's `TargetType`: "You can also use the
/// `TargetType` Amazon Web Services-provided key" (`DocumentKeyValuesFilter`).
pub const FILTER_KEY_TARGET_TYPE: &str = "TargetType";

/// The Amazon Web Services-provided keys a `Filters` entry may name, beside the
/// `tag:tagName` form: "Other valid keys include Owner, Name, PlatformTypes, DocumentType,
/// and TargetType" (`ListDocumentsRequest$Filters`). Held sorted, so the refusal that
/// lists them reads the same every time.
pub const DOCUMENT_KEY_VALUES_FILTER_KEYS: &[&str] = &[
    FILTER_KEY_DOCUMENT_TYPE,
    FILTER_KEY_NAME,
    FILTER_KEY_OWNER,
    FILTER_KEY_PLATFORM_TYPES,
    FILTER_KEY_TARGET_TYPE,
];

/// The `Owner` value naming the documents Systems Manager itself publishes, which is the
/// owner those documents report ([`PUBLISHED_DOCUMENT_OWNER`]).
pub const OWNER_AMAZON: &str = PUBLISHED_DOCUMENT_OWNER;

/// The `Owner` value naming the documents this account holds privately.
pub const OWNER_PRIVATE: &str = "Private";

/// The `Owner` value naming the documents shared with every account.
pub const OWNER_PUBLIC: &str = "Public";

/// The `Owner` value naming the documents this account created.
pub const OWNER_SELF: &str = "Self";

/// The `Owner` value naming the documents a third party published.
pub const OWNER_THIRD_PARTY: &str = "ThirdParty";

/// The owner classes an `Owner` filter may name, from the value list
/// `DocumentKeyValuesFilter` publishes for that key.
pub const DOCUMENT_OWNER_VALUES: &[&str] = &[
    OWNER_AMAZON,
    OWNER_PRIVATE,
    OWNER_PUBLIC,
    OWNER_SELF,
    OWNER_THIRD_PARTY,
];

/// How this service states a request naming more than one owner class: "Note that only one
/// `Owner` can be specified in a request" (`DocumentKeyValuesFilter`).
pub const ONE_OWNER_PER_REQUEST: &str = "Only one Owner can be specified in a request.";

/// How this service states a `Filters` key that names no attribute a document listing can
/// search on: the rejected key, then the fixed sentence Systems Manager words this class
/// with.
///
/// The key is the caller's own value, echoed back so a client can see WHICH of the keys it
/// sent was refused; nothing of the store is in the sentence.
pub fn unknown_document_filter_key_message(key: &str) -> String {
    format!("{key} is not a known filter key.")
}

/// Whether `value` names one of the owner classes an `Owner` filter may name.
pub fn is_document_owner(value: &str) -> bool {
    DOCUMENT_OWNER_VALUES.contains(&value)
}

/// What a request whose `Owner` filter names something that is not an owner class at all is
/// refused with: the rejected value and the classes `DocumentKeyValuesFilter` publishes for
/// that key, worded the way this service words a filter value it cannot search on
/// (`crate::ops::describe_parameters`).
pub fn invalid_owner_message(value: &str) -> String {
    format!(
        "Value '{value}' at filter key '{FILTER_KEY_OWNER}' isn't a document owner: valid \
         values are {}",
        DOCUMENT_OWNER_VALUES.join(", ")
    )
}

/// How this service states a request filtering on more than one tag key, the sentence
/// `ListDocumentsRequest$Filters` states the rule in.
pub const ONE_TAG_KEY_PER_REQUEST: &str =
    "This API operation only supports filtering documents by using a single tag key and one \
     or more tag values.";

/// The lower bound the modelled `MaxResults` of a document listing carries — the same
/// range 1..50 for both of this slice's listings (`ListDocumentsRequest$MaxResults` and
/// `ListDocumentVersionsRequest$MaxResults`), so one bound pair serves both.
pub const MIN_DOCUMENT_PAGE_SIZE: i32 = 1;

/// The upper bound of the same members, and the page size a request naming no
/// `MaxResults` is served up to (see [`super::page_size`]).
pub const MAX_DOCUMENT_PAGE_SIZE: i32 = 50;

/// The shortest and the longest a document name may be, from the modelled `DocumentARN`
/// pattern `^[a-zA-Z0-9_\-.:/]{3,128}$` — which is what a page's resume cursor is one of.
const MIN_DOCUMENT_NAME: usize = 3;

/// See [`MIN_DOCUMENT_NAME`].
const MAX_DOCUMENT_NAME: usize = 128;

/// The page size a listing of this slice serves — the documents of a Region or the
/// versions of one document, whose modelled `MaxResults` carry the same range (see
/// [`super::page_size`]).
pub fn document_page_size(max_results: Option<i32>) -> Result<usize, String> {
    super::page_size(max_results, MIN_DOCUMENT_PAGE_SIZE, MAX_DOCUMENT_PAGE_SIZE)
}

/// Whether `value` is spelled the way a document name is — the modelled `DocumentARN`
/// pattern `^[a-zA-Z0-9_\-.:/]{3,128}$`.
///
/// A page's resume cursor IS a document name, so a token spelled any other way was not
/// issued by this service, which is what `InvalidNextToken` reports.
pub fn is_document_name(value: &str) -> bool {
    (MIN_DOCUMENT_NAME..=MAX_DOCUMENT_NAME).contains(&value.len())
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':' | '/'))
}

/// Whether `key` names an attribute a document listing can filter on: one of the
/// Amazon Web Services-provided keys ([`DOCUMENT_KEY_VALUES_FILTER_KEYS`]) or a
/// `tag:tagName` key naming a tag.
///
/// The comparison is exact, as it is for every other filter of this service: a filter key
/// is a name, so another spelling or another case names no attribute of a document.
pub fn is_document_filter_key(key: &str) -> bool {
    if let Some(tag_key) = key.strip_prefix(TAG_FILTER_KEY_PREFIX) {
        return !tag_key.is_empty();
    }
    DOCUMENT_KEY_VALUES_FILTER_KEYS.contains(&key)
}

/// ONE filter of a document listing: the attribute it names and the values it searches
/// for.
///
/// The deprecated `DocumentFilterList` member and `Filters` are two spellings of that same
/// pair — the deprecated one carries exactly one value — so a request carrying both is
/// served the documents satisfying all of them.
#[derive(Debug, Clone)]
pub struct DocumentListingFilter {
    /// A key [`is_document_filter_key`] accepts.
    pub key: String,
    /// The values the filter searches for; a document matching ANY of them matches the
    /// filter.
    pub values: Vec<String>,
}

impl DocumentListingFilter {
    /// The filter naming `key` and searching for `values`.
    pub fn new(key: impl Into<String>, values: Vec<String>) -> Self {
        Self {
            key: key.into(),
            values,
        }
    }

    /// The tag key this filter names, or `None` when it filters on a property of a
    /// document rather than on a tag.
    pub fn tag_key(&self) -> Option<&str> {
        self.key.strip_prefix(TAG_FILTER_KEY_PREFIX)
    }

    /// Whether `version` of `record` carries one of the values this filter searches for.
    ///
    /// The attribute is read from the whole document (its name, owner, type and tags) or
    /// from the version being reported (its platform types and target type), which is the
    /// version the listing reports. A key naming no attribute matches nothing — the
    /// operation refuses such a key before the walk, so this is a backstop.
    pub fn matches(&self, record: &DocumentRecord, version: &DocumentVersionRecord) -> bool {
        if let Some(tag_key) = self.tag_key() {
            return record
                .tags
                .get(tag_key)
                .is_some_and(|held| self.values.iter().any(|wanted| wanted == held));
        }
        match self.key.as_str() {
            // "you can use a name prefix to return a list of documents".
            FILTER_KEY_NAME => self
                .values
                .iter()
                .any(|wanted| record.name.starts_with(wanted.as_str())),
            FILTER_KEY_OWNER => self
                .values
                .iter()
                .any(|wanted| owner_matches(record, wanted)),
            FILTER_KEY_PLATFORM_TYPES => self.values.iter().any(|wanted| {
                version
                    .parsed
                    .platform_types
                    .iter()
                    .any(|held| held == wanted)
            }),
            FILTER_KEY_DOCUMENT_TYPE => self
                .values
                .iter()
                .any(|wanted| &record.document_type == wanted),
            FILTER_KEY_TARGET_TYPE => version
                .target_type
                .as_deref()
                .is_some_and(|held| self.values.iter().any(|wanted| wanted == held)),
            _ => false,
        }
    }
}

/// Whether `record` belongs to the owner class `wanted` names.
///
/// `Self` and `Private` both name the documents this account holds: a document created
/// here is owned by this account. `Public` names the documents shared with every account,
/// which is the share `ModifyDocumentPermission` records
/// ([`super::document_permissions`]), so a document this account has shared publicly
/// belongs to that class too. Nothing here is published by a third party, so `ThirdParty`
/// names no document. A value outside the published classes ([`DOCUMENT_OWNER_VALUES`])
/// names no owner at all, which the listing refuses before the walk, so this arm is a
/// backstop.
fn owner_matches(record: &DocumentRecord, wanted: &str) -> bool {
    match wanted {
        OWNER_AMAZON => record.owner == PUBLISHED_DOCUMENT_OWNER,
        OWNER_SELF | OWNER_PRIVATE => record.owner == ::engine::emulator_account_id(),
        OWNER_PUBLIC => record.is_shared_publicly(),
        _ => false,
    }
}

/// The modelled `DocumentIdentifier` of `version` of `record`: the version's own members
/// beside the whole document's name, owner, type and tags.
///
/// What a listing reports of a document, derived from the same record
/// [`DocumentRecord::description_of`] reports a read from, so a document's identity in a
/// listing cannot disagree with its description.
pub fn document_identifier(
    record: &DocumentRecord,
    version: &DocumentVersionRecord,
) -> DocumentIdentifier {
    DocumentIdentifier {
        name: Some(record.name.clone()),
        created_date: Some(crate::state::wire_instant(version.created_date)),
        display_name: version.display_name.clone(),
        owner: Some(record.owner.clone()),
        version_name: version.version_name.clone(),
        platform_types: (!version.parsed.platform_types.is_empty())
            .then(|| version.parsed.platform_types.clone()),
        document_version: Some(version.version.to_string()),
        document_type: Some(record.document_type.clone()),
        schema_version: version.parsed.schema_version.clone(),
        document_format: Some(version.document_format.clone()),
        target_type: version.target_type.clone(),
        // "The tags, or metadata, that have been applied to the document"
        // (`DocumentIdentifier$Tags`, a `TagList` of length 0..1000): a document carrying
        // none has an EMPTY tag list rather than no list at all — the same way
        // [`DocumentRecord::description_of`] reports it — so a caller iterating `Tags`
        // never has to tell a missing member from an empty one, and the two reads of one
        // document cannot disagree.
        tags: Some(tagging::to_tag_list(&record.tags)),
        requires: version.requires.clone(),
        review_status: record.review_status(),
        // "The user in your organization who created the document": the emulator
        // authenticates no human author, so there is none to report.
        author: None,
        // "This field specifies the cloud providers that this version of the document can
        // run against": the version's own, which is what the listing reports of it.
        cloud_providers: Some(version.cloud_providers.clone()),
    }
}

/// One page of a document listing: the documents it reports, and the name the next page
/// resumes at when one document was left over.
#[derive(Debug, Clone, Default)]
pub struct DocumentPage {
    pub identifiers: Vec<DocumentIdentifier>,
    /// "The token to use when requesting the next set of items. If there are no additional
    /// items to return, the string is empty" — so `None` when this page is the last.
    pub next_token: Option<String>,
}

impl ComAmazonawsSsmAmazonssmState {
    /// Every document reachable through `region`'s endpoint, in ascending name order: the
    /// documents a caller created there and the ones Systems Manager publishes.
    ///
    /// A created document is borrowed from the store and a published one is projected from
    /// the catalogue, so a listing pays for no copy of what is already held. A name a
    /// create holds is served by the created document alone, the way
    /// [`ComAmazonawsSsmAmazonssmState::resolve_document`] resolves it.
    pub fn documents_of_region<'a>(&'a self, region: &str) -> Vec<Cow<'a, DocumentRecord>> {
        let created = self
            .documents
            .iter()
            .filter(|((held, _), _)| held == region)
            .map(|(_, record)| Cow::Borrowed(record));
        let published = PUBLISHED_DOCUMENTS.iter().filter_map(|published| {
            if self.document(region, published.name).is_some() {
                return None;
            }
            self.published_document_record(region, published.name)
                .map(Cow::Owned)
        });
        let mut documents: Vec<Cow<'a, DocumentRecord>> = created.chain(published).collect();
        documents.sort_by(|left, right| left.name.cmp(&right.name));
        documents
    }

    /// The page of `region`'s documents that `filters` admits, starting at the document
    /// `resume_from` names and holding at most `page_size` of them.
    ///
    /// Each document is reported once, by its DEFAULT version — "the default version", the
    /// one a read that names no version resolves to. A document whose default version is
    /// no longer held is reported at its latest, so a listing reports every document the
    /// Region holds.
    pub fn list_documents(
        &self,
        region: &str,
        filters: &[DocumentListingFilter],
        resume_from: Option<&str>,
        page_size: usize,
    ) -> DocumentPage {
        let mut identifiers = Vec::new();
        let mut next_token = None;
        for record in self.documents_of_region(region) {
            if resume_from.is_some_and(|from| record.name.as_str() < from) {
                continue;
            }
            let Some(version) = record
                .version(record.default_version)
                .or_else(|| record.latest())
            else {
                continue;
            };
            if !filters.iter().all(|filter| filter.matches(&record, version)) {
                continue;
            }
            if identifiers.len() == page_size {
                // The page is full and a document is still waiting: hand the caller the
                // name it resumes at, so the next request continues where this stopped.
                next_token = Some(record.name.clone());
                break;
            }
            identifiers.push(document_identifier(&record, version));
        }
        DocumentPage {
            identifiers,
            next_token,
        }
    }
}
