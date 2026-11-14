// SPDX-License-Identifier: CC0-1.0

//! The authentication and scan RESULT headers Amazon SES stamps on a received message when the
//! receipt rule that applies to it asks for the scan — the state of the message the rule's own
//! actions then act on.
//!
//! `ReceiptRule$ScanEnabled` is what asks: "If true, then messages that this receipt rule
//! applies to are scanned for spam and viruses." So a rule with the scan on has Amazon SES
//! report, ON the message, what it found — the result of the sender authentication it ran
//! (`Received-SPF` of RFC 7208 §9.1 and `Authentication-Results` of RFC 7601, under Amazon
//! SES's own authentication-service identifier) and the verdict of each scan
//! ([`SPAM_VERDICT_HEADER`], [`VIRUS_VERDICT_HEADER`]) — and a rule with it off has the message
//! saved exactly as it arrived.
//!
//! Every value here is READ off the message and off what this account holds, never asserted:
//!
//! * `spf=` is `pass` because the host that submitted the message IS this deployment's own
//!   sending host: the message was handed to Amazon SES by the account that owns the identity it
//!   is from, and relayed by Amazon SES under the envelope sender the identity's MAIL FROM
//!   setting gives it, so the domain of that envelope sender does designate the submitting host.
//!   No client address is reported, because no connection carried the message. The result is a
//!   fact about the SUBMITTING HOST rather than about the sender's own domain, which is why it
//!   does not vary: a message reaches this module only once this account has accepted it from a
//!   sending identity it holds, and Amazon SES is the host that relayed it either way.
//! * `dkim=` is the outcome of CHECKING the message's own `DKIM-Signature` against the key its
//!   signing identity holds ([`super::dkim::outcome`]), so a message whose signed content
//!   changed does not pass and one signed by a domain this account holds no key for reports
//!   that the key could not be found.
//! * `dmarc=` is the alignment rule of RFC 7489 §3.1: it passes when an authentication that
//!   passed is for a domain aligned with the one the `From` field claims.
//! * each VERDICT is the outcome of a scan of the message's own content — its raw bytes and the
//!   decoded content of each of its parts — for the published test signature of that scan
//!   ([`SPAM_TEST_PATTERN`], [`VIRUS_TEST_SIGNATURE`]): a message carrying one is reported as
//!   the thing that signature stands for, and a message carrying neither passes.

/// `Received-SPF`, the field RFC 7208 §9.1 reports an SPF check in.
pub const RECEIVED_SPF_HEADER: &str = "Received-SPF";

/// `Authentication-Results`, the field RFC 7601 §2 reports every authentication check in.
pub const AUTHENTICATION_RESULTS_HEADER: &str = "Authentication-Results";

/// `X-SES-Spam-Verdict`, the field Amazon SES reports the spam scan's verdict in.
pub const SPAM_VERDICT_HEADER: &str = "X-SES-Spam-Verdict";

/// `X-SES-Virus-Verdict`, the field Amazon SES reports the virus scan's verdict in.
pub const VIRUS_VERDICT_HEADER: &str = "X-SES-Virus-Verdict";

/// The authentication-service identifier Amazon SES reports its checks under — the `authserv-id`
/// of RFC 7601 §2.2, which names the receiver that made them.
pub const AUTHENTICATION_SERVICE_ID: &str = "amazonses.com";

/// The verdict of a scan that found nothing.
pub const VERDICT_PASS: &str = "PASS";

/// The verdict of a scan that found what it scans for.
pub const VERDICT_FAIL: &str = "FAIL";

/// The published spam test pattern — the Generic Test for Unsolicited Bulk Email, the one
/// pattern a spam filter is expected to report on so that reporting can be tested — which is
/// what the spam scan here looks for.
pub const SPAM_TEST_PATTERN: &str = "XJS*C4JDBQADN1.NSBN3*2IDNEN*GTUBE-STANDARD-ANTI-UBE-TEST-EMAIL*C.34X";

/// The published virus test signature — the EICAR anti-malware test file, the one content an
/// anti-virus scanner is expected to report on so that reporting can be tested — which is what
/// the virus scan here looks for.
pub const VIRUS_TEST_SIGNATURE: &str =
    r"X5O!P%@AP[4\PZX54(P^)7CC)7}$EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*";

/// How many parts of ONE message are scanned.
///
/// An EMULATOR ceiling: a message is a tree a sender wrote, so the scan walks a bounded number
/// of its parts. A message's own structure has a handful of parts, so no message a client really
/// sends reaches it.
const MAX_SCANNED_PARTS: usize = 64;

/// How many decoded bytes ONE message's parts are scanned over, across the whole walk.
///
/// An EMULATOR ceiling on the PRODUCT rather than on either factor: a container's own body is
/// the whole sub-message under it, so walking a tree decodes more bytes than the message
/// carries, and the part count alone would leave that product to the nesting a caller chose.
/// One message's worth of bytes ([`super::MAX_MESSAGE_BYTES`]) is far more than any message's
/// own parts decode to, since a part is decoded once and is smaller than the message holding it.
const MAX_SCANNED_BYTES: usize = super::MAX_MESSAGE_BYTES;

/// The result headers a scanning receipt rule has Amazon SES stamp on the message it applies
/// to, in the order they are added to the header block.
///
/// `envelope_from` is the envelope sender Amazon SES relayed the message under, and `from_domain`
/// the domain the message's `From` field claims — the two the authentication results are about.
#[must_use]
pub fn result_headers(
    state: &super::ComAmazonawsSesSimpleemailserviceState,
    raw: &[u8],
    envelope_from: &str,
    from_domain: &str,
) -> Vec<(String, String)> {
    let envelope_domain = super::identity_domain(super::addr_spec(envelope_from));
    let spf = spf_check(&envelope_domain.to_ascii_lowercase(), envelope_from);
    let dkim = super::dkim::outcome(state, raw);
    let dkim_domain = dkim.domain.clone().unwrap_or_default();
    let mut results = format!("{AUTHENTICATION_SERVICE_ID}; spf=pass {spf}");
    results.push_str(&format!("; dkim={}", dkim.result.as_wire()));
    if !dkim_domain.is_empty() {
        results.push_str(&format!(" header.i=@{dkim_domain}"));
    }
    // RFC 7489 §3.1: an identifier passes DMARC when an authentication that passed is for a
    // domain aligned with the `From` domain. SPF is about the envelope sender's domain and DKIM
    // about the signature's.
    let aligned_spf = aligned(envelope_domain, from_domain);
    let aligned_dkim =
        dkim.result == super::dkim::DkimResult::Pass && aligned(&dkim_domain, from_domain);
    let dmarc = if aligned_spf || aligned_dkim {
        "pass"
    } else {
        "fail"
    };
    results.push_str(&format!("; dmarc={dmarc} header.from={from_domain};"));
    vec![
        (RECEIVED_SPF_HEADER.to_string(), format!("pass {spf};")),
        (AUTHENTICATION_RESULTS_HEADER.to_string(), results),
        (
            SPAM_VERDICT_HEADER.to_string(),
            verdict(carries(raw, SPAM_TEST_PATTERN)).to_string(),
        ),
        (
            VIRUS_VERDICT_HEADER.to_string(),
            verdict(carries(raw, VIRUS_TEST_SIGNATURE)).to_string(),
        ),
    ]
}

/// The SPF check as both fields report it: the comment that says what was checked, and the
/// envelope sender it was checked for.
///
/// Written with no separator of its own, so the field each reports it in states its own: a
/// `Received-SPF` ends the result it carries, and an `Authentication-Results` separates the
/// methods it carries from one another (RFC 7601 §2.2).
///
/// The optional `client-ip` and `helo` of RFC 7208 §9.1 are left out: no connection carried
/// this message, so there is no address or EHLO name to report.
fn spf_check(envelope_domain: &str, envelope_from: &str) -> String {
    format!(
        "(spfCheck: domain of {envelope_domain} designates the Amazon SES sending host as \
         permitted sender) envelope-from={envelope_from}"
    )
}

/// Whether `authenticated` is aligned with the domain the `From` field claims: the same domain,
/// or one under it, which is the relaxed alignment of RFC 7489 §3.1.1 — a subdomain
/// authenticates for the organizational domain above it.
fn aligned(authenticated: &str, from_domain: &str) -> bool {
    !authenticated.is_empty()
        && (authenticated.eq_ignore_ascii_case(from_domain)
            || super::is_subdomain_of(authenticated, from_domain)
            || super::is_subdomain_of(from_domain, authenticated))
}

/// The verdict of a scan that did or did not find what it scans for.
fn verdict(found: bool) -> &'static str {
    if found {
        VERDICT_FAIL
    } else {
        VERDICT_PASS
    }
}

/// Whether the message `raw` carries `signature` — in its own bytes, or in the decoded content
/// of any of its parts, which is where a scanner looks for it.
///
/// The search is the maintained linear-time one over bytes, so scanning one span of bytes costs
/// the sum of the lengths rather than their product.
///
/// The message's PARTS are only looked at for a message a parser may be handed at all
/// ([`super::mime_bounds::within_parse_bounds`]): a MIME tree is built by descending once per
/// nesting level, and the nesting is the sender's to choose, so a message declaring more
/// containers than that descent may take is scanned on its own bytes and its parts are left
/// alone. Nothing refuses such a message — it was ACCEPTED before it was received — and the walk
/// that does run is bounded both in the parts it visits ([`MAX_SCANNED_PARTS`]) and in the bytes
/// it decodes across all of them ([`MAX_SCANNED_BYTES`]).
fn carries(raw: &[u8], signature: &str) -> bool {
    let signature = signature.as_bytes();
    if ::memchr::memmem::find(raw, signature).is_some() {
        return true;
    }
    if !super::mime_bounds::within_parse_bounds(raw) {
        ::tracing::warn!(
            bytes = raw.len(),
            "a received message past the tree this emulator reads is scanned on its own bytes \
             and its parts are not decoded"
        );
        return false;
    }
    let Ok(parsed) = ::mailparse::parse_mail(raw) else {
        return false;
    };
    let mut parts = vec![&parsed];
    let mut scanned = 0;
    let mut decoded = 0usize;
    while let Some(part) = parts.pop() {
        scanned += 1;
        if scanned > MAX_SCANNED_PARTS || decoded > MAX_SCANNED_BYTES {
            return false;
        }
        if let Ok(body) = part.get_body_raw() {
            decoded = decoded.saturating_add(body.len());
            if ::memchr::memmem::find(&body, signature).is_some() {
                return true;
            }
        }
        parts.extend(part.subparts.iter());
    }
    false
}
