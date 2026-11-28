// SPDX-License-Identifier: CC0-1.0

//! Listing the versions of ONE SSM document: the modelled identity each version is
//! reported by, and the page a `ListDocumentVersions` read is served.
//!
//! The sibling listing in [`super::document_listing`] walks the documents of a Region; this
//! one walks the versions of one document. It walks them NEWEST VERSION FIRST — the version
//! a client reading a history asks about first is the one in force, which is the order
//! Systems Manager serves one in — so a page's resume cursor is still total: the cursor is
//! the VERSION NUMBER the next page starts at, not an offset into a history an update may
//! have grown between two pages. Each version is reported by [`document_version_info`],
//! derived from the same record a describe of that version reports, so a version's identity
//! in a listing cannot disagree with its description.

use crate::ops::types::DocumentVersionInfo;

use super::{DocumentRecord, DocumentVersionRecord};

/// The modelled `DocumentVersionInfo` of `version` of `record`: the version's own members
/// beside the whole document's name and review status, and whether it is the DEFAULT
/// version.
///
/// Derived from the same record [`DocumentRecord::description_of`] describes a version
/// from, so a version's identity in a listing cannot disagree with its description.
pub fn document_version_info(
    record: &DocumentRecord,
    version: &DocumentVersionRecord,
) -> DocumentVersionInfo {
    DocumentVersionInfo {
        name: Some(record.name.clone()),
        display_name: version.display_name.clone(),
        document_version: Some(version.version.to_string()),
        version_name: version.version_name.clone(),
        created_date: Some(crate::state::wire_instant(version.created_date)),
        // "An identifier for the default version of the document": the version a read that
        // names no selector resolves to ([`DocumentRecord::select`]) is the one this flags.
        is_default_version: version.version == record.default_version,
        document_format: Some(version.document_format.clone()),
        status: Some(version.status.clone()),
        // "A message returned by Amazon Web Services Systems Manager that explains the
        // `Status` value", which it words for a failure; nothing here failed.
        status_information: None,
        review_status: record.review_status(),
        // "This field specifies the cloud providers that this version of the document can
        // run against": the version's own, as the version it was minted by named them.
        cloud_providers: Some(version.cloud_providers.clone()),
    }
}

/// One page of a document's version listing: the versions it reports, and the version the
/// next page resumes at when one was left over.
#[derive(Debug, Clone, Default)]
pub struct DocumentVersionPage {
    pub versions: Vec<DocumentVersionInfo>,
    /// "The token to use when requesting the next set of items. If there are no additional
    /// items to return, the string is empty" — so `None` when this page is the last.
    pub next_token: Option<String>,
}

impl DocumentRecord {
    /// The page of this document's versions starting at the version `resume_from` numbers
    /// and holding at most `page_size` of them, NEWEST FIRST.
    ///
    /// The walk resumes at the newest version still held at or before `resume_from`, so a
    /// page keeps making forward progress across a version deletion that removed the one a
    /// token named.
    pub fn list_versions(
        &self,
        resume_from: Option<u64>,
        page_size: usize,
    ) -> DocumentVersionPage {
        let mut versions = Vec::new();
        let mut next_token = None;
        // The store holds the versions oldest first, in the order the create and its updates
        // minted them; a listing serves them the other way round.
        for version in self.versions.iter().rev() {
            if resume_from.is_some_and(|from| version.version > from) {
                continue;
            }
            if versions.len() == page_size {
                // The page is full and a version is still waiting: hand the caller the
                // version it resumes at, so the next request continues where this stopped.
                next_token = Some(version.version.to_string());
                break;
            }
            versions.push(document_version_info(self, version));
        }
        DocumentVersionPage {
            versions,
            next_token,
        }
    }
}
