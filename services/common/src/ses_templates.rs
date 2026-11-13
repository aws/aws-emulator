// SPDX-License-Identifier: CC0-1.0

//! The ACCOUNT'S EMAIL TEMPLATES: the set of email templates one account holds, and the
//! content each of them carries.
//!
//! Amazon SES publishes TWO APIs over ONE set of account email templates: the classic
//! `awsQuery` surface (`CreateTemplate`, `GetTemplate`, `UpdateTemplate`, `ListTemplates`,
//! `DeleteTemplate`, and the templated sends that render one) and the v2 REST surface
//! (`CreateEmailTemplate`, `GetEmailTemplate`, `UpdateEmailTemplate`,
//! `ListEmailTemplates`, `DeleteEmailTemplate`, `TestRenderEmailTemplate` and its sends). A
//! template recorded through either one is the SAME account resource: the other surface
//! reads it, lists it, refuses a second create under its name, renders a send that names
//! it, and loses it when it is deleted.
//!
//! So the set lives HERE, once in the shared engine, rather than in each surface's own
//! store — exactly as the account's identities ([`super::ses_identities`]) and
//! configuration sets ([`super::ses_configuration_sets`]) do: a surface PUBLISHES the
//! templates it records and ADOPTS the ones it does not hold yet, and keeps beside that
//! only what its own API adds (the v2 surface's tags).
//!
//! Nothing here names a wire shape. The registry holds the template as its creator spelled
//! it, the three content parts both models carry — the subject line, the text part and the
//! HTML part — and the instant the template was recorded, which both surfaces report as
//! the creation timestamp of a listed template. Each surface spells those onto the shapes
//! ITS model declares (the classic `Template$SubjectPart` for the v2
//! `EmailTemplateContent$Subject`).
//!
//! Because the two surfaces page ONE list, the continuation token of a template listing is
//! minted HERE as well ([`page_token`]): it is authenticated under the registry's own key
//! ([`SesEmailTemplateDirectory::page_token_key`]), so a token either surface's template
//! listing issued names a position in that one list and continues it on either surface,
//! while a value no listing issued is refused by both.
//!
//! The registry is BOUNDED at [`MAX_REGISTERED_EMAIL_TEMPLATES`], the email-template quota
//! SES documents per account, so a caller looping on a create cannot grow it without end.

use std::collections::BTreeMap;
use std::time::SystemTime;

use super::page_tokens::PageTokenKey;

/// The email templates one account may hold, and so the ceiling on this registry: SES
/// documents a limit of 10,000 email templates per account, and a publication past it is
/// refused rather than letting caller input grow the registry without end.
pub const MAX_REGISTERED_EMAIL_TEMPLATES: usize = 10_000;

/// One email template the account holds, as the registry records it: the facts both SES
/// APIs publish about a template.
///
/// A publisher states ALL of the content parts, because both models carry all three: an
/// absent `Option` is a part the TEMPLATE does not have rather than one the publishing API
/// cannot express, so a part another surface cleared is cleared here too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SesEmailTemplateRecord {
    /// The template's name as the request that recorded it spelled it, which is the value
    /// both surfaces report in a listing and address the template by.
    pub name: String,
    /// The subject line (`Template$SubjectPart` / `EmailTemplateContent$Subject`).
    pub subject: Option<String>,
    /// The text-only body (`Template$TextPart` / `EmailTemplateContent$Text`).
    pub text: Option<String>,
    /// The HTML body (`Template$HtmlPart` / `EmailTemplateContent$Html`).
    pub html: Option<String>,
    /// When the template was first recorded, which both surfaces report as the creation
    /// timestamp of a listed template.
    pub created_at: SystemTime,
}

/// The registry itself: template name -> the template the account holds under it.
///
/// Registered in the shared engine under THIS type, so the two SES surfaces reach one set
/// of email templates instead of each holding its own.
///
/// Keyed by the template's NAME, compared exactly: neither surface normalises one case
/// into the other, and each reports a name it holds nothing under as absent. Ordered by it,
/// so the listing either surface pages is in one stable order for every caller — which is
/// what makes a continuation token resumable across both.
#[derive(Debug, Default)]
pub struct SesEmailTemplateDirectory {
    /// The records, by template name.
    templates: BTreeMap<String, SesEmailTemplateRecord>,
    /// The secret this account's template-listing tokens are authenticated under, drawn
    /// from the system's randomness when the registry is created and never revealed. One
    /// key for the account's one template list, which is what lets either surface verify a
    /// token the other minted.
    page_token_key: PageTokenKey,
}

impl SesEmailTemplateDirectory {
    /// Publish `record` as an email template of this account, and hand back the stored
    /// record.
    ///
    /// The publisher states the whole content ([`SesEmailTemplateRecord`]), so a template
    /// already registered takes it entire — that is what carries a part another surface
    /// CLEARED, which no "leave what you have no value for" rule could express — and keeps
    /// the instant it was first recorded, because the creation instant belongs to the
    /// create and an update leaves it alone.
    ///
    /// `None` when the registry already holds [`MAX_REGISTERED_EMAIL_TEMPLATES`] templates
    /// and this is a new one: the caller refuses the request rather than growing the
    /// registry past the account's quota.
    pub fn publish(
        &mut self,
        record: SesEmailTemplateRecord,
    ) -> Option<&SesEmailTemplateRecord> {
        let name = record.name.clone();
        let created_at = match self.templates.get(&name) {
            Some(held) => held.created_at,
            None if self.templates.len() >= MAX_REGISTERED_EMAIL_TEMPLATES => return None,
            None => record.created_at,
        };
        self.templates.insert(
            name.clone(),
            SesEmailTemplateRecord {
                created_at,
                ..record
            },
        );
        self.templates.get(&name)
    }

    /// Drop the template `name` names: it was deleted, so the account no longer holds it
    /// and neither surface reports it.
    ///
    /// Deleting a template the registry does not hold changes nothing, which is what makes
    /// a delete served on one surface and then replayed on the other harmless.
    pub fn forget(&mut self, name: &str) {
        self.templates.remove(name);
    }

    /// The template `name` names, or `None` when the account holds no such template.
    pub fn get(&self, name: &str) -> Option<&SesEmailTemplateRecord> {
        self.templates.get(name)
    }

    /// Every email template the account holds, ordered by name.
    pub fn records(&self) -> impl Iterator<Item = &SesEmailTemplateRecord> {
        self.templates.values()
    }

    /// How many email templates the account holds.
    pub fn len(&self) -> usize {
        self.templates.len()
    }

    /// Whether the account holds no email template at all.
    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }

    /// The key this account's template-listing tokens are authenticated under.
    ///
    /// Handed out by value — [`PageTokenKey`] is `Copy` — so a listing can take the key off
    /// the registry and go on to read the records it is about to report.
    pub fn page_token_key(&self) -> PageTokenKey {
        self.page_token_key
    }

    /// Bring one surface's own template `table` and this registry back into agreement, so
    /// every API over the account's email templates answers for ONE set of them.
    ///
    /// The whole reconciliation, written ONCE here rather than per surface, so no two of
    /// them can resolve a hand-off differently. Four steps, in this order:
    ///
    /// 1. a template the table holds that the registry does not know about yet is
    ///    PUBLISHED — which is how a record written straight into the table (a test
    ///    fixture) reaches the other surface;
    /// 2. a template the table holds that the registry no longer holds was DELETED through
    ///    another surface, so it goes from the table too;
    /// 3. a template the registry holds that the table does not is ADOPTED
    ///    ([`AccountEmailTemplateRecord::adopt`]);
    /// 4. a template BOTH sides hold takes the content the account holds for it
    ///    ([`AccountEmailTemplateRecord::take_shared`]), which is how content written
    ///    through one API becomes visible through the other instead of only the template
    ///    itself.
    ///
    /// Takes no instant: the creation instant of a template belongs to the create that
    /// recorded it and travels in the record itself, so nothing here is decided by the
    /// clock.
    ///
    /// The work is proportional to the account's template count, which
    /// [`MAX_REGISTERED_EMAIL_TEMPLATES`] bounds, and allocates only when there is a
    /// publication, an adoption or content the two sides disagree on.
    pub fn reconcile<R: AccountEmailTemplateRecord>(&mut self, table: &mut BTreeMap<String, R>) {
        if table.values().any(|held| !held.is_registered()) {
            let unpublished: Vec<(String, SesEmailTemplateRecord)> = table
                .values()
                .filter(|held| !held.is_registered())
                .map(|held| (held.template_name().to_string(), held.published()))
                .collect();
            for (key, record) in unpublished {
                // A publication the registry refuses (the account is at its quota) leaves
                // the record unpublished rather than marking it registered: it is this
                // surface's own template, not one another surface deleted.
                if self.publish(record).is_none() {
                    continue;
                }
                if let Some(held) = table.get_mut(&key) {
                    held.mark_registered();
                }
            }
        }

        table.retain(|_, held| !held.is_registered() || self.get(held.template_name()).is_some());

        if self.records().any(|record| !table.contains_key(&record.name)) {
            let adopted: Vec<(String, R)> = self
                .records()
                .filter(|record| !table.contains_key(&record.name))
                .map(|record| (record.name.clone(), R::adopt(record)))
                .collect();
            for (key, record) in adopted {
                table.insert(key, record);
            }
        }

        // What the ACCOUNT holds for a template both sides already hold is the account's,
        // not the holding surface's: a record kept from an earlier call takes it, so
        // content another API wrote after this surface recorded or adopted the template is
        // reported here too instead of only on the API that wrote it.
        for held in table.values_mut() {
            if let Some(published) = self.templates.get(held.template_name()) {
                held.take_shared(published);
            }
        }
    }
}

/// One surface's OWN record of an account email template, as
/// [`SesEmailTemplateDirectory::reconcile`] reaches it.
///
/// Each API over the account's templates keeps, beside the registry, what IT adds to a
/// template — the v2 surface's tags. This trait is the little each of those records has to
/// answer for the reconciliation to be written once: which template it is for, whether the
/// registry carries it yet, how the registry carries it, and how to make one from a
/// registry record.
///
/// A surface's table is keyed by the template's name, which is how both of them look a
/// template up, so there is no per-surface keying to state.
pub trait AccountEmailTemplateRecord: Sized {
    /// The template this record is for, as the request that recorded it spelled it.
    fn template_name(&self) -> &str;

    /// Whether the registry already carries this template.
    ///
    /// This is what tells a template DELETED through another surface — registered here,
    /// gone from the registry — from one this surface recorded but has not published yet.
    fn is_registered(&self) -> bool;

    /// Note that the registry now carries this template.
    fn mark_registered(&mut self);

    /// Take onto this record the whole content the ACCOUNT holds for the template, and the
    /// instant it was recorded, as `published` holds them.
    ///
    /// Called for every template this surface and the registry BOTH hold, so content
    /// another API wrote after this surface recorded or adopted the template is reported
    /// here as well. What this surface's own API adds beside the content is untouched.
    ///
    /// Idempotent, and it allocates only when the two sides disagree.
    fn take_shared(&mut self, published: &SesEmailTemplateRecord);

    /// How the registry carries this record: the template's name, the whole content this
    /// surface now holds for it, and the instant it was recorded.
    fn published(&self) -> SesEmailTemplateRecord;

    /// This surface's record of a template the registry holds and it does not.
    ///
    /// The account has held the template since `record.created_at`, so the adopted record
    /// carries THAT instant rather than the instant it happened to be adopted; everything
    /// this surface's own API adds starts where its create starts it, since none of it has
    /// been asked for.
    fn adopt(record: &SesEmailTemplateRecord) -> Self;
}

/// The `NextToken` a template listing hands back to resume at `cursor` — the template the
/// NEXT page starts at.
///
/// Opaque: the caller reads no position out of it and hands it back verbatim. Authenticated
/// under the registry's key, so a value the caller assembled names no position at all
/// ([`page_cursor`]), and rendered as base64, which travels in both surfaces' bodies.
///
/// ONE form for the account's one template list, which is what lets a token minted by
/// either surface's listing continue that list on either surface.
pub fn page_token(key: &PageTokenKey, cursor: &str) -> String {
    ::aws_smithy_types::base64::encode(key.mint(cursor))
}

/// The template a token of the account's template listing resumes AT, or `None` for any
/// value no listing of it minted: one that is not base64, one whose authenticator does not
/// match the cursor it travels with, and one minted under another registry's key.
///
/// The caller refuses such a token rather than restarting the listing at some other
/// position.
pub fn page_cursor(key: &PageTokenKey, token: &str) -> Option<String> {
    let bytes = ::aws_smithy_types::base64::decode(token).ok()?;
    key.cursor_of(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A stand-in for one surface's own record: the template's name, the registry flag, the
    /// content it holds, the instant it was recorded, and one member of its "own" that a
    /// reconciliation must leave alone.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Surface {
        name: String,
        registered: bool,
        subject: Option<String>,
        text: Option<String>,
        html: Option<String>,
        created_at: SystemTime,
        own: Option<String>,
    }

    impl Surface {
        fn new(name: &str, created_at: SystemTime) -> Self {
            Self {
                name: name.to_string(),
                registered: false,
                subject: Some(format!("Subject of {name}")),
                text: None,
                html: None,
                created_at,
                own: None,
            }
        }
    }

    impl AccountEmailTemplateRecord for Surface {
        fn template_name(&self) -> &str {
            &self.name
        }

        fn is_registered(&self) -> bool {
            self.registered
        }

        fn mark_registered(&mut self) {
            self.registered = true;
        }

        fn take_shared(&mut self, published: &SesEmailTemplateRecord) {
            self.subject = published.subject.clone();
            self.text = published.text.clone();
            self.html = published.html.clone();
            self.created_at = published.created_at;
        }

        fn published(&self) -> SesEmailTemplateRecord {
            SesEmailTemplateRecord {
                name: self.name.clone(),
                subject: self.subject.clone(),
                text: self.text.clone(),
                html: self.html.clone(),
                created_at: self.created_at,
            }
        }

        fn adopt(record: &SesEmailTemplateRecord) -> Self {
            Self {
                name: record.name.clone(),
                registered: true,
                subject: record.subject.clone(),
                text: record.text.clone(),
                html: record.html.clone(),
                created_at: record.created_at,
                own: None,
            }
        }
    }

    fn record(name: &str, created_at: SystemTime) -> SesEmailTemplateRecord {
        SesEmailTemplateRecord {
            name: name.to_string(),
            subject: Some("Your receipt".to_string()),
            text: None,
            html: None,
            created_at,
        }
    }

    fn table(records: &[Surface]) -> BTreeMap<String, Surface> {
        records
            .iter()
            .map(|held| (held.name.clone(), held.clone()))
            .collect()
    }

    #[test]
    fn a_template_one_surface_recorded_is_published_and_adopted_by_the_other() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_767_225_600);
        let mut directory = SesEmailTemplateDirectory::default();
        let mut one = table(&[Surface::new("receipts", now)]);
        directory.reconcile(&mut one);
        assert!(
            one["receipts"].registered,
            "case published: expected the record to be marked registered, got {:?}",
            one["receipts"]
        );

        let mut other: BTreeMap<String, Surface> = BTreeMap::new();
        directory.reconcile(&mut other);
        assert_eq!(
            other.keys().cloned().collect::<Vec<String>>(),
            vec!["receipts".to_string()],
            "case adopted: expected the other surface to adopt the template, got {:?}",
            other.keys().collect::<Vec<&String>>()
        );
        assert_eq!(
            other["receipts"].created_at, now,
            "case adopted: expected the instant the template was recorded, got {:?}",
            other["receipts"].created_at
        );
        assert_eq!(
            other["receipts"].subject.as_deref(),
            Some("Subject of receipts"),
            "case adopted: expected the recorded subject, got {:?}",
            other["receipts"].subject
        );
    }

    #[test]
    fn content_written_on_one_surface_is_reported_by_the_other() {
        let now = SystemTime::UNIX_EPOCH;
        let mut directory = SesEmailTemplateDirectory::default();
        let mut one = table(&[Surface::new("receipts", now)]);
        directory.reconcile(&mut one);
        let mut other: BTreeMap<String, Surface> = BTreeMap::new();
        directory.reconcile(&mut other);

        // One surface replaces the content and publishes the template as it now holds it.
        let held = one.get_mut("receipts").expect("the recorded template");
        held.subject = Some("Your invoice".to_string());
        held.html = Some("<p>invoice</p>".to_string());
        held.own = Some("this surface only".to_string());
        directory.publish(held.published());

        directory.reconcile(&mut other);
        assert_eq!(
            other["receipts"].subject.as_deref(),
            Some("Your invoice"),
            "case written content: expected the written subject, got {:?}",
            other["receipts"].subject
        );
        assert_eq!(
            other["receipts"].html.as_deref(),
            Some("<p>invoice</p>"),
            "case written content: expected the written HTML part, got {:?}",
            other["receipts"].html
        );
        assert_eq!(
            other["receipts"].own, None,
            "case own member: expected the other surface's own member untouched, got {:?}",
            other["receipts"].own
        );

        // And a part the FIRST surface then clears is gone from the second too, which is
        // what carries a replacement rather than only an addition.
        let held = one.get_mut("receipts").expect("the recorded template");
        held.html = None;
        directory.publish(held.published());
        directory.reconcile(&mut other);
        assert_eq!(
            other["receipts"].html, None,
            "case cleared part: expected no HTML part, got {:?}",
            other["receipts"].html
        );
    }

    #[test]
    fn a_deleted_template_goes_from_every_surface() {
        let now = SystemTime::UNIX_EPOCH;
        let mut directory = SesEmailTemplateDirectory::default();
        let mut one = table(&[Surface::new("receipts", now)]);
        directory.reconcile(&mut one);
        let mut other: BTreeMap<String, Surface> = BTreeMap::new();
        directory.reconcile(&mut other);

        directory.forget("receipts");
        directory.reconcile(&mut other);
        assert!(
            other.is_empty(),
            "case deleted: expected the template gone from the other surface, got {:?}",
            other.keys().collect::<Vec<&String>>()
        );
        directory.reconcile(&mut one);
        assert!(
            one.is_empty(),
            "case deleted: expected the template gone from the recording surface, got {:?}",
            one.keys().collect::<Vec<&String>>()
        );
    }

    #[test]
    fn the_registry_is_bounded() {
        let now = SystemTime::UNIX_EPOCH;
        let mut directory = SesEmailTemplateDirectory::default();
        for index in 0..MAX_REGISTERED_EMAIL_TEMPLATES {
            assert!(
                directory
                    .publish(record(&format!("template-{index}"), now))
                    .is_some(),
                "case under the quota: expected template-{index} to publish"
            );
        }
        assert!(
            directory.publish(record("one-too-many", now)).is_none(),
            "case at the quota: expected the publication to be refused, got {:?}",
            directory.get("one-too-many")
        );
        assert_eq!(
            directory.len(),
            MAX_REGISTERED_EMAIL_TEMPLATES,
            "case at the quota: expected the registry to stay at its ceiling, got {}",
            directory.len()
        );
    }

    #[test]
    fn a_publication_keeps_the_instant_the_template_was_recorded() {
        let first = SystemTime::UNIX_EPOCH;
        let later = first + Duration::from_secs(60);
        let mut directory = SesEmailTemplateDirectory::default();
        directory.publish(record("receipts", first));
        directory.publish(record("receipts", later));
        assert_eq!(
            directory.get("receipts").map(|held| held.created_at),
            Some(first),
            "case re-published: expected the first recorded instant, got {:?}",
            directory.get("receipts").map(|held| held.created_at)
        );
    }

    #[test]
    fn a_page_token_names_a_position_only_under_the_registry_that_minted_it() {
        let directory = SesEmailTemplateDirectory::default();
        let other = SesEmailTemplateDirectory::default();
        let key = directory.page_token_key();
        // Cursors spanning the template names a listing cuts a page at: one character, a
        // plain name, punctuation, and a non-ASCII name.
        for cursor in ["T", "receipts", "Order.Receipt_v2-2026", "領収書"] {
            let token = page_token(&key, cursor);
            assert_eq!(
                page_cursor(&key, &token).as_deref(),
                Some(cursor),
                "case {cursor:?}: expected the cursor to round-trip, got {:?}",
                page_cursor(&key, &token)
            );
            assert_eq!(
                page_cursor(&other.page_token_key(), &token),
                None,
                "case {cursor:?}: a token of another registry must name no position, got {:?}",
                page_cursor(&other.page_token_key(), &token)
            );
            // Every value a caller could send that this registry did not mint.
            for (case, made_up) in [
                ("the cursor sent bare", cursor.to_string()),
                ("an empty token", String::new()),
                ("not base64", "not a token!".to_string()),
                (
                    "a token with its last byte dropped",
                    ::aws_smithy_types::base64::encode(
                        &key.mint(cursor)[..key.mint(cursor).len() - 1],
                    ),
                ),
                (
                    "a made-up authenticator",
                    ::aws_smithy_types::base64::encode(
                        [
                            vec![0_u8; crate::page_tokens::AUTHENTICATOR_BYTES],
                            cursor.as_bytes().to_vec(),
                        ]
                        .concat()
                        .as_slice(),
                    ),
                ),
            ] {
                assert_eq!(
                    page_cursor(&key, &made_up),
                    None,
                    "case {cursor:?} / {case}: expected no position, got {:?}",
                    page_cursor(&key, &made_up)
                );
            }
        }
    }
}
