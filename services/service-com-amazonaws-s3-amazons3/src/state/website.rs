// SPDX-License-Identifier: CC0-1.0

//! A bucket's WEBSITE configuration: the record `PutBucketWebsite` stores and
//! `GetBucketWebsite` reports, the published limits and HTTP status classes its members are
//! judged against, and the schema a `WebsiteConfiguration` document has to validate against
//! before any of it is read.

use super::*;

/// How many routing rules one bucket's website configuration may carry.
///
/// The published limitation, from `PutBucketWebsite`'s own documentation: "Amazon S3 has a
/// limitation of 50 routing rules per website configuration. If you require more than 50
/// routing rules, you can use object redirect." The model bounds `RoutingRules` at neither
/// end (`smithy_query(query="shape",shape="RoutingRules")`), so this check is what enforces
/// the limitation, and it also bounds what one bucket's record parks in this process's
/// memory — a put past it is refused rather than recorded.
pub const MAX_WEBSITE_ROUTING_RULES: usize = 50;

/// The lowest and highest HTTP status a `Redirect$HttpRedirectCode` may name.
///
/// "The HTTP redirect code to use on the response" (that member's own documentation): a
/// response code that redirects is one of the 3xx class other than 300, which asks the
/// client to choose among representations rather than naming one to follow — the bounds S3's
/// own refusal of a code outside them states
/// ([`WebsiteConfigurationRejection::InvalidHttpRedirectCode`]). The
/// model types the member as a plain string
/// (`smithy_query(query="shape",shape="HttpRedirectCode")`), so nothing refuses a
/// non-redirect code before the store does.
pub const HTTP_REDIRECT_CODES: std::ops::RangeInclusive<u16> = 301..=399;

/// The lowest and highest HTTP status a `Condition$HttpErrorCodeReturnedEquals` may name.
///
/// "The HTTP error code when the redirect is applied. In the event of an error, if the error
/// code equals this value, then the specified redirect is applied" (that member's own
/// documentation): only a response that IS an error can equal it, which is the 4xx and 5xx
/// classes — the bounds S3's own refusal of a code outside them states
/// ([`WebsiteConfigurationRejection::InvalidHttpErrorCode`]). The model types the member as
/// a plain string
/// (`smithy_query(query="shape",shape="HttpErrorCodeReturnedEquals")`), so a rule
/// conditioned on a status that is not an error would never be applied by anything and is
/// refused rather than recorded.
pub const HTTP_ERROR_CODES: std::ops::RangeInclusive<u16> = 400..=599;

/// Where every request to a bucket's website endpoint is redirected, as the
/// `RedirectAllRequestsTo` container of the configuration named it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebsiteRedirectAllRequestsTo {
    /// `RedirectAllRequestsTo$HostName`, a REQUIRED member: "Name of the host where
    /// requests are redirected."
    pub host_name: String,
    /// `RedirectAllRequestsTo$Protocol`: one of [`PROTOCOL_VALUES`], or `None` when the
    /// container named none. Nothing is stored for an absent protocol because the default
    /// is a property of the REQUEST, not of the configuration — "The default is the
    /// protocol that is used in the original request" (that member's documentation) — so a
    /// reader reports no protocol rather than inventing one.
    pub protocol: Option<String>,
}

/// The condition a stored routing rule applies under: one `Condition` container.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebsiteRoutingCondition {
    /// `Condition$HttpErrorCodeReturnedEquals`: "The HTTP error code when the redirect is
    /// applied."
    pub http_error_code_returned_equals: Option<String>,
    /// `Condition$KeyPrefixEquals`: "The object key name prefix when the redirect is
    /// applied."
    pub key_prefix_equals: Option<String>,
}

impl WebsiteRoutingCondition {
    /// Whether the condition names NEITHER of its two members, in which case there is no
    /// condition to apply the rule under.
    ///
    /// Each member is required when the other is absent — "Required when parent element
    /// `Condition` is specified and sibling `KeyPrefixEquals` is not specified" and the
    /// mirror sentence on the other member — so a container naming neither is one S3
    /// refuses rather than records.
    pub fn is_empty(&self) -> bool {
        self.http_error_code_returned_equals.is_none() && self.key_prefix_equals.is_none()
    }
}

/// Where a stored routing rule redirects to: one `Redirect` container.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebsiteRedirect {
    /// `Redirect$HostName`: "The host name to use in the redirect request."
    pub host_name: Option<String>,
    /// `Redirect$HttpRedirectCode`, within [`HTTP_REDIRECT_CODES`].
    pub http_redirect_code: Option<String>,
    /// `Redirect$Protocol`: one of [`PROTOCOL_VALUES`].
    pub protocol: Option<String>,
    /// `Redirect$ReplaceKeyPrefixWith`: "The object key prefix to use in the redirect
    /// request." Mutually exclusive with [`Self::replace_key_with`].
    pub replace_key_prefix_with: Option<String>,
    /// `Redirect$ReplaceKeyWith`: "The specific object key to use in the redirect request."
    pub replace_key_with: Option<String>,
}

/// The prefixes a value of the object member `WebsiteRedirectLocation` has to begin with.
///
/// "If the bucket is configured as a website, redirects requests for this object to another
/// object in the same bucket or to an external URL"
/// (`PutObjectRequest$WebsiteRedirectLocation`, bound to the `x-amz-website-redirect-location`
/// header): the value is the `Location` the website endpoint would later answer with, so it
/// has to be addressable — an absolute URL under either scheme, or a path on the website host
/// itself. The three are the ones S3's own refusal of a value outside them names
/// ([`INVALID_REDIRECT_LOCATION_MESSAGE`]); the model types the member as a plain string
/// (`smithy_query(query="shape",shape="WebsiteRedirectLocation")`), so nothing refuses a
/// value that is none of them before this does.
pub const WEBSITE_REDIRECT_LOCATION_PREFIXES: [&str; 3] = ["http://", "https://", "/"];

/// The error code S3 answers a `WebsiteRedirectLocation` outside
/// [`WEBSITE_REDIRECT_LOCATION_PREFIXES`] with, and the HTTP status it is rendered at: a
/// value in a named header that is not valid for the request, which is a 400.
pub const INVALID_REDIRECT_LOCATION_CODE: &str = "InvalidRedirectLocation";
/// See [`INVALID_REDIRECT_LOCATION_CODE`].
pub const INVALID_REDIRECT_LOCATION_STATUS: u16 = 400;
/// See [`INVALID_REDIRECT_LOCATION_CODE`] — the sentence that code carries, which is ENTIRELY
/// fixed text: it states the requirement rather than anything about the value submitted.
pub const INVALID_REDIRECT_LOCATION_MESSAGE: &str =
    "The website redirect location must have a prefix of 'http://' or 'https://' or '/'.";

/// Whether `value` is a website redirect location S3 stores on an object: one beginning with
/// one of [`WEBSITE_REDIRECT_LOCATION_PREFIXES`].
///
/// The ONE reading for every write that carries the member — the single-request writes, the
/// copy and the initiation of a multipart upload — so a value one of them refuses cannot be
/// stored by another.
#[must_use]
pub fn website_redirect_location_is_addressable(value: &str) -> bool {
    WEBSITE_REDIRECT_LOCATION_PREFIXES
        .iter()
        .any(|prefix| value.starts_with(prefix))
}

/// One `RoutingRule` of a bucket's website configuration: when a redirect is applied, and
/// where to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebsiteRoutingRule {
    /// `RoutingRule$Condition`, or `None` when the rule names none — in which case the
    /// redirect applies to every request.
    pub condition: Option<WebsiteRoutingCondition>,
    /// `RoutingRule$Redirect`, a REQUIRED member.
    pub redirect: WebsiteRedirect,
}

/// One bucket's website configuration, as `PutBucketWebsite` last set it.
///
/// Two shapes of configuration, which the container's own documentation keeps apart: one
/// that redirects EVERY request ([`Self::redirect_all_requests_to`] alone — "If you specify
/// this property, you can't specify any other property"), and one that serves the bucket as
/// a website, which "must provide an index document for the bucket" and may add an error
/// document and routing rules. [`validate_website_configuration`] is what keeps a stored
/// configuration to one of those two shapes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BucketWebsite {
    /// `IndexDocument$Suffix`: "A suffix that is appended to a request that is for a
    /// directory on the website endpoint", or `None` for a redirect-everything
    /// configuration.
    pub index_document_suffix: Option<String>,
    /// `ErrorDocument$Key`: "The object key name to use when a 4XX class error occurs", or
    /// `None` when the configuration names no error document.
    pub error_document_key: Option<String>,
    /// `WebsiteConfiguration$RedirectAllRequestsTo`, or `None` for a configuration that
    /// serves the bucket as a website rather than redirecting every request.
    pub redirect_all_requests_to: Option<WebsiteRedirectAllRequestsTo>,
    /// `WebsiteConfiguration$RoutingRules`, in the order the request carried them, and
    /// EMPTY when the configuration carried no `RoutingRules` container.
    ///
    /// A plain `Vec` rather than an `Option`, because "no routing rules" is ONE state here:
    /// the XML boundary parses a `RoutingRules` container that names no `RoutingRule` to the
    /// same absent list as no container at all, so there is nothing for a `None` to mean
    /// that an empty list does not — and a reader reports the container exactly when there
    /// is a rule in it.
    pub routing_rules: Vec<WebsiteRoutingRule>,
}

/// The `<ArgumentValue>` S3's refusal of an ABSENT member carries: the member held no value,
/// which its error document spells as the JSON null literal rather than leaving the element
/// empty.
pub const ABSENT_ARGUMENT_VALUE: &str = "null";

/// Why a website configuration is not one S3 would apply to a bucket, and how S3 refuses it.
///
/// Each variant names the member that is wrong and carries what that member held, and
/// answers the three things S3's own refusal of that member states: the wire code
/// ([`Self::code`]), the sentence (its [`std::fmt::Display`]) and — for a refusal about one
/// member of the document — the `<ArgumentName>` / `<ArgumentValue>` pair naming it
/// ([`Self::argument`]).
///
/// The sentence of each is FIXED text for that class of fault; the only thing interpolated
/// into one is a value the REQUEST carried, where the sentence S3 publishes names it (a
/// rejected status code). Nothing about one request's wording reaches another's refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebsiteConfigurationRejection {
    /// A document that does not validate against the schema S3 publishes for the container
    /// — including one that is not well-formed XML at all. See
    /// [`website_document_rejection`].
    MalformedDocument,
    /// `RedirectAllRequestsTo` beside another member of the container: "If you specify this
    /// property, you can't specify any other property"
    /// (`WebsiteConfiguration$RedirectAllRequestsTo`).
    RedirectAllNotExclusive {
        /// `RedirectAllRequestsTo$HostName`, the "name of the host where requests are
        /// redirected" — the value the container the refusal names carries, as sent.
        host_name: String,
    },
    /// A configuration that neither redirects every request nor names an index document:
    /// "the website configuration must provide an index document for the bucket, because
    /// some requests might not be redirected" (`PutBucketWebsite`'s own documentation).
    NoIndexDocument,
    /// An `IndexDocument$Suffix` that is empty or carries a slash: "The suffix must not be
    /// empty and must not include a slash character" (that member's documentation).
    MalformedIndexDocumentSuffix {
        /// The suffix the request carried, as it spelled it.
        suffix: String,
    },
    /// An `ErrorDocument$Key` that is not an object key: the empty string, which
    /// `ObjectKey`'s `length {min 1}` forbids.
    MalformedErrorDocumentKey {
        /// The key the request carried, as it spelled it.
        key: String,
    },
    /// More routing rules than [`MAX_WEBSITE_ROUTING_RULES`].
    TooManyRoutingRules {
        /// How many the request carried.
        count: usize,
    },
    /// A `Condition` naming neither of its members. See
    /// [`WebsiteRoutingCondition::is_empty`].
    EmptyCondition,
    /// A `Redirect` naming both `ReplaceKeyPrefixWith` and `ReplaceKeyWith`: each "can be
    /// present only if" the other "is not provided" (both members' documentation).
    ConflictingKeyReplacement,
    /// A `Protocol` outside [`PROTOCOL_VALUES`], on either member that carries one. The
    /// sentence S3 refuses it with names the two published values itself and echoes nothing
    /// from the request, so the value is not carried.
    UnsupportedProtocol,
    /// An `HttpRedirectCode` outside [`HTTP_REDIRECT_CODES`].
    InvalidHttpRedirectCode {
        /// The value that is not an HTTP redirect code, as the request spelled it.
        code: String,
    },
    /// An `HttpErrorCodeReturnedEquals` outside [`HTTP_ERROR_CODES`].
    InvalidHttpErrorCode {
        /// The value that is not an HTTP error code, as the request spelled it.
        code: String,
    },
}

impl WebsiteConfigurationRejection {
    /// The wire error code S3 answers this refusal with.
    ///
    /// Three codes from S3's published "Error responses" list, all HTTP 400. A document that
    /// does not satisfy the published schema is [`MALFORMED_XML_CODE`]. A member of the
    /// document whose VALUE — or whose absence — is not one the member may carry is
    /// [`INVALID_ARGUMENT_CODE`], and the refusal names that member in its
    /// `<ArgumentName>` / `<ArgumentValue>` pair ([`Self::argument`]). A document whose
    /// members are each acceptable on their own but which asks for behaviour the service
    /// cannot carry out — a redirect conditioned on a status that is not an error, a rule
    /// naming two replacements for one key, a redirect over a scheme the service does not
    /// make, more rules than a configuration may hold — is [`INVALID_REQUEST_CODE`].
    pub fn code(&self) -> &'static str {
        match self {
            Self::MalformedDocument => MALFORMED_XML_CODE,
            Self::RedirectAllNotExclusive { .. }
            | Self::NoIndexDocument
            | Self::MalformedIndexDocumentSuffix { .. }
            | Self::MalformedErrorDocumentKey { .. } => INVALID_ARGUMENT_CODE,
            Self::TooManyRoutingRules { .. }
            | Self::EmptyCondition
            | Self::ConflictingKeyReplacement
            | Self::UnsupportedProtocol
            | Self::InvalidHttpRedirectCode { .. }
            | Self::InvalidHttpErrorCode { .. } => INVALID_REQUEST_CODE,
        }
    }

    /// The HTTP status this refusal is rendered with: 400 for all three codes
    /// ([`Self::code`]), read from that code's own constant rather than repeated here.
    pub fn status(&self) -> u16 {
        match self.code() {
            MALFORMED_XML_CODE => MALFORMED_XML_STATUS,
            INVALID_REQUEST_CODE => INVALID_REQUEST_STATUS,
            _ => INVALID_ARGUMENT_STATUS,
        }
    }

    /// The `<ArgumentName>` / `<ArgumentValue>` pair S3's error document names the refused
    /// member by, or `None` for a refusal that is about the document as a whole, or about a
    /// rule inside it, rather than about one member of the container.
    ///
    /// The name is the CONTAINER element the value arrived in, as the API publishes it, and
    /// the value is what that container held: the scalar the member carries, the host a
    /// redirect-everything container names, or [`ABSENT_ARGUMENT_VALUE`] where the refusal is
    /// that the member held nothing at all.
    ///
    /// Only a refusal [`Self::code`] answers [`INVALID_ARGUMENT_CODE`] names an argument: the
    /// pair is that code's own document (Amazon S3 API Reference, *Error responses*).
    pub fn argument(&self) -> Option<(&'static str, Option<&str>)> {
        match self {
            Self::NoIndexDocument => Some(("IndexDocument", Some(ABSENT_ARGUMENT_VALUE))),
            Self::MalformedIndexDocumentSuffix { suffix } => {
                Some(("IndexDocument", Some(suffix.as_str())))
            }
            Self::MalformedErrorDocumentKey { key } => {
                Some(("ErrorDocument", Some(key.as_str())))
            }
            Self::RedirectAllNotExclusive { host_name } => {
                Some(("RedirectAllRequestsTo", Some(host_name.as_str())))
            }
            _ => None,
        }
    }
}

impl std::fmt::Display for WebsiteConfigurationRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // The code's own FIXED published text, which states the fault class rather than
            // anything about this document.
            Self::MalformedDocument => write!(f, "{MALFORMED_XML_MESSAGE}"),
            Self::RedirectAllNotExclusive { .. } => write!(
                f,
                "RedirectAllRequestsTo cannot be provided in conjunction with other Routing \
                 Rules."
            ),
            Self::NoIndexDocument => write!(
                f,
                "A value for IndexDocument Suffix must be provided if RedirectAllRequestsTo \
                 is empty"
            ),
            Self::MalformedIndexDocumentSuffix { .. } => {
                write!(f, "The IndexDocument Suffix is not well formed")
            }
            Self::MalformedErrorDocumentKey { .. } => {
                write!(f, "The ErrorDocument Key is not well formed")
            }
            Self::TooManyRoutingRules { count } => write!(
                f,
                "{count} routing rules provided, the number of routing rules in a website \
                 configuration is limited to {MAX_WEBSITE_ROUTING_RULES}."
            ),
            Self::EmptyCondition => write!(
                f,
                "Condition cannot be empty. To redirect all requests without a condition, \
                 the condition element shouldn't be present."
            ),
            Self::ConflictingKeyReplacement => write!(
                f,
                "You can only define ReplaceKeyPrefix or ReplaceKey but not both."
            ),
            Self::UnsupportedProtocol => write!(
                f,
                "Invalid protocol, protocol can be http or https. If not defined the \
                 protocol will be selected automatically."
            ),
            // The two status-code sentences are the one place a REQUEST value reaches a
            // website refusal's text: S3 states the class the code had to be in and, in
            // parentheses, the code the request sent.
            Self::InvalidHttpRedirectCode { code } => write!(
                f,
                "The provided HTTP redirect code ({code}) is not valid. Valid codes are 3XX \
                 except 300."
            ),
            Self::InvalidHttpErrorCode { code } => write!(
                f,
                "The provided HTTP error code ({code}) is not valid. Valid codes are 4XX or \
                 5XX."
            ),
        }
    }
}

impl std::error::Error for WebsiteConfigurationRejection {}

/// Whether `configuration` is one S3 applies to a bucket, and what is wrong with it when it
/// is not.
///
/// Checked BEFORE anything is written, so a refused put leaves the bucket's current website
/// configuration exactly as it was — the same order every other bucket subresource put
/// keeps.
///
/// Two of the rules here are ALSO modelled constraints, which the request boundary would
/// refuse before any handler ran — `ErrorDocument$Key`'s `length {min 1}` and `Protocol`'s
/// value set — and S3 publishes a refusal of its own for each, which a boundary refusal
/// cannot state (it carries a code and a sentence, and the key's names the member and echoes
/// what it held). So a document carrying either is withheld from the codec
/// (`crate::service::wire`) and the refusal is read from the request's own document
/// ([`website_document_rejection`]), which is where those two values still exist. They are
/// still checked here because this function, not the wire, is what guards the STORE: a
/// configuration no reader could report back is never recorded, whoever writes it.
pub fn validate_website_configuration(
    configuration: &BucketWebsite,
) -> Result<(), WebsiteConfigurationRejection> {
    if let Some(redirect_all) = &configuration.redirect_all_requests_to {
        // "If you specify this property, you can't specify any other property"
        // (`WebsiteConfiguration$RedirectAllRequestsTo`).
        if configuration.index_document_suffix.is_some()
            || configuration.error_document_key.is_some()
            || !configuration.routing_rules.is_empty()
        {
            return Err(WebsiteConfigurationRejection::RedirectAllNotExclusive {
                host_name: redirect_all.host_name.clone(),
            });
        }
        check_protocol(&redirect_all.protocol)?;
        return Ok(());
    }

    // No redirect-everything container, so the configuration serves the bucket as a website
    // and "must provide an index document for the bucket" (`PutBucketWebsite`).
    let Some(suffix) = configuration.index_document_suffix.as_deref() else {
        return Err(WebsiteConfigurationRejection::NoIndexDocument);
    };
    if suffix.is_empty() || suffix.contains('/') {
        return Err(WebsiteConfigurationRejection::MalformedIndexDocumentSuffix {
            suffix: suffix.to_string(),
        });
    }
    if let Some(key) = configuration
        .error_document_key
        .as_deref()
        .filter(|key| key.is_empty())
    {
        return Err(WebsiteConfigurationRejection::MalformedErrorDocumentKey {
            key: key.to_string(),
        });
    }
    if configuration.routing_rules.len() > MAX_WEBSITE_ROUTING_RULES {
        return Err(WebsiteConfigurationRejection::TooManyRoutingRules {
            count: configuration.routing_rules.len(),
        });
    }

    for rule in &configuration.routing_rules {
        if let Some(condition) = rule.condition.as_ref() {
            if condition.is_empty() {
                return Err(WebsiteConfigurationRejection::EmptyCondition);
            }
            if let Some(code) = condition.http_error_code_returned_equals.as_deref() {
                let status = code.parse::<u16>().ok();
                if !status.is_some_and(|status| HTTP_ERROR_CODES.contains(&status)) {
                    return Err(WebsiteConfigurationRejection::InvalidHttpErrorCode {
                        code: code.to_string(),
                    });
                }
            }
        }
        if rule.redirect.replace_key_prefix_with.is_some()
            && rule.redirect.replace_key_with.is_some()
        {
            return Err(WebsiteConfigurationRejection::ConflictingKeyReplacement);
        }
        check_protocol(&rule.redirect.protocol)?;
        if let Some(code) = rule.redirect.http_redirect_code.as_deref() {
            let status = code.parse::<u16>().ok();
            if !status.is_some_and(|status| HTTP_REDIRECT_CODES.contains(&status)) {
                return Err(WebsiteConfigurationRejection::InvalidHttpRedirectCode {
                    code: code.to_string(),
                });
            }
        }
    }

    Ok(())
}

/// Refuse a `Protocol` value outside [`PROTOCOL_VALUES`].
pub(super) fn check_protocol(protocol: &Option<String>) -> Result<(), WebsiteConfigurationRejection> {
    match protocol.as_deref() {
        Some(value) if !PROTOCOL_VALUES.contains(&value) => {
            Err(WebsiteConfigurationRejection::UnsupportedProtocol)
        }
        _ => Ok(()),
    }
}

/// Every element the published `WebsiteConfiguration` schema declares, with the elements
/// each one may contain — the ROOT element first.
///
/// Source: the `WebsiteConfiguration` shape closure
/// (`smithy_query(query="shape",shape="WebsiteConfiguration,ErrorDocument,IndexDocument,
/// RedirectAllRequestsTo,RoutingRules,RoutingRule,Condition,Redirect",
/// model_path="services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json")`).
/// Each structure's children are its members under the element name restXml spells them with
/// — their own declared names, none of the closure's members being renamed — and the one
/// list in the closure, `RoutingRules`, under its member's `xmlName` (`RoutingRule`). A
/// member whose target is a plain string is a LEAF, so it declares no child element.
///
/// What makes this a *schema*, rather than a list of words a document may contain: an
/// element is judged against the children of the element it arrived IN, so a declared name
/// in a position the schema does not declare it in fails the same way an undeclared one
/// does.
///
/// `pub`, with [`WEBSITE_DOCUMENT_REQUIRED_ELEMENTS`]: `tests/all/bucket_website_refusals.rs`
/// re-derives both from the model itself, so a model update that renames, adds or removes an
/// element of this closure cannot leave the table behind.
pub const WEBSITE_DOCUMENT_SCHEMA: &[(&str, &[&str])] = &[
    (
        "WebsiteConfiguration",
        &[
            "ErrorDocument",
            "IndexDocument",
            "RedirectAllRequestsTo",
            "RoutingRules",
        ],
    ),
    ("ErrorDocument", &["Key"]),
    ("IndexDocument", &["Suffix"]),
    ("RedirectAllRequestsTo", &["HostName", "Protocol"]),
    ("RoutingRules", &["RoutingRule"]),
    ("RoutingRule", &["Condition", "Redirect"]),
    (
        "Condition",
        &["HttpErrorCodeReturnedEquals", "KeyPrefixEquals"],
    ),
    (
        "Redirect",
        &[
            "HostName",
            "HttpRedirectCode",
            "Protocol",
            "ReplaceKeyPrefixWith",
            "ReplaceKeyWith",
        ],
    ),
];

/// The child element each of [`WEBSITE_DOCUMENT_SCHEMA`]'s containers REQUIRES.
///
/// Source: the same shape closure, whose `required` members are `IndexDocument$Suffix`,
/// `ErrorDocument$Key`, `RedirectAllRequestsTo$HostName` and `RoutingRule$Redirect`. A
/// container that names none of them does not validate against the published schema, however
/// well spelled the children it does name are.
///
/// `pub` for the reason [`WEBSITE_DOCUMENT_SCHEMA`] is: the same test re-derives it from the
/// model.
pub const WEBSITE_DOCUMENT_REQUIRED_ELEMENTS: &[(&str, &str)] = &[
    ("IndexDocument", "Suffix"),
    ("ErrorDocument", "Key"),
    ("RedirectAllRequestsTo", "HostName"),
    ("RoutingRule", "Redirect"),
];

/// The child element the schema requires inside `element`, or `None` for one that requires
/// none.
pub(super) fn required_element(element: &str) -> Option<&'static str> {
    WEBSITE_DOCUMENT_REQUIRED_ELEMENTS
        .iter()
        .find(|(container, _)| *container == element)
        .map(|(_, required)| *required)
}

/// What S3 refuses the website document `document` for, read from the DOCUMENT itself rather
/// than from the configuration parsed out of it, or `None` for a document it refuses nothing
/// about.
///
/// Three refusals can only be read here, and each is one the REQUEST BOUNDARY would otherwise
/// answer before `PutBucketWebsite` ran — in a document that carries a code and a sentence and
/// no room for the `<ArgumentName>` / `<ArgumentValue>` pair S3 names a refused member by. So
/// a document this reports a rejection for is withheld from the codec in full
/// (`crate::service::wire`), leaving the operation the ONE place a website document's refusal
/// is worded:
///
/// * a document that does not validate against the schema S3 publishes for it
///   ([`website_document_validates`]), which is a DIFFERENT answer from a refusal of the
///   configuration the document describes: a codec reads an element it does not know by
///   ignoring it, so a document that misspells, misnests or invents an element parses to a
///   configuration MISSING the member that element was meant to carry, and refusing that
///   configuration would state the absence of a member the caller did spell, under a code
///   saying the value was wrong rather than the document;
/// * an `ErrorDocument` whose `Key` is EMPTY, which `ObjectKey`'s `length {min 1}` makes a
///   modelled violation; and
/// * a `Protocol` outside the two published schemes, which the modelled `Protocol` enum makes
///   one.
///
/// Read from the document and not from the input because that is the only place these values
/// survive: a parsed input carries no element the codec did not know, and carries nothing at
/// all for a request the boundary refused.
pub fn website_document_rejection(document: &[u8]) -> Option<WebsiteConfigurationRejection> {
    if !website_document_validates(document) {
        return Some(WebsiteConfigurationRejection::MalformedDocument);
    }
    if website_document_names_empty_error_document_key(document) {
        return Some(WebsiteConfigurationRejection::MalformedErrorDocumentKey {
            key: String::new(),
        });
    }
    website_document_names_unsupported_protocol(document)
        .then_some(WebsiteConfigurationRejection::UnsupportedProtocol)
}

/// Whether `document` names a `Protocol` — on either member that carries one — outside
/// [`PROTOCOL_VALUES`].
///
/// ANY one such element is enough, so the walk answers on the first: a document naming a
/// scheme the service does not make is refused for that element whatever the others carry.
///
/// The text is compared with its surrounding whitespace trimmed, which is how the codec reads
/// an element's text, so this and the parsed input cannot disagree about which values are the
/// two published schemes.
pub(super) fn website_document_names_unsupported_protocol(document: &[u8]) -> bool {
    let mut reader = ::quick_xml::Reader::from_reader(document);
    // Whether the reader is inside a `Protocol` element, and whether that ONE element has
    // carried a published scheme yet: an element that opens and closes with no text between
    // carries the empty string, which is no scheme either, so each element starts unsatisfied
    // and is settled by the text inside it.
    let mut inside = false;
    let mut published = false;
    loop {
        match reader.read_event() {
            Ok(::quick_xml::events::Event::Start(start)) => {
                inside = start.name().as_ref() == PROTOCOL_ELEMENT;
                published = false;
            }
            // A self-closing `<Protocol/>` carries no text at all, so it names no scheme.
            Ok(::quick_xml::events::Event::Empty(empty)) => {
                if empty.name().as_ref() == PROTOCOL_ELEMENT {
                    return true;
                }
            }
            Ok(::quick_xml::events::Event::Text(text)) if inside => {
                let spelled = text.xml10_content().unwrap_or_default();
                published = PROTOCOL_VALUES.contains(&spelled.trim());
            }
            Ok(::quick_xml::events::Event::End(_)) => {
                if inside && !published {
                    return true;
                }
                inside = false;
            }
            Ok(::quick_xml::events::Event::Eof) | Err(_) => return false,
            _ => {}
        }
    }
}

/// The element both members that carry a `Protocol` are spelled with: the member's own
/// declared name, which restXml writes it under (`smithy_query(query="shape",
/// shape="RedirectAllRequestsTo,Redirect")` reports no `xmlName` on either).
pub(super) const PROTOCOL_ELEMENT: &[u8] = b"Protocol";

/// Whether `document` names an `ErrorDocument` whose `Key` element carries no text — the
/// empty object key [`website_document_rejection`] reads from the document.
pub(super) fn website_document_names_empty_error_document_key(document: &[u8]) -> bool {
    let mut reader = ::quick_xml::Reader::from_reader(document);
    // Where in the `ErrorDocument` container the reader is, and whether the `Key` inside it
    // has carried any text.
    let mut inside_container = false;
    let mut inside_key = false;
    let mut empty = false;
    loop {
        match reader.read_event() {
            Ok(::quick_xml::events::Event::Start(start)) => {
                match start.name().as_ref() {
                    b"ErrorDocument" => inside_container = true,
                    b"Key" if inside_container => {
                        inside_key = true;
                        // Nothing read yet, so the key is empty until a text event says
                        // otherwise.
                        empty = true;
                    }
                    _ => {}
                }
            }
            // A self-closing `<Key/>` carries no text at all.
            Ok(::quick_xml::events::Event::Empty(start)) => {
                if inside_container && start.name().as_ref() == b"Key" {
                    empty = true;
                }
            }
            Ok(::quick_xml::events::Event::Text(text)) if inside_key => {
                empty = text.is_empty();
            }
            Ok(::quick_xml::events::Event::End(end)) => match end.name().as_ref() {
                b"Key" => inside_key = false,
                b"ErrorDocument" => return empty,
                _ => {}
            },
            Ok(::quick_xml::events::Event::Eof) | Err(_) => return empty,
            _ => {}
        }
    }
}

/// Whether `document` — the XML a `PutBucketWebsite` request carried as its payload —
/// validates against the schema S3 publishes for it ([`WEBSITE_DOCUMENT_SCHEMA`]).
///
/// `false` for a document that is not a well-formed XML document at all — bytes no XML
/// reader can walk, an element left unclosed, an end tag naming another element, or no
/// element whatever — which is the same refusal: the code's own sentence states both ("not
/// well-formed or did not validate against our published schema").
pub(super) fn website_document_validates(document: &[u8]) -> bool {
    let mut reader = ::quick_xml::Reader::from_reader(document);
    // The trail of element names from the root down to the element being read, each with the
    // child the schema requires inside it until that child is seen: an element is judged
    // against the children its PARENT declares, and a container against the child it owes.
    let mut open: Vec<(&'static str, Option<&'static str>)> = Vec::new();
    // Whether the document has a root element at all, which an XML document has by
    // definition and bytes that are not markup do not.
    let mut rooted = false;
    loop {
        let event = match reader.read_event() {
            Ok(event) => event,
            // Not well formed.
            Err(_) => return false,
        };
        let parent = open.last().map(|(element, _)| *element);
        match event {
            ::quick_xml::events::Event::Start(start) => {
                let Some(element) = declared_element(parent, start.name().as_ref()) else {
                    return false;
                };
                rooted = true;
                satisfy_required(&mut open, element);
                open.push((element, required_element(element)));
            }
            // A self-closing element (`<Condition/>`) is an element with no children at
            // all: the name is judged the same way, and a container that owes one is not
            // satisfied by it.
            ::quick_xml::events::Event::Empty(start) => {
                let Some(element) = declared_element(parent, start.name().as_ref()) else {
                    return false;
                };
                if required_element(element).is_some() {
                    return false;
                }
                rooted = true;
                satisfy_required(&mut open, element);
            }
            ::quick_xml::events::Event::End(_) => match open.pop() {
                // A container that never named the child the schema requires inside it.
                Some((_, Some(_))) => return false,
                Some((_, None)) => {}
                None => return false,
            },
            ::quick_xml::events::Event::Eof => return rooted && open.is_empty(),
            _ => {}
        }
    }
}

/// Note `element` as named inside the container it arrived in, which is what satisfies that
/// container's required child.
pub(super) fn satisfy_required(open: &mut [(&'static str, Option<&'static str>)], element: &'static str) {
    if let Some((_, required)) = open.last_mut() {
        if *required == Some(element) {
            *required = None;
        }
    }
}

/// The element [`WEBSITE_DOCUMENT_SCHEMA`] declares under `parent` (or as the ROOT, for a
/// `parent` of `None`) whose name is `element`, or `None` for a name the schema does not
/// declare there.
pub(super) fn declared_element(parent: Option<&str>, element: &[u8]) -> Option<&'static str> {
    let declared: &[&'static str] = match parent {
        // The root element of the document: the container the payload IS.
        None => &[WEBSITE_DOCUMENT_SCHEMA[0].0],
        Some(parent) => WEBSITE_DOCUMENT_SCHEMA
            .iter()
            .find(|(name, _)| *name == parent)
            // A LEAF carries text, not elements, so nothing is declared inside it.
            .map_or(&[], |(_, children)| *children),
    };
    declared
        .iter()
        .copied()
        .find(|name| name.as_bytes() == element)
}
