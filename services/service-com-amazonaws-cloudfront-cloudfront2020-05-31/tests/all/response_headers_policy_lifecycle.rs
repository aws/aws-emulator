// SPDX-License-Identifier: CC0-1.0

//! The CloudFront response headers policy control plane, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path
//! a client takes. The rules under test are the ones CloudFront's response headers policy
//! surface states:
//!
//! * a policy is PERSISTED by the call that creates it, which MINTS its identity (a UUID
//!   `Id` and a version token), so a later `GetResponseHeadersPolicy` answers from that
//!   record rather than from the request being served — down to the members the create
//!   left out;
//! * `ResponseHeadersPolicyConfig$Name`: "The name must be unique for response headers
//!   policies in this Amazon Web Services account", so a repeated name is refused and
//!   leaves the policy that holds it untouched;
//! * every CloudFront list travels in a wrapper that states its own length, and a request
//!   whose `Quantity` disagrees with its `Items` is refused with `InconsistentQuantities`
//!   and stores nothing;
//! * an id no policy holds is `NoSuchResponseHeadersPolicy`, whatever shape the id takes;
//! * a value outside a member's modelled constraint is refused before anything is stored;
//! * every member of `SecurityHeadersConfig` occupies an element of a serialized policy,
//!   and a security header the configuration does not set is the member's EMPTY element —
//!   the rule `crate::response_headers_policy_security_headers` asserts over the whole
//!   class, and which this suite reads back for every row of its own table.
//!
//! The table rows differ along everything these rules generalise over: which optional
//! configurations the policy carries at all, each list's length (none, one, many), the
//! enum values each enum member draws from (every value `ReferrerPolicyList` and
//! `FrameOptionsList` admit, from `smithy_query`), both booleans of every flag, both
//! published bounds of `SamplingRate` and one past each, and whether a text member is
//! ASCII, empty or carries XML metacharacters.

use std::collections::BTreeSet;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::ops::types::{
    FRAME_OPTIONS_LIST_VALUES, REFERRER_POLICY_LIST_VALUES,
    RESPONSE_HEADERS_POLICY_ACCESS_CONTROL_ALLOW_METHODS_VALUES_VALUES,
};
use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::MAX_RESPONSE_HEADERS_POLICIES;
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, emulator, is_minted_uuid, Answer, Node, API_VERSION, HOST,
};

// ---------------------------------------------------------------------------
// Calling the response headers policy surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/response-headers-policy` with the `ResponseHeadersPolicyConfig`
/// document `body` — the member the operation binds to the HTTP payload.
pub(crate) async fn create_policy(router: &MultiServiceRouter, body: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/response-headers-policy"),
        Some(body),
        None,
    )
    .await
}

/// `GET /2020-05-31/response-headers-policy/{Id}`.
///
/// `pub(crate)` so a sibling suite reads a policy back through the same route.
pub(crate) async fn get_policy(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/response-headers-policy/{id}"),
        None,
        None,
    )
    .await
}

// ---------------------------------------------------------------------------
// The configurations under test
// ---------------------------------------------------------------------------

/// `ResponseHeadersPolicyCorsConfig`, as a client submits it.
///
/// Its fields are `pub(crate)` so a sibling suite that drives a CORS configuration through
/// the EDGE states one with this builder rather than growing a copy of it.
#[derive(Debug, Clone)]
pub(crate) struct Cors {
    pub(crate) origins: Vec<String>,
    pub(crate) headers: Vec<String>,
    /// Values of `ResponseHeadersPolicyAccessControlAllowMethodsValues`.
    pub(crate) methods: Vec<String>,
    pub(crate) credentials: bool,
    /// `AccessControlExposeHeaders`, or `None` to leave the optional wrapper out.
    pub(crate) expose: Option<Vec<String>>,
    /// `AccessControlMaxAgeSec`, or `None` to leave the optional member out.
    pub(crate) max_age: Option<i32>,
    pub(crate) origin_override: bool,
}

/// `ResponseHeadersPolicyXSSProtection`.
#[derive(Debug, Clone)]
pub(crate) struct Xss {
    pub(crate) r#override: bool,
    pub(crate) protection: bool,
    pub(crate) mode_block: Option<bool>,
    pub(crate) report_uri: Option<String>,
}

/// `ResponseHeadersPolicyStrictTransportSecurity`.
#[derive(Debug, Clone)]
pub(crate) struct Hsts {
    pub(crate) r#override: bool,
    pub(crate) include_subdomains: Option<bool>,
    pub(crate) preload: Option<bool>,
    pub(crate) max_age: i32,
}

/// `ResponseHeadersPolicySecurityHeadersConfig`: every sub-structure optional, so a row
/// may carry any subset of them.
///
/// Its fields are `pub(crate)` so a sibling suite that has to state ONE security header —
/// the content security policy whose length CloudFront bounds — builds the same document
/// rather than growing a copy of it.
#[derive(Debug, Clone, Default)]
pub(crate) struct Security {
    pub(crate) xss: Option<Xss>,
    /// `(Override, FrameOption)`, a value of `FrameOptionsList`.
    pub(crate) frame: Option<(bool, String)>,
    /// `(Override, ReferrerPolicy)`, a value of `ReferrerPolicyList`.
    pub(crate) referrer: Option<(bool, String)>,
    /// `(Override, ContentSecurityPolicy)`.
    pub(crate) csp: Option<(bool, String)>,
    /// `ContentTypeOptions.Override`.
    pub(crate) content_type: Option<bool>,
    pub(crate) hsts: Option<Hsts>,
}

/// `ResponseHeadersPolicyServerTimingHeadersConfig`.
#[derive(Debug, Clone)]
pub(crate) struct ServerTiming {
    enabled: bool,
    /// `SamplingRate`, which the model bounds to `0..100`, or `None` to leave it out.
    sampling_rate: Option<f64>,
}

/// One `ResponseHeadersPolicyConfig` a client submits. A `None` means the member is left
/// out of the document entirely, which is what makes "the record reports what was
/// submitted" testable for absent members too.
#[derive(Debug, Clone, Default)]
pub(crate) struct Policy {
    pub(crate) case: &'static str,
    pub(crate) name: String,
    pub(crate) comment: Option<String>,
    pub(crate) cors: Option<Cors>,
    pub(crate) security: Option<Security>,
    pub(crate) server_timing: Option<ServerTiming>,
    /// `CustomHeadersConfig.Items` as `(Header, Value, Override)`, or `None` to leave the
    /// wrapper out. `Some(vec![])` is the wrapper carrying no items at all.
    pub(crate) custom_headers: Option<Vec<(String, String, bool)>>,
    /// `RemoveHeadersConfig.Items` headers, or `None` to leave the wrapper out.
    pub(crate) remove_headers: Option<Vec<String>>,
}

impl Policy {
    /// The `ResponseHeadersPolicyConfig` XML a client sends for this configuration, in
    /// the model's member declaration order.
    pub(crate) fn xml(&self) -> String {
        let mut out = format!(
            "<ResponseHeadersPolicyConfig \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">"
        );
        if let Some(comment) = &self.comment {
            out.push_str(&format!("<Comment>{}</Comment>", escape(comment)));
        }
        out.push_str(&format!("<Name>{}</Name>", escape(&self.name)));
        if let Some(cors) = &self.cors {
            out.push_str("<CorsConfig>");
            out.push_str(&wrapper("AccessControlAllowOrigins", "Origin", &cors.origins));
            out.push_str(&wrapper("AccessControlAllowHeaders", "Header", &cors.headers));
            out.push_str(&wrapper("AccessControlAllowMethods", "Method", &cors.methods));
            out.push_str(&format!(
                "<AccessControlAllowCredentials>{}</AccessControlAllowCredentials>",
                cors.credentials
            ));
            if let Some(expose) = &cors.expose {
                out.push_str(&wrapper("AccessControlExposeHeaders", "Header", expose));
            }
            if let Some(max_age) = cors.max_age {
                out.push_str(&format!(
                    "<AccessControlMaxAgeSec>{max_age}</AccessControlMaxAgeSec>"
                ));
            }
            out.push_str(&format!(
                "<OriginOverride>{}</OriginOverride>",
                cors.origin_override
            ));
            out.push_str("</CorsConfig>");
        }
        if let Some(security) = &self.security {
            out.push_str("<SecurityHeadersConfig>");
            if let Some(xss) = &security.xss {
                out.push_str(&format!(
                    "<XSSProtection><Override>{}</Override><Protection>{}</Protection>",
                    xss.r#override, xss.protection
                ));
                if let Some(mode_block) = xss.mode_block {
                    out.push_str(&format!("<ModeBlock>{mode_block}</ModeBlock>"));
                }
                if let Some(report_uri) = &xss.report_uri {
                    out.push_str(&format!("<ReportUri>{}</ReportUri>", escape(report_uri)));
                }
                out.push_str("</XSSProtection>");
            }
            if let Some((overridden, option)) = &security.frame {
                out.push_str(&format!(
                    "<FrameOptions><Override>{overridden}</Override>\
                     <FrameOption>{option}</FrameOption></FrameOptions>"
                ));
            }
            if let Some((overridden, policy)) = &security.referrer {
                out.push_str(&format!(
                    "<ReferrerPolicy><Override>{overridden}</Override>\
                     <ReferrerPolicy>{policy}</ReferrerPolicy></ReferrerPolicy>"
                ));
            }
            if let Some((overridden, csp)) = &security.csp {
                out.push_str(&format!(
                    "<ContentSecurityPolicy><Override>{overridden}</Override>\
                     <ContentSecurityPolicy>{}</ContentSecurityPolicy></ContentSecurityPolicy>",
                    escape(csp)
                ));
            }
            if let Some(overridden) = security.content_type {
                out.push_str(&format!(
                    "<ContentTypeOptions><Override>{overridden}</Override></ContentTypeOptions>"
                ));
            }
            if let Some(hsts) = &security.hsts {
                out.push_str(&format!(
                    "<StrictTransportSecurity><Override>{}</Override>",
                    hsts.r#override
                ));
                if let Some(include) = hsts.include_subdomains {
                    out.push_str(&format!(
                        "<IncludeSubdomains>{include}</IncludeSubdomains>"
                    ));
                }
                if let Some(preload) = hsts.preload {
                    out.push_str(&format!("<Preload>{preload}</Preload>"));
                }
                out.push_str(&format!(
                    "<AccessControlMaxAgeSec>{}</AccessControlMaxAgeSec>\
                     </StrictTransportSecurity>",
                    hsts.max_age
                ));
            }
            out.push_str("</SecurityHeadersConfig>");
        }
        if let Some(timing) = &self.server_timing {
            out.push_str(&format!(
                "<ServerTimingHeadersConfig><Enabled>{}</Enabled>",
                timing.enabled
            ));
            if let Some(rate) = timing.sampling_rate {
                out.push_str(&format!("<SamplingRate>{rate}</SamplingRate>"));
            }
            out.push_str("</ServerTimingHeadersConfig>");
        }
        if let Some(headers) = &self.custom_headers {
            out.push_str(&format!(
                "<CustomHeadersConfig><Quantity>{}</Quantity>",
                headers.len()
            ));
            if !headers.is_empty() {
                out.push_str("<Items>");
                for (header, value, overridden) in headers {
                    out.push_str(&format!(
                        "<ResponseHeadersPolicyCustomHeader><Header>{}</Header>\
                         <Value>{}</Value><Override>{overridden}</Override>\
                         </ResponseHeadersPolicyCustomHeader>",
                        escape(header),
                        escape(value)
                    ));
                }
                out.push_str("</Items>");
            }
            out.push_str("</CustomHeadersConfig>");
        }
        if let Some(headers) = &self.remove_headers {
            out.push_str(&format!(
                "<RemoveHeadersConfig><Quantity>{}</Quantity>",
                headers.len()
            ));
            if !headers.is_empty() {
                out.push_str("<Items>");
                for header in headers {
                    out.push_str(&format!(
                        "<ResponseHeadersPolicyRemoveHeader><Header>{}</Header>\
                         </ResponseHeadersPolicyRemoveHeader>",
                        escape(header)
                    ));
                }
                out.push_str("</Items>");
            }
            out.push_str("</RemoveHeadersConfig>");
        }
        out.push_str("</ResponseHeadersPolicyConfig>");
        out
    }
}

/// One CloudFront list wrapper: its `Quantity` and its `Items`, each item in the element
/// the model's `xmlName` gives that list's member.
fn wrapper(name: &str, item: &str, values: &[String]) -> String {
    let mut out = format!("<{name}><Quantity>{}</Quantity><Items>", values.len());
    for value in values {
        out.push_str(&format!("<{item}>{}</{item}>", escape(value)));
    }
    out.push_str(&format!("</Items></{name}>"));
    out
}

/// Escape the five entities XML predefines, so a value may carry any text.
///
/// `pub(crate)` so a sibling suite escapes a submitted value the same way rather than
/// growing a copy of this.
pub(crate) fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Every value `ReferrerPolicyList` admits. Taken from the crate's own model-derived
/// catalog (`ops::types::REFERRER_POLICY_LIST_VALUES`, which
/// `smithy_query(query="shape",shape="ReferrerPolicyList")` reports the same values for)
/// rather than written out here, so the catalog cannot drift from the model.
const REFERRER_POLICIES: &[&str] = REFERRER_POLICY_LIST_VALUES;

/// Every value `FrameOptionsList` admits, from the same model-derived catalog
/// (`ops::types::FRAME_OPTIONS_LIST_VALUES`).
const FRAME_OPTIONS: &[&str] = FRAME_OPTIONS_LIST_VALUES;

/// Every value `ResponseHeadersPolicyAccessControlAllowMethodsValues` admits, from the
/// same model-derived catalog.
const ALLOW_METHODS: &[&str] = RESPONSE_HEADERS_POLICY_ACCESS_CONTROL_ALLOW_METHODS_VALUES_VALUES;

/// A list of owned strings from string literals, for the table below.
fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

/// The configurations every rule below is asserted over.
///
/// The rows span the whole class the rule generalises over: a configuration carrying no
/// optional member at all, each of the five optional configurations alone and in
/// combination, lists of length zero, one and many, both values of each boolean flag,
/// both published bounds of `SamplingRate` (0 and 100), enum values drawn from the
/// model's `FrameOptionsList` and `ReferrerPolicyList`, and text that is empty,
/// non-ASCII or carries XML metacharacters.
///
/// `pub(crate)` so a sibling suite asserts a rule over the SAME rows rather than growing a
/// second table beside this one.
pub(crate) fn policy_table() -> Vec<Policy> {
    vec![
        Policy {
            case: "the name alone: every optional member left out",
            name: "name-only".to_string(),
            ..Policy::default()
        },
        Policy {
            case: "a comment and the custom-headers wrapper carrying no items",
            name: "empty-custom-headers".to_string(),
            comment: Some("no headers added".to_string()),
            custom_headers: Some(Vec::new()),
            ..Policy::default()
        },
        Policy {
            case: "CORS with one origin, one header and one method",
            name: "cors-singletons".to_string(),
            comment: Some(String::new()),
            cors: Some(Cors {
                origins: strings(&["https://shop.example.test"]),
                headers: strings(&["x-requested-with"]),
                methods: strings(&["GET"]),
                credentials: false,
                expose: None,
                max_age: None,
                origin_override: true,
            }),
            ..Policy::default()
        },
        Policy {
            case: "CORS with every allowed method, exposed headers and a max age",
            name: "cors-everything".to_string(),
            comment: Some("the whole CORS surface".to_string()),
            cors: Some(Cors {
                origins: strings(&["https://a.example.test", "https://b.example.test", "*"]),
                headers: strings(&["content-type", "authorization"]),
                methods: strings(ALLOW_METHODS),
                credentials: true,
                expose: Some(strings(&["etag", "x-amz-request-id"])),
                max_age: Some(600),
                origin_override: false,
            }),
            ..Policy::default()
        },
        Policy {
            case: "every security header, FrameOptions DENY",
            name: "security-all".to_string(),
            comment: Some("プロダクション — strict & complete".to_string()),
            security: Some(Security {
                xss: Some(Xss {
                    r#override: true,
                    protection: true,
                    mode_block: Some(true),
                    report_uri: Some("https://reports.example.test/xss?a=1&b=2".to_string()),
                }),
                frame: Some((true, FRAME_OPTIONS[0].to_string())),
                referrer: Some((true, REFERRER_POLICIES[6].to_string())),
                csp: Some((
                    true,
                    "default-src 'self'; script-src 'self' https://cdn.example.test".to_string(),
                )),
                content_type: Some(true),
                hsts: Some(Hsts {
                    r#override: true,
                    include_subdomains: Some(true),
                    preload: Some(true),
                    max_age: 63_072_000,
                }),
            }),
            ..Policy::default()
        },
        Policy {
            case: "the security headers' other values, every optional sub-member left out",
            name: "security-minimal".to_string(),
            security: Some(Security {
                xss: Some(Xss {
                    r#override: false,
                    protection: false,
                    mode_block: None,
                    report_uri: None,
                }),
                frame: Some((false, FRAME_OPTIONS[1].to_string())),
                referrer: Some((false, REFERRER_POLICIES[0].to_string())),
                csp: None,
                content_type: Some(false),
                hsts: Some(Hsts {
                    r#override: false,
                    include_subdomains: None,
                    preload: None,
                    max_age: 0,
                }),
            }),
            ..Policy::default()
        },
        Policy {
            case: "server timing enabled at the upper SamplingRate bound, three custom headers",
            name: "timing-upper-bound".to_string(),
            server_timing: Some(ServerTiming {
                enabled: true,
                sampling_rate: Some(100.0),
            }),
            custom_headers: Some(vec![
                ("x-tenant".to_string(), "acme".to_string(), true),
                (
                    "x-note".to_string(),
                    "a value with <angles> & \"quotes\"".to_string(),
                    false,
                ),
                ("x-empty".to_string(), String::new(), true),
            ]),
            ..Policy::default()
        },
        Policy {
            case: "server timing disabled at the lower SamplingRate bound, removed headers",
            name: "timing-lower-bound".to_string(),
            server_timing: Some(ServerTiming {
                enabled: false,
                sampling_rate: Some(0.0),
            }),
            remove_headers: Some(strings(&["server", "x-powered-by"])),
            ..Policy::default()
        },
        Policy {
            case: "every configuration at once, SamplingRate left out",
            name: "the-lot".to_string(),
            comment: Some("one policy carrying every configuration".to_string()),
            cors: Some(Cors {
                origins: strings(&["https://one.example.test"]),
                headers: strings(&["accept", "accept-language"]),
                methods: strings(&["GET", "HEAD", "OPTIONS"]),
                credentials: false,
                expose: Some(Vec::new()),
                max_age: Some(0),
                origin_override: true,
            }),
            security: Some(Security {
                xss: None,
                frame: Some((true, FRAME_OPTIONS[1].to_string())),
                referrer: Some((false, REFERRER_POLICIES[4].to_string())),
                csp: Some((false, "frame-ancestors 'none'".to_string())),
                content_type: None,
                hsts: None,
            }),
            server_timing: Some(ServerTiming {
                enabled: true,
                sampling_rate: None,
            }),
            custom_headers: Some(vec![(
                "x-edge".to_string(),
                "on".to_string(),
                false,
            )]),
            remove_headers: Some(strings(&["x-internal"])),
        },
    ]
}

// ---------------------------------------------------------------------------
// Reading what the service stored
// ---------------------------------------------------------------------------

/// Whether `value` has the shape of the UUID CloudFront mints as a policy id: five
/// lowercase-hex groups of 8-4-4-4-12 characters.
///
/// `pub(crate)`: every CloudFront policy kind is identified by a UUID in this shape, so a
/// sibling suite checks one the same way rather than growing a copy of this. The shape
/// itself is the harness's [`crate::distribution_lifecycle::is_minted_uuid`] — every
/// resource this service identifies by a UUID carries the same one — so there is ONE
/// predicate behind both names.
pub(crate) fn is_policy_id(value: &str) -> bool {
    is_minted_uuid(value)
}

/// Whether `value` has the shape of a CloudFront version token (`ETag`): `E` followed by
/// 13 uppercase alphanumerics.
fn is_version_token(value: &str) -> bool {
    match value.strip_prefix('E') {
        Some(body) => {
            body.len() == 13
                && body
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase())
        }
        None => false,
    }
}

/// The text at `path` under `node`, or a panic naming the case.
///
/// `pub(crate)` so a sibling suite reads one member of an answer the same way.
pub(crate) fn text(node: &Node, path: &str, case: &str, context: &str, raw: &str) -> String {
    node.text_at(path)
        .unwrap_or_else(|| panic!("case {case} ({context}): the answer carries no {path}: {raw}"))
        .to_string()
}

/// Assert the element at `path` is absent: a member the request left out is not invented
/// by the service.
///
/// `pub(crate)` so a sibling suite asserts the same thing the same way.
pub(crate) fn assert_absent(node: &Node, path: &str, case: &str, context: &str) {
    assert!(
        node.at(path).is_none(),
        "case {case} ({context}): expected no {path} element, got {:?}",
        node.at(path)
    );
}

/// Assert the element at `path` is PRESENT and carries nothing: the empty element a
/// serialized `SecurityHeadersConfig` member occupies when the configuration set no such
/// security header.
///
/// `pub(crate)` so the suite that asserts that rule across every response path reads an
/// empty element the same way.
pub(crate) fn assert_empty_element(node: &Node, path: &str, case: &str, context: &str) {
    let element = node.at(path).unwrap_or_else(|| {
        panic!("case {case} ({context}): the answer carries no {path} element")
    });
    assert!(
        element.text().is_empty() && element.children.is_empty(),
        "case {case} ({context}): expected {path} to be the empty element, got {element:?}"
    );
}

/// Assert one list wrapper reports `values` in order, beside a `Quantity` that states
/// their number.
fn assert_wrapper(
    node: &Node,
    path: &str,
    item: &str,
    values: &[String],
    case: &str,
    context: &str,
    raw: &str,
) {
    let quantity = text(node, &format!("{path}/Quantity"), case, context, raw);
    assert_eq!(
        quantity,
        values.len().to_string(),
        "case {case} ({context}): expected {path}.Quantity {}, got {quantity:?}",
        values.len()
    );
    let items: Vec<String> = node
        .at(&format!("{path}/Items"))
        .map(|items| {
            items
                .children_named(item)
                .into_iter()
                .map(|child| child.text().to_string())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        items, values,
        "case {case} ({context}): expected {path}.Items {values:?}, got {items:?}"
    );
}

/// Assert the `ResponseHeadersPolicyConfig` element `config` is the configuration
/// `policy` submitted — every member it named, and no member it did not.
///
/// `pub(crate)` so a sibling suite reads a configuration back the same way.
pub(crate) fn assert_config(config: &Node, policy: &Policy, context: &str, raw: &str) {
    let case = policy.case;

    let name = text(config, "Name", case, context, raw);
    assert_eq!(
        name, policy.name,
        "case {case} ({context}): expected the submitted Name, got {name:?}"
    );

    match &policy.comment {
        Some(comment) => {
            let actual = text(config, "Comment", case, context, raw);
            assert_eq!(
                &actual, comment,
                "case {case} ({context}): expected the submitted Comment, got {actual:?}"
            );
        }
        None => assert_absent(config, "Comment", case, context),
    }

    match &policy.cors {
        Some(cors) => {
            assert_wrapper(
                config,
                "CorsConfig/AccessControlAllowOrigins",
                "Origin",
                &cors.origins,
                case,
                context,
                raw,
            );
            assert_wrapper(
                config,
                "CorsConfig/AccessControlAllowHeaders",
                "Header",
                &cors.headers,
                case,
                context,
                raw,
            );
            assert_wrapper(
                config,
                "CorsConfig/AccessControlAllowMethods",
                "Method",
                &cors.methods,
                case,
                context,
                raw,
            );
            let credentials = text(
                config,
                "CorsConfig/AccessControlAllowCredentials",
                case,
                context,
                raw,
            );
            assert_eq!(
                credentials,
                cors.credentials.to_string(),
                "case {case} ({context}): expected AccessControlAllowCredentials {}, got \
                 {credentials:?}",
                cors.credentials
            );
            let origin_override = text(config, "CorsConfig/OriginOverride", case, context, raw);
            assert_eq!(
                origin_override,
                cors.origin_override.to_string(),
                "case {case} ({context}): expected OriginOverride {}, got {origin_override:?}",
                cors.origin_override
            );
            match &cors.expose {
                Some(expose) => assert_wrapper(
                    config,
                    "CorsConfig/AccessControlExposeHeaders",
                    "Header",
                    expose,
                    case,
                    context,
                    raw,
                ),
                None => assert_absent(
                    config,
                    "CorsConfig/AccessControlExposeHeaders",
                    case,
                    context,
                ),
            }
            match cors.max_age {
                Some(max_age) => {
                    let actual = text(
                        config,
                        "CorsConfig/AccessControlMaxAgeSec",
                        case,
                        context,
                        raw,
                    );
                    assert_eq!(
                        actual,
                        max_age.to_string(),
                        "case {case} ({context}): expected AccessControlMaxAgeSec {max_age}, got \
                         {actual:?}"
                    );
                }
                None => assert_absent(config, "CorsConfig/AccessControlMaxAgeSec", case, context),
            }
        }
        None => assert_absent(config, "CorsConfig", case, context),
    }

    match &policy.security {
        Some(security) => {
            match &security.xss {
                Some(xss) => {
                    assert_flag(
                        config,
                        "SecurityHeadersConfig/XSSProtection/Override",
                        xss.r#override,
                        case,
                        context,
                        raw,
                    );
                    assert_flag(
                        config,
                        "SecurityHeadersConfig/XSSProtection/Protection",
                        xss.protection,
                        case,
                        context,
                        raw,
                    );
                    match xss.mode_block {
                        Some(mode_block) => assert_flag(
                            config,
                            "SecurityHeadersConfig/XSSProtection/ModeBlock",
                            mode_block,
                            case,
                            context,
                            raw,
                        ),
                        None => assert_absent(
                            config,
                            "SecurityHeadersConfig/XSSProtection/ModeBlock",
                            case,
                            context,
                        ),
                    }
                    match &xss.report_uri {
                        Some(report_uri) => {
                            let actual = text(
                                config,
                                "SecurityHeadersConfig/XSSProtection/ReportUri",
                                case,
                                context,
                                raw,
                            );
                            assert_eq!(
                                &actual, report_uri,
                                "case {case} ({context}): expected the submitted ReportUri, got \
                                 {actual:?}"
                            );
                        }
                        None => assert_absent(
                            config,
                            "SecurityHeadersConfig/XSSProtection/ReportUri",
                            case,
                            context,
                        ),
                    }
                }
                None => assert_empty_element(
                    config,
                    "SecurityHeadersConfig/XSSProtection",
                    case,
                    context,
                ),
            }
            match &security.frame {
                Some((overridden, option)) => {
                    assert_flag(
                        config,
                        "SecurityHeadersConfig/FrameOptions/Override",
                        *overridden,
                        case,
                        context,
                        raw,
                    );
                    let actual = text(
                        config,
                        "SecurityHeadersConfig/FrameOptions/FrameOption",
                        case,
                        context,
                        raw,
                    );
                    assert_eq!(
                        &actual, option,
                        "case {case} ({context}): expected FrameOption {option}, got {actual:?}"
                    );
                }
                None => assert_empty_element(
                    config,
                    "SecurityHeadersConfig/FrameOptions",
                    case,
                    context,
                ),
            }
            match &security.referrer {
                Some((overridden, referrer)) => {
                    assert_flag(
                        config,
                        "SecurityHeadersConfig/ReferrerPolicy/Override",
                        *overridden,
                        case,
                        context,
                        raw,
                    );
                    let actual = text(
                        config,
                        "SecurityHeadersConfig/ReferrerPolicy/ReferrerPolicy",
                        case,
                        context,
                        raw,
                    );
                    assert_eq!(
                        &actual, referrer,
                        "case {case} ({context}): expected ReferrerPolicy {referrer}, got \
                         {actual:?}"
                    );
                }
                None => assert_empty_element(
                    config,
                    "SecurityHeadersConfig/ReferrerPolicy",
                    case,
                    context,
                ),
            }
            match &security.csp {
                Some((overridden, csp)) => {
                    assert_flag(
                        config,
                        "SecurityHeadersConfig/ContentSecurityPolicy/Override",
                        *overridden,
                        case,
                        context,
                        raw,
                    );
                    let actual = text(
                        config,
                        "SecurityHeadersConfig/ContentSecurityPolicy/ContentSecurityPolicy",
                        case,
                        context,
                        raw,
                    );
                    assert_eq!(
                        &actual, csp,
                        "case {case} ({context}): expected the submitted \
                         ContentSecurityPolicy, got {actual:?}"
                    );
                }
                None => assert_empty_element(
                    config,
                    "SecurityHeadersConfig/ContentSecurityPolicy",
                    case,
                    context,
                ),
            }
            match security.content_type {
                Some(overridden) => assert_flag(
                    config,
                    "SecurityHeadersConfig/ContentTypeOptions/Override",
                    overridden,
                    case,
                    context,
                    raw,
                ),
                None => assert_empty_element(
                    config,
                    "SecurityHeadersConfig/ContentTypeOptions",
                    case,
                    context,
                ),
            }
            match &security.hsts {
                Some(hsts) => {
                    assert_flag(
                        config,
                        "SecurityHeadersConfig/StrictTransportSecurity/Override",
                        hsts.r#override,
                        case,
                        context,
                        raw,
                    );
                    let max_age = text(
                        config,
                        "SecurityHeadersConfig/StrictTransportSecurity/AccessControlMaxAgeSec",
                        case,
                        context,
                        raw,
                    );
                    assert_eq!(
                        max_age,
                        hsts.max_age.to_string(),
                        "case {case} ({context}): expected the HSTS AccessControlMaxAgeSec {}, \
                         got {max_age:?}",
                        hsts.max_age
                    );
                    match hsts.include_subdomains {
                        Some(include) => assert_flag(
                            config,
                            "SecurityHeadersConfig/StrictTransportSecurity/IncludeSubdomains",
                            include,
                            case,
                            context,
                            raw,
                        ),
                        None => assert_absent(
                            config,
                            "SecurityHeadersConfig/StrictTransportSecurity/IncludeSubdomains",
                            case,
                            context,
                        ),
                    }
                    match hsts.preload {
                        Some(preload) => assert_flag(
                            config,
                            "SecurityHeadersConfig/StrictTransportSecurity/Preload",
                            preload,
                            case,
                            context,
                            raw,
                        ),
                        None => assert_absent(
                            config,
                            "SecurityHeadersConfig/StrictTransportSecurity/Preload",
                            case,
                            context,
                        ),
                    }
                }
                None => assert_empty_element(
                    config,
                    "SecurityHeadersConfig/StrictTransportSecurity",
                    case,
                    context,
                ),
            }
        }
        None => assert_absent(config, "SecurityHeadersConfig", case, context),
    }

    match &policy.server_timing {
        Some(timing) => {
            assert_flag(
                config,
                "ServerTimingHeadersConfig/Enabled",
                timing.enabled,
                case,
                context,
                raw,
            );
            match timing.sampling_rate {
                Some(rate) => {
                    let actual = text(
                        config,
                        "ServerTimingHeadersConfig/SamplingRate",
                        case,
                        context,
                        raw,
                    );
                    let parsed: f64 = actual.parse().unwrap_or_else(|_| {
                        panic!(
                            "case {case} ({context}): expected a numeric SamplingRate, got \
                             {actual:?}"
                        )
                    });
                    assert!(
                        (parsed - rate).abs() < f64::EPSILON,
                        "case {case} ({context}): expected SamplingRate {rate}, got {parsed}"
                    );
                }
                None => assert_absent(
                    config,
                    "ServerTimingHeadersConfig/SamplingRate",
                    case,
                    context,
                ),
            }
        }
        None => assert_absent(config, "ServerTimingHeadersConfig", case, context),
    }

    match &policy.custom_headers {
        Some(headers) => {
            let quantity = text(config, "CustomHeadersConfig/Quantity", case, context, raw);
            assert_eq!(
                quantity,
                headers.len().to_string(),
                "case {case} ({context}): expected CustomHeadersConfig.Quantity {}, got \
                 {quantity:?}",
                headers.len()
            );
            let items: Vec<(String, String, String)> = config
                .at("CustomHeadersConfig/Items")
                .map(|items| {
                    items
                        .children_named("ResponseHeadersPolicyCustomHeader")
                        .into_iter()
                        .map(|item| {
                            (
                                item.text_at("Header").unwrap_or_default().to_string(),
                                item.text_at("Value").unwrap_or_default().to_string(),
                                item.text_at("Override").unwrap_or_default().to_string(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            let expected: Vec<(String, String, String)> = headers
                .iter()
                .map(|(header, value, overridden)| {
                    (header.clone(), value.clone(), overridden.to_string())
                })
                .collect();
            assert_eq!(
                items, expected,
                "case {case} ({context}): expected the submitted custom headers {expected:?}, \
                 got {items:?}"
            );
        }
        None => assert_absent(config, "CustomHeadersConfig", case, context),
    }

    match &policy.remove_headers {
        Some(headers) => {
            let quantity = text(config, "RemoveHeadersConfig/Quantity", case, context, raw);
            assert_eq!(
                quantity,
                headers.len().to_string(),
                "case {case} ({context}): expected RemoveHeadersConfig.Quantity {}, got \
                 {quantity:?}",
                headers.len()
            );
            let items: Vec<String> = config
                .at("RemoveHeadersConfig/Items")
                .map(|items| {
                    items
                        .children_named("ResponseHeadersPolicyRemoveHeader")
                        .into_iter()
                        .map(|item| item.text_at("Header").unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                &items, headers,
                "case {case} ({context}): expected the submitted removed headers {headers:?}, \
                 got {items:?}"
            );
        }
        None => assert_absent(config, "RemoveHeadersConfig", case, context),
    }
}

/// Assert one boolean member reports `expected`.
fn assert_flag(node: &Node, path: &str, expected: bool, case: &str, context: &str, raw: &str) {
    let actual = text(node, path, case, context, raw);
    assert_eq!(
        actual,
        expected.to_string(),
        "case {case} ({context}): expected {path} {expected}, got {actual:?}"
    );
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// A create PERSISTS the policy it mints, and the read answers from that one record: the
/// configuration exactly as submitted (down to the members the request left out), a UUID
/// id, and the version token the create handed back. Across the table, no two policies
/// share an id or a version token, and each one's `Location` names its own URI.
#[tokio::test]
async fn create_persists_the_policy_the_get_then_reports() {
    let router = emulator();
    let mut ids = BTreeSet::new();
    let mut etags = BTreeSet::new();

    for policy in policy_table() {
        let case = policy.case;
        let created = create_policy(&router, &policy.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            created.status, created.raw
        );

        let document = created.root();
        let id = text(document, "Id", case, "the create", &created.raw);
        assert!(
            is_minted_uuid(&id),
            "case {case}: expected a UUID Id, got {id:?}"
        );
        let last_modified = text(
            document,
            "LastModifiedTime",
            case,
            "the create",
            &created.raw,
        );
        assert!(
            !last_modified.is_empty(),
            "case {case}: expected a LastModifiedTime, got {last_modified:?}"
        );
        let config = document
            .at("ResponseHeadersPolicyConfig")
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: the create carries no ResponseHeadersPolicyConfig: {}",
                    created.raw
                )
            });
        assert_config(config, &policy, "the create", &created.raw);

        let etag = created.header("etag").to_string();
        assert!(
            is_version_token(&etag),
            "case {case}: expected a version token in the ETag header, got {etag:?}"
        );
        let location = created.header("location").to_string();
        assert_eq!(
            location,
            format!("https://{HOST}/{API_VERSION}/response-headers-policy/{id}"),
            "case {case}: expected the new policy's URI in Location, got {location:?}"
        );
        assert!(ids.insert(id.clone()), "case {case}: the id {id} repeated");
        assert!(
            etags.insert(etag.clone()),
            "case {case}: the version token {etag} repeated"
        );

        // The read answers from the stored record, not from any request.
        let fetched = get_policy(&router, &id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected HTTP 200 from the get, got {} with {}",
            fetched.status, fetched.raw
        );
        let fetched_document = fetched.root();
        assert_eq!(
            fetched_document.text_at("Id"),
            Some(id.as_str()),
            "case {case}: expected the get to report the created Id {id}, got {}",
            fetched.raw
        );
        assert_eq!(
            fetched_document.text_at("LastModifiedTime"),
            Some(last_modified.as_str()),
            "case {case}: expected the stored LastModifiedTime {last_modified}, got {}",
            fetched.raw
        );
        let fetched_config = fetched_document
            .at("ResponseHeadersPolicyConfig")
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: the get carries no ResponseHeadersPolicyConfig: {}",
                    fetched.raw
                )
            });
        assert_config(fetched_config, &policy, "the get", &fetched.raw);
        assert_eq!(
            fetched.header("etag"),
            etag,
            "case {case}: expected the create's version token from the get, got {:?}",
            fetched.header("etag")
        );
    }
}

/// "The name must be unique for response headers policies in this Amazon Web Services
/// account": the second create under a name is refused with
/// `ResponseHeadersPolicyAlreadyExists` (HTTP 409), and the policy that holds the name
/// keeps the identity, the configuration and the version token its own create minted.
#[tokio::test]
async fn a_name_names_at_most_one_response_headers_policy() {
    let router = emulator();

    for policy in policy_table() {
        let case = policy.case;
        let first = create_policy(&router, &policy.xml()).await;
        assert_eq!(
            first.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            first.status, first.raw
        );
        let id = text(first.root(), "Id", case, "the first create", &first.raw);
        let etag = first.header("etag").to_string();

        // The same name again, carrying a configuration that DIFFERS, so a policy that
        // was replaced rather than refused would be visible in the read below.
        let repeat = Policy {
            comment: Some("a second create under a name already taken".to_string()),
            cors: None,
            security: None,
            server_timing: None,
            custom_headers: Some(vec![(
                "x-second".to_string(),
                "yes".to_string(),
                true,
            )]),
            remove_headers: None,
            ..policy.clone()
        };
        let second = create_policy(&router, &repeat.xml()).await;
        assert_eq!(
            second.status, 409,
            "case {case}: expected HTTP 409 for a repeated name, got {} with {}",
            second.status, second.raw
        );
        assert_eq!(
            second.error_code(),
            "ResponseHeadersPolicyAlreadyExists",
            "case {case}: expected ResponseHeadersPolicyAlreadyExists, got {}",
            second.raw
        );

        let fetched = get_policy(&router, &id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected the first policy to survive the refusal, got {} with {}",
            fetched.status, fetched.raw
        );
        let config = fetched
            .root()
            .at("ResponseHeadersPolicyConfig")
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: the get carries no ResponseHeadersPolicyConfig: {}",
                    fetched.raw
                )
            });
        assert_config(config, &policy, "after a refusal", &fetched.raw);
        assert_eq!(
            fetched.header("etag"),
            etag,
            "case {case}: the refused create must not have spent a version token"
        );
    }
}

/// Every value the model's `FrameOptionsList` and `ReferrerPolicyList` admit round-trips:
/// the create stores it and the get reports it back, including the values no recorded
/// request named.
#[tokio::test]
async fn every_security_header_enum_value_round_trips() {
    let router = emulator();

    for (index, referrer) in REFERRER_POLICIES.iter().enumerate() {
        // Pair each referrer policy with one of the two frame options, so both values of
        // `FrameOptionsList` are exercised across the catalog.
        let frame = FRAME_OPTIONS[index % FRAME_OPTIONS.len()];
        let policy = Policy {
            case: "a modelled security-header enum value",
            name: format!("enum-{referrer}-{frame}"),
            security: Some(Security {
                frame: Some((index % 2 == 0, frame.to_string())),
                referrer: Some((index % 2 == 1, (*referrer).to_string())),
                ..Security::default()
            }),
            ..Policy::default()
        };

        let created = create_policy(&router, &policy.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {referrer}/{frame}: expected HTTP 201, got {} with {}",
            created.status, created.raw
        );
        let id = text(created.root(), "Id", referrer, "the create", &created.raw);

        let fetched = get_policy(&router, &id).await;
        assert_eq!(
            fetched.status, 200,
            "case {referrer}/{frame}: expected HTTP 200 from the get, got {} with {}",
            fetched.status, fetched.raw
        );
        let config = fetched
            .root()
            .at("ResponseHeadersPolicyConfig")
            .unwrap_or_else(|| {
                panic!(
                    "case {referrer}/{frame}: the get carries no ResponseHeadersPolicyConfig: {}",
                    fetched.raw
                )
            });
        assert_config(config, &policy, "the get", &fetched.raw);
    }
}

/// Every CloudFront list travels in a wrapper that states its own length, and a request
/// whose `Quantity` disagrees with its `Items` is refused with `InconsistentQuantities`
/// (HTTP 400) — for EVERY wrapper a `ResponseHeadersPolicyConfig` reaches, and without
/// storing anything: the name is still free afterwards.
#[tokio::test]
async fn a_wrapper_whose_quantity_disagrees_with_its_items_is_refused() {
    let router = emulator();

    // One row per wrapper, with the disagreement in both directions: a `Quantity` over
    // the number of items and one under it.
    let rows: [(&str, &str); 6] = [
        (
            "CorsConfig.AccessControlAllowOrigins overstated",
            "<CorsConfig>\
             <AccessControlAllowOrigins><Quantity>2</Quantity>\
             <Items><Origin>https://one.example.test</Origin></Items>\
             </AccessControlAllowOrigins>\
             <AccessControlAllowHeaders><Quantity>1</Quantity>\
             <Items><Header>accept</Header></Items></AccessControlAllowHeaders>\
             <AccessControlAllowMethods><Quantity>1</Quantity>\
             <Items><Method>GET</Method></Items></AccessControlAllowMethods>\
             <AccessControlAllowCredentials>false</AccessControlAllowCredentials>\
             <OriginOverride>false</OriginOverride></CorsConfig>",
        ),
        (
            "CorsConfig.AccessControlAllowHeaders understated",
            "<CorsConfig>\
             <AccessControlAllowOrigins><Quantity>1</Quantity>\
             <Items><Origin>https://one.example.test</Origin></Items>\
             </AccessControlAllowOrigins>\
             <AccessControlAllowHeaders><Quantity>1</Quantity>\
             <Items><Header>accept</Header><Header>authorization</Header></Items>\
             </AccessControlAllowHeaders>\
             <AccessControlAllowMethods><Quantity>1</Quantity>\
             <Items><Method>GET</Method></Items></AccessControlAllowMethods>\
             <AccessControlAllowCredentials>true</AccessControlAllowCredentials>\
             <OriginOverride>true</OriginOverride></CorsConfig>",
        ),
        (
            "CorsConfig.AccessControlAllowMethods overstated",
            "<CorsConfig>\
             <AccessControlAllowOrigins><Quantity>1</Quantity>\
             <Items><Origin>https://one.example.test</Origin></Items>\
             </AccessControlAllowOrigins>\
             <AccessControlAllowHeaders><Quantity>1</Quantity>\
             <Items><Header>accept</Header></Items></AccessControlAllowHeaders>\
             <AccessControlAllowMethods><Quantity>3</Quantity>\
             <Items><Method>GET</Method><Method>HEAD</Method></Items>\
             </AccessControlAllowMethods>\
             <AccessControlAllowCredentials>false</AccessControlAllowCredentials>\
             <OriginOverride>false</OriginOverride></CorsConfig>",
        ),
        (
            "CorsConfig.AccessControlExposeHeaders overstated",
            "<CorsConfig>\
             <AccessControlAllowOrigins><Quantity>1</Quantity>\
             <Items><Origin>https://one.example.test</Origin></Items>\
             </AccessControlAllowOrigins>\
             <AccessControlAllowHeaders><Quantity>1</Quantity>\
             <Items><Header>accept</Header></Items></AccessControlAllowHeaders>\
             <AccessControlAllowMethods><Quantity>1</Quantity>\
             <Items><Method>GET</Method></Items></AccessControlAllowMethods>\
             <AccessControlAllowCredentials>false</AccessControlAllowCredentials>\
             <AccessControlExposeHeaders><Quantity>1</Quantity></AccessControlExposeHeaders>\
             <OriginOverride>false</OriginOverride></CorsConfig>",
        ),
        (
            "CustomHeadersConfig understated",
            "<CustomHeadersConfig><Quantity>1</Quantity><Items>\
             <ResponseHeadersPolicyCustomHeader><Header>x-one</Header><Value>1</Value>\
             <Override>true</Override></ResponseHeadersPolicyCustomHeader>\
             <ResponseHeadersPolicyCustomHeader><Header>x-two</Header><Value>2</Value>\
             <Override>false</Override></ResponseHeadersPolicyCustomHeader>\
             </Items></CustomHeadersConfig>",
        ),
        (
            "RemoveHeadersConfig overstated",
            "<RemoveHeadersConfig><Quantity>4</Quantity><Items>\
             <ResponseHeadersPolicyRemoveHeader><Header>server</Header>\
             </ResponseHeadersPolicyRemoveHeader></Items></RemoveHeadersConfig>",
        ),
    ];

    for (case, fragment) in rows {
        let name = format!("mismatch-{}", case.replace(' ', "-"));
        let body = format!(
            "<ResponseHeadersPolicyConfig \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Name>{name}</Name>{fragment}</ResponseHeadersPolicyConfig>"
        );

        let answer = create_policy(&router, &body).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InconsistentQuantities",
            "case {case}: expected InconsistentQuantities, got {}",
            answer.raw
        );

        // Nothing was stored: the name the refused create named is still free, so a
        // well-formed create under it succeeds.
        let repaired = Policy {
            case,
            name: name.clone(),
            ..Policy::default()
        };
        let accepted = create_policy(&router, &repaired.xml()).await;
        assert_eq!(
            accepted.status, 201,
            "case {case}: the refused create must store nothing, got {} with {}",
            accepted.status, accepted.raw
        );
    }
}

/// An id no policy holds is `NoSuchResponseHeadersPolicy` (HTTP 404), whatever shape the
/// id takes — and a create under one id leaves every other id unfound.
#[tokio::test]
async fn get_refuses_an_id_no_policy_holds() {
    let router = emulator();
    let present = Policy {
        case: "the one policy that exists",
        name: "present-policy".to_string(),
        ..Policy::default()
    };
    let created = create_policy(&router, &present.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201, got {} with {}",
        present.case, created.status, created.raw
    );
    let id = text(created.root(), "Id", present.case, "the create", &created.raw);

    // Rows: a UUID never minted, the existing id with its last character changed, the
    // existing id in upper case, a non-UUID token and a managed-policy-shaped name.
    let mut flipped: Vec<char> = id.chars().collect();
    let last = flipped.len() - 1;
    flipped[last] = if flipped[last] == 'a' { 'b' } else { 'a' };
    let absent: [(&str, String); 5] = [
        (
            "a UUID never minted",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "the existing id with one character changed",
            flipped.into_iter().collect(),
        ),
        ("the existing id in upper case", id.to_uppercase()),
        ("a token that is no UUID", "not-a-policy-id".to_string()),
        ("a policy name rather than an id", present.name.clone()),
    ];

    for (case, candidate) in absent {
        let answer = get_policy(&router, &candidate).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404 for {candidate:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchResponseHeadersPolicy",
            "case {case}: expected NoSuchResponseHeadersPolicy for {candidate:?}, got {}",
            answer.raw
        );
    }

    // The policy that does exist is still readable, unchanged by the refusals.
    let fetched = get_policy(&router, &id).await;
    assert_eq!(
        fetched.status, 200,
        "case the existing policy: expected HTTP 200, got {} with {}",
        fetched.status, fetched.raw
    );
}

/// `SamplingRate` is `double, range 0..100` (from `smithy_query` on
/// `ResponseHeadersPolicyServerTimingHeadersConfig`): both published bounds are accepted
/// and stored, and a value past either bound is refused before anything is stored.
#[tokio::test]
async fn a_sampling_rate_outside_the_modelled_range_is_refused() {
    let router = emulator();

    for (case, rate) in [("the lower bound", 0.0), ("the upper bound", 100.0), ("a rate inside the range", 12.5)] {
        let policy = Policy {
            case,
            name: format!("sampling-{rate}"),
            server_timing: Some(ServerTiming {
                enabled: true,
                sampling_rate: Some(rate),
            }),
            ..Policy::default()
        };
        let created = create_policy(&router, &policy.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {case} ({rate}): expected HTTP 201, got {} with {}",
            created.status, created.raw
        );
        let id = text(created.root(), "Id", case, "the create", &created.raw);
        let fetched = get_policy(&router, &id).await;
        let config = fetched
            .root()
            .at("ResponseHeadersPolicyConfig")
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: the get carries no ResponseHeadersPolicyConfig: {}",
                    fetched.raw
                )
            });
        assert_config(config, &policy, "the get", &fetched.raw);
    }

    for (case, rate) in [
        ("one past the upper bound", 100.5),
        ("one past the lower bound", -0.5),
    ] {
        let name = format!("sampling-out-of-range-{rate}");
        let policy = Policy {
            case,
            name: name.clone(),
            server_timing: Some(ServerTiming {
                enabled: true,
                sampling_rate: Some(rate),
            }),
            ..Policy::default()
        };
        let answer = create_policy(&router, &policy.xml()).await;
        assert_eq!(
            answer.status, 400,
            "case {case} ({rate}): expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );

        // Nothing was stored under the refused name.
        let repaired = Policy {
            case,
            name,
            ..Policy::default()
        };
        let accepted = create_policy(&router, &repaired.xml()).await;
        assert_eq!(
            accepted.status, 201,
            "case {case} ({rate}): the refused create must store nothing, got {} with {}",
            accepted.status, accepted.raw
        );
    }
}

/// The store's bound on how many policies it holds is an entry count, not a per-request
/// one: the table's creates all land well inside [`MAX_RESPONSE_HEADERS_POLICIES`], so
/// each of them is accepted and readable.
#[tokio::test]
async fn creates_inside_the_store_bound_are_all_accepted() {
    let router = emulator();
    let table = policy_table();
    assert!(
        table.len() < MAX_RESPONSE_HEADERS_POLICIES,
        "case the store bound: the table must sit inside the bound {MAX_RESPONSE_HEADERS_POLICIES}, \
         got {} rows",
        table.len()
    );

    let mut ids = Vec::new();
    for policy in &table {
        let created = create_policy(&router, &policy.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {}: expected HTTP 201, got {} with {}",
            policy.case, created.status, created.raw
        );
        ids.push(text(
            created.root(),
            "Id",
            policy.case,
            "the create",
            &created.raw,
        ));
    }

    // Every policy the run created is still readable: a create never evicts another.
    for (policy, id) in table.iter().zip(&ids) {
        let fetched = get_policy(&router, id).await;
        assert_eq!(
            fetched.status, 200,
            "case {}: expected HTTP 200 for {id}, got {} with {}",
            policy.case, fetched.status, fetched.raw
        );
    }
}
