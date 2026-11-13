// SPDX-License-Identifier: CC0-1.0

//! The DOMAIN an app's hosting is reported under, and the host one branch's site answers
//! on.
//!
//! `App$defaultDomain` is "the default domain for the Amplify app"
//! (`smithy_query(query="doc",shape="App",member="defaultDomain")`) and
//! `Branch$displayName` "is used as the default domain prefix"
//! (`smithy_query(query="doc",shape="Branch",member="displayName")`), so a client that was
//! told both composes `https://<displayName>.<defaultDomain>/` — the deployed page, the
//! branch's basic-authorization challenge, or the redirect a rule states.
//!
//! That domain is a property OF THE APP and of nothing else: it is the app's own id under
//! the one zone Amplify hosts apps on ([`amplify_zone_default_domain`]), so it is a DNS
//! name whose first label is the minted app id, it is the same string for the same app on
//! every read, and no part of the request that created the app — its host, its endpoint, or
//! the port this process happens to be served on — reaches it. The emulator's own endpoint
//! is where a client SENDS a request; what a record reports is what the service publishes
//! for the app, and a reported value that moved with the connection would differ between
//! two reads of one app.
//!
//! The same two members decide which requests the edge serves, so the host a branch answers
//! on is composed HERE too ([`hosted_branch_host`]), from the stored record, and compared
//! against the host a request addressed in one normal form ([`hosted_site_host`]): a client
//! that was told an app's domain reaches the site by addressing that host — on this
//! emulator's endpoint, the one place it answers — and a host is the same host however its
//! port, its userinfo, its trailing root dot or its case are written.

/// The default domain an app of this id carries under Amplify's OWN hosting zone: the value
/// the shape `App$defaultDomain` documents, derived from the app's minted id alone and
/// stored once when the app is created, so every later read reports the same string.
pub fn amplify_zone_default_domain(app_id: &str) -> String {
    format!("{app_id}.{AMPLIFY_HOSTING_ZONE}")
}

/// The zone Amplify serves an app's default domain under.
const AMPLIFY_HOSTING_ZONE: &str = "amplifyapp.com";

/// The host one branch's site answers at, as a name to compare an addressed host against:
/// the branch's display name, then the default domain of its app, in the one normal form
/// [`hosted_site_host`] reads a host in.
pub fn hosted_branch_host(display_name: &str, default_domain: &str) -> String {
    hosted_site_host(&format!("{display_name}.{default_domain}"))
}

/// A host in the one normal form this edge compares in: no userinfo, no port, no trailing
/// root dot, lowercased — the four ways the same host is written.
///
/// The port is dropped on BOTH sides, so an announced domain that carries this emulator's
/// port is matched by the request that dialled it, and a host is still the same host when
/// a client writes the default port for its scheme or leaves it out.
pub fn hosted_site_host(host: &str) -> String {
    ::service_common::data_plane_addressing::host_only(host)
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_apps_domain_is_its_own_id_under_the_hosting_zone() {
        // The rows span the shapes a minted app id takes: the modelled `d`-and-suffix
        // form at both ends of its length, and one whose suffix is all digits.
        let cases: &[&str] = &["d1", "dexamplezone", "d2k7yq1ab3cd4ef5gh6i", "d1234567890"];
        for app_id in cases {
            let domain = amplify_zone_default_domain(app_id);
            assert_eq!(
                domain,
                format!("{app_id}.amplifyapp.com"),
                "case {app_id:?}: expected the app's id under the hosting zone, got {domain:?}"
            );
        }
    }

    #[test]
    fn a_branchs_host_and_an_addressed_host_read_the_same() {
        // The rows span the ways the same host is written: the app's domain dialled
        // plainly, with a port, rooted, in mixed case and carrying userinfo.
        let cases: &[(&str, &str, &str, &str)] = &[
            ("the app's own domain", "main", "d1example.amplifyapp.com", "main.d1example.amplifyapp.com"),
            ("the same domain dialled with a port", "main", "d1example.amplifyapp.com", "main.d1example.amplifyapp.com:44285"),
            ("a display name in mixed case", "Main-Two", "d1example.amplifyapp.com", "MAIN-TWO.d1example.amplifyapp.com"),
            ("a rooted host", "main", "d1example.amplifyapp.com", "main.d1example.amplifyapp.com."),
            ("a host carrying userinfo", "main", "d1example.amplifyapp.com", "visitor@main.d1example.amplifyapp.com"),
        ];
        for (case, display_name, default_domain, addressed) in cases {
            let branch = hosted_branch_host(display_name, default_domain);
            assert_eq!(
                branch,
                hosted_site_host(addressed),
                "case {case:?}: the branch's host {branch:?} must match the addressed \
                 {addressed:?}"
            );
        }
    }

    #[test]
    fn a_host_naming_another_branch_or_another_app_does_not_read_as_this_one() {
        // The rows span what must NOT match: another branch's label, another app's
        // domain, the app's domain with no branch label at all, and a label that only
        // shares a prefix with the branch's.
        let cases: &[(&str, &str)] = &[
            ("another branch", "other.d1example.amplifyapp.com"),
            ("another app's domain", "main.d2example.amplifyapp.com"),
            ("the domain with no branch label", "d1example.amplifyapp.com"),
            ("a label sharing a prefix", "mainline.d1example.amplifyapp.com"),
        ];
        let branch = hosted_branch_host("main", "d1example.amplifyapp.com");
        for (case, addressed) in cases {
            assert_ne!(
                branch,
                hosted_site_host(addressed),
                "case {case:?}: {addressed:?} must not read as the branch's host {branch:?}"
            );
        }
    }
}
