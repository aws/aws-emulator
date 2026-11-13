// SPDX-License-Identifier: CC0-1.0

//! This surface's view of the ACCOUNT's email templates, and the token its template
//! listing pages with.
//!
//! The email-template table beside this module
//! ([`super::ComAmazonawsSesv2SimpleemailserviceV2State::email_templates`]) is this API's
//! VIEW of one account-wide set of email templates, which lives in the shared engine as
//! [`::service_common::ses_templates::SesEmailTemplateDirectory`] and which the classic SES
//! surface records into and reads as well: a template belongs to the ACCOUNT rather than to
//! the API that created it. So an operation of this surface answers for every template the
//! account holds — including the ones recorded as `CreateTemplate` — and for none it has
//! deleted. What a record here adds to that set is what THIS API adds to a template: its
//! tags.
//!
//! Because the two surfaces page ONE list, the continuation token of a template listing is
//! the registry's ([`template_page_token`]), authenticated under the key the registry holds,
//! so a token either surface's template listing issued continues that list on either one.

use ::service_common::ses_templates::{
    AccountEmailTemplateRecord, SesEmailTemplateDirectory, SesEmailTemplateRecord,
};

use super::EmailTemplateRecord;

/// The key the account's template-listing tokens are authenticated under: see
/// [`template_page_token`].
pub use ::service_common::page_tokens::PageTokenKey;

/// The `NextToken` the template listing hands back to resume at the template the NEXT page
/// starts at, and the reading of one it is handed.
///
/// The registry's, not this surface's: the account's two template APIs page one list, so one
/// token form serves both ([`::service_common::ses_templates::page_token`]).
pub use ::service_common::ses_templates::{
    page_cursor as template_page_cursor, page_token as template_page_token,
};

/// The account's email-template registry in the shared engine, which this service and the
/// classic SES surface both record into and read: call it with a handler's `&*self.engine`.
///
/// An operation that CREATES, REPLACES or DELETES a template publishes the change here, so
/// the other surface answers for it from the moment the call returns. Take this lock while
/// holding this service's own store, never the other way round, so two callers on different
/// surfaces cannot wait on each other.
pub fn template_registry<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<SesEmailTemplateDirectory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<SesEmailTemplateDirectory>)
}

/// The key this account's template-listing tokens are authenticated under, or `None` when
/// the engine holds no registry at all.
///
/// Taken by value — [`PageTokenKey`] is `Copy` — so a listing reads the key and then goes on
/// to read the templates it is about to report without holding the registry's lock.
pub fn template_page_token_key<Eng: Send + Sync + 'static>(engine: &Eng) -> Option<PageTokenKey> {
    Some(template_registry(engine)?.lock().page_token_key())
}

/// Bring this service's email-template table and the account's email-template registry back
/// into agreement, so every API over the account's templates answers for ONE set of them.
///
/// The reconciliation itself is the registry's ([`SesEmailTemplateDirectory::reconcile`]),
/// written once there rather than per surface; what this service contributes is its
/// [`AccountEmailTemplateRecord`] impl below.
pub(super) fn reconcile_templates(
    state: &::engine::ServiceState<super::ComAmazonawsSesv2SimpleemailserviceV2State>,
    registry: &::engine::ServiceState<SesEmailTemplateDirectory>,
) {
    // The lock order of this service: its own store, then the registry.
    let mut state = state.lock();
    let mut registry = registry.lock();
    registry.reconcile(&mut state.email_templates);
}

/// Publish the template `name` as this service now holds it — the subject line, the HTML
/// part and the text part of its content — to the account's email-template registry, so
/// every other API over the account's templates reports what this one last wrote.
///
/// What an operation that CREATES or REPLACES a template calls once it has written it.
/// Changes nothing when the account holds no such template, and writes nothing but the
/// registry's record of it. Reads this service's store WITHOUT reconciling it
/// ([`super::own_state`]): the change being published is newer than the registry's copy of
/// it.
pub fn publish_template<Eng: Send + Sync + 'static>(engine: &Eng, name: &str) {
    let Some(state) = super::own_state(engine) else {
        return;
    };
    let Some(registry) = template_registry(engine) else {
        return;
    };
    // The lock order of this service: its own store, then the registry.
    let mut state = state.lock();
    let mut registry = registry.lock();
    let Some(held) = state.email_templates.get(name) else {
        return;
    };
    if registry.publish(held.published()).is_some() {
        if let Some(held) = state.email_templates.get_mut(name) {
            held.registered = true;
        }
    }
}

/// Remove the template `name` names from this account — from this service's own table and
/// from the account registry both — so neither SES surface reports it afterwards.
///
/// Answers whether the account held it, which is what lets the delete operation refuse a
/// name it holds nothing under instead of reporting a deletion it did not perform.
pub fn forget_template<Eng: Send + Sync + 'static>(engine: &Eng, name: &str) -> bool {
    let Some(state) = super::own_state(engine) else {
        return false;
    };
    let Some(registry) = template_registry(engine) else {
        return false;
    };
    let mut state = state.lock();
    let mut registry = registry.lock();
    let held = state.email_templates.remove(name).is_some();
    registry.forget(name);
    held
}

impl AccountEmailTemplateRecord for EmailTemplateRecord {
    fn template_name(&self) -> &str {
        &self.name
    }

    fn is_registered(&self) -> bool {
        self.registered
    }

    fn mark_registered(&mut self) {
        self.registered = true;
    }

    /// The account's content for the template, onto the members this surface's own model
    /// spells them with, and the instant the account has held the template since — which
    /// belongs to the create that recorded it, whichever API that was. The tags this API
    /// adds beside the content are untouched.
    fn take_shared(&mut self, published: &SesEmailTemplateRecord) {
        if self.content.subject != published.subject {
            self.content.subject = published.subject.clone();
        }
        if self.content.text != published.text {
            self.content.text = published.text.clone();
        }
        if self.content.html != published.html {
            self.content.html = published.html.clone();
        }
        self.created_at = published.created_at;
    }

    fn published(&self) -> SesEmailTemplateRecord {
        SesEmailTemplateRecord {
            name: self.name.clone(),
            subject: self.content.subject.clone(),
            text: self.content.text.clone(),
            html: self.content.html.clone(),
            created_at: self.created_at,
        }
    }

    /// A template the account holds through the other surface carries no tags of this API:
    /// tagging a template is a call of this surface's own, and none has been made.
    fn adopt(record: &SesEmailTemplateRecord) -> Self {
        let mut adopted = Self::new(
            &record.name,
            crate::ops::types::EmailTemplateContent {
                subject: record.subject.clone(),
                text: record.text.clone(),
                html: record.html.clone(),
            },
            record.created_at,
        );
        adopted.registered = true;
        adopted
    }
}
