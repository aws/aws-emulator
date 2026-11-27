// SPDX-License-Identifier: CC0-1.0

//! What a BATCH operation of this service shares: how one entry's case
//! identifier resolves to a case of the domain, what the per-case outcome of an
//! entry is reported as, and the answer a replay under the same client token
//! hands back.
//!
//! The two batch operations ("Each case is processed independently. If some
//! cases fail, the remaining cases are still updated",
//! `smithy_query(query="doc", shape="BatchUpdateCase")`) take the same `cases`
//! list of `CaseIdentifierUnion`s, answer the same `CaseIdentifier` per entry
//! and refuse the same request-level mistakes, so the resolution and the
//! vocabulary live here rather than once per operation.

use super::*;

use crate::ops::types::{CaseIdentifier, CaseIdentifierUnion};

/// The per-entry error code a batch reports for an identifier that names no
/// case of the domain: "CASE_NOT_FOUND - No case exists with the specified case
/// ID" (`smithy_query(query="operation", operation="BatchUpdateCase")`, member
/// `failedCases`). A per-entry code is the service's own vocabulary and is not
/// the name of a modelled exception shape — the model targets `errorCode` at
/// `String` and enumerates nothing.
pub const BATCH_CASE_NOT_FOUND_CODE: &str = "CASE_NOT_FOUND";

/// The per-entry error MESSAGE a batch reports for an identifier that names no
/// case of the domain. Fixed text with no variable part: the entry already
/// carries the identifier that failed in its own required `caseIdentifier`
/// member, which is where a client reads it from.
pub const BATCH_CASE_NOT_FOUND_MESSAGE: &str = "No case exists with the specified case ID.";

/// The per-entry error code a batch reports for an entry refused because
/// another operation holds the case: "CONFLICT - Another operation is in
/// progress on the case, such as a concurrent update or a merge of the case
/// into another case" (`BatchUpdateCase`, member `failedCases`).
pub const BATCH_CONFLICT_CODE: &str = "CONFLICT";

/// The per-entry error code a batch reports for an entry refused because it
/// would exceed a quota: "SERVICE_QUOTA_EXCEEDED - The request exceeded a
/// service quota" (`BatchCreateRelatedItem`, member `failedItems`).
pub const BATCH_SERVICE_QUOTA_EXCEEDED_CODE: &str = "SERVICE_QUOTA_EXCEEDED";

/// Most entries one batch request's `cases` list may carry: the model's
/// `..100` cap on `BatchUpdateCaseRequest$cases` and
/// `BatchCreateRelatedItemRequest$cases`
/// (`smithy_query(query="operation", operation="BatchUpdateCase,BatchCreateRelatedItem")`),
/// re-checked in the handler for the same reason as [`MAX_CASE_FIELD_IDS`].
pub const MAX_BATCH_CASES: usize = 100;

/// One entry of a batch request's `cases` list, resolved against the domain.
#[derive(Debug, Clone)]
pub struct BatchCase {
    /// The case this entry addresses, as a batch answer reports it.
    pub identifier: CaseIdentifier,
    /// The case the entry resolves to, or `None` when the domain holds no case
    /// it names — an entry that fails on its own ([`BATCH_CASE_NOT_FOUND_CODE`])
    /// while the other entries are still processed.
    pub case_id: Option<String>,
}

/// One entry's outcome, as a batch answer reports it and as a replay under the
/// same client token hands it back.
#[derive(Debug, Clone)]
pub struct BatchEntryOutcome {
    pub identifier: CaseIdentifier,
    /// The related item the entry associated, as `(relatedItemId,
    /// relatedItemArn)`; `None` for an entry of an operation that associates
    /// none, and for one that failed.
    pub related_item: Option<(String, String)>,
    /// The `(errorCode, errorMessage)` the entry failed with; `None` for one
    /// that succeeded.
    pub error: Option<(String, String)>,
}

impl BatchEntryOutcome {
    /// The outcome of an entry that succeeded, associating no related item.
    pub fn done(identifier: CaseIdentifier) -> Self {
        Self {
            identifier,
            related_item: None,
            error: None,
        }
    }

    /// The outcome of an entry that associated a related item.
    pub fn associated(identifier: CaseIdentifier, related_item_id: &str, arn: &str) -> Self {
        Self {
            identifier,
            related_item: Some((related_item_id.to_string(), arn.to_string())),
            error: None,
        }
    }

    /// The outcome of an entry that failed on its own, under the per-entry code
    /// `code` and the sentence `message`.
    pub fn failed(identifier: CaseIdentifier, code: &str, message: String) -> Self {
        Self {
            identifier,
            related_item: None,
            error: Some((code.to_string(), message)),
        }
    }

    /// Whether this entry failed, which is which of a batch answer's two lists
    /// it belongs in.
    pub fn is_failure(&self) -> bool {
        self.error.is_some()
    }
}

/// A remembered answer of ONE batch call: the request it answered and the
/// per-entry outcomes it reported.
///
/// "If you retry a request with the same client token, the service returns the
/// original response instead of processing the request again. If you reuse a
/// client token with different request parameters, the request fails with a
/// `ConflictException`"
/// (`smithy_query(query="operation", operation="BatchUpdateCase")`, member
/// `clientToken`), so the record holds both halves: the digest tells a retry
/// from a reuse, and the outcomes are what a retry is answered with.
#[derive(Debug, Clone)]
pub struct BatchReplay {
    /// A digest of the request's own parameters, the client token aside — not
    /// the parameters themselves, so nothing a request carried is readable back
    /// out of the store.
    pub request_digest: String,
    pub entries: Vec<BatchEntryOutcome>,
}

/// A remembered batch answer's key: the operation, the domain it was made in
/// and the client token. The operation is part of the key because the same
/// token is a different request for a different operation, and the domain for
/// the reason it is part of [`IdempotencyKey`].
pub type BatchReplayKey = (String, String, String);

impl ComAmazonawsConnectcasesAmazonconnectcasesState {
    /// The answer `client_token` already got from `operation` in `domain_id`,
    /// if the store still remembers it.
    pub fn batch_replay(
        &self,
        operation: &str,
        domain_id: &str,
        client_token: &str,
    ) -> Option<&BatchReplay> {
        self.batch_replays.get(&(
            operation.to_string(),
            domain_id.to_string(),
            client_token.to_string(),
        ))
    }

    /// Remember the answer `client_token` got from `operation` in `domain_id`,
    /// evicting the oldest remembered answer once [`MAX_IDEMPOTENCY_TOKENS`] is
    /// reached — the same window a remembered `CreateCase` token keeps.
    pub fn remember_batch_replay(
        &mut self,
        operation: &str,
        domain_id: &str,
        client_token: &str,
        replay: BatchReplay,
    ) {
        let key = (
            operation.to_string(),
            domain_id.to_string(),
            client_token.to_string(),
        );
        if self.batch_replays.insert(key.clone(), replay).is_none() {
            self.batch_replay_order.push_back(key);
        }
        while self.batch_replay_order.len() > MAX_IDEMPOTENCY_TOKENS {
            if let Some(evicted) = self.batch_replay_order.pop_front() {
                self.batch_replays.remove(&evicted);
            }
        }
    }
}

impl Domain {
    /// The live field this domain holds under the name `name`, or `None` when
    /// it holds none.
    ///
    /// A field's NAME is distinct from its id, and identifying a field by name
    /// "is supported only in BatchUpdateCase"
    /// (`smithy_query(query="operation", operation="BatchUpdateCase")`, member
    /// `fields`), which is what resolves one through here. A deleted field is
    /// not part of the domain's live schema, so it resolves no name.
    pub fn field_named(&self, name: &str) -> Option<&Field> {
        self.fields
            .values()
            .find(|field| !field.deleted && field.name == name)
    }

    /// The case this domain holds whose `reference_id` is `reference_id`, or
    /// `None` when it holds none.
    pub fn case_with_reference_id(&self, reference_id: &str) -> Option<&Case> {
        self.cases
            .values()
            .find(|case| case.text_field(CASE_REFERENCE_ID_FIELD_ID) == Some(reference_id))
    }

    /// Every entry of a batch request's `cases` list, resolved against this
    /// domain in the order the request gave them, or the modelled 400 the WHOLE
    /// request is refused with.
    ///
    /// Two request-level mistakes fail the whole call rather than one entry:
    /// "A reference ID doesn't match any case in the domain" and "The request
    /// contains duplicate case IDs"
    /// (`smithy_query(query="doc", shape="BatchUpdateCase")`). An entry that
    /// names a case ID the domain does not hold is NOT one of them: it fails on
    /// its own, since each case "is processed independently".
    pub fn resolve_batch_cases(
        &self,
        entries: &[&CaseIdentifierUnion],
    ) -> Result<Vec<BatchCase>, ValidationException> {
        let mut resolved: Vec<BatchCase> = Vec::with_capacity(entries.len());
        for entry in entries {
            resolved.push(match entry {
                CaseIdentifierUnion::caseId(case_id) => match self.cases.get(case_id.as_str()) {
                    Some(case) => BatchCase {
                        identifier: case.identifier(),
                        case_id: Some(case.case_id.clone()),
                    },
                    None => BatchCase {
                        identifier: CaseIdentifier {
                            case_id: Some(case_id.clone()),
                            reference_number: None,
                            reference_id: None,
                        },
                        case_id: None,
                    },
                },
                CaseIdentifierUnion::referenceId(reference_id) => {
                    let case = self
                        .case_with_reference_id(reference_id)
                        .ok_or_else(|| unresolvable_reference_id(reference_id))?;
                    BatchCase {
                        identifier: case.identifier(),
                        case_id: Some(case.case_id.clone()),
                    }
                }
            });
        }
        let addressed: Vec<&str> = resolved
            .iter()
            .filter_map(|entry| {
                entry
                    .case_id
                    .as_deref()
                    .or(entry.identifier.case_id.as_deref())
            })
            .collect();
        let duplicates = duplicate_ids(addressed);
        if !duplicates.is_empty() {
            return Err(duplicate_batch_cases(&duplicates));
        }
        Ok(resolved)
    }
}

/// The modelled 400 the whole batch is refused with for a `referenceId` that
/// names no case of the domain.
pub fn unresolvable_reference_id(reference_id: &str) -> ValidationException {
    ValidationException {
        message: format!("No case in this domain has the reference id '{reference_id}'."),
    }
}

/// The modelled 400 the whole batch is refused with for a `cases` list that
/// addresses one case more than once: two entries for one case say two things
/// about it at once, which is ambiguous rather than resolvable by an arbitrary
/// choice — exactly as two values for one field id are on the write paths
/// ([`duplicate_ids`]).
pub fn duplicate_batch_cases(duplicates: &[String]) -> ValidationException {
    ValidationException {
        message: format!(
            "cases addresses more than one entry to the cases: {}",
            name_ids(duplicates)
        ),
    }
}

/// The modelled 409 a retry under a client token that already answered a
/// DIFFERENT request is refused with.
///
/// The token itself is not quoted: a client that reused one knows it, and the
/// refusal is about the parameters rather than about the token's text.
pub fn client_token_reused(operation: &str) -> ConflictException {
    ConflictException {
        message: format!(
            "The clientToken of this request was already used for a {operation} request with \
             different parameters."
        ),
    }
}

/// The modelled 400 for a batch update that asks for no update at all: "The
/// request doesn't specify any update. Provide at least one entry in `fields`,
/// or a `title`, `status`, or `customerId`"
/// (`smithy_query(query="doc", shape="BatchUpdateCase")`).
pub fn no_update_requested() -> ValidationException {
    ValidationException {
        message: "The request specifies no update; provide at least one entry in fields, or a \
                  title, a status or a customerId."
            .to_string(),
    }
}

/// The modelled 400 for a batch update whose `fields` names one of the case
/// values the request sets through a top-level parameter instead: "Do not
/// include `title`, `status`, or `customerId` in this list; set those values
/// only through their top-level parameters. Duplicating them in `fields`
/// results in a validation error"
/// (`smithy_query(query="operation", operation="BatchUpdateCase")`, member
/// `fields`).
pub fn field_set_only_at_the_top_level(field_id: &str) -> ValidationException {
    ValidationException {
        message: format!(
            "fields must not carry the field '{field_id}'; set it through the request's own \
             parameter for it instead."
        ),
    }
}

/// The characters a field NAME a request addresses a field by may not carry: a
/// pipe and a comma, which are the two the service reserves — "A field name
/// contains a pipe (|) or comma (,) character" is one of the mistakes that fail
/// a whole batch update (`smithy_query(query="doc", shape="BatchUpdateCase")`).
pub const CHARACTERS_RESERVED_IN_A_FIELD_NAME: &[char] = &['|', ','];

/// The modelled 400 for a field name a request may not address a field by, or
/// `None` when the name carries none of
/// [`CHARACTERS_RESERVED_IN_A_FIELD_NAME`].
pub fn field_name_not_addressable(name: &str) -> Option<ValidationException> {
    let reserved: String = CHARACTERS_RESERVED_IN_A_FIELD_NAME
        .iter()
        .filter(|character| name.contains(**character))
        .map(|character| character.to_string())
        .collect::<Vec<String>>()
        .join(" ");
    (!reserved.is_empty()).then(|| ValidationException {
        message: format!(
            "A field cannot be addressed by a name carrying {reserved}; address it by its id \
             instead."
        ),
    })
}

/// The modelled 400 for a field NAME the domain holds no live field under.
///
/// The sentence names the name rather than an id, because a name is what the
/// request carried: an id it could not resolve is reported by
/// [`unknown_fields`], which is what every other reference refusal of this
/// service renders.
pub fn unknown_field_name(name: &str) -> ValidationException {
    ValidationException {
        message: format!("No field in this domain has the name '{name}'."),
    }
}

/// The modelled 400 for a batch association whose content is not a comment:
/// "This operation supports only the Comment related item type" and "the
/// content isn't comment content" fails the whole request
/// (`smithy_query(query="doc", shape="BatchCreateRelatedItem")`).
pub fn content_is_not_a_comment(item_type: &str) -> ValidationException {
    ValidationException {
        message: format!(
            "content carries {item_type} content; only Comment content can be associated with \
             several cases in one request."
        ),
    }
}

/// A digest of one batch request's parameters, the client token aside: what
/// tells a RETRY of a remembered request from a REUSE of its token.
///
/// The parameters are hashed rather than stored, so nothing a request carried
/// can be read back out of the store, and the digest is a pure function of
/// them, so a retry of the same request renders the same digest whatever the
/// store holds. SHA-256, as the ids are minted and a page token's scope is
/// stamped; this crate implements no hash of its own.
pub fn request_digest(rendered: &str) -> String {
    scope_digest(rendered)
}
